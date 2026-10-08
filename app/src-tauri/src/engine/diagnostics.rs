//! Bounded, metadata-only diagnostics. Capture never waits for this recorder.
//! No disk IO, transcript, PCM, prompt, credentials or host text enters it.
use crate::core::{ModeSnapshot, ProviderConfig};
use crate::cloud::asr::{AudioCounts,AsrTelemetry,AsrTelemetryView};
use serde::Serialize;
use std::{collections::VecDeque, sync::{Mutex, atomic::{AtomicU64, Ordering}}, time::Instant};
use uuid::Uuid;

const MAX_TRACES: usize = 64;
pub const TIMING_SCOPE:&str="Rust monotonic offsets; socketWritten is a client write, not provider ACK; capture input samples use the native sample rate; bridge/asr samples use 16 kHz; UI render receipt is a proxy, not optical visibility or host confirmation";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all="camelCase")]
pub enum Stage {
    IntentReceived, CaptureFirstAccepted, ProviderReadyReceived,
    ProviderSpeechStartedReceived, FirstCaptionReceived, CaptionRenderReceiptProxy,
    StopIntentReceived, CaptureCutoff, CaptureDrained, FinalAsrReceived,
    SourceCheckpointCommitted, RewriteStarted, RewriteValidated, ReadyCommitted,
    DeliveryClaim, NativeDeliveryReceipt, CancelIntent, ProcessingWorkerReleased,
    AsrFinishWritten, AsrSessionFinishedReceived,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all="camelCase")]
pub enum Origin { UiCommand, HotkeyHook, AsrRetry, RewriteRetry, SaveRetry }

#[derive(Clone)]
pub struct TraceHandle { pub run_id: String }

#[derive(Clone, Serialize)]
#[serde(rename_all="camelCase")]
pub struct Mark { pub stage: Stage, pub offset_ms: f64 }

#[derive(Debug,Clone,Copy,Default)]
pub struct BridgeTelemetry {
    pub observed:AudioCounts,pub enqueued:AudioCounts,
    pub last_frame_enqueued_at:Option<Instant>,pub finish_enqueued_at:Option<Instant>,
}
impl BridgeTelemetry {
    pub fn accepted(&mut self,counts:Option<(usize,usize)>) {
        if let Some((real,padding))=counts {self.enqueued.add_frame(real,padding);self.last_frame_enqueued_at=Some(Instant::now());}
        else {self.finish_enqueued_at=Some(Instant::now());}
    }
}
#[derive(Clone,Default,Serialize)]
#[serde(rename_all="camelCase")]
pub struct BridgeView {
    pub observed:AudioCounts,pub enqueued:AudioCounts,
    pub last_frame_enqueued_ms:Option<f64>,pub finish_enqueued_ms:Option<f64>,
}
#[derive(Clone,Serialize)]
#[serde(rename_all="camelCase")]
pub struct CaptureView {
    pub input_samples:u64,pub input_sample_rate:u32,pub emitted_frames:u64,
    pub complete:bool,pub cancelled:bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all="camelCase")]
pub struct TraceView {
    pub run_id: String, pub session_id: String, pub generation: u64,
    pub origin: Origin, pub started_at: String, pub provider: String,
    pub region: Option<String>, pub asr_model: String, pub rewrite_model: String,
    pub mode_id: String, pub mode_revision: u64, pub asr_attempt_id: Option<String>,
    pub capture_duration_ms: Option<u64>, pub source_char_count: Option<usize>,
    pub bridge_queue_peak_frames: usize, pub outcome: Option<&'static str>,
    pub error_code: Option<String>, pub marks: Vec<Mark>, pub durations: Durations,
    pub capture:Option<CaptureView>,pub bridge:BridgeView,pub asr:AsrTelemetryView,
}
#[derive(Default, Clone, Serialize)]
#[serde(rename_all="camelCase")]
pub struct Durations {
    pub intent_to_capture_ms: Option<f64>, pub intent_to_first_caption_ms: Option<f64>,
    pub caption_to_render_receipt_proxy_ms: Option<f64>, pub stop_to_final_asr_ms: Option<f64>,
    pub rewrite_round_trip_ms: Option<f64>, pub stop_to_ready_ms: Option<f64>,
    pub stop_to_finish_written_ms:Option<f64>,pub finish_written_to_session_finished_ms:Option<f64>,
}
struct Trace { origin: Instant, view: TraceView }
#[derive(Default)]
struct Store { traces: VecDeque<Trace>, evicted: u64 }
#[derive(Default)]
pub struct DiagnosticsRecorder { store: Mutex<Store>, dropped: AtomicU64 }
#[derive(Serialize)]
#[serde(rename_all="camelCase")]
pub struct Snapshot {
    pub schema_version: u8, pub app_version: &'static str, pub in_memory_only: bool,
    pub timing_scope: &'static str, pub dropped_events: u64, pub evicted_traces: u64,
    pub traces: Vec<TraceView>,
}
impl DiagnosticsRecorder {
    pub fn begin(&self, session_id: &str, generation: u64, origin: Origin, at: Instant,
                 config: &ProviderConfig, mode: &ModeSnapshot) -> Option<TraceHandle> {
        let Ok(mut store)=self.store.try_lock() else {self.dropped.fetch_add(1,Ordering::Relaxed);return None;};
        let run_id=Uuid::new_v4().to_string();
        let view=TraceView {run_id:run_id.clone(),session_id:session_id.into(),generation,origin,
            started_at:chrono::Utc::now().to_rfc3339(),provider:config.provider.clone(),region:config.region.clone(),
            asr_model:config.asr_model.clone(),rewrite_model:config.rewrite_model.clone(),mode_id:mode.mode_id.clone(),
            mode_revision:mode.revision,asr_attempt_id:None,capture_duration_ms:None,source_char_count:None,
            bridge_queue_peak_frames:0,outcome:None,error_code:None,
            capture:None,bridge:BridgeView::default(),asr:AsrTelemetryView::default(),
            marks:vec![Mark{stage:Stage::IntentReceived,offset_ms:0.0}],durations:Durations::default()};
        if store.traces.len()==MAX_TRACES {store.traces.pop_front();store.evicted+=1;}
        store.traces.push_back(Trace{origin:at,view});
        Some(TraceHandle{run_id})
    }
    fn update(&self, handle: Option<&TraceHandle>, f: impl FnOnce(&mut Trace)) {
        let Some(handle)=handle else{return;};
        let Ok(mut store)=self.store.try_lock() else {self.dropped.fetch_add(1,Ordering::Relaxed);return;};
        if let Some(trace)=store.traces.iter_mut().find(|trace|trace.view.run_id==handle.run_id){f(trace);}
    }
    pub fn mark(&self, handle: Option<&TraceHandle>, stage: Stage) {self.mark_at(handle,stage,Instant::now());}
    pub fn mark_at(&self, handle: Option<&TraceHandle>, stage: Stage, at: Instant) {
        self.update(handle,|trace|{
            if trace.view.marks.iter().any(|mark|mark.stage==stage){return;}
            let Some(elapsed)=at.checked_duration_since(trace.origin) else {self.dropped.fetch_add(1,Ordering::Relaxed);return;};
            trace.view.marks.push(Mark{stage,offset_ms:elapsed.as_secs_f64()*1000.0});
        });
    }
    pub fn attempt(&self, handle: Option<&TraceHandle>, attempt: &str) {
        self.update(handle,|trace|trace.view.asr_attempt_id=Some(attempt.into()));
    }
    pub fn mode(&self,handle:Option<&TraceHandle>,mode:&ModeSnapshot){
        self.update(handle,|trace|{trace.view.mode_id=mode.mode_id.clone();trace.view.mode_revision=mode.revision;});
    }
    pub fn lengths(&self, handle: Option<&TraceHandle>, duration_ms: u64, chars: Option<usize>) {
        self.update(handle,|trace|{trace.view.capture_duration_ms=Some(duration_ms);if chars.is_some(){trace.view.source_char_count=chars;}});
    }
    pub fn bridge_peak(&self, handle: Option<&TraceHandle>, frames: usize) {
        self.update(handle,|trace|trace.view.bridge_queue_peak_frames=trace.view.bridge_queue_peak_frames.max(frames));
    }
    pub fn capture(&self,handle:Option<&TraceHandle>,input_samples:u64,input_sample_rate:u32,emitted_frames:u64,complete:bool,cancelled:bool) {
        self.update(handle,|trace|trace.view.capture=Some(CaptureView{input_samples,input_sample_rate,emitted_frames,complete,cancelled}));
    }
    /// Cumulative snapshots recover counts after a dropped diagnostic update.
    pub fn bridge(&self,handle:Option<&TraceHandle>,metadata:&BridgeTelemetry) {
        self.update(handle,|trace|{
            let offset=|at:Instant|at.checked_duration_since(trace.origin).map(|time|time.as_secs_f64()*1000.0);
            trace.view.bridge=BridgeView{observed:metadata.observed,enqueued:metadata.enqueued,
                last_frame_enqueued_ms:metadata.last_frame_enqueued_at.and_then(offset),finish_enqueued_ms:metadata.finish_enqueued_at.and_then(offset)};
        });
    }
    pub fn asr(&self,handle:Option<&TraceHandle>,metadata:&AsrTelemetry) {
        self.update(handle,|trace|{
            trace.view.asr=metadata.relative_to(trace.origin);
            for (stage,at) in [(Stage::AsrFinishWritten,metadata.finish_written_at),
                (Stage::AsrSessionFinishedReceived,metadata.session_finished_received_at)] {
                if trace.view.marks.iter().any(|mark|mark.stage==stage){continue;}
                if let Some(offset)=at.and_then(|at|at.checked_duration_since(trace.origin)) {
                    trace.view.marks.push(Mark{stage,offset_ms:offset.as_secs_f64()*1000.0});
                }
            }
        });
    }
    pub fn outcome(&self, handle: Option<&TraceHandle>, outcome: &'static str, code: Option<&str>) {
        self.update(handle,|trace|{
            trace.view.outcome=Some(outcome);
            trace.view.error_code=code.filter(|code|code.len()<=80&&code.bytes().all(|c|c.is_ascii_alphanumeric()||c==b'_'||c==b'-')).map(str::to_owned);
        });
    }
    pub fn snapshot(&self) -> Result<Snapshot, ()> {
        let store=self.store.lock().map_err(|_|())?;
        let traces=store.traces.iter().map(|trace|{
            let mut view=trace.view.clone();
            let time=|stage|view.marks.iter().find(|mark|mark.stage==stage).map(|mark|mark.offset_ms);
            let delta=|start,end|time(start).zip(time(end)).and_then(|(a,b)|(b>=a).then_some(b-a));
            view.durations=Durations {intent_to_capture_ms:time(Stage::CaptureFirstAccepted),
                intent_to_first_caption_ms:time(Stage::FirstCaptionReceived),
                caption_to_render_receipt_proxy_ms:delta(Stage::FirstCaptionReceived,Stage::CaptionRenderReceiptProxy),
                stop_to_final_asr_ms:delta(Stage::StopIntentReceived,Stage::FinalAsrReceived),
                rewrite_round_trip_ms:delta(Stage::RewriteStarted,Stage::RewriteValidated),
                stop_to_ready_ms:delta(Stage::StopIntentReceived,Stage::ReadyCommitted),
                stop_to_finish_written_ms:delta(Stage::StopIntentReceived,Stage::AsrFinishWritten),
                finish_written_to_session_finished_ms:delta(Stage::AsrFinishWritten,Stage::AsrSessionFinishedReceived)};
            view
        }).collect();
        Ok(Snapshot{schema_version:2,app_version:env!("CARGO_PKG_VERSION"),in_memory_only:true,
            timing_scope:TIMING_SCOPE,
            dropped_events:self.dropped.load(Ordering::Relaxed),evicted_traces:store.evicted,traces})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::BaseMode;
    use std::time::Duration;
    fn begin(recorder:&DiagnosticsRecorder,at:Instant)->TraceHandle {
        let config=ProviderConfig::default();
        let mode=ModeSnapshot{mode_id:"auto".into(),name:"private name excluded".into(),base_mode:BaseMode::Auto,rules:"private prompt excluded".into(),revision:1};
        recorder.begin("session-a",7,Origin::UiCommand,at,&config,&mode).unwrap()
    }
    #[test] fn offsets_do_not_mix_clocks_and_duplicate_stop_does_not_reset_wait() {
        let r=DiagnosticsRecorder::default();let at=Instant::now();let h=begin(&r,at);
        r.mark_at(Some(&h),Stage::StopIntentReceived,at+Duration::from_secs(1));
        r.mark_at(Some(&h),Stage::StopIntentReceived,at+Duration::from_secs(4));
        r.mark_at(Some(&h),Stage::ReadyCommitted,at+Duration::from_secs(3));
        let s=r.snapshot().unwrap();assert_eq!(s.traces[0].durations.stop_to_ready_ms,Some(2000.0));
        assert!(s.traces[0].durations.intent_to_capture_ms.is_none());
    }
    #[test] fn trace_is_bounded_and_old_handle_cannot_change_a_new_run() {
        let r=DiagnosticsRecorder::default();let at=Instant::now();let old=begin(&r,at);
        for _ in 0..MAX_TRACES {begin(&r,at);}
        r.mark(Some(&old),Stage::ReadyCommitted);
        let s=r.snapshot().unwrap();assert_eq!(s.traces.len(),MAX_TRACES);assert_eq!(s.evicted_traces,1);
        assert!(s.traces.iter().all(|trace|trace.marks.len()==1));
    }
    #[test] fn contention_and_invalid_timestamp_do_not_block_or_fail_business() {
        let r=DiagnosticsRecorder::default();let at=Instant::now();let h=begin(&r,at);
        let lock=r.store.lock().unwrap();r.mark(Some(&h),Stage::FirstCaptionReceived);drop(lock);
        r.mark_at(Some(&h),Stage::CaptureFirstAccepted,at-Duration::from_millis(1));
        let s=r.snapshot().unwrap();assert_eq!(s.dropped_events,2);assert_eq!(s.traces[0].marks.len(),1);
    }
    #[test] fn serialized_metadata_excludes_mode_name_rules_and_error_messages() {
        let r=DiagnosticsRecorder::default();let h=begin(&r,Instant::now());
        r.outcome(Some(&h),"failed",Some("Bearer secret / transcript"));
        let json=serde_json::to_string(&r.snapshot().unwrap()).unwrap();
        assert!(!json.contains("private"));assert!(!json.contains("Bearer"));
        assert!(!json.contains("transcript"));assert!(!json.contains("workspace"));
    }
    #[test] fn queue_finish_and_socket_finish_are_separate_from_final_completeness() {
        let r=DiagnosticsRecorder::default();let at=Instant::now();let h=begin(&r,at);
        r.mark_at(Some(&h),Stage::StopIntentReceived,at+Duration::from_secs(1));
        let bridge=BridgeTelemetry{observed:AudioCounts{frames:2,real_samples:1603,padding_samples:1597},
            enqueued:AudioCounts{frames:2,real_samples:1603,padding_samples:1597},
            last_frame_enqueued_at:Some(at+Duration::from_secs(2)),finish_enqueued_at:Some(at+Duration::from_secs(2))};
        r.bridge(Some(&h),&bridge);r.capture(Some(&h),4809,48_000,2,true,false);
        let wire=AsrTelemetry{dequeued:bridge.enqueued,socket_written:bridge.enqueued,
            finish_dequeued_at:Some(at+Duration::from_secs(4)),finish_expected_frames:Some(2),
            last_frame_written_at:Some(at+Duration::from_secs(4)),finish_written_at:Some(at+Duration::from_secs(5)),
            session_finished_received_at:Some(at+Duration::from_secs(7)),pending_items:1,final_items:1,terminated:true,..Default::default()};
        r.asr(Some(&h),&wire);let snapshot=r.snapshot().unwrap();let trace=&snapshot.traces[0];
        assert_eq!(snapshot.schema_version,2);assert_eq!(trace.capture.as_ref().unwrap().input_sample_rate,48_000);
        assert_eq!(trace.bridge.finish_enqueued_ms,Some(2000.0));assert_eq!(trace.asr.finish_written_ms,Some(5000.0));
        assert_eq!(trace.durations.stop_to_finish_written_ms,Some(4000.0));
        assert_eq!(trace.durations.finish_written_to_session_finished_ms,Some(2000.0));
        assert_eq!(trace.asr.pending_items,1);assert!(trace.durations.stop_to_final_asr_ms.is_none());
    }
    #[test] fn capture_and_wire_contention_drop_metadata_without_waiting_and_later_snapshots_recover_counts() {
        let r=DiagnosticsRecorder::default();let at=Instant::now();let h=begin(&r,at);
        let bridge=BridgeTelemetry{observed:AudioCounts{frames:8,real_samples:12800,padding_samples:0},
            enqueued:AudioCounts{frames:7,real_samples:11200,padding_samples:0},..Default::default()};
        let wire=AsrTelemetry{socket_written:AudioCounts{frames:6,real_samples:9600,padding_samples:0},..Default::default()};
        let lock=r.store.lock().unwrap();
        // Same-thread lock ownership would deadlock if any diagnostic used lock().
        r.bridge(Some(&h),&bridge);r.asr(Some(&h),&wire);r.capture(Some(&h),12800,16000,8,true,false);drop(lock);
        r.bridge(Some(&h),&bridge);r.asr(Some(&h),&wire);r.capture(Some(&h),12800,16000,8,true,false);
        let snapshot=r.snapshot().unwrap();assert_eq!(snapshot.dropped_events,3);
        assert_eq!(snapshot.traces[0].bridge.enqueued.frames,7);assert_eq!(snapshot.traces[0].asr.socket_written.frames,6);
    }
}
