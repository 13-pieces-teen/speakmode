//! Updater admission only: synthetic capture states and fixture SQLite. No
//! installer, microphone, real hotkey, clipboard or user profile is accessed.
use super::*;

fn assert_refusal_preserves_session(h:&AppHub,code:&str) {
    let before={let inner=h.inner.lock().unwrap();inner.active.as_ref().map(|s|(wire(s.view()).unwrap(),s.cancel.is_cancelled(),s.source.clone(),s.cache.clone()))};
    let epoch=h.lifecycle_epoch.load(Ordering::SeqCst);
    assert_eq!(h.begin_update_shutdown_with_capture(true).unwrap_err().code,code);
    let inner=h.inner.lock().unwrap();
    assert_eq!(inner.active.as_ref().map(|s|(wire(s.view()).unwrap(),s.cancel.is_cancelled(),s.source.clone(),s.cache.clone())),before);
    assert!(!inner.update_shutdown_started);assert!(!h.quitting.load(Ordering::SeqCst));
    assert!(h.capture_available.load(Ordering::SeqCst));assert_eq!(h.lifecycle_epoch.load(Ordering::SeqCst),epoch);
}

#[test]
fn update_refuses_capture_processing_delivery_and_recoverable_content_without_cancelling() {
    for phase in [SessionPhase::Starting,SessionPhase::Recording,SessionPhase::FinalizingAsr,SessionPhase::Rewriting] {
        let h=hub();install(&h,"busy",phase,true);assert_refusal_preserves_session(&h,"update_busy");
    }
    for content in 0..5 {
        let h=hub();install(&h,"recovery",SessionPhase::Incomplete,true);
        {let mut inner=h.inner.lock().unwrap();let s=inner.active.as_mut().unwrap();match content {
            0=>s.source=Some(source("recovery")),
            1=>s.captions.push(CaptionItem{id:"partial".into(),text:"已有识别文字".into(),revision:1,is_final:false,capture_order:0}),
            2=>{s.cache=Some(vec![1;1600]);s.cache_complete=true;s.cache_created=Some(Instant::now());},
            3=>s.pending=Some(("pending-source".into(),PendingCommit::Source(SessionSeed{id:"recovery".into(),generation:7,
                started_at:s.started_at.clone(),ended_at:s.ended_at.clone().unwrap(),capture_duration_ms:12_000,
                end_local_date:"2026-10-04".into(),time_zone:"+08:00".into(),storage_policy:StoragePolicy::History,mode_snapshot:s.mode.clone()},source("recovery")))),
            _=>s.delivery=Some(DeliveryAttempt{id:"prepared".into(),session_id:"recovery".into(),generation:7,
                output_revision_id:"draft".into(),target_fingerprint:"fixture".into(),claimed_at:now(),state:DeliveryState::Prepared,reason:None}),
        }}
        assert_refusal_preserves_session(&h,"update_busy");
    }
}

#[test]
fn update_refuses_retained_recovery_and_unreleased_native_or_settings_owners() {
    let h=hub();install(&h,"retained",SessionPhase::Incomplete,true);
    {let mut inner=h.inner.lock().unwrap();let s=inner.active.take().unwrap();inner.recoveries.push(s);}
    assert_refusal_preserves_session(&h,"update_busy");assert_eq!(h.inner.lock().unwrap().recoveries.len(),1);
    for blocker in 0..5 {
        let h=hub();
        match blocker {
            0=>{h.inner.lock().unwrap().settings_pending=Some("saving".into());},
            1=>{h.inner.lock().unwrap().shortcut_capture=Some("shortcut".into());},
            2=>{h.inner.lock().unwrap().auxiliary_workers.insert("cancelled-but-owned".into(),CancellationToken::new());},
            3=>{let native=h.native_io.lock().unwrap();assert_refusal_preserves_session(&h,"update_busy");drop(native);continue;},
            _=>{assert_eq!(h.begin_update_shutdown_with_capture(false).unwrap_err().code,"update_busy");assert!(!h.quitting.load(Ordering::SeqCst));continue;},
        }
        assert_refusal_preserves_session(&h,"update_busy");
    }
}

#[test]
fn update_preserves_private_temporary_ready_but_allows_durable_history_ready() {
    for private in [true,false] {
        let h=hub();install(&h,"ready",SessionPhase::FinalizingAsr,true);
        h.inner.lock().unwrap().active.as_mut().unwrap().prefs.keep_history=!private;
        ready(&h,"ready");let saved=h.db.get_result("ready").unwrap();
        if private {
            // Also cover the previous temporary result when another session no
            // longer references its body as the active session.
            h.inner.lock().unwrap().active=None;
            assert_refusal_preserves_session(&h,"update_temporary_result");
            assert_eq!(h.db.snapshot().temporary_result.unwrap(),saved);
        }else{
            let before=wire(view(&h)).unwrap();let epoch=h.lifecycle_epoch.load(Ordering::SeqCst);
            h.begin_update_shutdown_with_capture(true).unwrap();
            assert!(h.quitting.load(Ordering::SeqCst));assert!(!h.capture_available.load(Ordering::SeqCst));
            assert_eq!(h.lifecycle_epoch.load(Ordering::SeqCst),epoch+1);
            assert_eq!(wire(view(&h)).unwrap(),before);assert!(!h.inner.lock().unwrap().active.as_ref().unwrap().cancel.is_cancelled());
            assert_eq!(h.db.get_result("ready").unwrap(),saved);
        }
    }
}

#[tokio::test]
async fn update_closes_new_edit_copy_read_and_direct_source_adoption_admission() {
    let h=hub();install(&h,"saved",SessionPhase::FinalizingAsr,true);ready(&h,"saved");
    let saved=h.db.get_result("saved").unwrap();h.begin_update_shutdown_with_capture(true).unwrap();
    for name in ["revision.edit","dictionary.save","preferences.update","history.get","result.copy","session.start"] {
        let error=command(&h,name,json!({"requestId":Uuid::new_v4().to_string(),"sessionId":"saved"})).await.unwrap_err();
        assert_eq!(error.code,"update_in_progress", "{name} reached an operation after updater reservation");
    }
    assert_eq!(h.use_source(&identity("saved")).unwrap_err().code,"update_in_progress");
    assert_eq!(h.copy_current_recorder_with("result.copy",&identity("saved"),|_|panic!("updater reservation cannot touch clipboard")).unwrap_err().code,"update_in_progress");
    command(&h,"state.snapshot",json!({})).await.unwrap();
    assert_eq!(h.db_command_workers.load(Ordering::SeqCst),0);assert_eq!(h.db.outstanding(),0);
    assert_eq!(h.db.get_result("saved").unwrap(),saved);
}

#[test]
fn registered_unspawned_request_and_direct_content_lease_both_block_update() {
    let h=hub();let request=Arc::new(RequestRecord{fingerprint:"unspawned-start".into(),outcome:Mutex::new(None),completed:tokio::sync::Notify::new()});
    h.inner.lock().unwrap().requests.insert("main:pending-start".into(),request.clone());
    assert_eq!(h.db_command_workers.load(Ordering::SeqCst),0);
    assert_refusal_preserves_session(&h,"update_busy");
    *request.outcome.lock().unwrap()=Some(Ok(ReplayValue::Safe(Value::Null)));
    let lease=h.reserve_content_operation().unwrap();assert_refusal_preserves_session(&h,"update_busy");drop(lease);
    let reader=h.reserve_update_read("audio.listDevices").unwrap().unwrap();
    assert_refusal_preserves_session(&h,"update_busy");assert_eq!(h.db_command_workers.load(Ordering::SeqCst),0);
    assert_ne!(h.prepare_start_state(Instant::now(),Origin::UiCommand).unwrap_err().code,"storage_busy",
        "an updater read lease must not block ordinary capture admission");
    drop(reader);
    h.begin_update_shutdown_with_capture(true).unwrap();
}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn in_flight_history_edit_blocks_update_until_its_durable_receipt() {
    let(h,path)=file_hub();install(&h,"edit",SessionPhase::FinalizingAsr,true);ready(&h,"edit");
    let original=h.db.get_result("edit").unwrap();h.inner.lock().unwrap().active=None;
    let writer=second_writer(&path);let worker=h.clone();let revision=original.output.as_ref().unwrap().id.clone();
    let mut blocks=original.output.as_ref().unwrap().blocks.clone();blocks[0].text="用户明确修改后的文字".into();
    let edit=tokio::spawn(async move{command(&worker,"revision.edit",json!({"sessionId":"edit","requestId":"install-edit-race",
        "expectedCurrentRevisionId":revision,"blocks":blocks})).await});
    wait_until(||h.db.outstanding()>0&&h.db_command_workers.load(Ordering::SeqCst)>0).await;
    assert_refusal_preserves_session(&h,"update_busy");
    writer.execute_batch("ROLLBACK;").unwrap();edit.await.unwrap().unwrap();
    assert_ne!(h.db.get_result("edit").unwrap().output.as_ref().unwrap().id,original.output.as_ref().unwrap().id);
    assert_eq!(h.db_command_workers.load(Ordering::SeqCst),0);h.begin_update_shutdown_with_capture(true).unwrap();
}
