//! Recovery lifetime checks use completed synthetic capture states only.
//! No provider, credential, microphone, clipboard or host input is touched.
use super::*;
use super::tests::{hub, install};

fn cached_failure(h:&AppHub,id:&str)->Instant {
    install(h,id,SessionPhase::Incomplete,true);
    let created=Instant::now();
    let mut inner=h.inner.lock().unwrap();let session=inner.active.as_mut().unwrap();
    session.cache=Some(vec![1;1600]);session.cache_complete=true;session.cache_created=Some(created);
    session.prefs.keep_history=false;
    created
}

fn configure_capture_state_fixture(h:&AppHub) {
    let mut config=h.db.snapshot().provider_config;let version=config.version;
    config.region=Some("beijing".into());
    h.db.submit(DbCommand::SaveProviderConfig{config,expected_version:version},None,DbPriority::Normal).unwrap().wait_blocking().unwrap();
    h.inner.lock().unwrap().key_configured=true;
}

#[test]
fn expired_recovery_snapshot_releases_pcm_instead_of_only_hiding_retry() {
    let h = hub();
    install(&h, "expired", SessionPhase::Incomplete, true);
    {
        let mut inner = h.inner.lock().unwrap();
        let session = inner.active.as_mut().unwrap();
        session.cache = Some(vec![1; 1600]);
        session.cache_complete = true;
        session.cache_created = Some(Instant::now() - Duration::from_secs(1801));
    }
    let snapshot = h.snapshot("main").unwrap();
    assert_eq!(snapshot["session"]["canRetryAsr"], false);
    let inner = h.inner.lock().unwrap();
    assert!(inner.active.as_ref().unwrap().cache.is_none(),
        "the retention deadline must release PCM, not only disable its retry button");
}

#[tokio::test]
async fn retention_deadline_releases_pcm_without_ui_or_snapshot_activity() {
    let h = hub();
    install(&h, "idle-expiry", SessionPhase::Incomplete, true);
    let created = Instant::now() - AUDIO_CACHE_RETENTION + Duration::from_millis(30);
    {
        let mut inner = h.inner.lock().unwrap();
        let session = inner.active.as_mut().unwrap();
        session.cache = Some(vec![1; 1600]);
        session.cache_complete = true;
        session.cache_created = Some(created);
    }
    h.schedule_audio_cache_expiry("idle-expiry".into(), created);
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert!(h.inner.lock().unwrap().active.as_ref().unwrap().cache.is_none(),
        "expired PCM must be released while the panel remains closed");
}

#[test]
fn starting_another_dictation_retains_the_previous_failed_audio_without_persisting_it() {
    let h=hub();cached_failure(&h,"previous");configure_capture_state_fixture(&h);
    let (next_id,_,_,_,_,_)=h.prepare_start_state(Instant::now(),Origin::UiCommand).unwrap();
    let snapshot=h.snapshot("main").unwrap();
    assert_eq!(snapshot["session"]["sessionId"],next_id);
    assert_eq!(snapshot["recovery"]["session"]["sessionId"],"previous",
        "starting another dictation must not discard a retryable failed recording");
    assert_eq!(snapshot["recovery"]["session"]["canRetryAsr"],true);
    assert!(h.db.snapshot().temporary_result.is_none());
}

#[tokio::test]
async fn explicit_recovery_activation_restores_failed_audio_with_a_new_generation_and_no_cloud_or_input() {
    let h=hub();let created=cached_failure(&h,"previous");configure_capture_state_fixture(&h);
    let (_,next_generation,_,_,_,_)=h.prepare_start_state(Instant::now(),Origin::UiCommand).unwrap();
    // Synthetic completion of the later recording; no microphone or provider.
    {let mut i=h.inner.lock().unwrap();let s=i.active.as_mut().unwrap();s.phase=SessionPhase::Ready;s.processing_worker_released=true;}
    let restored=h.command("main",CommandRequest{name:"recovery.activate".into(),payload:json!({
        "sessionId":"previous","expectedGeneration":7,"requestId":"open-previous"
    })}).await.unwrap();
    assert_eq!(restored["sessionId"],"previous");
    assert!(restored["generation"].as_u64().unwrap()>next_generation);
    assert_eq!(restored["canRetryAsr"],true);
    assert_eq!(restored["asrAttemptId"],"attempt-previous");
    assert!(h.snapshot("main").unwrap()["recovery"].is_null());
    let inner=h.inner.lock().unwrap();let s=inner.active.as_ref().unwrap();
    assert_eq!(s.cache_created,Some(created));
    assert_eq!(s.reason,CreationReason::Retry);
    assert!(s.original_delivery_revoked&&s.target.is_none()&&s.delivery.is_none());
    assert!(!s.busy());assert!(h.db.snapshot().temporary_result.is_none());
}

fn finished_later_failure(h:&AppHub)->(String,u64,Instant) {
    let created=Instant::now();let mut inner=h.inner.lock().unwrap();let s=inner.active.as_mut().unwrap();
    s.phase=SessionPhase::Incomplete;s.processing_worker_released=true;s.cache=Some(vec![2;3200]);
    s.cache_complete=true;s.cache_created=Some(created);s.asr_attempt_id=Some("attempt-later".into());
    (s.id.clone(),s.generation,created)
}

fn source_fixture(id:&str,generation:u64)->SourceRevision {
    SourceRevision{id:format!("source-{id}"),session_id:id.into(),asr_attempt_id:format!("attempt-{id}"),generation,
        provider:"aliyun".into(),model:"synthetic".into(),adapter_version:"fixture".into(),complete:true,created_at:now(),
        items:vec![SourceItem{id:"source-item".into(),provider_item_id:"0".into(),capture_order:0,text:"不要删除测试文件。".into(),is_final:true,start_ms:Some(0),end_ms:Some(100)}]}
}

#[test]
fn full_recovery_slot_preserves_both_failures_and_explicit_swap_preserves_each_deadline() {
    let h=hub();let first_created=cached_failure(&h,"first");configure_capture_state_fixture(&h);
    h.prepare_start_state(Instant::now(),Origin::UiCommand).unwrap();
    let(later_id,later_generation,later_created)=finished_later_failure(&h);
    let error=h.prepare_start_state(Instant::now(),Origin::UiCommand).err().unwrap();
    assert_eq!(error.code,"recovery_capacity");
    let snapshot=h.snapshot("main").unwrap();assert_eq!(snapshot["session"]["sessionId"],later_id);assert_eq!(snapshot["recovery"]["session"]["sessionId"],"first");
    let first=h.activate_recovery(&json!({"sessionId":"first","expectedGeneration":7})).unwrap();
    assert!(first["generation"].as_u64().unwrap()>later_generation);
    {let inner=h.inner.lock().unwrap();assert_eq!(inner.active.as_ref().unwrap().cache_created,Some(first_created));assert_eq!(inner.recovery.as_ref().unwrap().cache_created,Some(later_created));}
    let later=h.activate_recovery(&json!({"sessionId":later_id,"expectedGeneration":later_generation})).unwrap();
    assert!(later["generation"].as_u64().unwrap()>first["generation"].as_u64().unwrap());
    {let inner=h.inner.lock().unwrap();assert_eq!(inner.active.as_ref().unwrap().cache_created,Some(later_created));assert_eq!(inner.recovery.as_ref().unwrap().cache_created,Some(first_created));}
    let before=h.snapshot("main").unwrap()["session"].clone();
    assert_eq!(h.activate_recovery(&json!({"sessionId":"first","expectedGeneration":7})).unwrap_err().code,"stale_recovery");
    assert_eq!(h.snapshot("main").unwrap()["session"],before);
    assert!(h.db.snapshot().temporary_result.is_none());
}

#[test]
fn activation_waits_for_current_workers_and_global_resource_admission_without_moving_either_slot() {
    let h=hub();cached_failure(&h,"first");configure_capture_state_fixture(&h);
    h.prepare_start_state(Instant::now(),Origin::UiCommand).unwrap();
    let request=json!({"sessionId":"first","expectedGeneration":7});
    assert_eq!(h.activate_recovery(&request).unwrap_err().code,"session_busy");
    let(later,_,_)=finished_later_failure(&h);
    for blocker in 0..5 {
        match blocker {
            0=>h.capture_available.store(false,Ordering::SeqCst),
            1=>h.db_command_workers.store(1,Ordering::SeqCst),
            2=>h.inner.lock().unwrap().settings_pending=Some("pending-setting".into()),
            3=>h.inner.lock().unwrap().shortcut_capture=Some("key-capture".into()),
            _=>{h.inner.lock().unwrap().auxiliary_workers.insert("preview".into(),CancellationToken::new());},
        }
        assert!(h.activate_recovery(&request).is_err());
        let snapshot=h.snapshot("main").unwrap();assert_eq!(snapshot["session"]["sessionId"],later);assert_eq!(snapshot["recovery"]["session"]["sessionId"],"first");
        h.capture_available.store(true,Ordering::SeqCst);h.db_command_workers.store(0,Ordering::SeqCst);
        let mut inner=h.inner.lock().unwrap();inner.settings_pending=None;inner.shortcut_capture=None;inner.auxiliary_workers.clear();
    }
    let native=h.native_io.lock().unwrap();assert_eq!(h.activate_recovery(&request).unwrap_err().code,"native_busy");drop(native);
}

#[test]
fn activation_and_new_capture_preserve_an_unsaved_source_checkpoint_or_ready_failure() {
    let h=hub();cached_failure(&h,"first");configure_capture_state_fixture(&h);
    let(id,generation,_,_,_,_)=h.prepare_start_state(Instant::now(),Origin::UiCommand).unwrap();
    h.prepare_checkpoint(&id,generation,source_fixture(&id,generation)).unwrap();
    {let mut inner=h.inner.lock().unwrap();let s=inner.active.as_mut().unwrap();s.phase=SessionPhase::Failed;s.processing_worker_released=true;s.db_pending=0;}
    assert_eq!(h.activate_recovery(&json!({"sessionId":"first","expectedGeneration":7})).unwrap_err().code,"recovery_conflict");
    assert_eq!(h.prepare_start_state(Instant::now(),Origin::UiCommand).err().unwrap().code,"save_recovery_pending");
    {let mut inner=h.inner.lock().unwrap();let s=inner.active.as_mut().unwrap();assert!(s.pending.is_some());s.pending=None;}
    assert_eq!(h.activate_recovery(&json!({"sessionId":"first","expectedGeneration":7})).unwrap_err().code,"recovery_conflict");
    assert_eq!(h.snapshot("main").unwrap()["recovery"]["session"]["sessionId"],"first");
}

#[tokio::test]
async fn retained_audio_is_main_only_and_discard_requires_exact_identity_without_touching_current() {
    let h=hub();cached_failure(&h,"first");configure_capture_state_fixture(&h);
    let(id,generation,_,_,_,_)=h.prepare_start_state(Instant::now(),Origin::UiCommand).unwrap();
    assert!(h.snapshot("recording").unwrap().get("recovery").is_none());
    let payload=json!({"sessionId":"first","expectedGeneration":7,"requestId":"discard-first"});
    assert_eq!(h.command("recording",CommandRequest{name:"recovery.discard".into(),payload:payload.clone()}).await.unwrap_err().code,"forbidden");
    for p in [json!({"sessionId":"wrong","expectedGeneration":7}),json!({"sessionId":"first","expectedGeneration":6})]{assert_eq!(h.discard_recovery(&p).unwrap_err().code,"stale_recovery");}
    let result=h.command("main",CommandRequest{name:"recovery.discard".into(),payload:payload.clone()}).await.unwrap();assert_eq!(result["discarded"],true);
    assert_eq!(h.command("main",CommandRequest{name:"recovery.discard".into(),payload}).await.unwrap(),result);
    let snapshot=h.snapshot("main").unwrap();assert!(snapshot["recovery"].is_null());assert_eq!(snapshot["session"]["sessionId"],id);assert_eq!(snapshot["session"]["generation"],generation);
    assert!(h.inner.lock().unwrap().guards["first"].is_cancelled());
}

#[tokio::test]
async fn expiry_cancels_an_owned_retry_but_waits_for_release_and_rejects_late_completions() {
    let h=hub();cached_failure(&h,"running");
    let token={let mut inner=h.inner.lock().unwrap();let s=inner.active.as_mut().unwrap();
        s.reason=CreationReason::Retry;s.phase=SessionPhase::FinalizingAsr;s.processing_worker_released=false;
        s.cache_created=Some(Instant::now()-AUDIO_CACHE_RETENTION-Duration::from_secs(1));s.cancel.clone()};
    let lease=h.processing_lease("running",7);
    let(tx,_rx)=mpsc::channel(1);let ready=Arc::new(AtomicBool::new(false));let notify=Arc::new(Notify::new());
    let producer_token=token.clone();let producer=tokio::spawn(async move {retry_audio::replay(vec![1;3200],tx,producer_token,ready,notify,&DiagnosticsRecorder::default(),None).await});
    let snapshot=h.snapshot("main").unwrap();assert!(token.is_cancelled());assert_eq!(snapshot["session"]["workerReleased"],false);assert_eq!(snapshot["session"]["state"],"incomplete");
    assert_eq!(snapshot["session"]["errorCode"],"asr_retry_expired");
    h.fail("running",7,ApiError::new("cancelled","asr","late provider cancellation"),true);
    assert!(h.prepare_checkpoint("running",7,source_fixture("running",7)).is_err());
    tokio::time::timeout(Duration::from_millis(200),producer).await.unwrap().unwrap();
    drop(lease);let snapshot=h.snapshot("main").unwrap();assert_eq!(snapshot["session"]["workerReleased"],true);assert_eq!(snapshot["session"]["errorCode"],"asr_retry_expired");
    assert_eq!(snapshot["session"]["canRetryAsr"],false);assert!(h.db.snapshot().temporary_result.is_none());
}

#[tokio::test]
async fn retained_expiry_notifies_main_even_when_recorder_snapshot_already_cleaned_it() {
    let h=hub();let created=cached_failure(&h,"first");configure_capture_state_fixture(&h);
    h.prepare_start_state(Instant::now(),Origin::UiCommand).unwrap();
    let seen=Arc::new(Mutex::new(Vec::new()));let sink=seen.clone();h.subscribe("main",Arc::new(move|event|sink.lock().unwrap().push(event.kind))).unwrap();
    let expired=created-AUDIO_CACHE_RETENTION-Duration::from_secs(1);
    h.inner.lock().unwrap().recovery.as_mut().unwrap().cache_created=Some(expired);
    h.snapshot("recording").unwrap();assert!(h.inner.lock().unwrap().recovery.is_none());
    h.schedule_audio_cache_expiry("first".into(),expired);
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(seen.lock().unwrap().iter().any(|kind|kind=="recovery.changed"));
}

#[tokio::test]
async fn locking_or_quitting_discards_both_audio_slots_and_cancels_old_guards() {
    for shutdown in [false,true] {
        let h=hub();cached_failure(&h,"first");configure_capture_state_fixture(&h);
        h.prepare_start_state(Instant::now(),Origin::UiCommand).unwrap();finished_later_failure(&h);
        if shutdown{h.shutdown().unwrap();}else{h.interrupt_session("Locked").await.unwrap();}
        let inner=h.inner.lock().unwrap();assert!(inner.recovery.is_none());assert!(inner.active.as_ref().unwrap().cache.is_none());assert!(inner.guards["first"].is_cancelled());
    }
}

#[tokio::test]
async fn locking_after_a_later_ready_result_releases_both_audio_slots_without_erasing_ready_text() {
    let h=hub();cached_failure(&h,"first");configure_capture_state_fixture(&h);
    h.prepare_start_state(Instant::now(),Origin::UiCommand).unwrap();
    let(id,generation,_)=finished_later_failure(&h);
    let source=source_fixture(&id,generation);
    let output={let mut inner=h.inner.lock().unwrap();let s=inner.active.as_mut().unwrap();
        let output=OutputRevision{id:"ready-output".into(),session_id:id.clone(),parent_revision_id:None,base_revision_id:None,
            source_revision_id:source.id.clone(),mode_snapshot:s.mode.clone(),resolved_mode:BaseMode::Light,rewrite_fallback:None,
            blocks:vec![OutputBlock{id:"ready-block".into(),text:"不要删除测试文件。".into(),source_refs:vec!["source-item".into()]}],
            kind:RevisionKind::Initial,creation_reason:CreationReason::NormalInitial,created_at:now()};
        s.source=Some(source.clone());s.output=Some(output.clone());s.phase=SessionPhase::Ready;output};
    h.interrupt_session("Locked").await.unwrap();
    let inner=h.inner.lock().unwrap();let s=inner.active.as_ref().unwrap();
    assert!(inner.recovery.is_none());
    assert!(s.cache.is_none(),"locking must release Ready audio even when cancellation reports alreadyCompleted");
    assert!(!s.cache_complete&&s.cache_created.is_none());
    assert_eq!(s.phase,SessionPhase::Ready);
    assert_eq!(s.source.as_ref().unwrap().text(),source.text());
    assert_eq!(s.output.as_ref().unwrap().id,output.id);
    assert_eq!(s.output.as_ref().unwrap().text(),output.text());
}

#[test]
fn late_capture_cannot_refill_cancelled_audio_when_database_cleanup_failed() {
    let h=hub();install(&h,"cancelled-cleanup",SessionPhase::Failed,false);
    {let mut inner=h.inner.lock().unwrap();let s=inner.active.as_mut().unwrap();
        s.cancel.cancel();s.error=Some(ApiError::new("cancel_cleanup_failed","storage","synthetic cleanup failure"));}
    let summary=CaptureSummary{complete:true,cancelled:false,captured_samples:1600,input_sample_rate:16000,
        emitted_frames:1,first_samples_received:true,error:None,conditioning:None};
    h.accept_capture_cache("cancelled-cleanup",7,&summary,vec![1;1600],Instant::now());
    let inner=h.inner.lock().unwrap();let s=inner.active.as_ref().unwrap();
    assert!(s.cache.is_none(),"a late capture result must not restore audio after cancellation, even if DB cleanup failed");
    assert!(!s.cache_complete&&s.cache_created.is_none());
    assert!(s.worker_released);assert_eq!(s.capture_duration_ms,100);
    assert_eq!(s.error.as_ref().unwrap().code,"cancel_cleanup_failed");
}
