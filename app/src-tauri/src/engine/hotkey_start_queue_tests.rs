//! Production FIFO/admission/window/cancel paths with only credential reads
//! and the final audio/cloud launch replaced. No native capture or cloud I/O.
use super::*;
use crate::platform::hotkeys::{HotkeyIntent,process_hotkey_queue};

fn configured(h:&AppHub) {
    let mut config=h.db.snapshot().provider_config;let version=config.version;config.region=Some("beijing".into());
    h.db.run(DbCommand::SaveProviderConfig{config,expected_version:version}).unwrap();h.inner.lock().unwrap().key_configured=true;
}
fn synthetic_keys()->credentials::SessionCredentials {credentials::SessionCredentials{asr:"synthetic".into(),rewrite:"synthetic".into()}}

#[tokio::test(flavor="current_thread")]
async fn ordered_second_down_cancels_during_slow_start_at_100_and_500ms_before_any_pipeline_launch() {
    for interval_ms in [100,500] {
        let h=hub();configured(&h);let start=Instant::now()-Duration::from_secs(2);
        let(entered,seen)=tokio::sync::oneshot::channel();let entered=Mutex::new(Some(entered));
        let(release,wait)=std::sync::mpsc::channel();let wait=Mutex::new(wait);
        let surfaces=Arc::new(Mutex::new(Vec::new()));let surface=surfaces.clone();
        h.set_native_actions(Arc::new(move|name,enabled| {
            assert_eq!(name,"recording");surface.lock().unwrap().push(enabled);
            if enabled {entered.lock().unwrap().take().unwrap().send(()).unwrap();wait.lock().unwrap().recv_timeout(Duration::from_secs(5)).unwrap();}
            Ok(())
        }));
        let hides=Arc::new(std::sync::atomic::AtomicUsize::new(0));let hidden=hides.clone();let weak=Arc::downgrade(&h);
        h.set_recorder_dismiss_control(Arc::new(move|id,generation| {
            weak.upgrade().unwrap().validate_recorder_dismiss(&id,generation)?;hidden.fetch_add(1,Ordering::SeqCst);Ok(())
        }));
        let launches=Arc::new(std::sync::atomic::AtomicUsize::new(0));let errors=Arc::new(Mutex::new(Vec::new()));
        let(tx,rx)=tokio::sync::mpsc::channel(32);let(done,mut completed)=tokio::sync::mpsc::unbounded_channel();
        let owner=h.clone();let launched=launches.clone();let observed=errors.clone();
        let queue=tokio::spawn(process_hotkey_queue(rx,move|intent| {
            let h=owner.clone();let launch=launched.clone();let errors=observed.clone();let done=done.clone();
            async move {
                let result=h.dispatch_hotkey_intent_with_start_worker(intent,move|error|errors.lock().unwrap().push(error),move|hub,prepared|async move {
                    hub.complete_prepared_start_with(prepared,|_|Ok(synthetic_keys()),move|_lease,_keys|{launch.fetch_add(1,Ordering::SeqCst);}).await
                }).await;
                done.send(result).unwrap();
            }
        }));
        tx.send(HotkeyIntent{at:start,lifecycle_epoch:0}).await.unwrap();completed.recv().await.unwrap().unwrap();
        // Admission has already installed the session before the FIFO advances.
        let first=view(&h);let first_id=first.session_id.clone();assert_eq!(first.state,SessionPhase::Starting);assert!(!first.worker_released);
        seen.await.unwrap();tx.send(HotkeyIntent{at:start+Duration::from_millis(interval_ms),lifecycle_epoch:0}).await.unwrap();
        tokio::time::timeout(Duration::from_secs(1),completed.recv()).await.expect("second key-down must not wait for startup ACK").unwrap().unwrap();
        assert_eq!(view(&h).state,SessionPhase::Cancelled);assert_eq!(hides.load(Ordering::SeqCst),1);
        assert!(!view(&h).worker_released,"the old native owner still waits for its real ACK");assert_eq!(launches.load(Ordering::SeqCst),0);
        assert!(h.inner.lock().unwrap().active.as_ref().unwrap().cancel.is_cancelled());
        release.send(()).unwrap();wait_until(||view(&h).worker_released&&!errors.lock().unwrap().is_empty()).await;
        assert_eq!(launches.load(Ordering::SeqCst),0);assert_eq!(*surfaces.lock().unwrap(),vec![true,false]);
        assert_eq!(errors.lock().unwrap()[0].code,"cancelled");
        // An old event already queued before cancellation completed cannot
        // become a new recording when the background native owner exits.
        tx.send(HotkeyIntent{at:start+Duration::from_millis(interval_ms+1),lifecycle_epoch:0}).await.unwrap();completed.recv().await.unwrap().unwrap();
        assert_eq!(view(&h).session_id,first_id);assert_eq!(view(&h).state,SessionPhase::Cancelled);
        assert!(h.inner.lock().unwrap().active.as_ref().unwrap().asr_attempt_id.is_none());
        assert_eq!(h.db.query_count("SELECT COUNT(*) FROM sessions").unwrap(),0);assert_eq!(h.db.query_count("SELECT COUNT(*) FROM usage_events").unwrap(),0);
        drop(tx);queue.await.unwrap();
    }
}

#[tokio::test(flavor="current_thread")]
async fn second_down_cancels_while_native_credentials_are_slow_and_owner_lasts_until_read_finishes() {
    let h=hub();configured(&h);let start=Instant::now()-Duration::from_secs(2);
    let(entered,seen)=tokio::sync::oneshot::channel();let(release,wait)=std::sync::mpsc::channel();
    let launches=Arc::new(AtomicBool::new(false));let launch=launches.clone();let errors=Arc::new(Mutex::new(Vec::new()));let observed=errors.clone();
    h.set_native_actions(Arc::new(|_,_|panic!("cancelled credentials must not reveal the recorder")));
    h.dispatch_hotkey_intent_with_start_worker(HotkeyIntent{at:start,lifecycle_epoch:0},move|error|observed.lock().unwrap().push(error),move|hub,prepared|async move {
        hub.complete_prepared_start_with(prepared,move|_| {
            entered.send(()).unwrap();wait.recv_timeout(Duration::from_secs(5)).unwrap();Ok(synthetic_keys())
        },move|_lease,_keys|launch.store(true,Ordering::SeqCst)).await
    }).await.unwrap();
    seen.await.unwrap();
    h.dispatch_hotkey_intent_with_start_error(HotkeyIntent{at:start+Duration::from_millis(100),lifecycle_epoch:0},|_|panic!("cancel has no startup callback")).await.unwrap();
    assert_eq!(view(&h).state,SessionPhase::Cancelled);assert!(!view(&h).worker_released);assert!(!launches.load(Ordering::SeqCst));
    assert_eq!(h.prepare_start_state(Instant::now(),Origin::HotkeyHook).unwrap_err().code,"session_busy");
    release.send(()).unwrap();wait_until(||view(&h).worker_released&&!errors.lock().unwrap().is_empty()).await;
    assert!(!launches.load(Ordering::SeqCst));assert_eq!(errors.lock().unwrap()[0].code,"cancelled");
    h.prepare_start_state(Instant::now(),Origin::HotkeyHook).unwrap();
}

#[tokio::test(flavor="current_thread")]
async fn background_start_error_keeps_its_session_identity_and_releases_the_next_admission() {
    let h=hub();configured(&h);let errors=Arc::new(Mutex::new(Vec::new()));let observed=errors.clone();
    h.set_native_actions(Arc::new(|_,_|Err(ApiError::new("window_unavailable","native","synthetic recorder failure"))));
    h.dispatch_hotkey_intent_with_start_worker(HotkeyIntent{at:Instant::now(),lifecycle_epoch:0},move|error|observed.lock().unwrap().push(error),|hub,prepared|async move {
        hub.complete_prepared_start_with(prepared,|_|Ok(synthetic_keys()),|_,_|panic!("a failed window cannot start a pipeline")).await
    }).await.unwrap();
    let admitted=view(&h);wait_until(||view(&h).worker_released&&!errors.lock().unwrap().is_empty()).await;
    let error=errors.lock().unwrap()[0].clone();assert_eq!(error.code,"window_unavailable");
    assert_eq!(error.session_id.as_deref(),Some(admitted.session_id.as_str()));assert_eq!(error.generation,Some(admitted.generation));
    assert_eq!(view(&h).state,SessionPhase::Failed);assert_eq!(view(&h).error_code.as_deref(),Some("window_unavailable"));
    h.prepare_start_state(Instant::now(),Origin::HotkeyHook).unwrap();
}

#[tokio::test]
async fn hotkey_cancel_still_dismisses_and_records_finished_boundary_when_cleanup_reports_errors() {
    for (refresh_failed,hide_failed) in [(true,false),(false,true),(true,true)] {
        let h=hub();install(&h,"s",SessionPhase::Recording,true);let start=Instant::now()-Duration::from_secs(2);
        h.inner.lock().unwrap().active.as_mut().unwrap().started_intent=start;
        if refresh_failed {*h.hooks.lock().unwrap()=Some(platform::hotkeys::HotkeyService::test_refresh_service(||Err("synthetic refresh failure".into())));}
        let hides=Arc::new(std::sync::atomic::AtomicUsize::new(0));let hidden=hides.clone();
        h.set_recorder_dismiss_control(Arc::new(move|_,_|{hidden.fetch_add(1,Ordering::SeqCst);
            if hide_failed {Err(ApiError::new("window_unavailable","native","synthetic hide failure"))}else{Ok(())}}));
        let old=start+Duration::from_millis(100);
        let error=h.dispatch_hotkey_intent_with_start_error(HotkeyIntent{at:old,lifecycle_epoch:0},|_|panic!("cancel cannot start another worker")).await.unwrap_err();
        assert_eq!(error.code,if refresh_failed{"shortcut_unavailable"}else{"window_unavailable"});assert_eq!(error.session_id.as_deref(),Some("s"));
        assert_eq!(hides.load(Ordering::SeqCst),1);assert_eq!(view(&h).state,SessionPhase::Cancelled);
        assert!(h.inner.lock().unwrap().active.as_ref().unwrap().finished_intent.is_some());assert_eq!(h.hotkey_action_at(old).unwrap(),HotkeyAction::Ignore);
        if refresh_failed {h.hooks.lock().unwrap().as_mut().unwrap().shutdown(Duration::from_secs(1)).unwrap();}
    }
}

#[tokio::test(flavor="current_thread")]
async fn hotkey_cancel_dismisses_revoked_recording_before_slow_hook_refresh_finishes() {
    let h=hub();install(&h,"s",SessionPhase::Recording,true);let start=Instant::now()-Duration::from_secs(2);
    h.inner.lock().unwrap().active.as_mut().unwrap().started_intent=start;
    let(refreshing,refresh_seen)=tokio::sync::oneshot::channel();let refreshing=Mutex::new(Some(refreshing));
    let(release,wait)=std::sync::mpsc::channel();let wait=Mutex::new(wait);
    *h.hooks.lock().unwrap()=Some(platform::hotkeys::HotkeyService::test_refresh_service(move|| {
        refreshing.lock().unwrap().take().unwrap().send(()).unwrap();wait.lock().unwrap().recv_timeout(Duration::from_secs(5)).unwrap();Ok(())
    }));
    let(hidden,hidden_seen)=tokio::sync::oneshot::channel();let hidden=Mutex::new(Some(hidden));let weak=Arc::downgrade(&h);
    h.set_recorder_dismiss_control(Arc::new(move|id,generation| {
        let h=weak.upgrade().unwrap();h.validate_recorder_dismiss(&id,generation)?;
        assert!(h.inner.lock().unwrap().active.as_ref().unwrap().cancel.is_cancelled());hidden.lock().unwrap().take().unwrap().send(()).unwrap();Ok(())
    }));
    let owner=h.clone();let task=tokio::spawn(async move {owner.dispatch_hotkey_intent_with_start_error(
        HotkeyIntent{at:start+Duration::from_millis(100),lifecycle_epoch:0},|_|panic!("cancel cannot start a worker")).await});
    refresh_seen.await.unwrap();tokio::time::timeout(Duration::from_secs(1),hidden_seen).await.expect("hide cannot wait behind native hook refresh").unwrap();
    assert!(!task.is_finished());assert!(view(&h).cancel_pending);assert!(!view(&h).worker_released);
    release.send(()).unwrap();task.await.unwrap().unwrap();assert_eq!(view(&h).state,SessionPhase::Cancelled);
    h.hooks.lock().unwrap().as_mut().unwrap().shutdown(Duration::from_secs(1)).unwrap();
}

#[tokio::test(flavor="current_thread")]
async fn rapid_second_down_cancels_an_initial_start_failure_instead_of_reopening_it() {
    for interval_ms in [100,500] {
        let h=hub();configured(&h);let start=Instant::now()-Duration::from_secs(2);let errors=Arc::new(Mutex::new(Vec::new()));let observed=errors.clone();
        h.set_native_actions(Arc::new(|_,_|Err(ApiError::new("window_timeout","native","synthetic startup failure"))));
        h.dispatch_hotkey_intent_with_start_worker(HotkeyIntent{at:start,lifecycle_epoch:0},move|error|observed.lock().unwrap().push(error),|hub,prepared|async move {
            hub.complete_prepared_start_with(prepared,|_|Ok(synthetic_keys()),|_,_|panic!("failed startup cannot launch")).await
        }).await.unwrap();
        let first=view(&h);wait_until(||view(&h).state==SessionPhase::Failed&&view(&h).worker_released&&!errors.lock().unwrap().is_empty()).await;
        h.dispatch_hotkey_intent_with_start_error(HotkeyIntent{at:start+Duration::from_millis(interval_ms),lifecycle_epoch:0},|_|panic!("second rapid press must cancel the failed attempt")).await.unwrap();
        assert_eq!(view(&h).state,SessionPhase::Cancelled);assert_eq!(view(&h).session_id,first.session_id);assert_eq!(view(&h).generation,first.generation);
        assert_eq!(errors.lock().unwrap().len(),1);
    }
}

#[tokio::test(flavor="current_thread")]
async fn panicked_background_start_reports_failure_and_does_not_leave_starting_busy() {
    let h=hub();configured(&h);let errors=Arc::new(Mutex::new(Vec::new()));let observed=errors.clone();
    h.dispatch_hotkey_intent_with_start_worker(HotkeyIntent{at:Instant::now(),lifecycle_epoch:0},move|error|observed.lock().unwrap().push(error),|_,_prepared|async move {
        panic!("synthetic startup task panic");
        #[allow(unreachable_code)] Ok(Value::Null)
    }).await.unwrap();
    wait_until(||view(&h).worker_released&&!errors.lock().unwrap().is_empty()).await;
    assert_eq!(view(&h).state,SessionPhase::Failed);assert_eq!(errors.lock().unwrap()[0].code,"startup_interrupted");
    h.prepare_start_state(Instant::now(),Origin::HotkeyHook).unwrap();
}

#[tokio::test(flavor="current_thread")]
async fn second_down_after_500ms_still_selects_normal_stop_while_the_recorder_ack_is_pending() {
    let h=hub();configured(&h);let start=Instant::now()-Duration::from_secs(2);
    let(entered,seen)=tokio::sync::oneshot::channel();let entered=Mutex::new(Some(entered));
    let(release,wait)=std::sync::mpsc::channel();let wait=Mutex::new(wait);
    h.set_native_actions(Arc::new(move|_,enabled| {
        if enabled {entered.lock().unwrap().take().unwrap().send(()).unwrap();wait.lock().unwrap().recv_timeout(Duration::from_secs(5)).unwrap();}Ok(())
    }));
    let(done,finished)=tokio::sync::oneshot::channel();let weak=Arc::downgrade(&h);
    h.dispatch_hotkey_intent_with_start_worker(HotkeyIntent{at:start,lifecycle_epoch:0},|_|panic!("normal stop keeps the original startup"),move|hub,prepared|async move {
        let result=hub.complete_prepared_start_with(prepared,|_|Ok(synthetic_keys()),move|_lease,_keys| {
            let owner=weak.upgrade().unwrap();let mut inner=owner.inner.lock().unwrap();let s=inner.active.as_mut().unwrap();
            assert!(s.stop.load(Ordering::SeqCst));assert!(!s.cancel.is_cancelled());s.worker_released=true;
        }).await;done.send(result.clone()).unwrap();result
    }).await.unwrap();
    seen.await.unwrap();let first=view(&h);let second=start+Duration::from_millis(501);
    assert_eq!(h.hotkey_action_at(second).unwrap(),HotkeyAction::Stop(first.session_id.clone(),first.generation));
    // Use the production stop transition's existing target seam, without
    // reading the real foreground editor or invoking accessibility APIs.
    h.stop_session_at_with_target(&json!({"sessionId":first.session_id,"expectedGeneration":first.generation}),second,||None,|_|panic!("no target in this fixture")).await.unwrap();
    assert_eq!(view(&h).state,SessionPhase::FinalizingAsr);assert!(!h.inner.lock().unwrap().active.as_ref().unwrap().cancel.is_cancelled());
    release.send(()).unwrap();finished.await.unwrap().unwrap();wait_until(||view(&h).worker_released).await;
    h.cancel_session_async(&json!({"sessionId":view(&h).session_id,"expectedGeneration":view(&h).generation})).await.unwrap();
}
