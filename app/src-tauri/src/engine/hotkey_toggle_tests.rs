//! Gesture timing, ordered dispatch and real cancellation IPC/storage gates.
//! No credentials, microphone, cloud, native target or clipboard are accessed.
use super::*;
use crate::platform::hotkeys::{HotkeyIntent,process_hotkey_queue};

fn configured(h:&AppHub) {
    let mut config=h.db.snapshot().provider_config;let version=config.version;config.region=Some("beijing".into());
    h.db.run(DbCommand::SaveProviderConfig{config,expected_version:version}).unwrap();
    h.inner.lock().unwrap().key_configured=true;
}
fn hide_spy(h:&Arc<AppHub>)->Arc<std::sync::atomic::AtomicUsize> {
    let hides=Arc::new(std::sync::atomic::AtomicUsize::new(0));let count=hides.clone();let weak=Arc::downgrade(h);
    h.set_recorder_dismiss_control(Arc::new(move|id,generation| {
        weak.upgrade().unwrap().validate_recorder_dismiss(&id,generation)?;
        count.fetch_add(1,Ordering::SeqCst);Ok(())
    }));hides
}

#[test]
fn every_configured_shortcut_shares_the_same_start_cancel_stop_timing_policy() {
    for shortcut in ["f8","f9","ctrl+k","alt_right","shift_right"] {
        let h=hub();assert_eq!(h.hotkey_action_at(Instant::now()).unwrap(),HotkeyAction::Start);
        for phase in [SessionPhase::Starting,SessionPhase::Recording] {
            install(&h,"s",phase,true);let start=Instant::now();
            {let mut i=h.inner.lock().unwrap();let s=i.active.as_mut().unwrap();s.started_intent=start;s.prefs.shortcut=shortcut.into();}
            for ms in [0,1,100,499,500] {
                assert_eq!(h.hotkey_action_at(start+Duration::from_millis(ms)).unwrap(),HotkeyAction::Cancel("s".into(),7),"{shortcut} {phase:?} at {ms}ms");
            }
            for ms in [501,1000,5000] {
                assert_eq!(h.hotkey_action_at(start+Duration::from_millis(ms)).unwrap(),HotkeyAction::Stop("s".into(),7),"{shortcut} {phase:?} at {ms}ms");
            }
            assert_eq!(h.hotkey_action_at(start-Duration::from_millis(1)).unwrap(),HotkeyAction::Ignore);
        }
    }
}

#[tokio::test]
async fn rapid_second_trigger_cancels_and_hides_without_final_asr_rewrite_or_delivery() {
    for phase in [SessionPhase::Starting,SessionPhase::Recording] {
        let h=hub();configured(&h);let start=Instant::now();
        let(id,generation,_,_,_,_)=h.prepare_start_state(start,Origin::HotkeyHook).unwrap();
        let processing=h.processing_lease(&id,generation);
        h.inner.lock().unwrap().active.as_mut().unwrap().phase=phase;
        h.set_stop_target_control(Arc::new(|_|panic!("accidental cancellation must not enter stop target inspection")));
        let hides=hide_spy(&h);
        h.dispatch_hotkey_intent(HotkeyIntent{at:start+ACCIDENTAL_HOTKEY_WINDOW,lifecycle_epoch:0}).await.unwrap();
        assert_eq!(view(&h).state,SessionPhase::Cancelled);assert_eq!(hides.load(Ordering::SeqCst),1);
        assert!(!view(&h).worker_released,"cancelling never impersonates a processing worker exit");
        assert!(!view(&h).can_retry_asr);assert!(!view(&h).can_retry_rewrite);
        {let i=h.inner.lock().unwrap();let s=i.active.as_ref().unwrap();assert!(s.stop.load(Ordering::SeqCst));assert!(s.cancel.is_cancelled());assert!(s.source.is_none());assert!(s.output.is_none());}
        assert_eq!(h.db.query_count("SELECT COUNT(*) FROM sessions").unwrap(),0);
        assert_eq!(h.db.query_count("SELECT COUNT(*) FROM usage_events").unwrap(),0);
        assert_eq!(h.db.query_count("SELECT COUNT(*) FROM delivery_attempts").unwrap(),0);
        drop(processing);assert!(view(&h).worker_released);
        h.prepare_start_state(start+Duration::from_secs(2),Origin::HotkeyHook).unwrap();
    }
}

#[tokio::test]
async fn ordinary_second_trigger_selects_stop_and_cuts_audio_without_cancelling() {
    let h=hub();install(&h,"s",SessionPhase::Recording,true);let start=Instant::now();
    h.inner.lock().unwrap().active.as_mut().unwrap().started_intent=start;
    let intent=start+ACCIDENTAL_HOTKEY_WINDOW+Duration::from_millis(1);
    assert_eq!(h.hotkey_action_at(intent).unwrap(),HotkeyAction::Stop("s".into(),7));
    h.stop_session_at_with_target(&identity("s"),intent,||None,|_|panic!("no owned target in this fixture")).await.unwrap();
    assert_eq!(view(&h).state,SessionPhase::FinalizingAsr);
    let i=h.inner.lock().unwrap();let s=i.active.as_ref().unwrap();
    assert!(s.stop.load(Ordering::SeqCst));assert!(!s.cancel.is_cancelled());
}

#[tokio::test]
async fn rapid_cancel_late_hide_cannot_close_the_next_recording() {
    let h=hub();install(&h,"old",SessionPhase::Recording,true);let start=Instant::now();
    h.inner.lock().unwrap().active.as_mut().unwrap().started_intent=start;
    let weak=Arc::downgrade(&h);
    h.set_recorder_dismiss_control(Arc::new(move|id,generation| {
        let h=weak.upgrade().unwrap();install(&h,"new",SessionPhase::Recording,true);
        h.validate_recorder_dismiss(&id,generation)
    }));
    h.toggle_from_hotkey_at(start+Duration::from_millis(100)).await.unwrap();
    assert!(h.valid_active("new",7));assert_eq!(view(&h).state,SessionPhase::Recording);
}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn rapid_queued_intents_keep_physical_order_and_original_time_despite_slow_startup() {
    let h=hub();configured(&h);let hides=hide_spy(&h);
    // Processing now is already five seconds later; classifying at dequeue
    // time instead of the original intent would incorrectly choose Stop.
    let start=Instant::now()-Duration::from_secs(5);
    let (tx,rx)=tokio::sync::mpsc::channel(32);
    let (entered,seen)=tokio::sync::oneshot::channel();let entered=Arc::new(Mutex::new(Some(entered)));
    let (release,wait)=tokio::sync::oneshot::channel();let wait=Arc::new(Mutex::new(Some(wait)));
    let calls=Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let worker=h.clone();let count=calls.clone();
    let task=tokio::spawn(process_hotkey_queue(rx,move|intent| {
        let h=worker.clone();let entered=entered.clone();let wait=wait.clone();let count=count.clone();
        async move {
            if count.fetch_add(1,Ordering::SeqCst)==0 {
                let(id,generation,_,_,_,_)=h.prepare_start_state(intent.at,Origin::HotkeyHook).unwrap();
                let processing=h.processing_lease(&id,generation);
                let receiver={wait.lock().unwrap().take().unwrap()};
                entered.lock().unwrap().take().unwrap().send(()).unwrap();receiver.await.unwrap();drop(processing);
            }else {
                assert!(matches!(h.hotkey_action_at(intent.at).unwrap(),HotkeyAction::Cancel(_, _)),"queued short gesture must stay cancellation before any native stop path");
                h.dispatch_hotkey_intent(intent).await.unwrap();
            }
        }
    }));
    tx.send(HotkeyIntent{at:start,lifecycle_epoch:0}).await.unwrap();seen.await.unwrap();
    tx.send(HotkeyIntent{at:start+Duration::from_millis(100),lifecycle_epoch:0}).await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst),1);assert_eq!(view(&h).state,SessionPhase::Starting);
    release.send(()).unwrap();drop(tx);task.await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst),2);assert_eq!(view(&h).state,SessionPhase::Cancelled);
    assert_eq!(hides.load(Ordering::SeqCst),1);
}

#[tokio::test]
async fn queued_shortcut_from_an_older_lifecycle_cannot_restart_after_resume() {
    let h=hub();let intent=HotkeyIntent{at:Instant::now(),lifecycle_epoch:0};
    h.reserve_lifecycle_epoch();h.capture_available.store(true,Ordering::SeqCst);
    h.dispatch_hotkey_intent(intent).await.unwrap();
    assert!(h.inner.lock().unwrap().active.is_none());
}

#[tokio::test]
async fn old_shortcut_time_cannot_cancel_or_stop_a_newer_session() {
    let h=hub();install(&h,"new",SessionPhase::Recording,true);let start=Instant::now();
    h.inner.lock().unwrap().active.as_mut().unwrap().started_intent=start;
    h.toggle_from_hotkey_at(start-Duration::from_millis(1)).await.unwrap();
    assert!(h.valid_active("new",7));assert!(!h.inner.lock().unwrap().active.as_ref().unwrap().stop.load(Ordering::SeqCst));
}

#[test]
fn later_processing_shortcuts_are_busy_and_do_not_erase_the_current_result() {
    for phase in [SessionPhase::FinalizingAsr,SessionPhase::Rewriting] {
        let h=hub();install(&h,"s",phase,true);let start=Instant::now();
        h.inner.lock().unwrap().active.as_mut().unwrap().started_intent=start;
        assert_eq!(h.hotkey_action_at(start+Duration::from_millis(100)).unwrap(),HotkeyAction::Busy("s".into(),7));
        assert!(h.valid_active("s",7));
    }
}

#[tokio::test]
async fn gestures_queued_before_manual_cancel_completion_cannot_reopen_the_recorder() {
    let h=hub();install(&h,"s",SessionPhase::Recording,true);let start=Instant::now()-Duration::from_secs(2);
    h.inner.lock().unwrap().active.as_mut().unwrap().started_intent=start;
    let old=HotkeyIntent{at:start+Duration::from_secs(1),lifecycle_epoch:0};
    h.cancel_session_async(&identity("s")).await.unwrap();
    assert_eq!(h.hotkey_action_at(old.at).unwrap(),HotkeyAction::Ignore);
    h.dispatch_hotkey_intent(old).await.unwrap();assert_eq!(view(&h).state,SessionPhase::Cancelled);
    assert_eq!(h.hotkey_action_at(Instant::now()+Duration::from_millis(1)).unwrap(),HotkeyAction::Start);
}

#[test]
fn gestures_from_before_a_slow_stop_completed_do_not_become_new_start_requests() {
    let h=hub();install(&h,"s",SessionPhase::Recording,true);let start=Instant::now()-Duration::from_secs(5);
    h.inner.lock().unwrap().active.as_mut().unwrap().started_intent=start;
    h.stop_with_capture(&identity("s"),||None).unwrap();
    h.inner.lock().unwrap().active.as_mut().unwrap().phase=SessionPhase::Ready;
    h.hotkey_operation_finished("s",7);
    assert_eq!(h.hotkey_action_at(start+Duration::from_secs(2)).unwrap(),HotkeyAction::Ignore);
    assert_eq!(h.hotkey_action_at(Instant::now()+Duration::from_millis(1)).unwrap(),HotkeyAction::Start);
    install(&h,"new",SessionPhase::Recording,true);h.hotkey_operation_finished("s",7);
    assert!(h.inner.lock().unwrap().active.as_ref().unwrap().finished_intent.is_none());
}
