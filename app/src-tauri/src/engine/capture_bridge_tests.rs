use super::*;
use super::super::capture_bridge::{CaptureSource,FrameLimit};
use crossbeam_channel::{bounded,Receiver};
use std::sync::atomic::AtomicUsize;

struct FixtureCapture {frames:Receiver<AudioFrame>,events:Receiver<AudioEvent>,summary:Option<CaptureSummary>,stopped:Arc<AtomicUsize>,cancelled:Arc<AtomicUsize>,duration:u64}
impl CaptureSource for FixtureCapture {
    fn frames(&self)->&Receiver<AudioFrame>{&self.frames}
    fn events(&self)->&Receiver<AudioEvent>{&self.events}
    fn duration_ms(&self)->u64{self.duration}
    fn request_stop(&self){self.stopped.fetch_add(1,Ordering::SeqCst);}
    fn cancel(&self){self.cancelled.fetch_add(1,Ordering::SeqCst);}
    fn wait_drained(&mut self,_:Duration)->platform::Result<CaptureSummary>{let mut summary=self.summary.take().expect("one final summary");if self.cancelled.load(Ordering::SeqCst)>0{summary.complete=false;summary.cancelled=true;}Ok(summary)}
}
fn fixture(complete:bool)->FixtureCapture{
    let (tx,frames)=bounded(2);for sequence in 0..2{tx.send(AudioFrame{sequence,pcm16:vec![123;1600],padding_samples:0}).unwrap();}drop(tx);
    let (tx,events)=bounded(32);for _ in 0..32{tx.try_send(AudioEvent::Level(0.1)).unwrap();}
    assert!(tx.try_send(AudioEvent::Error("lost terminal event".into())).is_err());drop(tx);
    FixtureCapture{frames,events,stopped:Arc::new(AtomicUsize::new(0)),cancelled:Arc::new(AtomicUsize::new(0)),duration:200,
        summary:Some(CaptureSummary{complete,cancelled:false,captured_samples:3200,input_sample_rate:16000,emitted_frames:2,first_samples_received:true,error:(!complete).then(||platform::audio::PCM_STALL_ERROR.into()),conditioning:None})}
}

#[tokio::test]
async fn product_cutoff_drains_finish_and_preserves_batch_audio_then_previews_result(){
    let h=hub();install(&h,"s",SessionPhase::Recording,false);
    {let mut i=h.inner.lock().unwrap();let s=i.active.as_mut().unwrap();s.recording_limit_ms=100;s.config.provider="google".into();}
    let capture=fixture(true);let stopped=capture.stopped.clone();let cancelled=capture.cancelled.clone();
    let (tx,mut rx)=mpsc::channel(4);let (cancel,stop)={let i=h.inner.lock().unwrap();let s=i.active.as_ref().unwrap();(s.cancel.clone(),s.stop.clone())};
    let (summary,pcm)=pump_capture(&h,"s",7,tx,cancel,stop,capture).unwrap();
    assert!(summary.complete);assert_eq!(pcm,vec![123;1600]);assert!(stopped.load(Ordering::SeqCst)>0);assert_eq!(cancelled.load(Ordering::SeqCst),0);
    assert!(matches!(rx.try_recv(),Ok(AsrInput::Frame{sequence:0,..})));assert!(matches!(rx.try_recv(),Ok(AsrInput::Finish{next_sequence:1})));assert!(rx.try_recv().is_err());
    h.accept_capture_cache("s",7,&summary,pcm,Instant::now());assert_eq!(h.inner.lock().unwrap().active.as_ref().unwrap().cache.as_ref().unwrap().len(),1600);
    checkpoint(&h,"s");h.finish_rewrite_with_policy("s".into(),7,|_,_,source,_,_|async move{Ok(super::rewrite_fallback::source_fallback(&source))},None::<RewriteWaitPolicy>,|h,id,generation|h.initial_delivery_with(id,generation,|_|panic!("cutoff cannot target input"),|_,_,_,_|panic!("cutoff cannot inject"))).await;
    assert_eq!(view(&h).state,SessionPhase::Ready);assert!(h.db.get_result("s").unwrap().output.is_some());assert_eq!(h.db.query_count("SELECT COUNT(*) FROM delivery_attempts").unwrap(),0);
}

#[tokio::test]
async fn manual_stop_at_limit_and_late_drain_tail_still_finish_only_as_preview(){
    // Stop intent wins the progress poll; the final native summary must still
    // settle cutoff, including samples accepted after the last heartbeat.
    for heartbeat_ms in [99,200] {
        let h=hub();install(&h,"s",SessionPhase::Recording,false);
        {let mut i=h.inner.lock().unwrap();let s=i.active.as_mut().unwrap();s.recording_limit_ms=100;}
        h.begin_stop_target(&identity("s"),Instant::now()).unwrap();
        let (cancel,stop,ended,permit)={let mut i=h.inner.lock().unwrap();let s=i.active.as_mut().unwrap();
            s.target=Some(delivery_target_fixture());s.target_application=Some("outlook.exe".into());
            s.target_capture_pending=false;
            let permit=Arc::new(platform::delivery::DeliveryPermit::from_committed_attempt("cutoff-permit".into(),7));s.delivery_permit=Some(permit.clone());
            assert_eq!(s.prefs.delivery_mode,DeliveryMode::Direct);assert!(s.stop.load(Ordering::SeqCst));
            (s.cancel.clone(),s.stop.clone(),s.ended_at.clone(),permit)};
        let mut capture=fixture(true);capture.duration=heartbeat_ms;
        let (tx,mut rx)=mpsc::channel(4);
        let (summary,pcm)=pump_capture(&h,"s",7,tx,cancel,stop,capture).unwrap();
        assert!(summary.complete);assert_eq!(pcm,vec![123;1600]);
        assert!(matches!(rx.try_recv(),Ok(AsrInput::Frame{sequence:0,..})));
        assert!(matches!(rx.try_recv(),Ok(AsrInput::Finish{next_sequence:1})));
        {let inner=h.inner.lock().unwrap();let s=inner.active.as_ref().unwrap();
            assert_eq!(s.prefs.delivery_mode,DeliveryMode::Preview);assert!(s.original_delivery_revoked);
            assert!(s.target.is_none()&&s.target_application.is_none());assert!(s.input_context_frozen);
            assert_eq!(s.recording_limit_remaining_ms,Some(0));assert_eq!(s.ended_at,ended);assert!(permit.consumed());}
        h.accept_capture_cache("s",7,&summary,pcm,Instant::now());checkpoint(&h,"s");
        h.finish_rewrite_with_policy("s".into(),7,|_,_,source,_,_|async move{Ok(super::rewrite_fallback::source_fallback(&source))},None::<RewriteWaitPolicy>,|h,id,generation|h.initial_delivery_with(id,generation,|_|panic!("manual stop at limit cannot validate a paste target"),|_,_,_,_|panic!("trimmed source cannot automatically paste"))).await;
        assert_eq!(view(&h).state,SessionPhase::Ready);assert_eq!(h.db.query_count("SELECT COUNT(*) FROM delivery_attempts").unwrap(),0);
    }
}

#[test]
fn final_capture_below_limit_keeps_direct_intent_and_full_tail(){
    let h=hub();install(&h,"s",SessionPhase::Recording,false);
    {let mut i=h.inner.lock().unwrap();i.active.as_mut().unwrap().recording_limit_ms=100;}
    h.begin_stop_target(&identity("s"),Instant::now()).unwrap();
    let (cancel,stop,target)={let mut i=h.inner.lock().unwrap();let s=i.active.as_mut().unwrap();
        s.target=Some(delivery_target_fixture());s.target_capture_pending=false;
        (s.cancel.clone(),s.stop.clone(),s.target.clone())};
    let mut capture=fixture(true);capture.duration=99;
    let (tx,frames)=bounded(1);let mut pcm=vec![123;1600];pcm[1599]=0;
    tx.send(AudioFrame{sequence:0,pcm16:pcm,padding_samples:1}).unwrap();drop(tx);capture.frames=frames;
    capture.summary=Some(CaptureSummary{complete:true,cancelled:false,captured_samples:1599,input_sample_rate:16000,emitted_frames:1,first_samples_received:true,error:None,conditioning:None});
    let (tx,mut rx)=mpsc::channel(4);let(summary,cache)=pump_capture(&h,"s",7,tx,cancel,stop,capture).unwrap();
    assert!(summary.complete);assert_eq!(cache.len(),1599);
    assert!(matches!(rx.try_recv(),Ok(AsrInput::Frame{sequence:0,padding_samples:1,..})));
    assert!(matches!(rx.try_recv(),Ok(AsrInput::Finish{next_sequence:1})));
    let i=h.inner.lock().unwrap();let s=i.active.as_ref().unwrap();assert_eq!(s.prefs.delivery_mode,DeliveryMode::Direct);assert!(!s.original_delivery_revoked);assert_eq!(s.target,target);assert_ne!(s.recording_limit_remaining_ms,Some(0));
}

#[test]
fn final_cutoff_cannot_change_a_cancelled_or_replaced_session(){
    let summary=CaptureSummary{complete:true,cancelled:false,captured_samples:3200,input_sample_rate:16000,emitted_frames:2,first_samples_received:true,error:None,conditioning:None};
    for (id,generation,cancelled) in [("s",7,true),("s",6,false),("other",7,false)] {
        let h=hub();install(&h,"s",SessionPhase::FinalizingAsr,true);
        {let mut i=h.inner.lock().unwrap();let s=i.active.as_mut().unwrap();s.recording_limit_ms=100;if cancelled{s.cancel.cancel();}}
        h.finalize_capture_limit(id,generation,&summary,true);
        let i=h.inner.lock().unwrap();let s=i.active.as_ref().unwrap();assert_eq!(s.prefs.delivery_mode,DeliveryMode::Direct);assert!(!s.original_delivery_revoked);assert_ne!(s.recording_limit_remaining_ms,Some(0));
    }
}

#[tokio::test]
async fn cutoff_and_manual_preview_can_recover_without_input_permission(){
    let h=hub();install(&h,"s",SessionPhase::Recording,true);
    assert!(h.recording_progress("s",7,RECORDING_LIMIT_MS));checkpoint(&h,"s");
    h.finish_rewrite_with_recovery_policy("s".into(),7,|_,_,_,_,_,_|async{Err(CloudError::new("rewrite_network","rewrite","transient failure"))},|_,_,source,_,_,_|async move{Ok(super::rewrite_fallback::source_fallback(&source))},None::<RewriteWaitPolicy>,|h,id,generation|h.initial_delivery_with(id,generation,|_|panic!("preview cannot target input"),|_,_,_,_|panic!("preview cannot inject"))).await;
    assert_eq!(view(&h).state,SessionPhase::Ready);assert!(view(&h).rewrite_recovery.is_some());assert_eq!(h.db.query_count("SELECT COUNT(*) FROM delivery_attempts").unwrap(),0);
}

#[test]
fn disconnected_frames_collect_error_summary_even_when_terminal_event_was_lost(){
    let h=hub();install(&h,"s",SessionPhase::Recording,false);let capture=fixture(false);
    let (tx,mut rx)=mpsc::channel(4);
    let (summary,cache)=pump_capture(&h,"s",7,tx,CancellationToken::new(),Arc::new(AtomicBool::new(false)),capture).unwrap();
    assert_eq!(capture_failure(&summary).unwrap().code,"audio_capture_stalled");assert_eq!(cache.len(),3200);
    while let Ok(input)=rx.try_recv(){assert!(matches!(input,AsrInput::Frame{..}));}
}

#[tokio::test]
async fn pending_frame_backpressure_preserves_each_frame_and_exact_finish(){
    let h=hub();install(&h,"s",SessionPhase::Recording,false);let capture=fixture(true);
    let (tx,mut rx)=mpsc::channel(1);let worker=h.clone();
    let task=tokio::task::spawn_blocking(move||pump_capture(&worker,"s",7,tx,CancellationToken::new(),Arc::new(AtomicBool::new(false)),capture));
    tokio::time::sleep(Duration::from_millis(20)).await;
    for expected in 0..2{assert!(matches!(rx.recv().await,Some(AsrInput::Frame{sequence,..}) if sequence==expected));}
    assert!(matches!(rx.recv().await,Some(AsrInput::Finish{next_sequence:2})));
    let (summary,cache)=task.await.unwrap().unwrap();assert!(summary.complete);assert_eq!(cache.len(),3200);
}

#[tokio::test]
async fn cancellation_while_backpressured_never_sends_finish(){
    let h=hub();install(&h,"s",SessionPhase::Recording,false);let capture=fixture(true);
    let (tx,mut rx)=mpsc::channel(1);let cancel=CancellationToken::new();let stopping=cancel.clone();let worker=h.clone();
    let task=tokio::task::spawn_blocking(move||pump_capture(&worker,"s",7,tx,stopping,Arc::new(AtomicBool::new(false)),capture));
    tokio::time::sleep(Duration::from_millis(20)).await;cancel.cancel();
    let (summary,_)=tokio::time::timeout(Duration::from_secs(2),task).await.unwrap().unwrap().unwrap();assert!(summary.cancelled&&!summary.complete);
    while let Ok(input)=rx.try_recv(){assert!(matches!(input,AsrInput::Frame{..}));}
}

#[test]
fn cutoff_checks_native_order_even_after_the_product_sample_budget(){
    let mut frames=FrameLimit::new(100);
    assert!(frames.take(AudioFrame{sequence:0,pcm16:vec![1;1600],padding_samples:0}).unwrap().is_some());
    assert!(frames.take(AudioFrame{sequence:2,pcm16:vec![1;1600],padding_samples:0}).is_err());
    let mut frames=FrameLimit::new(101);frames.take(AudioFrame{sequence:0,pcm16:vec![1;1600],padding_samples:0}).unwrap();
    let tail=frames.take(AudioFrame{sequence:1,pcm16:vec![2;1600],padding_samples:0}).unwrap().unwrap();
    assert_eq!(tail.padding_samples,1584);assert!(tail.pcm16[16..].iter().all(|v|*v==0));assert!(frames.finish(3).is_err());
}

#[tokio::test]
async fn permanent_cloud_errors_never_start_same_account_recovery(){
    for (code,stage) in [("authentication_failed","rewrite"),("rate_limited","rewrite"),("rewrite_request_rejected","rewrite"),("rewrite_refused","rewrite"),("network_setup","configuration"),("key_required","credentials"),("rewrite_source_too_long","rewrite_capacity")] {
        let h=hub();install(&h,"s",SessionPhase::FinalizingAsr,true);checkpoint(&h,"s");
        h.finish_rewrite_with_recovery_policy("s".into(),7,move|_,_,_,_,_,_|async move{Err(CloudError::new(code,stage,"synthetic permanent failure"))},|_,_,_,_,_,_|async{panic!("permanent error must not retry")},None::<RewriteWaitPolicy>,|_,_,_|panic!("failed output cannot deliver")).await;
        assert_eq!(view(&h).error_code.as_deref(),Some(code));assert!(view(&h).can_retry_rewrite);assert!(h.db.get_result("s").unwrap().output.is_none());
    }
}

#[test]
fn rewrite_stage_receipts_require_current_rewriting_identity(){
    let h=hub();install(&h,"s",SessionPhase::Rewriting,true);
    h.accept_rewrite_stage("s",7,rewrite::execution::RequestStage::Review);assert_eq!(view(&h).processing_stage,Some("review"));
    h.accept_rewrite_stage("s",6,rewrite::execution::RequestStage::Repair);assert_eq!(view(&h).processing_stage,Some("review"));
    h.inner.lock().unwrap().active.as_mut().unwrap().processing_stage=Some("recoveryGeneration");
    h.accept_rewrite_stage("s",7,rewrite::execution::RequestStage::FinalReview);assert_eq!(view(&h).processing_stage,Some("recoveryFinalReview"));
}
