//! Production startup admission and window acknowledgement with synthetic
//! native callbacks. No Credential Manager, microphone or cloud is accessed.
use super::*;

fn prepare(h:&AppHub)->(String,u64) {
    let mut config=h.db.snapshot().provider_config;let version=config.version;config.region=Some("beijing".into());
    h.db.run(DbCommand::SaveProviderConfig{config,expected_version:version}).unwrap();
    h.inner.lock().unwrap().key_configured=true;
    let(id,generation,_,_,_,_)=h.prepare_start_state(Instant::now(),Origin::HotkeyHook).unwrap();
    (id,generation)
}

fn assert_no_recording_work(h:&AppHub) {
    let i=h.inner.lock().unwrap();let s=i.active.as_ref().unwrap();
    assert!(s.worker_released);assert!(s.processing_worker_released);assert!(s.asr_attempt_id.is_none());
    assert!(!s.first_samples);assert!(s.source.is_none());assert!(s.output.is_none());
}

#[tokio::test]
async fn failed_recorder_ack_stops_before_pipeline_and_releases_next_start_admission() {
    for code in ["window_timeout","window_unavailable","desktop_required"] {
        let h=hub();let(id,generation)=prepare(&h);let preflight=h.processing_lease(&id,generation);
        let events=Arc::new(Mutex::new(Vec::new()));let observed=events.clone();
        h.subscribe("main",Arc::new(move|event|observed.lock().unwrap().push(event))).unwrap();
        h.set_native_actions(Arc::new(move|name,enabled| {
            assert_eq!((name,enabled),("recording",true));
            Err(ApiError::new(code,"native","录音窗口未能显示，请重试"))
        }));
        let launched=Arc::new(AtomicBool::new(false));let launch=launched.clone();
        let error=h.start_prepared_recording(&id,generation,preflight,move|_lease|launch.store(true,Ordering::SeqCst)).await.unwrap_err();
        assert_eq!(error.code,code);assert_eq!(error.session_id.as_deref(),Some(id.as_str()));assert_eq!(error.generation,Some(generation));
        assert!(!launched.load(Ordering::SeqCst));assert_no_recording_work(&h);
        let session=view(&h);assert_eq!(session.state,SessionPhase::Failed);assert_eq!(session.error_code.as_deref(),Some(code));
        assert!(session.worker_released);assert!(!session.can_retry_asr);assert!(!session.can_retry_rewrite);
        assert!(events.lock().unwrap().iter().any(|event|event.kind=="session"&&event.payload["state"]=="failed"&&event.payload["errorCode"]==code));
        assert_eq!(h.db.query_count("SELECT COUNT(*) FROM sessions").unwrap(),0);
        assert_eq!(h.db.query_count("SELECT COUNT(*) FROM usage_events").unwrap(),0);
        assert_eq!(h.db.query_count("SELECT COUNT(*) FROM delivery_attempts").unwrap(),0);
        h.prepare_start_state(Instant::now(),Origin::HotkeyHook).unwrap();
    }
}

#[tokio::test(flavor="current_thread")]
async fn slow_recorder_ack_yields_executor_and_precedes_pipeline_launch() {
    let h=hub();let(id,generation)=prepare(&h);let preflight=h.processing_lease(&id,generation);
    let(entered,seen)=tokio::sync::oneshot::channel();let entered=Mutex::new(Some(entered));
    let(release,wait)=std::sync::mpsc::channel();let wait=Mutex::new(wait);
    let acknowledged=Arc::new(AtomicBool::new(false));let ack=acknowledged.clone();
    h.set_native_actions(Arc::new(move|name,enabled| {
        assert_eq!((name,enabled),("recording",true));entered.lock().unwrap().take().unwrap().send(()).unwrap();
        wait.lock().unwrap().recv_timeout(Duration::from_secs(5)).expect("the Tokio executor must run while the UI waits");
        ack.store(true,Ordering::SeqCst);Ok(())
    }));
    let launched=Arc::new(AtomicBool::new(false));let launch=launched.clone();let owner=h.clone();let job_id=id.clone();
    let task=tokio::spawn(async move {owner.start_prepared_recording(&job_id,generation,preflight,move|_lease| {
        assert!(acknowledged.load(Ordering::SeqCst));launch.store(true,Ordering::SeqCst);
    }).await});
    seen.await.unwrap();tokio::task::yield_now().await;
    assert!(!task.is_finished());assert!(!launched.load(Ordering::SeqCst));
    assert!(!view(&h).worker_released,"startup lease must still close admission while the window waits");
    release.send(()).unwrap();task.await.unwrap().unwrap();assert!(launched.load(Ordering::SeqCst));
}

#[tokio::test(flavor="current_thread")]
async fn cancel_or_exit_during_recorder_ack_hides_late_surface_without_launching_pipeline() {
    for interruption in ["cancel","quitting","exitReserved"] {
        let h=hub();let(id,generation)=prepare(&h);let preflight=h.processing_lease(&id,generation);
        let(entered,seen)=tokio::sync::oneshot::channel();let entered=Mutex::new(Some(entered));
        let(release,wait)=std::sync::mpsc::channel();let wait=Mutex::new(wait);
        let calls=Arc::new(Mutex::new(Vec::new()));let recorded=calls.clone();
        h.set_native_actions(Arc::new(move|name,enabled| {
            assert_eq!(name,"recording");recorded.lock().unwrap().push(enabled);
            if enabled {entered.lock().unwrap().take().unwrap().send(()).unwrap();wait.lock().unwrap().recv_timeout(Duration::from_secs(5)).unwrap();}
            Ok(())
        }));
        let launched=Arc::new(AtomicBool::new(false));let launch=launched.clone();let owner=h.clone();let job_id=id.clone();
        let task=tokio::spawn(async move {owner.start_prepared_recording(&job_id,generation,preflight,move|_lease|launch.store(true,Ordering::SeqCst)).await});
        seen.await.unwrap();
        match interruption {
            "quitting"=>{h.quitting.store(true,Ordering::SeqCst);h.capture_available.store(false,Ordering::SeqCst);},
            "exitReserved"=>{h.reserve_lifecycle_epoch();assert!(!h.quitting.load(Ordering::SeqCst));},
            _=>{h.cancel_session_async(&json!({"sessionId":id,"expectedGeneration":generation})).await.unwrap();}
        }
        assert!(!view(&h).worker_released);assert!(!task.is_finished());assert!(!launched.load(Ordering::SeqCst));
        release.send(()).unwrap();let error=task.await.unwrap().unwrap_err();assert_eq!(error.code,"cancelled");
        assert!(!launched.load(Ordering::SeqCst));assert_no_recording_work(&h);assert_eq!(*calls.lock().unwrap(),vec![true,false]);
        if interruption!="cancel" {
            assert_eq!(h.prepare_start_state(Instant::now(),Origin::HotkeyHook).unwrap_err().code,"system_unavailable");
            h.quitting.store(false,Ordering::SeqCst);h.capture_available.store(true,Ordering::SeqCst);
        }else {assert_eq!(view(&h).state,SessionPhase::Cancelled);}
        h.prepare_start_state(Instant::now(),Origin::HotkeyHook).unwrap();
    }
}

#[tokio::test(flavor="current_thread")]
async fn dropped_start_reply_keeps_native_owner_until_cancelled_surface_is_hidden() {
    let h=hub();let(id,generation)=prepare(&h);let preflight=h.processing_lease(&id,generation);
    let(entered,seen)=tokio::sync::oneshot::channel();let entered=Mutex::new(Some(entered));
    let(release,wait)=std::sync::mpsc::channel();let wait=Mutex::new(wait);
    let calls=Arc::new(Mutex::new(Vec::new()));let recorded=calls.clone();
    h.set_native_actions(Arc::new(move|name,enabled| {
        assert_eq!(name,"recording");recorded.lock().unwrap().push(enabled);
        if enabled {entered.lock().unwrap().take().unwrap().send(()).unwrap();wait.lock().unwrap().recv_timeout(Duration::from_secs(5)).unwrap();}
        Ok(())
    }));
    let launched=Arc::new(AtomicBool::new(false));let launch=launched.clone();let owner=h.clone();let job_id=id.clone();
    let reply=tokio::spawn(async move {owner.start_prepared_recording(&job_id,generation,preflight,move|_lease|launch.store(true,Ordering::SeqCst)).await});
    seen.await.unwrap();h.cancel_session_async(&json!({"sessionId":id,"expectedGeneration":generation})).await.unwrap();
    reply.abort();assert!(reply.await.unwrap_err().is_cancelled());
    assert!(!view(&h).worker_released);assert_eq!(h.prepare_start_state(Instant::now(),Origin::HotkeyHook).unwrap_err().code,"session_busy");
    release.send(()).unwrap();wait_until(||view(&h).worker_released).await;
    assert!(!launched.load(Ordering::SeqCst));assert_no_recording_work(&h);assert_eq!(*calls.lock().unwrap(),vec![true,false]);
    h.prepare_start_state(Instant::now(),Origin::HotkeyHook).unwrap();
}
