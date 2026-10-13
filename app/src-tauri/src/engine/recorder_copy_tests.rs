//! Real recorder IPC authorization and copy gate; no native clipboard writes.
use super::*;

fn denied(_: &str)->Result<()> {panic!("rejected recorder copy must never touch clipboard")}

#[tokio::test]
async fn recorder_copy_ipc_routes_only_current_identity_and_rejects_extra_selectors() {
    let h=hub();install(&h,"current",SessionPhase::FinalizingAsr,true);ready(&h,"current");
    for name in ["result.copy","recoverable.copy"] {
        let mut stale=identity("old-history");
        let error=h.command("recording",CommandRequest{name:name.into(),payload:stale.clone()}).await.unwrap_err();
        assert_eq!(error.code,"stale_session","allowed copy must reach current-session authorization");
        stale["sessionId"]=json!("current");stale["expectedGeneration"]=json!(6);stale["requestId"]=json!(Uuid::new_v4().to_string());
        assert_eq!(h.command("recording",CommandRequest{name:name.into(),payload:stale}).await.unwrap_err().code,"stale_session");
        for (field,value) in [("sourceOnly",json!(true)),("force",json!(true)),("resultId",json!("old-history")),("sourceRevisionId",json!("old-source"))] {
            let mut payload=identity("current");payload[field]=value;
            assert_eq!(h.command("recording",CommandRequest{name:name.into(),payload}).await.unwrap_err().code,"forbidden");
        }
    }
    let mut missing=identity("current");missing.as_object_mut().unwrap().remove("expectedGeneration");
    assert_eq!(h.command("recording",CommandRequest{name:"result.copy".into(),payload:missing}).await.unwrap_err().code,"invalid_payload");
    assert!(view(&h).delivery_state.is_none());assert_eq!(usage_count(&h),1);
}

#[test]
fn recorder_ready_copy_uses_current_output_without_adoption_or_delivery() {
    let h=hub();install(&h,"s",SessionPhase::FinalizingAsr,true);ready(&h,"s");
    let saved=h.db.get_result("s").unwrap();let copied=Mutex::new(String::new());
    let result=h.copy_current_recorder_with("result.copy",&identity("s"),|text|{*copied.lock().unwrap()=text.to_owned();Ok(())}).unwrap();
    assert_eq!(result,json!({"copied":true}));assert_eq!(*copied.lock().unwrap(),saved.output.as_ref().unwrap().text());
    assert_eq!(h.db.get_result("s").unwrap(),saved);assert!(view(&h).delivery_state.is_none());assert_eq!(usage_count(&h),1);
}

#[test]
fn recorder_copy_blocks_pending_delivery_even_before_a_ledger_entry_exists() {
    for prepared in [false,true] {
        let h=hub();install(&h,"s",SessionPhase::FinalizingAsr,true);ready(&h,"s");
        {let mut inner=h.inner.lock().unwrap();let s=inner.active.as_mut().unwrap();
            if prepared {s.delivery=Some(DeliveryAttempt{id:"attempt".into(),session_id:"s".into(),generation:7,output_revision_id:"output-s".into(),
                target_fingerprint:"synthetic".into(),claimed_at:"now".into(),state:DeliveryState::Prepared,reason:None});}
            else{s.delivery_worker_released=false;}
        }
        let error=h.copy_current_recorder_with("result.copy",&identity("s"),denied).unwrap_err();
        assert_eq!(error.code,if prepared{"delivery_busy"}else{"result_not_ready"});
    }
}

#[test]
fn recorder_recovery_copy_is_partial_and_never_forges_a_ready_result() {
    let h=hub();install(&h,"s",SessionPhase::Incomplete,true);
    {let mut inner=h.inner.lock().unwrap();inner.active.as_mut().unwrap().captions=vec![CaptionItem{id:"partial".into(),text:"尚未收齐的文字".into(),revision:1,is_final:false,capture_order:0}];}
    let mut payload=identity("s");payload["recoverableContentId"]=json!("s");
    let copied=Mutex::new(String::new());let answer=h.copy_current_recorder_with("recoverable.copy",&payload,|text|{*copied.lock().unwrap()=text.into();Ok(())}).unwrap();
    assert_eq!(answer,json!({"copied":true,"adopted":false}));assert_eq!(*copied.lock().unwrap(),"尚未收齐的文字");
    assert_eq!(view(&h).state,SessionPhase::Incomplete);assert_eq!(usage_count(&h),0);assert!(h.db.get_result("s").is_err());
    payload["recoverableContentId"]=json!("other");
    assert_eq!(h.copy_current_recorder_with("recoverable.copy",&payload,denied).unwrap_err().code,"stale_session");
    assert_eq!(h.copy_current_recorder_with("result.copy",&identity("s"),denied).unwrap_err().code,"result_not_ready");
}

#[test]
fn recorder_recovery_copy_respects_phase_cancellation_and_expired_audio_boundary() {
    for phase in [SessionPhase::Recording,SessionPhase::Rewriting,SessionPhase::Ready,SessionPhase::Cancelled] {
        let h=hub();install(&h,"s",phase,true);
        assert_eq!(h.copy_current_recorder_with("recoverable.copy",&identity("s"),denied).unwrap_err().code,"recovery_unavailable");
    }
    let h=hub();install(&h,"s",SessionPhase::Incomplete,true);
    {let mut inner=h.inner.lock().unwrap();let s=inner.active.as_mut().unwrap();s.source=Some(source("s"));s.cancel.cancel();}
    assert_eq!(h.copy_current_recorder_with("recoverable.copy",&identity("s"),denied).unwrap_err().code,"cancelled");
    // Audio retention expiry cancels its worker, but deliberately retains text.
    h.inner.lock().unwrap().active.as_mut().unwrap().error=Some(ApiError::new("asr_retry_expired","recovery","synthetic expiry"));
    assert_eq!(h.copy_current_recorder_with("recoverable.copy",&identity("s"),|text|{assert_eq!(text,source("s").text());Ok(())}).unwrap()["adopted"],false);
}

#[test]
fn waiting_recorder_copy_rechecks_replacement_and_cancel_after_native_lock() {
    for replace in [false,true] {
        let h=hub();install(&h,"s",SessionPhase::FinalizingAsr,true);ready(&h,"s");
        let native=h.native_io.lock().unwrap();let worker=h.clone();let (started,waiting)=std::sync::mpsc::channel();
        let task=std::thread::spawn(move||{started.send(()).unwrap();worker.copy_current_recorder_with("result.copy",&identity("s"),denied)});
        waiting.recv_timeout(Duration::from_secs(2)).unwrap();
        if replace{install(&h,"replacement",SessionPhase::Recording,true);}else{h.inner.lock().unwrap().active.as_mut().unwrap().cancel.cancel();}
        drop(native);
        assert_eq!(task.join().unwrap().unwrap_err().code,if replace{"stale_session"}else{"cancelled"});
    }
}

#[test]
fn recorder_copy_failure_does_not_retry_or_mutate_delivery() {
    let h=hub();install(&h,"s",SessionPhase::FinalizingAsr,true);ready(&h,"s");
    let calls=std::sync::atomic::AtomicUsize::new(0);
    let error=h.copy_current_recorder_with("result.copy",&identity("s"),|_|{calls.fetch_add(1,Ordering::SeqCst);Err(ApiError::new("clipboard_unavailable","native","synthetic failure"))}).unwrap_err();
    assert_eq!(error.code,"clipboard_unavailable");assert_eq!(calls.load(Ordering::SeqCst),1);assert!(view(&h).delivery_state.is_none());
    assert_eq!(h.db.query_count("SELECT COUNT(*) FROM delivery_attempts").unwrap(),0);
}
