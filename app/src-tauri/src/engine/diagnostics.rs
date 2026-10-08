//! Bounded, metadata-only diagnostics. Capture never waits for this recorder.
//! No disk IO, transcript, PCM, prompt, credentials or host text enters it.
use crate::core::{ModeSnapshot, ProviderConfig};
use serde::Serialize;
use std::{collections::VecDeque, sync::{Mutex, atomic::{AtomicU64, Ordering}}, time::Instant};
use uuid::Uuid;

const MAX_TRACES: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all="camelCase")]
pub enum Stage {
    IntentReceived, CaptureFirstAccepted, ProviderReadyReceived,
    ProviderSpeechStartedReceived, FirstCaptionReceived, CaptionRenderReceiptProxy,
    StopIntentReceived, CaptureCutoff, CaptureDrained, FinalAsrReceived,
    SourceCheckpointCommitted, RewriteStarted, RewriteValidated, ReadyCommitted,
    DeliveryClaim, NativeDeliveryReceipt, CancelIntent, ProcessingWorkerReleased,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all="camelCase")]
pub enum Origin { UiCommand, HotkeyHook, AsrRetry, RewriteRetry, SaveRetry }

#[derive(Clone)]
pub struct TraceHandle { pub run_id: String }

#[derive(Clone, Serialize)]
#[serde(rename_all="camelCase")]
pub struct Mark { pub stage: Stage, pub offset_ms: f64 }

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
}
#[derive(Default, Clone, Serialize)]
#[serde(rename_all="camelCase")]
pub struct Durations {
    pub intent_to_capture_ms: Option<f64>, pub intent_to_first_caption_ms: Option<f64>,
    pub caption_to_render_receipt_proxy_ms: Option<f64>, pub stop_to_final_asr_ms: Option<f64>,
    pub rewrite_round_trip_ms: Option<f64>, pub stop_to_ready_ms: Option<f64>,
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
                stop_to_ready_ms:delta(Stage::StopIntentReceived,Stage::ReadyCommitted)};
            view
        }).collect();
        Ok(Snapshot{schema_version:1,app_version:env!("CARGO_PKG_VERSION"),in_memory_only:true,
            timing_scope:"Rust monotonic offsets; UI render receipt is a proxy, not optical visibility or host confirmation",
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
}
