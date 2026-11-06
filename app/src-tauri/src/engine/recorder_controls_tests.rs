//! Real IPC/state/store with synthetic sessions and native callback spies only.
use super::*;

#[tokio::test]
async fn dismiss_keeps_recovery_and_rejects_active_or_stale_sessions() {
    let h=hub();install(&h,"recovery",SessionPhase::Incomplete,true);
    {let mut i=h.inner.lock().unwrap();let s=i.active.as_mut().unwrap();
        s.cache=Some(vec![9;1600]);s.cache_complete=true;s.cache_created=Some(Instant::now());
        s.captions=vec![CaptionItem{id:"part".into(),text:"已有文字".into(),revision:1,is_final:false,capture_order:0}];}
    let hides=Arc::new(std::sync::atomic::AtomicUsize::new(0));let count=hides.clone();let weak=Arc::downgrade(&h);
    h.set_recorder_dismiss_control(Arc::new(move|id,generation|{
        weak.upgrade().unwrap().validate_recorder_dismiss(&id,generation)?;
        count.fetch_add(1,Ordering::SeqCst);Ok(())
    }));
    h.command("recording",CommandRequest{name:"window.dismissRecorder".into(),payload:identity("recovery")}).await.unwrap();
    assert_eq!(hides.load(Ordering::SeqCst),1);
    {let i=h.inner.lock().unwrap();let s=i.active.as_ref().unwrap();assert_eq!(s.phase,SessionPhase::Incomplete);assert_eq!(s.cache.as_ref().unwrap().len(),1600);assert!(!s.cancel.is_cancelled());assert_eq!(s.captions[0].text,"已有文字");}
    install(&h,"next",SessionPhase::Recording,false);
    for (id,code) in [("recovery","stale_session"),("next","session_busy")] {
        assert_eq!(h.command("recording",CommandRequest{name:"window.dismissRecorder".into(),payload:identity(id)}).await.unwrap_err().code,code);
    }
    assert_eq!(hides.load(Ordering::SeqCst),1);
}

#[tokio::test]
async fn dismiss_rechecks_identity_at_native_dispatch_and_blocks_pending_delivery() {
    let h=hub();install(&h,"old",SessionPhase::Ready,false);
    assert_eq!(h.validate_recorder_dismiss("old",7).unwrap_err().code,"session_busy");
    install(&h,"old",SessionPhase::Incomplete,true);
    let weak=Arc::downgrade(&h);
    h.set_recorder_dismiss_control(Arc::new(move|id,generation|{
        let h=weak.upgrade().unwrap();install(&h,"new",SessionPhase::Recording,false);
        h.validate_recorder_dismiss(&id,generation)
    }));
    let error=h.command("recording",CommandRequest{name:"window.dismissRecorder".into(),payload:identity("old")}).await.unwrap_err();
    assert_eq!(error.code,"stale_session");assert_eq!(view(&h).session_id,"new");
    assert_eq!(command(&h,"window.dismissRecorder",identity("new")).await.unwrap_err().code,"forbidden");
}

#[tokio::test]
async fn current_result_intent_survives_snapshot_and_only_matching_ack_clears_it() {
    let h=hub();install(&h,"result",SessionPhase::FinalizingAsr,true);ready(&h,"result");
    h.set_native_actions(Arc::new(|name,_|{assert_eq!(name,"showMain");Ok(())}));
    let mut payload=identity("result");payload["currentResult"]=json!(true);
    h.command("recording",CommandRequest{name:"app.showMain".into(),payload}).await.unwrap();
    let request=h.snapshot("main").unwrap()["mainResultRequest"].clone();
    assert_eq!(request["sessionId"],"result");assert_eq!(request["target"],"result");
    assert!(h.snapshot("recording").unwrap().get("mainResultRequest").is_none());
    command(&h,"app.ackMainResult",json!({"requestId":"older"})).await.unwrap();
    assert_eq!(h.snapshot("main").unwrap()["mainResultRequest"],request);
    command(&h,"app.ackMainResult",json!({"requestId":request["requestId"]})).await.unwrap();
    assert!(h.snapshot("main").unwrap()["mainResultRequest"].is_null());
    assert!(view(&h).delivery_state.is_none());assert_eq!(usage_count(&h),1);
    install(&h,"partial",SessionPhase::Incomplete,true);
    let mut stale=identity("result");stale["currentResult"]=json!(true);
    assert_eq!(h.command("recording",CommandRequest{name:"app.showMain".into(),payload:stale}).await.unwrap_err().code,"stale_session");
    let mut partial=identity("partial");partial["currentResult"]=json!(true);
    h.command("recording",CommandRequest{name:"app.showMain".into(),payload:partial}).await.unwrap();
    assert_eq!(h.snapshot("main").unwrap()["mainResultRequest"]["target"],"recovery");
}

#[tokio::test]
async fn caption_preference_defaults_off_and_is_versioned_in_both_window_snapshots() {
    let mut legacy=serde_json::to_value(Preferences::default()).unwrap();
    legacy.as_object_mut().unwrap().remove("recorderCaptionsEnabled");legacy["keepHistory"]=json!(false);
    let loaded:Preferences=serde_json::from_value(legacy).unwrap();
    assert!(!loaded.recorder_captions_enabled);assert!(!loaded.keep_history);
    let h=hub();let before=h.db.snapshot().preferences;
    command(&h,"preferences.update",json!({"requestId":"captions-on","expectedVersion":before.version,"patch":{"recorderCaptionsEnabled":true}})).await.unwrap();
    let after=h.db.snapshot().preferences;assert!(after.recorder_captions_enabled);
    assert_eq!(after.shortcut,before.shortcut);assert_eq!(after.keep_history,before.keep_history);assert_eq!(after.version,before.version+1);
    assert_eq!(h.snapshot("recording").unwrap()["recorderCaptionsEnabled"],true);
    assert_eq!(h.snapshot("main").unwrap()["preferences"]["recorderCaptionsEnabled"],true);
    assert_eq!(command(&h,"preferences.update",json!({"requestId":"captions-stale","expectedVersion":before.version,"patch":{"recorderCaptionsEnabled":false}})).await.unwrap_err().code,"revisionConflict");
}
