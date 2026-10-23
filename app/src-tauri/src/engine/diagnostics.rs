//! Bounded, metadata-only diagnostics. Capture never waits for this recorder.
//! No disk IO, transcript, PCM, prompt, credentials or host text enters it.
use crate::core::{ModeSnapshot, ProviderConfig, RewriteFallbackReason};
use crate::cloud::asr::{AudioCounts,AsrTelemetry,AsrTelemetryView};
use crate::cloud::rewrite::execution::{RequestEvent,RequestStage,RequestOutcome};
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
    SourceCheckpointCommitted, RewriteStarted, RewriteValidated, RewriteFallbackSelected, ReadyCommitted,
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
    pub conditioning:Option<crate::platform::audio_conditioner::ConditioningSummary>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all="camelCase")]
pub enum RewriteOutcome { Validated, Fallback, Failed, Cancelled }

#[derive(Clone, Serialize)]
#[serde(rename_all="camelCase")]
pub struct RewriteRequestView {
    pub(crate) stage:RequestStage, pub start_ms:f64, pub end_ms:Option<f64>,
    pub duration_ms:Option<f64>, pub remaining_budget_ms:u64,
    pub(crate) outcome:Option<RequestOutcome>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all="camelCase")]
pub struct RewriteView {
    pub protocol_version: &'static str, pub configured_budget_ms: u64,
    pub outcome: Option<RewriteOutcome>, pub fallback_reason: Option<RewriteFallbackReason>,
    pub error_code: Option<&'static str>, pub duration_ms: Option<f64>,
    // At most four requests; no request body, transcript, response or key.
    pub requests: Vec<RewriteRequestView>,
}

// Only fixed implementation labels enter rewrite metadata. In particular, a
// sanitized-looking arbitrary string must not become a transcript or secret.
fn rewrite_error_code(code: Option<&str>) -> Option<&'static str> {
    const CODES: &[&str] = &[
        "rewrite_timeout", "rewrite_network", "network_setup", "rewrite_service_error",
        "authentication_failed", "rate_limited", "invalid_response", "invalid_output", "invalid_local_repair", "stale_rewrite",
        "empty_output", "response_too_large", "truncated_output", "invalid_source_mapping",
        "incomplete_source_coverage", "rewrite_meaning_risk", "rewrite_admission_rejected", "rewrite_semantic_rejected", "cancelled", "source_incomplete",
        "rewrite_literal_changed", "protected_terms_missing", "protected_terms_new_latin", "protected_terms_marker_unknown", "protected_terms_marker_invalid",
        "protected_terms_limit", "protected_terms_empty", "protected_terms_nonce_invalid", "protected_terms_nonce_collision", "stale_protected_draft",
        "invalid_source", "invalid_mode", "unsupported_model", "unsupported_rewrite_model",
        "unsupported_provider", "unsupported_region", "invalid_config", "invalid_workspace",
    ];
    code.and_then(|code| CODES.iter().copied().find(|allowed| *allowed == code))
}

#[derive(Clone, Serialize)]
#[serde(rename_all="camelCase")]
pub struct TraceView {
    pub run_id: String, pub session_id: String, pub generation: u64,
    pub origin: Origin, pub started_at: String, pub provider: String,
    pub region: Option<String>, pub asr_model: String, pub rewrite_provider: String, pub rewrite_model: String,
    pub mode_id: String, pub mode_revision: u64, pub asr_attempt_id: Option<String>,
    pub capture_duration_ms: Option<u64>, pub source_char_count: Option<usize>,
    pub bridge_queue_peak_frames: usize, pub outcome: Option<&'static str>,
    pub error_code: Option<String>, pub marks: Vec<Mark>, pub durations: Durations,
    pub capture:Option<CaptureView>,pub bridge:BridgeView,pub asr:AsrTelemetryView,
    // Additive to schema 2. Old receipts without this object remain readable.
    pub rewrite:Option<RewriteView>,
}
#[derive(Default, Clone, Serialize)]
#[serde(rename_all="camelCase")]
pub struct Durations {
    pub intent_to_capture_ms: Option<f64>, pub intent_to_first_caption_ms: Option<f64>,
    pub caption_to_render_receipt_proxy_ms: Option<f64>, pub stop_to_final_asr_ms: Option<f64>,
    pub rewrite_round_trip_ms: Option<f64>, pub stop_to_ready_ms: Option<f64>,
    // Native dispatch can be skipped, failed or unconfirmed. These measure receipt
    // latency only; neither field proves that a host inserted the text.
    pub ready_to_native_delivery_receipt_ms: Option<f64>,
    pub stop_to_native_delivery_receipt_ms: Option<f64>,
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
            asr_model:config.asr_model.clone(),rewrite_provider:config.rewrite_provider.clone(),rewrite_model:config.rewrite_model.clone(),mode_id:mode.mode_id.clone(),
            mode_revision:mode.revision,asr_attempt_id:None,capture_duration_ms:None,source_char_count:None,
            bridge_queue_peak_frames:0,outcome:None,error_code:None,
            capture:None,bridge:BridgeView::default(),asr:AsrTelemetryView::default(),rewrite:None,
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
    pub fn rewrite_started(&self,handle:Option<&TraceHandle>,protocol_version:&'static str,configured_budget_ms:u64) {
        self.rewrite_started_at(handle,protocol_version,configured_budget_ms,Instant::now());
    }
    fn rewrite_started_at(&self,handle:Option<&TraceHandle>,protocol_version:&'static str,configured_budget_ms:u64,at:Instant) {
        self.update(handle,|trace|{
            if trace.view.rewrite.is_some() {return;}
            let Some(elapsed)=at.checked_duration_since(trace.origin) else {self.dropped.fetch_add(1,Ordering::Relaxed);return;};
            if !trace.view.marks.iter().any(|mark|mark.stage==Stage::RewriteStarted) {
                trace.view.marks.push(Mark{stage:Stage::RewriteStarted,offset_ms:elapsed.as_secs_f64()*1000.0});
            }
            trace.view.rewrite=Some(RewriteView{protocol_version,configured_budget_ms,outcome:None,
                fallback_reason:None,error_code:None,duration_ms:None,requests:Vec::new()});
        });
    }
    pub(crate) fn rewrite_request(&self,handle:Option<&TraceHandle>,event:RequestEvent) {
        self.update(handle,|trace|{
            let Some(start)=event.started_at.checked_duration_since(trace.origin).map(|d|d.as_secs_f64()*1000.0) else {return;};
            let Some(rewrite)=trace.view.rewrite.as_mut() else {return;};
            // A completion event can recover a dropped start event. A stale or
            // duplicate completion never reopens a request or a different trace.
            let index=if let Some(index)=rewrite.requests.iter().position(|r|r.stage==event.stage){index}
                else if rewrite.requests.len()<4 {
                    rewrite.requests.push(RewriteRequestView{stage:event.stage,start_ms:start,end_ms:None,duration_ms:None,
                        remaining_budget_ms:event.remaining_budget_ms,outcome:None});rewrite.requests.len()-1
                } else {return;};
            let request=&mut rewrite.requests[index];
            if request.outcome.is_some(){return;}
            if let Some(end)=event.finished_at.and_then(|at|at.checked_duration_since(trace.origin)).map(|d|d.as_secs_f64()*1000.0) {
                if end<request.start_ms{return;}
                request.end_ms=Some(end);request.duration_ms=Some(end-request.start_ms);request.outcome=event.outcome;
            }
        });
    }
    pub fn rewrite_finished(&self,handle:Option<&TraceHandle>,outcome:RewriteOutcome,fallback_reason:Option<RewriteFallbackReason>,code:Option<&str>) {
        self.rewrite_finished_at(handle,outcome,fallback_reason,code,Instant::now());
    }
    fn rewrite_finished_at(&self,handle:Option<&TraceHandle>,outcome:RewriteOutcome,fallback_reason:Option<RewriteFallbackReason>,code:Option<&str>,at:Instant) {
        self.update(handle,|trace|{
            let started=trace.view.marks.iter().find(|mark|mark.stage==Stage::RewriteStarted).map(|mark|mark.offset_ms);
            let Some(rewrite)=trace.view.rewrite.as_mut().filter(|rewrite|rewrite.outcome.is_none()) else{return;};
            rewrite.outcome=Some(outcome);
            rewrite.fallback_reason=if outcome==RewriteOutcome::Fallback {fallback_reason} else {None};
            rewrite.error_code=if outcome==RewriteOutcome::Validated {None} else {rewrite_error_code(code)};
            rewrite.duration_ms=at.checked_duration_since(trace.origin).map(|time|time.as_secs_f64()*1000.0)
                .zip(started).and_then(|(end,start)|(end>=start).then_some(end-start));
        });
    }
    pub fn lengths(&self, handle: Option<&TraceHandle>, duration_ms: u64, chars: Option<usize>) {
        self.update(handle,|trace|{trace.view.capture_duration_ms=Some(duration_ms);if chars.is_some(){trace.view.source_char_count=chars;}});
    }
    pub fn bridge_peak(&self, handle: Option<&TraceHandle>, frames: usize) {
        self.update(handle,|trace|trace.view.bridge_queue_peak_frames=trace.view.bridge_queue_peak_frames.max(frames));
    }
    pub fn capture(&self,handle:Option<&TraceHandle>,input_samples:u64,input_sample_rate:u32,emitted_frames:u64,complete:bool,cancelled:bool) {
        self.update(handle,|trace|trace.view.capture=Some(CaptureView{input_samples,input_sample_rate,emitted_frames,complete,cancelled,conditioning:None}));
    }
    pub fn conditioning(&self,handle:Option<&TraceHandle>,metadata:&crate::platform::audio_conditioner::ConditioningSummary) {
        self.update(handle,|trace|{if let Some(capture)=trace.view.capture.as_mut(){capture.conditioning=Some(metadata.clone());}});
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
                rewrite_round_trip_ms:view.rewrite.as_ref().and_then(|rewrite|rewrite.duration_ms)
                    .or_else(||delta(Stage::RewriteStarted,Stage::RewriteValidated))
                    .or_else(||delta(Stage::RewriteStarted,Stage::RewriteFallbackSelected)),
                stop_to_ready_ms:delta(Stage::StopIntentReceived,Stage::ReadyCommitted),
                ready_to_native_delivery_receipt_ms:delta(Stage::ReadyCommitted,Stage::NativeDeliveryReceipt),
                stop_to_native_delivery_receipt_ms:delta(Stage::StopIntentReceived,Stage::NativeDeliveryReceipt),
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
    #[test] fn rewrite_fallback_retains_cause_and_duration_after_ready_and_duplicate_completion() {
        let r=DiagnosticsRecorder::default();let at=Instant::now();let h=begin(&r,at);
        r.rewrite_started_at(Some(&h),"mode-contract-v1",5000,at+Duration::from_secs(1));
        r.rewrite_started_at(Some(&h),"ignored-duplicate",20000,at+Duration::from_secs(3));
        r.mark_at(Some(&h),Stage::RewriteFallbackSelected,at+Duration::from_secs(6));
        r.rewrite_finished_at(Some(&h),RewriteOutcome::Fallback,Some(RewriteFallbackReason::Timeout),Some("rewrite_timeout"),at+Duration::from_secs(6));
        r.outcome(Some(&h),"ready",None);
        r.rewrite_finished_at(Some(&h),RewriteOutcome::Validated,None,None,at+Duration::from_secs(8));
        let snapshot=r.snapshot().unwrap();let trace=&snapshot.traces[0];let rewrite=trace.rewrite.as_ref().unwrap();
        assert_eq!(trace.outcome,Some("ready"));assert!(trace.error_code.is_none());
        assert_eq!(rewrite.protocol_version,"mode-contract-v1");assert_eq!(rewrite.configured_budget_ms,5000);
        assert_eq!(rewrite.outcome,Some(RewriteOutcome::Fallback));assert_eq!(rewrite.fallback_reason,Some(RewriteFallbackReason::Timeout));
        assert_eq!(rewrite.error_code,Some("rewrite_timeout"));assert_eq!(rewrite.duration_ms,Some(5000.0));
        assert_eq!(trace.durations.rewrite_round_trip_ms,Some(5000.0));
        assert!(!trace.marks.iter().any(|mark|mark.stage==Stage::RewriteValidated));
    }
    #[test] fn rewrite_terminal_states_are_independent_and_never_copy_unlisted_error_text() {
        for outcome in [RewriteOutcome::Validated,RewriteOutcome::Failed,RewriteOutcome::Cancelled] {
            let r=DiagnosticsRecorder::default();let at=Instant::now();let h=begin(&r,at);
            r.rewrite_started_at(Some(&h),"mode-contract-v1",20000,at+Duration::from_millis(100));
            r.rewrite_finished_at(Some(&h),outcome,Some(RewriteFallbackReason::MeaningRisk),Some("private_speech_or_secret"),at+Duration::from_millis(350));
            let snapshot=r.snapshot().unwrap();let rewrite=snapshot.traces[0].rewrite.as_ref().unwrap();
            assert_eq!(rewrite.outcome,Some(outcome));assert!(rewrite.fallback_reason.is_none());assert!(rewrite.error_code.is_none());
            assert_eq!(rewrite.duration_ms,Some(250.0));
            assert!(!serde_json::to_string(&snapshot).unwrap().contains("private_speech_or_secret"));
        }
        assert_eq!(rewrite_error_code(Some("incomplete_source_coverage")),Some("incomplete_source_coverage"));
        assert_eq!(rewrite_error_code(Some("invalid_source_mapping")),Some("invalid_source_mapping"));
        assert_eq!(rewrite_error_code(Some("rewrite_meaning_risk")),Some("rewrite_meaning_risk"));
        assert_eq!(rewrite_error_code(Some("rewrite_admission_rejected")),Some("rewrite_admission_rejected"));
        assert_eq!(rewrite_error_code(Some("rewrite_semantic_rejected")),Some("rewrite_semantic_rejected"));
        assert_eq!(rewrite_error_code(Some("rewrite_admission_rejected:private-key")),None);
        assert_eq!(rewrite_error_code(Some("rewrite_semantic_rejected:private-quote")),None);
    }
    #[test] fn rewrite_metadata_does_not_fabricate_start_or_block_on_contention() {
        let r=DiagnosticsRecorder::default();let at=Instant::now();let h=begin(&r,at);
        r.rewrite_finished_at(Some(&h),RewriteOutcome::Failed,None,Some("invalid_source"),at+Duration::from_secs(1));
        assert!(r.snapshot().unwrap().traces[0].rewrite.is_none());
        let lock=r.store.lock().unwrap();r.rewrite_started(Some(&h),"mode-contract-v1",5000);drop(lock);
        assert!(r.snapshot().unwrap().traces[0].rewrite.is_none());
        r.rewrite_started_at(Some(&h),"mode-contract-v1",5000,at+Duration::from_secs(2));
        r.rewrite_finished_at(Some(&h),RewriteOutcome::Cancelled,None,Some("cancelled"),at+Duration::from_secs(1));
        let snapshot=r.snapshot().unwrap();let rewrite=snapshot.traces[0].rewrite.as_ref().unwrap();
        assert!(rewrite.duration_ms.is_none());assert_eq!(rewrite.error_code,Some("cancelled"));assert_eq!(snapshot.dropped_events,1);
    }
    #[test] fn legacy_fallback_marks_report_elapsed_without_implying_validation() {
        let r=DiagnosticsRecorder::default();let at=Instant::now();let h=begin(&r,at);
        r.mark_at(Some(&h),Stage::RewriteStarted,at+Duration::from_secs(1));
        r.mark_at(Some(&h),Stage::RewriteFallbackSelected,at+Duration::from_secs(4));
        let snapshot=r.snapshot().unwrap();let trace=&snapshot.traces[0];
        assert!(trace.rewrite.is_none());assert_eq!(trace.durations.rewrite_round_trip_ms,Some(3000.0));
    }
    #[test] fn conditioning_reports_only_finite_metadata_and_never_creates_capture_for_retry() {
        use crate::platform::audio_conditioner::{AudioConditioner,ConditioningMode};
        let r=DiagnosticsRecorder::default();let h=begin(&r,Instant::now());
        let mut processor=AudioConditioner::new(ConditioningMode::SoftVoice);
        processor.process(&mut [0.0;160]);let metadata=processor.summary();
        r.conditioning(Some(&h),&metadata);
        assert!(r.snapshot().unwrap().traces[0].capture.is_none());
        r.capture(Some(&h),480,48000,1,true,false);r.conditioning(Some(&h),&metadata);
        let snapshot=serde_json::to_value(r.snapshot().unwrap()).unwrap();
        let value=&snapshot["traces"][0]["capture"]["conditioning"];
        assert_eq!(value["profile"],"softVoice");assert_eq!(value["realSamples"],160);
        assert_eq!(value["inputRmsDbfs"],-120.0);
        assert!(value.as_object().unwrap().values().all(|v|!v.is_array()&&!v.is_object()));
    }
    #[test] fn offsets_do_not_mix_clocks_and_duplicate_stop_does_not_reset_wait() {
        let r=DiagnosticsRecorder::default();let at=Instant::now();let h=begin(&r,at);
        r.mark_at(Some(&h),Stage::StopIntentReceived,at+Duration::from_secs(1));
        r.mark_at(Some(&h),Stage::StopIntentReceived,at+Duration::from_secs(4));
        r.mark_at(Some(&h),Stage::ReadyCommitted,at+Duration::from_secs(3));
        let s=r.snapshot().unwrap();assert_eq!(s.traces[0].durations.stop_to_ready_ms,Some(2000.0));
        assert!(s.traces[0].durations.intent_to_capture_ms.is_none());
    }
    #[test] fn native_receipt_durations_keep_first_marks_and_do_not_assert_host_confirmation() {
        let r=DiagnosticsRecorder::default();let at=Instant::now();let h=begin(&r,at);
        for (stage,ms) in [(Stage::StopIntentReceived,1000),(Stage::ReadyCommitted,2300),
            (Stage::NativeDeliveryReceipt,2500),(Stage::StopIntentReceived,1800),
            (Stage::ReadyCommitted,2400),(Stage::NativeDeliveryReceipt,2900)] {
            r.mark_at(Some(&h),stage,at+Duration::from_millis(ms));
        }
        r.outcome(Some(&h),"ready",None);
        let s=r.snapshot().unwrap();let trace=&s.traces[0];
        assert_eq!(trace.durations.ready_to_native_delivery_receipt_ms,Some(200.0));
        assert_eq!(trace.durations.stop_to_native_delivery_receipt_ms,Some(1500.0));
        assert_eq!(trace.marks.len(),4);
        assert_eq!(trace.outcome,Some("ready"));
        let json=serde_json::to_value(trace).unwrap();
        assert_eq!(json["durations"]["readyToNativeDeliveryReceiptMs"],200.0);
        assert_eq!(json["durations"]["stopToNativeDeliveryReceiptMs"],1500.0);
        assert!(json.get("deliveryConfirmed").is_none());
    }
    #[test] fn native_receipt_durations_require_each_endpoint_and_monotonic_order() {
        // Source-only retries may have no stop; cancelled/failed runs may have no
        // ready or receipt. Never fabricate a zero or successful-delivery latency.
        for (stop,ready,receipt,expected_stop,expected_ready) in [
            (Some(1000),Some(2000),None,None,None),
            (None,Some(2000),Some(2500),None,Some(500.0)),
            (Some(1000),None,Some(2500),Some(1500.0),None),
            (Some(3000),Some(2000),Some(1500),None,None),
            (Some(1000),Some(2000),Some(2000),Some(1000.0),Some(0.0)),
        ] {
            let r=DiagnosticsRecorder::default();let at=Instant::now();let h=begin(&r,at);
            for (stage,ms) in [(Stage::StopIntentReceived,stop),(Stage::ReadyCommitted,ready),
                (Stage::NativeDeliveryReceipt,receipt)] {
                if let Some(ms)=ms {r.mark_at(Some(&h),stage,at+Duration::from_millis(ms));}
            }
            let s=r.snapshot().unwrap();let durations=&s.traces[0].durations;
            assert_eq!(durations.stop_to_native_delivery_receipt_ms,expected_stop);
            assert_eq!(durations.ready_to_native_delivery_receipt_ms,expected_ready);
        }
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
