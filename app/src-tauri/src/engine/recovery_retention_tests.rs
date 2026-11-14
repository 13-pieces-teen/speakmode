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

fn private_checkpoint_recovery(h:&AppHub,id:&str) {
    install(h,id,SessionPhase::FinalizingAsr,true);
    {let mut inner=h.inner.lock().unwrap();let s=inner.active.as_mut().unwrap();
        s.prefs.keep_history=false;s.cache_created=Some(Instant::now());}
    h.checkpoint(id,7,source_fixture(id,7)).unwrap();
    h.inner.lock().unwrap().active.as_mut().unwrap().phase=SessionPhase::Failed;
    configure_capture_state_fixture(h);h.prepare_start_state(Instant::now(),Origin::UiCommand).unwrap();
    {let mut inner=h.inner.lock().unwrap();let s=inner.active.as_mut().unwrap();s.phase=SessionPhase::Ready;s.processing_worker_released=true;}
}

#[tokio::test]
async fn private_checkpoint_is_released_on_discard_expiry_timer_and_lock() {
    for cleanup in 0..4 {
        let h=hub();private_checkpoint_recovery(&h,"private-source");
        assert!(h.db.submit(DbCommand::GetResult{session_id:"private-source".into()},None,DbPriority::Normal).unwrap().wait().await.is_ok());
        match cleanup {
            0=>{h.discard_recovery(&json!({"sessionId":"private-source","expectedGeneration":7})).unwrap();},
            1=>{h.inner.lock().unwrap().recoveries[0].cache_created=Some(Instant::now()-AUDIO_CACHE_RETENTION-Duration::from_secs(1));h.snapshot("main").unwrap();},
            2=>{
                let expired=Instant::now()-AUDIO_CACHE_RETENTION-Duration::from_secs(1);
                h.inner.lock().unwrap().recoveries[0].cache_created=Some(expired);
                h.schedule_audio_cache_expiry("private-source".into(),expired);
                tokio::time::timeout(Duration::from_secs(2),async{while !h.inner.lock().unwrap().recoveries.is_empty(){tokio::task::yield_now().await;}}).await.unwrap();
            },
            _=>{h.interrupt_session("Locked").await.unwrap();},
        }
        tokio::time::timeout(Duration::from_secs(2),async{while h.db.outstanding()!=0{tokio::task::yield_now().await;}}).await.unwrap();
        assert!(h.inner.lock().unwrap().recoveries.is_empty());
        assert!(h.inner.lock().unwrap().guards["private-source"].is_cancelled());
        assert!(matches!(h.db.submit(DbCommand::GetResult{session_id:"private-source".into()},None,DbPriority::Normal).unwrap().wait().await,Err(StoreError::NotFound)),"cleanup path {cleanup} retained a private source");
        assert!(h.db.snapshot().temporary_result.is_none());
    }
}

#[test]
fn stale_use_source_generation_cannot_reserve_a_permanent_busy_count() {
    let h=hub();install(&h,"source",SessionPhase::FinalizingAsr,true);
    h.checkpoint("source",7,source_fixture("source",7)).unwrap();
    h.inner.lock().unwrap().active.as_mut().unwrap().phase=SessionPhase::Failed;
    assert_eq!(h.use_source(&json!({"sessionId":"source","expectedGeneration":6})).unwrap_err().code,"stale_session");
    let inner=h.inner.lock().unwrap();let s=inner.active.as_ref().unwrap();
    assert_eq!(s.db_pending,0);assert!(!s.busy());assert!(s.view().can_retry_rewrite);
}

#[test]
fn retry_configuration_is_explicit_versioned_and_keeps_the_recording_original() {
    let h=hub();install(&h,"retry-config",SessionPhase::Incomplete,true);
    let original=h.inner.lock().unwrap().active.as_ref().unwrap().original_config.clone();
    let mut config=h.db.snapshot().provider_config;let expected=config.version;
    config.region=Some("beijing".into());config.rewrite_provider="openai".into();config.rewrite_model="gpt-5.4-mini".into();
    let DbValue::ProviderConfig(current)=h.db.submit(DbCommand::SaveProviderConfig{config,expected_version:expected},None,DbPriority::Normal).unwrap().wait_blocking().unwrap()else{panic!("provider receipt");};
    let mut inner=h.inner.lock().unwrap();let s=inner.active.as_mut().unwrap();s.config=current.clone();
    assert_eq!(h.retry_configuration(&json!({}),s).unwrap(),original);
    assert_eq!(h.retry_configuration(&json!({"retryConfiguration":"original"}),s).unwrap(),original);
    assert_eq!(h.retry_configuration(&json!({"retryConfiguration":"current","expectedProviderVersion":current.version}),s).unwrap(),current);
    assert_eq!(h.retry_configuration(&json!({"retryConfiguration":"current","expectedProviderVersion":expected}),s).unwrap_err().code,"revisionConflict");
    assert_eq!(h.retry_configuration(&json!({"retryConfiguration":"unknown"}),s).unwrap_err().code,"invalid_retry_configuration");
    assert_eq!(s.original_config,original);assert_eq!(s.generation,7);assert!(s.source.is_none());
}

#[test]
fn repeated_incomplete_recordings_allow_the_next_capture_and_preserve_all_deadlines() {
    let h=hub();let first_created=cached_failure(&h,"first");configure_capture_state_fixture(&h);
    h.prepare_start_state(Instant::now(),Origin::UiCommand).unwrap();
    let(later_id,later_generation,later_created)=finished_later_failure(&h);
    let(next_id,_,_,_,_,_)=h.prepare_start_state(Instant::now(),Origin::HotkeyHook).unwrap();
    let snapshot=h.snapshot("main").unwrap();
    assert_eq!(snapshot["session"]["sessionId"],next_id);
    assert_eq!(snapshot["recoveries"].as_array().unwrap().len(),2);
    {let mut inner=h.inner.lock().unwrap();let s=inner.active.as_mut().unwrap();s.phase=SessionPhase::Ready;s.processing_worker_released=true;}
    let first=h.activate_recovery(&json!({"sessionId":"first","expectedGeneration":7})).unwrap();
    assert!(first["generation"].as_u64().unwrap()>later_generation);
    {let inner=h.inner.lock().unwrap();assert_eq!(inner.active.as_ref().unwrap().cache_created,Some(first_created));assert_eq!(inner.recoveries.first().unwrap().cache_created,Some(later_created));}
    let later=h.activate_recovery(&json!({"sessionId":later_id,"expectedGeneration":later_generation})).unwrap();
    assert!(later["generation"].as_u64().unwrap()>first["generation"].as_u64().unwrap());
    {let inner=h.inner.lock().unwrap();assert_eq!(inner.active.as_ref().unwrap().cache_created,Some(later_created));assert_eq!(inner.recoveries.first().unwrap().cache_created,Some(first_created));}
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
fn next_capture_and_activation_preserve_an_unsaved_source_checkpoint() {
    let h=hub();cached_failure(&h,"first");configure_capture_state_fixture(&h);
    let(id,generation,_,_,_,_)=h.prepare_start_state(Instant::now(),Origin::UiCommand).unwrap();
    h.prepare_checkpoint(&id,generation,source_fixture(&id,generation)).unwrap();
    {let mut inner=h.inner.lock().unwrap();let s=inner.active.as_mut().unwrap();s.phase=SessionPhase::Failed;s.processing_worker_released=true;s.db_pending=0;}
    let pending=h.inner.lock().unwrap().active.as_ref().unwrap().pending.clone().unwrap().0;
    h.activate_recovery(&json!({"sessionId":"first","expectedGeneration":7})).unwrap();
    let restored=h.activate_recovery(&json!({"sessionId":id,"expectedGeneration":generation})).unwrap();
    assert_eq!(restored["canRetrySave"],true);assert_eq!(restored["pendingCommitId"],pending);
    assert_eq!(restored["generation"],generation);
    h.prepare_start_state(Instant::now(),Origin::UiCommand).unwrap();
    let inner=h.inner.lock().unwrap();
    let retained=inner.recoveries.iter().find(|s|s.id==id).unwrap();
    assert_eq!(retained.pending.as_ref().unwrap().0,pending);
    assert_eq!(retained.source.as_ref().unwrap().text(),"不要删除测试文件。");
    assert!(retained.original_delivery_revoked);
}

#[test]
fn partial_captions_without_retryable_audio_survive_a_new_recording_and_late_failure() {
    let h=hub();cached_failure(&h,"partial");configure_capture_state_fixture(&h);
    {let mut inner=h.inner.lock().unwrap();let s=inner.active.as_mut().unwrap();
        s.cache_complete=false;s.captions=vec![CaptionItem{id:"partial-item".into(),text:"保留这半句话".into(),is_final:false,revision:1,capture_order:0}];}
    let(id,generation,_,_,_,_)=h.prepare_start_state(Instant::now(),Origin::HotkeyHook).unwrap();
    h.fail("partial",7,ApiError::new("asr_closed","asr","late failure"),true);
    let snapshot=h.snapshot("main").unwrap();
    assert_eq!(snapshot["session"]["sessionId"],id);assert_eq!(snapshot["session"]["generation"],generation);
    assert_eq!(snapshot["recoveries"][0]["session"]["partial"],"保留这半句话");
    assert_eq!(snapshot["recoveries"][0]["session"]["canRetryAsr"],false);
    assert_eq!(snapshot["recoveries"][0]["session"]["canCopyRecovery"],true);
}

#[test]
fn incomplete_phase_waits_for_real_worker_release_before_parking_or_starting() {
    let h=hub();cached_failure(&h,"held");configure_capture_state_fixture(&h);
    h.inner.lock().unwrap().active.as_mut().unwrap().processing_worker_released=false;
    assert_eq!(h.prepare_start_state(Instant::now(),Origin::HotkeyHook).err().unwrap().code,"session_busy");
    assert!(h.inner.lock().unwrap().recoveries.is_empty());
    h.inner.lock().unwrap().active.as_mut().unwrap().processing_worker_released=true;
    assert!(h.prepare_start_state(Instant::now(),Origin::HotkeyHook).is_ok());
    assert_eq!(h.inner.lock().unwrap().recoveries[0].id,"held");
}

#[test]
fn adopting_a_parked_source_finishes_its_recovery_without_replacing_current_recording() {
    let h=hub();install(&h,"saved-source",SessionPhase::FinalizingAsr,true);
    h.checkpoint("saved-source",7,source_fixture("saved-source",7)).unwrap();
    h.inner.lock().unwrap().active.as_mut().unwrap().phase=SessionPhase::Failed;
    configure_capture_state_fixture(&h);
    let(id,_,_,_,_,_)=h.prepare_start_state(Instant::now(),Origin::UiCommand).unwrap();
    h.use_source(&json!({"sessionId":"saved-source","expectedGeneration":7})).unwrap();
    let inner=h.inner.lock().unwrap();assert!(inner.recoveries.is_empty());
    assert_eq!(inner.active.as_ref().unwrap().id,id);
    assert_eq!(inner.active.as_ref().unwrap().phase,SessionPhase::Starting);
}

#[test]
fn removing_a_recovery_card_does_not_revoke_the_saved_source_in_history() {
    let h=hub();install(&h,"saved-source",SessionPhase::FinalizingAsr,true);
    h.checkpoint("saved-source",7,source_fixture("saved-source",7)).unwrap();
    h.inner.lock().unwrap().active.as_mut().unwrap().phase=SessionPhase::Failed;
    configure_capture_state_fixture(&h);
    h.prepare_start_state(Instant::now(),Origin::UiCommand).unwrap();
    h.discard_recovery(&json!({"sessionId":"saved-source","expectedGeneration":7})).unwrap();
    assert!(!h.inner.lock().unwrap().guards["saved-source"].is_cancelled());
    h.use_source(&json!({"sessionId":"saved-source","expectedGeneration":7})).unwrap();
}

#[tokio::test]
async fn parked_text_without_audio_expires_without_ui_activity() {
    let h=hub();cached_failure(&h,"text-only");configure_capture_state_fixture(&h);
    let created=Instant::now()-AUDIO_CACHE_RETENTION+Duration::from_millis(30);
    {let mut inner=h.inner.lock().unwrap();let s=inner.active.as_mut().unwrap();
        s.cache=None;s.cache_complete=false;s.cache_created=Some(created);
        s.captions=vec![CaptionItem{id:"text-item".into(),text:"保留文字".into(),revision:1,is_final:false,capture_order:0}];}
    h.prepare_start_state(Instant::now(),Origin::UiCommand).unwrap();h.schedule_retained_expiry();
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert!(h.inner.lock().unwrap().recoveries.is_empty());
}

#[tokio::test]
async fn retained_audio_is_main_only_and_discard_requires_exact_identity_without_touching_current() {
    let h=hub();cached_failure(&h,"first");configure_capture_state_fixture(&h);
    let(id,generation,_,_,_,_)=h.prepare_start_state(Instant::now(),Origin::UiCommand).unwrap();
    assert!(h.snapshot("recording").unwrap().get("recovery").is_none());
    let payload=json!({"sessionId":"first","expectedGeneration":7,"requestId":"discard-first"});
    assert_eq!(h.command("recording",CommandRequest{name:"recovery.discard".into(),payload:payload.clone()}).await.unwrap_err().code,"forbidden");
    assert_eq!(h.discard_recovery(&json!({"sessionId":"wrong","expectedGeneration":7})).unwrap_err().code,"recovery_expired");
    assert_eq!(h.discard_recovery(&json!({"sessionId":"first","expectedGeneration":6})).unwrap_err().code,"stale_recovery");
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
    h.inner.lock().unwrap().recoveries.first_mut().unwrap().cache_created=Some(expired);
    h.snapshot("recording").unwrap();assert!(h.inner.lock().unwrap().recoveries.is_empty());
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
        let inner=h.inner.lock().unwrap();assert!(inner.recoveries.is_empty());assert!(inner.active.as_ref().unwrap().cache.is_none());assert!(inner.guards["first"].is_cancelled());
    }
}

#[tokio::test]
async fn locking_after_a_later_ready_result_releases_both_audio_slots_without_erasing_ready_text() {
    let h=hub();cached_failure(&h,"first");configure_capture_state_fixture(&h);
    h.prepare_start_state(Instant::now(),Origin::UiCommand).unwrap();
    let(id,generation,_)=finished_later_failure(&h);
    let source=source_fixture(&id,generation);
    let output={let mut inner=h.inner.lock().unwrap();let s=inner.active.as_mut().unwrap();
        let output=OutputRevision{writing_receipt:None, confirmed_context:None, id:"ready-output".into(),session_id:id.clone(),parent_revision_id:None,base_revision_id:None,
            source_revision_id:source.id.clone(),mode_snapshot:s.mode.clone(),resolved_mode:BaseMode::Light,rewrite_fallback:None, rewrite_recovery:None,
            blocks:vec![OutputBlock{id:"ready-block".into(),text:"不要删除测试文件。".into(),source_refs:vec!["source-item".into()]}],
            kind:RevisionKind::Initial,creation_reason:CreationReason::NormalInitial,created_at:now()};
        s.source=Some(source.clone());s.output=Some(output.clone());s.phase=SessionPhase::Ready;output};
    h.interrupt_session("Locked").await.unwrap();
    let inner=h.inner.lock().unwrap();let s=inner.active.as_ref().unwrap();
    assert!(inner.recoveries.is_empty());
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
