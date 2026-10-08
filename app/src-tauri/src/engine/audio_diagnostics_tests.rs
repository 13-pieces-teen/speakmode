//! Queue admission is distinct from observation, retrying Full and socket writes.
use super::*;
use crate::core::BaseMode;

#[test]
fn full_bridge_retries_do_not_duplicate_enqueue_counts_or_claim_unadmitted_finish() {
    let diagnostics=DiagnosticsRecorder::default();let mode=ModeSnapshot{mode_id:"auto".into(),name:"excluded".into(),
        base_mode:BaseMode::Auto,rules:String::new(),revision:1};
    let trace=diagnostics.begin("bridge-test",1,Origin::UiCommand,Instant::now(),&ProviderConfig::default(),&mode).unwrap();
    let (tx,mut rx)=mpsc::channel(1);let mut bridge=BridgeTelemetry::default();
    bridge.observed.add_frame(1600,0);bridge.observed.add_frame(3,1597);
    enqueue_observed(&tx,AsrInput::Frame{sequence:0,pcm16:vec![1;1600],padding_samples:0},&diagnostics,Some(&trace),&mut bridge).unwrap();
    let mut tail=vec![0;1600];tail[..3].copy_from_slice(&[1,2,3]);
    let tail=enqueue_observed(&tx,AsrInput::Frame{sequence:1,pcm16:tail,padding_samples:1597},&diagnostics,Some(&trace),&mut bridge).unwrap_err().into_inner();
    assert_eq!(bridge.enqueued.frames,1);assert_eq!(bridge.observed.frames,2);
    rx.try_recv().unwrap();enqueue_observed(&tx,tail,&diagnostics,Some(&trace),&mut bridge).unwrap();
    let finish=enqueue_observed(&tx,AsrInput::Finish{next_sequence:2},&diagnostics,Some(&trace),&mut bridge).unwrap_err().into_inner();
    assert!(bridge.finish_enqueued_at.is_none());rx.try_recv().unwrap();
    enqueue_observed(&tx,finish,&diagnostics,Some(&trace),&mut bridge).unwrap();
    let snapshot=diagnostics.snapshot().unwrap();let view=&snapshot.traces[0];
    assert_eq!(view.bridge.enqueued,asr::AudioCounts{frames:2,real_samples:1603,padding_samples:1597});
    assert!(view.bridge.finish_enqueued_ms.is_some());assert_eq!(view.asr.socket_written.frames,0);
    assert!(view.asr.finish_written_ms.is_none());
}

#[test]
fn cancelled_bounded_tail_does_not_advance_queue_or_finish_diagnostics() {
    let diagnostics=DiagnosticsRecorder::default();let (tx,mut rx)=mpsc::channel(1);let mut bridge=BridgeTelemetry::default();
    let cancel=CancellationToken::new();cancel.cancel();
    assert_eq!(send_bounded(&tx,AsrInput::Finish{next_sequence:0},&cancel,Instant::now()+Duration::from_secs(1),
        &diagnostics,None,&mut bridge).unwrap_err().code,"cancelled");
    assert!(rx.try_recv().is_err());assert!(bridge.finish_enqueued_at.is_none());assert_eq!(bridge.enqueued.frames,0);
}
