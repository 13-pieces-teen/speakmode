use super::*;
use crate::cloud::asr::{self,AsrInput,AsrProgress,CaptionItem};
use crate::platform::audio::{AudioEvent,AudioFrame,CaptureHandle,CaptureSummary,CaptureFailureReason,classify_capture_error};
use serde::Serialize;
use std::{sync::atomic::{AtomicBool,Ordering},time::{Duration,Instant}};
use tokio::sync::{mpsc,oneshot,Notify};
use super::diagnostics::{Stage,Origin,TraceHandle,BridgeTelemetry,DiagnosticsRecorder,RewriteOutcome};
#[path="retry_audio.rs"]
mod retry_audio;
#[path="recovery.rs"]
mod recovery;
#[path="rewrite_fallback.rs"]
mod rewrite_fallback;
use rewrite_fallback::RewriteWaitPolicy;
#[path="recorder_copy.rs"]
mod recorder_copy;
#[path="app_writing.rs"]
mod app_writing;
use crate::core::writing_context::{InputContext,WritingContext};

#[derive(Debug,Clone,Copy,Serialize,PartialEq,Eq)]
#[serde(rename_all="camelCase")]
pub enum InputContextStatus { Disabled, Pending, Available, Unavailable }

fn stop_capture_failure_reason(error: &platform::PlatformError) -> &'static str {
    match error {
        platform::PlatformError::TargetHigherIntegrity => "targetHigherIntegrity",
        platform::PlatformError::TargetCapture(reason) => reason.delivery_reason(),
        platform::PlatformError::Timeout(_) => platform::TargetCaptureFailure::TargetInspectionTimeout.delivery_reason(),
        _ => "targetUnavailableAtStop",
    }
}

/// The desktop callback is an API boundary. Classify only agreed error codes,
/// never provider strings or the callback's arbitrary message.
fn stop_control_failure(error: ApiError) -> platform::PlatformError {
    use platform::{PlatformError, TargetCaptureFailure};
    let reason = match error.code.as_str() {
        "target_higher_integrity" => return PlatformError::TargetHigherIntegrity,
        "stop_target_changed" | "stop_focus_changed" => TargetCaptureFailure::StopFocusChanged,
        "stop_target_unavailable" | "stop_anchor_unavailable" => TargetCaptureFailure::StopAnchorUnavailable,
        "target_not_editable" => TargetCaptureFailure::TargetNotEditable,
        "target_capability_unsupported" => TargetCaptureFailure::TargetCapabilityUnsupported,
        "window_timeout" | "target_inspection_timeout" => TargetCaptureFailure::TargetInspectionTimeout,
        _ => return PlatformError::Unavailable("stop target coordination unavailable".into()),
    };
    PlatformError::TargetCapture(reason)
}

#[cfg(test)]
#[path="tests.rs"]
mod tests;

#[cfg(test)]
#[path="audio_diagnostics_tests.rs"]
mod audio_diagnostics_tests;

#[cfg(test)]
#[path="dictionary_rewrite_tests.rs"]
mod dictionary_rewrite_tests;

#[cfg(test)]
#[path="retry_asr_experiment_tests.rs"]
mod retry_asr_experiment_tests;

#[cfg(test)]
#[path="quality_foundation_tests.rs"]
mod quality_foundation_tests;

#[cfg(test)]
#[path="recovery_retention_tests.rs"]
mod recovery_retention_tests;

#[derive(Debug,Clone,Copy,PartialEq,Eq,Serialize)]
#[serde(rename_all="camelCase")]
pub enum SessionPhase {Starting,Recording,FinalizingAsr,Rewriting,Ready,Failed,Incomplete,Cancelled}
const RECORDING_LIMIT_MS:u64=600_000;
const RECORDING_WARNING_MS:u64=30_000;
const AUDIO_CACHE_RETENTION:Duration=Duration::from_secs(1800);
fn recording_limit_remaining(duration_ms:u64)->Option<u64>{
    let remaining=RECORDING_LIMIT_MS.saturating_sub(duration_ms);
    (remaining<=RECORDING_WARNING_MS).then_some(remaining)
}
#[derive(Clone,Serialize)]
#[serde(rename_all="camelCase")]
pub struct LiveSession {
    pub id:String,pub session_id:String,pub generation:u64,pub state:SessionPhase,
    pub mode_snapshot:ModeSnapshot,pub level:f32,pub partial:String,pub error:Option<String>,
    pub delivery_state:Option<DeliveryState>,pub delivery_failure_reason:Option<String>,pub recoverable_content_id:Option<String>,
    pub pending_commit_id:Option<String>,pub commit_stage:Option<String>,pub worker_released:bool,
    pub asr_attempt_id:Option<String>,pub can_retry_asr:bool,pub can_retry_rewrite:bool,pub can_use_source:bool,pub can_copy_recovery:bool,
    pub source_revision_id:Option<String>,
    pub can_retry_save:bool,pub retry_kind:Option<String>,pub error_code:Option<String>,
    pub recording_limit_remaining_ms:Option<u64>,pub cancel_pending:bool,
    pub captions:Vec<CaptionItem>,pub capture_ready:bool,pub provider_ready:bool,
    pub caption_render_token:Option<String>,
    #[serde(skip_serializing_if="Option::is_none")]
    pub rewrite_fallback:Option<RewriteFallbackReason>,
    #[serde(skip_serializing_if="Option::is_none")]
    pub writing_scene:Option<AppWritingScene>,
    pub input_context_status:InputContextStatus,
    pub writing_language:DraftLanguage,
}
#[derive(Clone)]
enum PendingCommit {Source(SessionSeed,SourceRevision),Ready(OutputRevision)}
pub(crate) struct ActiveSession {
    diagnostic_trace:Option<TraceHandle>,caption_render_token:Option<String>,
    pub id:String,pub generation:u64,phase:SessionPhase,prefs:Preferences,config:ProviderConfig,mode:ModeSnapshot,dictionary:DictionarySnapshot,
    started_at:String,ended_at:Option<String>,capture_duration_ms:u64,
    recording_limit_remaining_ms:Option<u64>,
    first_samples:bool,provider_ready:bool,level:f32,captions:Vec<CaptionItem>,error:Option<ApiError>,
    stop:Arc<AtomicBool>,cancel:CancellationToken,worker_released:bool,
    source:Option<SourceRevision>,output:Option<OutputRevision>,
    pending:Option<(String,PendingCommit)>,cache:Option<Vec<i16>>,cache_complete:bool,cache_created:Option<Instant>,
    target:Option<platform::delivery::TargetSnapshot>,target_capture_pending:bool,
    target_application:Option<String>,writing_scene:Option<AppWritingScene>,
    input_context:Option<app_writing::CapturedInput>,input_context_status:InputContextStatus,input_context_frozen:bool,
    target_capture_failure_reason:Option<String>,
    delivery_failure_reason:Option<String>,delivery:Option<DeliveryAttempt>,reason:CreationReason,
    asr_attempt_id:Option<String>,
    delivery_permit:Option<Arc<platform::delivery::DeliveryPermit>>,delivery_worker_released:bool,
    original_delivery_revoked:bool,revision_intent_sequence:u64,revision_receipt_sequence:u64,
    processing_worker_released:bool,db_pending:usize,cancel_pending:bool,
}
impl ActiveSession {
    fn discard_input_context(&mut self) {
        self.input_context=None;self.input_context_frozen=true;
        self.input_context_status=if self.prefs.app_writing.input_context_enabled {InputContextStatus::Unavailable}else{InputContextStatus::Disabled};
    }
    pub(crate) fn expire_audio_cache(&mut self)->bool {
        if self.cache.is_none()||self.cache_created.is_none_or(|created|created.elapsed()<AUDIO_CACHE_RETENTION){return false;}
        self.cache=None;self.cache_complete=false;self.cache_created=None;self.discard_input_context();
        if self.source.is_none()&&self.reason==CreationReason::Retry&&self.phase==SessionPhase::FinalizingAsr {
            self.cancel.cancel();
            self.phase=SessionPhase::Incomplete;
            self.error=Some(ApiError::new("asr_retry_expired","recovery","音频恢复期限已到，重试已停止。已有字幕可复制，请重新口述。").for_session(&self.id,self.generation));
        }
        true
    }
    pub(crate) fn reserve_revision_intent(&mut self)->u64{
        self.original_delivery_revoked=true;
        if let Some(permit)=&self.delivery_permit{permit.cancel_before_injection();}
        self.revision_intent_sequence=self.revision_intent_sequence.saturating_add(1);self.revision_intent_sequence
    }
    pub(crate) fn sync_revision_receipt(&mut self,detail:&HistoryDetail,sequence:u64){
        if self.id==detail.session.seed.id&&self.generation==detail.session.seed.generation&&self.source.as_ref().is_some_and(|source|source.id==detail.source.id)&&sequence>self.revision_receipt_sequence{
            if let Some(output)=detail.output.clone(){self.output=Some(output);self.revision_receipt_sequence=sequence;}
        }
    }
    pub(crate) fn revoke_for_deletion(&mut self){
        self.stop.store(true,Ordering::SeqCst);self.cancel.cancel();self.discard_input_context();if let Some(permit)=&self.delivery_permit{permit.cancel_before_injection();}
    }
    pub(crate) fn discard_deleted_content(&mut self){
        self.phase=SessionPhase::Cancelled;self.discard_input_context();self.pending=None;self.cache=None;self.cache_complete=false;self.cache_created=None;self.source=None;self.output=None;self.captions.clear();
        if let Some(delivery)=self.delivery.as_mut(){if matches!(delivery.state,DeliveryState::Prepared|DeliveryState::Dispatched){delivery.state=DeliveryState::Unconfirmed;delivery.reason=Some("deletedBeforeReceipt".into());}}
    }
    pub(crate) fn view(&self)->LiveSession {
        let released=self.worker_released&&self.delivery_worker_released&&self.processing_worker_released&&!self.target_capture_pending&&self.db_pending==0&&!self.cancel_pending;
        let problem=matches!(self.phase,SessionPhase::Failed|SessionPhase::Incomplete)&&!self.error.as_ref().is_some_and(|e|e.code=="cancel_cleanup_failed");
        let can_retry_save=released&&problem&&self.pending.is_some();
        let can_retry_asr=released&&problem&&self.pending.is_none()&&self.source.is_none()&&self.cache_complete&&self.cache.as_ref().is_some_and(|pcm|!pcm.is_empty())&&self.cache_created.is_some_and(|t|t.elapsed()<AUDIO_CACHE_RETENTION);
        let can_retry_rewrite=released&&problem&&self.source.is_some()&&self.pending.is_none();
        LiveSession{writing_language:self.prefs.app_writing.output_language,input_context_status:self.input_context_status,writing_scene:self.writing_scene,rewrite_fallback:self.output.as_ref().and_then(|output|output.rewrite_fallback),captions:self.captions.clone(),capture_ready:self.first_samples,provider_ready:self.provider_ready,caption_render_token:self.caption_render_token.clone(),id:self.id.clone(),session_id:self.id.clone(),generation:self.generation,state:self.phase,mode_snapshot:self.mode.clone(),level:self.level,partial:self.captions.iter().map(|i|i.text.as_str()).collect::<Vec<_>>().join(" "),error:self.error.as_ref().map(|e|e.message.clone()),delivery_state:self.delivery.as_ref().map(|d|d.state).or_else(||self.delivery_failure_reason.as_ref().map(|_|DeliveryState::Failed)),delivery_failure_reason:self.delivery.as_ref().and_then(|d|d.reason.clone()).or_else(||self.delivery_failure_reason.clone()),recoverable_content_id:(!self.captions.is_empty()||self.source.is_some()).then(||self.id.clone()),pending_commit_id:self.pending.as_ref().map(|(id,_)|id.clone()),commit_stage:self.pending.as_ref().map(|(_,p)|match p{PendingCommit::Source(_,_)=>"sourceCheckpoint",PendingCommit::Ready(_)=>"ready"}.into()),worker_released:released,asr_attempt_id:self.asr_attempt_id.clone(),can_retry_asr,can_retry_rewrite,can_use_source:can_retry_rewrite,can_copy_recovery:!self.captions.is_empty()||self.source.is_some(),source_revision_id:self.source.as_ref().map(|s|s.id.clone()),can_retry_save,retry_kind:if can_retry_save{Some("save".into())}else if can_retry_asr{Some("asr".into())}else if can_retry_rewrite{Some("rewrite".into())}else{None},error_code:self.error.as_ref().map(|e|e.code.clone()),recording_limit_remaining_ms:self.recording_limit_remaining_ms,cancel_pending:self.cancel_pending}
    }
    pub(crate) fn adopt_first_history_result(&mut self,detail:&HistoryDetail) {
        if self.id!=detail.session.seed.id||self.generation!=detail.session.seed.generation||self.source.as_ref().is_none_or(|s|s.id!=detail.source.id){return;}
        let Some(output)=detail.output.clone()else{return;};
        // A late failed initial/retry pipeline cannot replace this explicit
        // adoption. Actual resource leases remain closed until their owners exit.
        self.stop.store(true,Ordering::SeqCst);self.cancel.cancel();self.discard_input_context();
        if let Some(permit)=&self.delivery_permit{permit.cancel_before_injection();}
        self.phase=SessionPhase::Ready;self.source=Some(detail.source.clone());self.output=Some(output);
        self.pending=None;self.error=None;self.cache=None;self.cache_complete=false;self.cache_created=None;
        self.delivery=None;self.delivery_failure_reason=None;self.delivery_permit=None;self.reason=CreationReason::History;self.original_delivery_revoked=true;
    }
    pub(crate) fn busy(&self)->bool {
        self.db_pending>0 || self.cancel_pending || self.target_capture_pending || !self.worker_released || !self.delivery_worker_released || !self.processing_worker_released || self.error.as_ref().is_some_and(|e|e.code=="cancel_cleanup_failed") || matches!(self.phase,SessionPhase::Starting|SessionPhase::Recording|SessionPhase::FinalizingAsr|SessionPhase::Rewriting) || self.delivery.as_ref().is_some_and(|d|matches!(d.state,DeliveryState::Prepared|DeliveryState::Dispatched))
    }
}

/// Dropping an async task also releases its ownership. Cancelling its token
/// alone must never announce that an ASR/LLM worker has already stopped.
struct ProcessingLease {hub:Arc<AppHub>,id:String,generation:u64,trace:Option<TraceHandle>}
impl Drop for ProcessingLease {
    fn drop(&mut self){
        {let mut i=self.hub.inner.lock().unwrap();if let Some(s)=i.active.as_mut().filter(|s|s.id==self.id&&s.generation==self.generation){s.processing_worker_released=true;}}
        self.hub.diagnostics.mark(self.trace.as_ref(),Stage::ProcessingWorkerReleased);
        self.hub.publish_session();
    }
}

/// Stop target inspection owns a separate lease: Ready may be committed while
/// UI Automation is still resolving, but delivery/new capture/exit must wait.
struct StopTargetLease<'a> {hub:&'a AppHub,id:&'a str,generation:u64,armed:bool}
impl Drop for StopTargetLease<'_> {
    fn drop(&mut self){
        if !self.armed{return;}
        {let mut i=self.hub.inner.lock().unwrap();if let Some(s)=i.active.as_mut().filter(|s|s.id==self.id&&s.generation==self.generation){s.target_capture_pending=false;}}
        self.hub.publish_session();
    }
}

fn matching<'a>(inner:&'a mut Inner,p:&Value)->Result<&'a mut ActiveSession> {
    let s=inner.active.as_mut().ok_or_else(||ApiError::new("no_session","session","没有正在进行的口述"))?;
    if p.get("sessionId").or_else(||p.get("resultId")).and_then(Value::as_str).is_some_and(|id|id!=s.id)||p.get("expectedGeneration").or_else(||p.get("generation")).and_then(Value::as_u64).is_some_and(|g|g!=s.generation){return Err(ApiError::new("stale_session","session","这次口述已变化"));}
    Ok(s)
}

impl AppHub {
    fn accept_capture_cache(&self,id:&str,generation:u64,summary:&CaptureSummary,cache:Vec<i16>,cache_created:Instant) {
        let mut inner=self.inner.lock().unwrap();
        if let Some(session)=inner.active.as_mut().filter(|s|s.id==id&&s.generation==generation){
            session.worker_released=true;session.capture_duration_ms=summary.duration_ms();
            if session.phase!=SessionPhase::Cancelled&&!session.cancel.is_cancelled(){
                session.cache_complete=summary.complete;session.cache=Some(cache);session.cache_created=Some(cache_created);
            }
            if session.ended_at.is_none(){session.ended_at=Some(chrono::Local::now().to_rfc3339());}
        }
    }
    fn schedule_audio_cache_expiry(self:&Arc<Self>,id:String,created:Instant) {
        // This owner retains no PCM or strong AppHub reference. Moving a failed
        // session must never reset its original monotonic retention deadline.
        let weak=Arc::downgrade(self);
        tokio::spawn(async move {
            tokio::time::sleep(AUDIO_CACHE_RETENTION.saturating_sub(created.elapsed())).await;
            let Some(hub)=weak.upgrade()else{return;};
            let expired={let mut inner=hub.inner.lock().unwrap();
                let active=inner.active.as_mut().filter(|s|s.id==id&&s.cache_created==Some(created)).is_some_and(ActiveSession::expire_audio_cache);
                let retained=inner.recovery.as_mut().filter(|s|s.id==id&&s.cache_created==Some(created)).is_some_and(ActiveSession::expire_audio_cache);
                if retained{inner.recovery=None;}active||retained};
            if expired{hub.publish_session();}
            // A concurrent snapshot may already have released this lease. The
            // main panel must still learn that its retained entry has expired.
            hub.publish("recovery.changed",Value::Null);
        });
    }
    fn trace(&self,id:&str,generation:u64)->Option<TraceHandle>{
        self.inner.lock().unwrap().active.as_ref().filter(|s|s.id==id&&s.generation==generation).and_then(|s|s.diagnostic_trace.clone())
    }
    fn accept_audio_level(&self,id:&str,generation:u64,level:f32)->bool{
        if !level.is_finite(){return false;}
        let level=level.clamp(0.0,1.0);
        {let mut i=self.inner.lock().unwrap();let Some(s)=i.active.as_mut().filter(|s|s.id==id&&s.generation==generation&&!s.cancel.is_cancelled()&&matches!(s.phase,SessionPhase::Starting|SessionPhase::Recording))else{return false;};s.level=level;}
        self.publish("level",json!({"sessionId":id,"generation":generation,"level":level}));true
    }
    fn processing_lease(self:&Arc<Self>,id:&str,generation:u64)->ProcessingLease{
        ProcessingLease{hub:Arc::clone(self),id:id.into(),generation,trace:self.trace(id,generation)}
    }
    fn accept_asr_progress(&self,id:&str,generation:u64,attempt:&str,progress:AsrProgress){
        let received_at=Instant::now();
        {let mut i=self.inner.lock().unwrap();if let Some(s)=i.active.as_mut().filter(|s|s.id==id&&s.generation==generation&&s.asr_attempt_id.as_deref()==Some(attempt)&&!s.cancel.is_cancelled()){
            let trace=s.diagnostic_trace.as_ref();
            match progress{
                AsrProgress::Configured=>{s.provider_ready=true;self.diagnostics.mark_at(trace,Stage::ProviderReadyReceived,received_at);if s.first_samples&&s.phase==SessionPhase::Starting{s.phase=SessionPhase::Recording;}},
                AsrProgress::Caption(items)=>{
                    if items.iter().any(|item|!item.text.trim().is_empty()){
                        self.diagnostics.mark_at(trace,Stage::FirstCaptionReceived,received_at);
                        s.caption_render_token.get_or_insert_with(||Uuid::new_v4().to_string());
                    }
                    s.captions=items;
                },
                AsrProgress::SpeechStart=>self.diagnostics.mark_at(trace,Stage::ProviderSpeechStartedReceived,received_at),
                AsrProgress::Diagnostics(metadata)=>self.diagnostics.asr(trace,&metadata),
            }
        }}self.publish_session();
    }
    pub(super) fn caption_rendered(&self,p:&Value)->Result<Value>{
        let received_at=Instant::now();
        let id=text(p,"sessionId")?;let generation=number(p,"expectedGeneration")?;
        let attempt=text(p,"asrAttemptId")?;let token=text(p,"token")?;
        let i=self.inner.lock().unwrap();
        let valid=i.active.as_ref().filter(|s|s.id==id&&s.generation==generation&&!s.cancel.is_cancelled()&&s.asr_attempt_id.as_deref()==Some(attempt)&&s.caption_render_token.as_deref()==Some(token)&&s.captions.iter().any(|item|!item.text.trim().is_empty()));
        if let Some(s)=valid{self.diagnostics.mark_at(s.diagnostic_trace.as_ref(),Stage::CaptionRenderReceiptProxy,received_at);}
        Ok(json!({"accepted":valid.is_some()}))
    }
    pub async fn start_session(self:&Arc<Self>)->Result<Value> {
        self.start_session_at(Instant::now(),Origin::UiCommand).await
    }
    fn prepare_start_state(&self,intent:Instant,origin:Origin)->Result<(String,u64,ProviderConfig,Preferences,ModeSnapshot,DictionarySnapshot)> {
            let _native=self.native_io.try_lock().map_err(|_|ApiError::new("native_busy","session","上一条复制或输入操作尚未结束"))?;
            let mut i=self.inner.lock().unwrap();
            if self.quitting.load(Ordering::SeqCst)||self.db.owner_released()||!self.capture_available.load(Ordering::SeqCst){return Err(ApiError::new("system_unavailable","session","系统录音权限或快捷键尚未恢复，请稍候"));}
            if self.db.outstanding()>0||self.db_command_workers.load(Ordering::SeqCst)>0{return Err(ApiError::new("storage_busy","session","上一条本机处理尚未确认，请稍候"));}
            if i.settings_pending.is_some(){return Err(ApiError::new("settings_busy","session","设置正在保存，请稍候"));}
            if i.shortcut_capture.is_some(){return Err(ApiError::new("capture_busy","session","请先结束快捷键录制"));}
            if !i.auxiliary_workers.is_empty(){return Err(ApiError::new("processing_busy","session","文字处理任务尚未释放，请稍候"));}
            if !platform::audio::capture_resources_released(){return Err(ApiError::new("capture_busy","session","上一条采音尚未释放，请稍候"));}
            if i.active.as_ref().is_some_and(ActiveSession::busy){return Err(ApiError::new("session_busy","session","正在处理或释放上一条口述，请稍候"));}
            let cache=self.db.snapshot();let prefs=cache.preferences;let config=cache.provider_config;let dictionary=cache.dictionary;cloud::endpoints(&config)?;
            if !i.key_configured{return Err(ApiError::new("key_required","configuration","请先在设置中配置云服务 Key"));}
            let mode:ModeSnapshot=cache.modes.into_iter().find(|m|m.id==prefs.default_mode_id).ok_or_else(||ApiError::new("notFound","mode","默认整理方式已删除"))?.into();
            let generation=i.active.as_ref().map(|s|s.generation+1).unwrap_or(1);
            let id=Uuid::new_v4().to_string();
            i.park_retryable_failure()?;
            i.guards.insert(id.clone(),Arc::new(CommitGuard::new(id.clone(),generation)));
            i.active=Some(ActiveSession{diagnostic_trace:self.diagnostics.begin(&id,generation,origin,intent,&config,&mode),caption_render_token:None,id:id.clone(),generation,phase:SessionPhase::Starting,prefs:prefs.clone(),config:config.clone(),mode:mode.clone(),dictionary:dictionary.clone(),started_at:now(),ended_at:None,capture_duration_ms:0,recording_limit_remaining_ms:None,first_samples:false,provider_ready:false,level:0.0,captions:Vec::new(),error:None,stop:Arc::new(AtomicBool::new(false)),cancel:CancellationToken::new(),worker_released:true,source:None,output:None,pending:None,cache:None,cache_complete:false,cache_created:None,target:None,target_application:None,writing_scene:None,input_context_frozen:false,input_context:None,input_context_status:if prefs.app_writing.input_context_enabled {InputContextStatus::Pending}else{InputContextStatus::Disabled},target_capture_pending:false,target_capture_failure_reason:None,delivery_failure_reason:None,delivery:None,reason:CreationReason::NormalInitial,asr_attempt_id:None,delivery_permit:None,delivery_worker_released:true,original_delivery_revoked:false,revision_intent_sequence:0,revision_receipt_sequence:0,processing_worker_released:false,db_pending:0,cancel_pending:false});
            Ok((id,generation,config,prefs,mode,dictionary))
    }
    async fn start_session_at(self:&Arc<Self>,intent:Instant,origin:Origin)->Result<Value> {
        let (id,generation,config,prefs,mode,dictionary)=self.prepare_start_state(intent,origin)?;
        self.publish("recovery.changed",Value::Null);
        let preflight=self.processing_lease(&id,generation);
        // Input position is frozen at the stop intent, so browsing or moving the
        // caret while recording does not invalidate the final delivery.
        if !self.valid_active(&id,generation){return Err(ApiError::new("cancelled","session","口述已取消").for_session(&id,generation));}
        let secret=match credentials::read(&config){Ok(key)=>key,Err(e)=>{self.fail(&id,generation,e.into(),true);return Err(ApiError::new("key_required","credentials","无法读取本应用 Key").for_session(&id,generation));}};
        {let mut i=self.inner.lock().unwrap();let Some(s)=i.active.as_mut().filter(|s|s.id==id&&s.generation==generation&&!s.cancel.is_cancelled())else{return Err(ApiError::new("cancelled","session","口述已取消").for_session(&id,generation));};s.worker_released=false;}
        self.publish_session(); let _=self.native_action("recording",true);
        let hub=Arc::clone(self);let job_id=id.clone();
        tokio::spawn(async move {let _processing=preflight;hub.record_pipeline(job_id,generation,config,prefs,mode,dictionary,secret).await;});
        Ok(json!({"sessionId":id,"generation":generation,"state":"starting"}))
    }
    fn begin_stop_target(&self,p:&Value,intent:Instant)->Result<Option<(String,u64)>> {
        let target={let mut i=self.inner.lock().unwrap();let s=matching(&mut i,p)?;
            self.diagnostics.mark_at(s.diagnostic_trace.as_ref(),Stage::StopIntentReceived,intent);
            if !matches!(s.phase,SessionPhase::Starting|SessionPhase::Recording){None}
            else{
                // Cut recording before any native/UIA wait. Never keep sampling
                // merely because an editor's accessibility provider is slow.
                s.stop.store(true,Ordering::SeqCst);s.ended_at=Some(chrono::Local::now().to_rfc3339());s.phase=SessionPhase::FinalizingAsr;s.level=0.0;
                s.target=None;s.target_application=None;s.writing_scene=None;s.input_context_frozen=false;s.input_context=None;s.input_context_status=if s.prefs.app_writing.input_context_enabled {InputContextStatus::Pending}else{InputContextStatus::Disabled};s.target_capture_failure_reason=None;s.delivery_failure_reason=None;
                s.target_capture_pending=s.prefs.delivery_mode==DeliveryMode::Direct
                    || s.prefs.app_writing.input_context_enabled
                    || (s.prefs.app_writing.enabled && s.mode.mode_id=="auto" && s.mode.rules.trim().is_empty());
                s.target_capture_pending.then(||(s.id.clone(),s.generation))
            }
        };Ok(target)
    }
    fn resolve_stop_target_result(&self,id:&str,generation:u64,capture:impl FnOnce()->platform::Result<Option<platform::delivery::TargetSnapshot>>) {
        self.resolve_stop_target_with_application(id,generation,capture,platform::application::name_for_target);
    }
    fn resolve_stop_target_with_application(&self,id:&str,generation:u64,
        capture:impl FnOnce()->platform::Result<Option<platform::delivery::TargetSnapshot>>,
        application:impl FnOnce(&platform::delivery::TargetSnapshot)->Option<String>) {
        self.resolve_stop_target_with_context(id,generation,capture,application,platform::delivery::capture_input_context);
    }
    fn resolve_stop_target_with_context(&self,id:&str,generation:u64,
        capture:impl FnOnce()->platform::Result<Option<platform::delivery::TargetSnapshot>>,
        application:impl FnOnce(&platform::delivery::TargetSnapshot)->Option<String>,
        read_context:impl FnOnce(&platform::delivery::TargetSnapshot)->platform::Result<InputContext>) {
        let _lease=StopTargetLease{hub:self,id,generation,armed:true};
        let target=capture();
        let prefs={let i=self.inner.lock().unwrap();i.active.as_ref().filter(|s|s.id==id&&s.generation==generation
            &&!s.cancel.is_cancelled()&&!s.original_delivery_revoked&&s.target_capture_pending).map(|s|s.prefs.app_writing.clone())};
        let Some(prefs)=prefs else{return;};
        let application=target.as_ref().ok().and_then(Option::as_ref).and_then(application);
        let needs_context={let i=self.inner.lock().unwrap();i.active.as_ref().is_some_and(|s|s.id==id&&s.generation==generation
            &&!s.input_context_frozen&&!s.cancel.is_cancelled()&&!s.original_delivery_revoked)};
        let context=if needs_context && application.as_deref().is_some_and(|app|prefs.permits_input_context(app)) {
            target.as_ref().ok().and_then(Option::as_ref).and_then(|target|
                read_context(target).ok().filter(|c|!c.is_empty())
                    .map(|context|app_writing::CapturedInput{target:target.clone(),context,captured_at:Instant::now()}))
        }else{None};
        let mut i=self.inner.lock().unwrap();
        if let Some(s)=i.active.as_mut().filter(|s|s.id==id&&s.generation==generation&&!s.cancel.is_cancelled()&&!s.original_delivery_revoked&&s.target_capture_pending){
            if !s.input_context_frozen {
                s.input_context_status=if !prefs.input_context_enabled {InputContextStatus::Disabled}
                    else if context.is_some(){InputContextStatus::Available}else{InputContextStatus::Unavailable};
                s.input_context=context;
            }
            match target {
                Ok(target)=>{s.target=target;s.target_application=application;s.target_capture_failure_reason=None;}
                Err(error)=>{s.target=None;s.target_application=None;s.target_capture_failure_reason=Some(stop_capture_failure_reason(&error).into());}
            }
        }
    }

    async fn stop_session_at(self:&Arc<Self>,p:&Value,intent:Instant)->Result<Value> {
        self.stop_session_at_with_target_result(p,intent,||platform::delivery::capture_foreground_anchor().map(Some),|anchor|platform::delivery::capture_target_for_anchor(&anchor).map(Some)).await
    }
    async fn stop_session_at_with_target_result(self:&Arc<Self>,p:&Value,intent:Instant,
        anchor:impl FnOnce()->platform::Result<Option<platform::delivery::ForegroundAnchor>>,
        capture:impl FnOnce(platform::delivery::ForegroundAnchor)->platform::Result<Option<platform::delivery::TargetSnapshot>>+Send+'static)->Result<Value> {
        if let Some((id,generation))=self.begin_stop_target(p,intent)? {
            // Freeze cheap OS identity on this stop operation, before yielding
            // to a worker. Otherwise a window switched during scheduling could
            // become a different, apparently valid automatic paste target.
            let mut stop_lease=StopTargetLease{hub:self,id:&id,generation,armed:true};
            let control=self.stop_target_control.lock().unwrap().clone();
            let anchor=anchor().and_then(|anchor|match (anchor,control){
                (Some(anchor),Some(control))=>control(anchor).map(Some).map_err(stop_control_failure),
                (None,_)=>Err(platform::PlatformError::TargetCapture(platform::TargetCaptureFailure::StopAnchorUnavailable)),
                (anchor,_)=>Ok(anchor),
            });
            // Freeze/close Mode before announcing a phase change that asks
            // the renderer to close it too. UI control is bounded to 3s;
            // expensive accessibility work stays on its separate worker.
            self.publish_session();
            let hub=Arc::clone(self);
            let job_id=id.clone();
            let inspection=tokio::task::spawn_blocking(move||hub.resolve_stop_target_result(&job_id,generation,||{
                // A Mode menu can temporarily own focus. Close it and restore
                // only its verified pre-menu foreground before freezing caret.
                // The normal no-activate bar does not change external focus.
                match anchor {Ok(Some(anchor))=>capture(anchor),Ok(None)=>Ok(None),Err(error)=>Err(error)}
            }));
            // The queued/running blocking worker now owns release. Dropping an
            // IPC observer must not announce that this native work has ended.
            stop_lease.armed=false;
            inspection.await.map_err(|_|ApiError::new("target_capture_failed","native","录音已结束，输入位置未能确认；成稿将保留供复制"))?;
        }else{self.publish_session();}
        let mut i=self.inner.lock().unwrap();wire(matching(&mut i,p)?.view())
    }
    pub async fn stop_session(self:&Arc<Self>,p:&Value)->Result<Value> {
        self.stop_session_at(p,Instant::now()).await
    }
    #[cfg(test)]
    async fn stop_session_at_with_target(self:&Arc<Self>,p:&Value,intent:Instant,
        anchor:impl FnOnce()->Option<platform::delivery::ForegroundAnchor>,
        capture:impl FnOnce(platform::delivery::ForegroundAnchor)->Option<platform::delivery::TargetSnapshot>+Send+'static)->Result<Value> {
        self.stop_session_at_with_target_result(p,intent,||Ok(anchor()),move|anchor|Ok(capture(anchor))).await
    }
    #[cfg(test)]
    fn resolve_stop_target(&self,id:&str,generation:u64,capture:impl FnOnce()->Option<platform::delivery::TargetSnapshot>) {
        self.resolve_stop_target_result(id,generation,||Ok(capture()));
    }
    #[cfg(test)]
    fn stop_with_capture(&self,p:&Value,capture:impl FnOnce()->Option<platform::delivery::TargetSnapshot>)->Result<Value> {
        if let Some((id,generation))=self.begin_stop_target(p,Instant::now())?{self.resolve_stop_target(&id,generation,capture);}
        let mut i=self.inner.lock().unwrap();wire(matching(&mut i,p)?.view())
    }
    fn begin_cancel(&self,p:&Value)->Result<(String,u64,Arc<CommitGuard>,bool,Option<String>)> {
        let mut i=self.inner.lock().unwrap();let s=matching(&mut i,p)?;
        self.diagnostics.mark(s.diagnostic_trace.as_ref(),Stage::CancelIntent);
        s.stop.store(true,Ordering::SeqCst);s.cancel.cancel();s.discard_input_context();
        s.level=0.0;
        let revoke=s.delivery_permit.as_ref().is_some_and(|permit|permit.cancel_before_injection());
        if revoke{if let Some(d)=s.delivery.as_mut(){if d.state==DeliveryState::Prepared{d.state=DeliveryState::Skipped;}}}
        let id=s.id.clone();let generation=s.generation;let completed=s.phase==SessionPhase::Ready;
        let attempt=s.delivery.as_ref().filter(|d|d.state==DeliveryState::Skipped).map(|d|d.id.clone());
        let guard=i.guards.entry(id.clone()).or_insert_with(||Arc::new(CommitGuard::new(id.clone(),generation))).clone();
        let decision=guard.cancel();let completed=completed||decision==CancelDecision::AlreadyCompleted;
        let s=i.active.as_mut().unwrap();s.cancel_pending=!completed;
        if !completed{s.db_pending+=1;}
        // A successful Ready whose actor reply is still in flight is already
        // authoritative. Cancellation cannot erase it or its once-only Usage.
        if completed&&s.phase!=SessionPhase::Ready{if let Some((_,PendingCommit::Ready(output)))=&s.pending{s.output=Some(output.clone());s.phase=SessionPhase::Ready;s.pending=None;s.error=None;}}
        Ok((id,generation,guard,completed,attempt))
    }
    fn finish_cancel(&self,id:&str,generation:u64,result:Result<DbValue>)->Result<Value> {
        let completed=matches!(&result,Err(error) if error.code=="alreadyCompleted");
        {let mut i=self.inner.lock().unwrap();if let Some(s)=i.active.as_mut().filter(|s|s.id==id&&s.generation==generation){
            s.db_pending=s.db_pending.saturating_sub(1);s.cancel_pending=false;
            if completed{if let Some((_,PendingCommit::Ready(output)))=&s.pending{s.output=Some(output.clone());s.phase=SessionPhase::Ready;s.pending=None;s.error=None;}}
            else if result.is_ok(){self.diagnostics.outcome(s.diagnostic_trace.as_ref(),"cancelled",None);s.discard_deleted_content();}
            else{let error=ApiError::new("cancel_cleanup_failed","storage","口述已停止，未能清理本机未完成内容；请再次放弃恢复").for_session(id,generation);s.phase=SessionPhase::Failed;s.error=Some(error);}
        }}self.publish_session();self.changed();
        if completed{return Ok(json!({"status":"alreadyCompleted"}));}
        result.map_err(|_|ApiError::new("cancel_cleanup_failed","storage","口述已停止，未能清理本机未完成内容；请再次放弃恢复").for_session(id,generation))?;
        Ok(json!({"status":"cancelled"}))
    }
    /// Non-UI compatibility entry. Immediate stop/cancel is decided before any
    /// worker wait. Desktop calls this only on its blocking cleanup worker.
    pub fn cancel_session(&self,p:&Value)->Result<Value> {
        let(id,generation,guard,completed,attempt)=self.begin_cancel(p)?;self.publish_session();
        if completed{if let Some(attempt)=attempt{let _=self.db_blocking(DbCommand::TransitionDelivery{attempt_id:attempt,expected:DeliveryState::Prepared,next:DeliveryState::Skipped,reason:Some("cancelledBeforeInjection".into())},Some(guard),DbPriority::Critical);}return Ok(json!({"status":"alreadyCompleted"}));}
        let result=self.db_blocking(DbCommand::CancelUnfinished{session_id:id.clone()},Some(guard),DbPriority::Critical);
        self.finish_cancel(&id,generation,result)
    }
    pub async fn cancel_session_async(self:&Arc<Self>,p:&Value)->Result<Value> {
        let(id,generation,guard,completed,attempt)=self.begin_cancel(p)?;self.publish_session();
        if completed{if let Some(attempt)=attempt{let _=self.db_write(DbCommand::TransitionDelivery{attempt_id:attempt,expected:DeliveryState::Prepared,next:DeliveryState::Skipped,reason:Some("cancelledBeforeInjection".into())},guard,DbPriority::Critical).await;}return Ok(json!({"status":"alreadyCompleted"}));}
        let result=self.db_write(DbCommand::CancelUnfinished{session_id:id.clone()},guard,DbPriority::Critical).await;
        self.finish_cancel(&id,generation,result)
    }
    fn apply_session_mode(&self,p:&Value,mode:ModeSnapshot)->Result<Value> {
        let value={let mut i=self.inner.lock().unwrap();let s=matching(&mut i,p)?;
            if s.cancel.is_cancelled()||!matches!(s.phase,SessionPhase::Starting|SessionPhase::Recording){return Err(ApiError::new("mode_frozen","session","本次整理方式已经固定"));}s.mode=mode;self.diagnostics.mode(s.diagnostic_trace.as_ref(),&s.mode);wire(s.view())?};self.publish_session();Ok(value)
    }
    pub fn select_session_mode(&self,p:&Value)->Result<Value> {
        let mode=db_mode(self.db_blocking(DbCommand::ModeSnapshot{id:text(p,"modeId")?.into(),revision:p.get("revision").and_then(Value::as_u64)},None,DbPriority::Normal)?)?;self.apply_session_mode(p,mode)
    }
    pub async fn select_session_mode_async(&self,p:&Value)->Result<Value> {
        let mode=db_mode(self.db_read(DbCommand::ModeSnapshot{id:text(p,"modeId")?.into(),revision:p.get("revision").and_then(Value::as_u64)}).await?)?;self.apply_session_mode(p,mode)
    }
    pub fn shutdown(&self)->Result<()> {
        let mut first_error=None;
        let current={let mut i=self.inner.lock().unwrap();self.quitting.store(true,Ordering::SeqCst);self.capture_available.store(false,Ordering::SeqCst);self.lifecycle_epoch.fetch_add(1,Ordering::SeqCst);i.discard_retained_recovery();if let Some(s)=i.active.as_mut(){s.cache=None;s.cache_complete=false;s.cache_created=None;}i.active.as_ref().map(|s|json!({"sessionId":s.id,"generation":s.generation}))};
        self.publish("recovery.changed",Value::Null);
        self.cancel_auxiliary();
        if self.db.owner_released(){let mut i=self.inner.lock().unwrap();if let Some(s)=i.active.as_mut(){s.revoke_for_deletion();}}
        else if let Some(p)=current{if let Err(error)=self.cancel_session(&p){first_error=Some(error);}}
        if let Err(error)=self.cancel_active_shortcut_capture(){first_error.get_or_insert(error);}
        let hooks_stopped={let mut hooks=self.hooks.lock().unwrap();match hooks.as_mut(){Some(h)=>match h.shutdown(Duration::from_secs(5)){Ok(())=>true,Err(error)=>{first_error.get_or_insert_with(||error.into());false}},None=>false}};
        // Native owner exit is stronger evidence than an OFF ACK. Do not clear
        // an unresolved capture if no listener exit has been established.
        if hooks_stopped{let mut i=self.inner.lock().unwrap();if let Some(guard)=i.capture_commit_guard.take(){guard.cancel();}i.shortcut_capture=None;}
        if !self.db.owner_released(){self.db_blocking(DbCommand::ClearTemporary,Some(self.operation_guard()),DbPriority::Critical)?;}match first_error{Some(error)=>Err(error),None=>Ok(())}
    }
    pub async fn toggle_from_hotkey(self:&Arc<Self>)->Result<()> {
        self.toggle_from_hotkey_at(Instant::now()).await
    }
    pub async fn toggle_from_hotkey_at(self:&Arc<Self>,intent:Instant)->Result<()> {
        let current={let i=self.inner.lock().unwrap();if i.shortcut_capture.is_some(){return Err(ApiError::new("capture_busy","shortcut","请先结束快捷键录制"));}i.active.as_ref().map(|s|(s.phase,s.busy(),json!({"sessionId":s.id,"expectedGeneration":s.generation})))};
        if let Some((phase,busy,p))=current {
            if matches!(phase,SessionPhase::Starting|SessionPhase::Recording){self.stop_session_at(&p,intent).await.map_err(|e|e.for_session(p["sessionId"].as_str().unwrap(),p["expectedGeneration"].as_u64().unwrap()))?;return Ok(());}
            if busy{return Err(ApiError::new("session_busy","session","正在处理或释放上一条口述，请稍候").for_session(p["sessionId"].as_str().unwrap(),p["expectedGeneration"].as_u64().unwrap()));}
        }
        self.start_session_at(intent,Origin::HotkeyHook).await.map(|_|())
    }
    pub async fn interrupt_session(self:&Arc<Self>,reason:&str)->Result<()> {
        let epoch=self.reserve_lifecycle_epoch();self.interrupt_session_at_epoch(reason,epoch).await
    }
    /// Called synchronously at the native event boundary. Async scheduling must
    /// not turn an older Locked event into a newer lifecycle decision.
    pub fn reserve_lifecycle_epoch(&self)->u64 {
        self.capture_available.store(false,Ordering::SeqCst);self.lifecycle_epoch.fetch_add(1,Ordering::SeqCst)+1
    }
    pub async fn interrupt_session_at_epoch(self:&Arc<Self>,reason:&str,epoch:u64)->Result<()> {
        let (current,capture)={let mut i=self.inner.lock().unwrap();if self.lifecycle_epoch.load(Ordering::SeqCst)!=epoch{return Ok(());}self.capture_available.store(false,Ordering::SeqCst);i.discard_retained_recovery();
            // Ready cancellation may return alreadyCompleted without cleanup.
            // Release audio at the lifecycle boundary while retaining its text.
            if let Some(s)=i.active.as_mut(){s.cache=None;s.cache_complete=false;s.cache_created=None;}
            (i.active.as_ref().map(|s|json!({"sessionId":s.id,"expectedGeneration":s.generation})),i.shortcut_capture.clone())};
        self.publish("recovery.changed",Value::Null);
        let mut first_error=None;
        if !self.cancel_auxiliary_at_epoch(epoch){return Ok(());}
        if let Some(p)=current{if let Err(error)=self.cancel_session_async(&p).await{first_error=Some(error);}}
        // Native OFF uses a bounded ACK; run it away from the async executor.
        let hub=Arc::clone(self);
        if let Some(capture)=capture{if let Err(error)=tokio::task::spawn_blocking(move||hub.cancel_shortcut_capture(&json!({"captureId":capture})).map(|_|())).await.map_err(|_|ApiError::new("system_cleanup_failed","native","快捷键录制未能释放"))?{first_error.get_or_insert(error);}}
        if self.lifecycle_epoch.load(Ordering::SeqCst)!=epoch{return Ok(());}
        self.publish("interrupted",json!({"reason":reason}));let _=self.native_action("recording",false);
        let deadline=Instant::now()+Duration::from_secs(5);
        loop {
                if self.lifecycle_epoch.load(Ordering::SeqCst)!=epoch{return Ok(());}
                let released={let i=self.inner.lock().unwrap();i.shortcut_capture.is_none()&&i.auxiliary_workers.is_empty()&&i.active.as_ref().is_none_or(|s|s.worker_released&&s.processing_worker_released&&s.delivery_worker_released&&!s.target_capture_pending&&s.db_pending==0&&!s.cancel_pending&&!s.error.as_ref().is_some_and(|e|e.code=="cancel_cleanup_failed"))};
                if released&&self.db.outstanding()==0&&self.db_command_workers.load(Ordering::SeqCst)==0&&platform::audio::capture_resources_released()&&self.native_io.try_lock().is_ok(){break;}
                if Instant::now()>=deadline{return Err(ApiError::new("system_cleanup_busy","native","上一条录音尚未释放，恢复后请稍候"));}
                tokio::time::sleep(Duration::from_millis(20)).await;
        }
        if let Some(error)=first_error{return Err(error);}
        if matches!(reason,"Unlocked"|"Resumed"|"Connected"|"HooksReinstalled")&&!self.quitting.load(Ordering::SeqCst) {
            let hub=Arc::clone(self);
            tokio::task::spawn_blocking(move|| {
                let _io=hub.shortcut_io.lock().unwrap();
                if hub.lifecycle_epoch.load(Ordering::SeqCst)!=epoch||hub.quitting.load(Ordering::SeqCst){return Ok(());}
                {let hooks=hub.hooks.lock().unwrap();if let Some(hooks)=hooks.as_ref(){hooks.resume()?;if hub.lifecycle_epoch.load(Ordering::SeqCst)!=epoch||hub.quitting.load(Ordering::SeqCst){hooks.suspend()?;return Ok(());}}}
                if hub.lifecycle_epoch.load(Ordering::SeqCst)==epoch&&!hub.quitting.load(Ordering::SeqCst){hub.capture_available.store(true,Ordering::SeqCst);}Ok::<_,ApiError>(())
            }).await.map_err(|_|ApiError::new("system_resume_failed","native","系统恢复失败"))??;
        }
        Ok(())
    }
    pub async fn shutdown_ready(self:&Arc<Self>)->Result<()> {
        let hub=Arc::clone(self);
        let cleanup=tokio::task::spawn_blocking(move||hub.shutdown()).await.map_err(|_|ApiError::new("shutdown_failed","native","退出清理未完成"))?;
        let deadline=Instant::now()+Duration::from_secs(5);
        loop {
            let released={let i=self.inner.lock().unwrap();i.shortcut_capture.is_none()&&i.auxiliary_workers.is_empty()&&i.active.as_ref().is_none_or(|s|s.worker_released&&s.processing_worker_released&&s.delivery_worker_released&&!s.target_capture_pending&&s.db_pending==0&&!s.cancel_pending)};
            if released&&self.db.outstanding()==0&&self.db_command_workers.load(Ordering::SeqCst)==0&&platform::audio::capture_resources_released()&&self.native_io.try_lock().is_ok(){break;}
            if Instant::now()>=deadline{return Err(ApiError::new("shutdown_busy","native","采音或输入尚未确认释放，请稍候再退出"));}
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        cleanup?;
        self.db.submit(DbCommand::Shutdown,None,DbPriority::Critical)?.wait().await?;
        while !self.db.owner_released(){if Instant::now()>=deadline{return Err(ApiError::new("shutdown_busy","storage","数据库所有者尚未释放"));}tokio::time::sleep(Duration::from_millis(10)).await;}
        Ok(())
    }
    fn fail(&self,id:&str,generation:u64,error:ApiError,resources_released:bool) {
        let error=error.for_session(id,generation);
        {let mut i=self.inner.lock().unwrap();if let Some(s)=i.active.as_mut().filter(|s|s.id==id&&s.generation==generation){s.worker_released=resources_released;
            if s.phase!=SessionPhase::Ready {s.discard_input_context();}
            if !matches!(s.phase,SessionPhase::Cancelled|SessionPhase::Ready)&&!s.error.as_ref().is_some_and(|e|matches!(e.code.as_str(),"cancel_cleanup_failed"|"asr_retry_expired")){s.phase=if matches!(error.code.as_str(),"asr_finish_timeout"|"asr_disconnected"|"audio_incomplete"|"audio_capture_stalled"|"audio_capture_device_error"|"audio_capture_queue_overflow"|"audio_capture_malformed"|"recording_limit"|"cache_overflow"|"capture_overflow"|"missing_order"|"invalid_order"|"conflicting_order"|"conflicting_final"|"incomplete_source"|"asr_closed"|"asr_send_timeout"|"asr_receive_failed"|"invalid_final"|"invalid_asr_event")||(error.stage=="asr"&&s.first_samples){SessionPhase::Incomplete}else{SessionPhase::Failed};self.diagnostics.outcome(s.diagnostic_trace.as_ref(),"failed",Some(&error.code));s.error=Some(error);}
        }}self.publish_session();self.changed();
    }
    fn valid_active(&self,id:&str,generation:u64)->bool {self.inner.lock().unwrap().active.as_ref().is_some_and(|s|s.id==id&&s.generation==generation&&!s.cancel.is_cancelled())}
    fn recording_progress(&self,id:&str,generation:u64,duration_ms:u64)->bool {
        let (publish,reached)={let mut i=self.inner.lock().unwrap();
            let Some(s)=i.active.as_mut().filter(|s|s.id==id&&s.generation==generation&&!s.cancel.is_cancelled()&&!s.stop.load(Ordering::SeqCst)&&matches!(s.phase,SessionPhase::Starting|SessionPhase::Recording))else{return false;};
            s.capture_duration_ms=duration_ms;
            let remaining=recording_limit_remaining(duration_ms);
            let publish=remaining.map(|ms|ms.div_ceil(1000))!=s.recording_limit_remaining_ms.map(|ms|ms.div_ceil(1000));
            s.recording_limit_remaining_ms=remaining;
            let reached=remaining==Some(0);
            if reached {
                s.phase=SessionPhase::Incomplete;
                s.error=Some(ApiError::new("recording_limit","audio","已到本次录音时长上限。已有字幕可复制；本次不会自动整理或输入，请重新口述剩余内容。").for_session(id,generation));
                s.ended_at=Some(chrono::Local::now().to_rfc3339());
            }
            (publish||reached,reached)
        };
        if publish {self.publish_session();}
        reached
    }

    async fn record_pipeline(self:Arc<Self>,id:String,generation:u64,config:ProviderConfig,prefs:Preferences,_mode:ModeSnapshot,dictionary:DictionarySnapshot,secret:String) {
        let trace=self.trace(&id,generation);
        if !self.valid_active(&id,generation){self.fail(&id,generation,ApiError::new("cancelled","session","口述已取消"),platform::audio::capture_resources_released());return;}
        let attempt=Uuid::new_v4().to_string();
        let (cancel,stop)={let mut i=self.inner.lock().unwrap();let s=i.active.as_mut().unwrap();s.asr_attempt_id=Some(attempt.clone());(s.cancel.clone(),s.stop.clone())};
        self.diagnostics.attempt(trace.as_ref(),&attempt);
        let (audio_tx,audio_rx)=mpsc::channel(4);
        let (done_tx,done_rx)=oneshot::channel();
        let capture_hub=Arc::clone(&self);let capture_id=id.clone();let capture_cancel=cancel.clone();
        std::thread::spawn(move|| {
            let result=pump_audio(&capture_hub,&capture_id,generation,prefs.microphone_id.as_deref(),prefs.soft_voice_enabled,audio_tx,capture_cancel,stop);
            // A startup/drain timeout may return before the WASAPI owner drops.
            // Keep admission closed until the actual platform owner releases.
            while !platform::audio::capture_resources_released(){std::thread::sleep(Duration::from_millis(10));}
            {let mut i=capture_hub.inner.lock().unwrap();if let Some(s)=i.active.as_mut().filter(|s|s.id==capture_id&&s.generation==generation){s.worker_released=true;}}
            capture_hub.publish_session();
            let _=done_tx.send(result);
        });
        let event_hub=Arc::clone(&self);let event_id=id.clone();let event_attempt=attempt.clone();let callback_trace=trace.clone();
        // Diagnostic progress has no AppHub state lock, UI publish or business side effect.
        let callback=Arc::new(move|progress:AsrProgress|match progress {
            AsrProgress::Diagnostics(metadata)=>event_hub.diagnostics.asr(callback_trace.as_ref(),&metadata),
            progress=>event_hub.accept_asr_progress(&event_id,generation,&event_attempt,progress),
        });
        let result=asr::run(config.clone(),secret.clone(),prefs.language.clone(),dictionary,attempt.clone(),audio_rx,cancel.clone(),callback).await;
        if result.is_ok(){self.diagnostics.mark(trace.as_ref(),Stage::FinalAsrReceived);}
        // Any ASR early failure aborts capture; normal finish has already drained it.
        if result.is_err(){let i=self.inner.lock().unwrap();if let Some(s)=i.active.as_ref().filter(|s|s.id==id&&s.generation==generation){s.stop.store(true,Ordering::SeqCst);}}
        let capture=tokio::time::timeout(Duration::from_secs(5),done_rx).await;
        let (summary,cache)=match capture {
            Ok(Ok(Ok(v)))=>v,
            Ok(Ok(Err(e)))=>{self.fail(&id,generation,e,platform::audio::capture_resources_released());return;}
            _=>{self.fail(&id,generation,ApiError::new("capture_cleanup_failed","audio","采音资源尚未确认释放，请退出重开"),platform::audio::capture_resources_released());return;}
        };
        self.diagnostics.lengths(trace.as_ref(),summary.duration_ms(),None);
        let cache_created=Instant::now();
        self.accept_capture_cache(&id,generation,&summary,cache,cache_created);
        self.schedule_audio_cache_expiry(id.clone(),cache_created);
        if let Some(error)=capture_failure(&summary){self.fail(&id,generation,error,true);return;}
        let items=match result {Ok(v) if summary.complete=>v,Ok(_)=>{self.fail(&id,generation,ApiError::new("audio_incomplete","audio","采音未完整结束，可复制已有字幕"),true);return;},Err(e)=>{self.fail(&id,generation,e.into(),true);return;}};
        if items.is_empty(){self.fail(&id,generation,ApiError::new("empty_recording","asr","未识别到有效语音"),true);return;}
        let source=SourceRevision{id:Uuid::new_v4().to_string(),session_id:id.clone(),asr_attempt_id:attempt,generation,provider:config.provider.clone(),model:config.asr_model.clone(),adapter_version:config.adapter_version.clone(),complete:true,items,created_at:now()};
        self.diagnostics.lengths(trace.as_ref(),summary.duration_ms(),Some(source.text().chars().count()));
        if !self.valid_active(&id,generation){return;}
        if let Err(error)=self.checkpoint_async(&id,generation,source).await{self.fail(&id,generation,error,true);return;}
        self.finish_rewrite(id,generation,secret).await;
    }

    fn prepare_checkpoint(&self,id:&str,generation:u64,source:SourceRevision)->Result<(SessionSeed,String,Arc<CommitGuard>)> {
        let guard=self.record_guard(id,generation);let mut i=self.inner.lock().unwrap();let s=i.active.as_mut().filter(|s|s.id==id&&s.generation==generation&&!s.cancel.is_cancelled()).ok_or_else(||ApiError::new("cancelled","session","口述已取消").for_session(id,generation))?;
        let end=s.ended_at.clone().unwrap_or_else(||chrono::Local::now().to_rfc3339());let local=chrono::DateTime::parse_from_rfc3339(&end).map_err(|_|ApiError::new("invalid_clock","session","本机结束时间无效").for_session(id,generation))?;
        let seed=SessionSeed{id:id.into(),generation,started_at:s.started_at.clone(),ended_at:end.clone(),capture_duration_ms:s.capture_duration_ms,end_local_date:local.format("%Y-%m-%d").to_string(),time_zone:local.offset().to_string(),storage_policy:if s.prefs.keep_history{StoragePolicy::History}else{StoragePolicy::MemoryOnly},mode_snapshot:s.mode.clone()};
        s.source=Some(source.clone());s.phase=SessionPhase::Rewriting;let pending_id=Uuid::new_v4().to_string();s.pending=Some((pending_id.clone(),PendingCommit::Source(seed.clone(),source)));s.db_pending+=1;
        Ok((seed,pending_id,guard))
    }
    fn checkpoint_receipt(&self,id:&str,generation:u64,pending_id:String,result:Result<DbValue>)->Result<()> {
        let mut i=self.inner.lock().unwrap();if let Some(s)=i.active.as_mut().filter(|s|s.id==id&&s.generation==generation){s.db_pending=s.db_pending.saturating_sub(1);
            if result.is_ok()&&s.pending.as_ref().is_some_and(|(key,_)|key==&pending_id){s.pending=None;self.diagnostics.mark(s.diagnostic_trace.as_ref(),Stage::SourceCheckpointCommitted);}}
        result.map_err(|mut error|{error=error.for_session(id,generation);error.pending_commit_id=Some(pending_id);error.commit_stage=Some("sourceCheckpoint".into());error.can_retry_save=true;error.retry_kind=Some("save".into());error})?;Ok(())
    }
    fn checkpoint(&self,id:&str,generation:u64,source:SourceRevision)->Result<()> {
        let(seed,pending,guard)=self.prepare_checkpoint(id,generation,source.clone())?;
        let result=self.db_blocking(DbCommand::Checkpoint{seed,source},Some(guard),DbPriority::Critical);self.checkpoint_receipt(id,generation,pending,result)
    }
    async fn checkpoint_async(self:&Arc<Self>,id:&str,generation:u64,source:SourceRevision)->Result<()> {
        let(seed,pending,guard)=self.prepare_checkpoint(id,generation,source.clone())?;let hub=Arc::clone(self);let id=id.to_owned();
        self.db_command_workers.fetch_add(1,Ordering::SeqCst);
        tokio::spawn(async move{let _lease=CommandLease(Arc::clone(&hub));let result=hub.db_write(DbCommand::Checkpoint{seed,source},guard,DbPriority::Critical).await;hub.checkpoint_receipt(&id,generation,pending,result)}).await.map_err(|_|ApiError::new("save_interrupted","storage","来源保存未确认，请核对本机记录"))?
    }
    fn recording_rewrite_dictionary(&self,id:&str,generation:u64)->Option<DictionarySnapshot> {
        let i=self.inner.lock().unwrap();
        i.active.as_ref().filter(|s|s.id==id&&s.generation==generation&&!s.cancel.is_cancelled()).map(|s|s.dictionary.clone())
    }
    async fn finish_rewrite(self:&Arc<Self>,id:String,generation:u64,secret:String) {
        // Reuse the vocabulary frozen when this recording began, not a live
        // dictionary that the user may have changed while speaking.
        let Some(dictionary)=self.recording_rewrite_dictionary(&id,generation)else{return;};
        self.finish_rewrite_with_context_policy(id,generation,move|config,mode,source,cancel,execution,context|async move{
            if let Some(context)=context {
                rewrite::rewrite_with_writing_context(&config,&secret,&mode,&source,&dictionary,cancel,execution,&context).await
            } else {rewrite::rewrite_with_execution(&config,&secret,&mode,&source,&dictionary,cancel,execution).await}
        },None::<RewriteWaitPolicy>,|hub,id,generation| {
            let hub=Arc::clone(hub);let id=id.to_owned();
            tokio::task::spawn_blocking(move||hub.initial_delivery(&id,generation));
        }).await;
    }

    // Private completion seam: tests supply a parsed fixture future while using
    // the same checkpoint, cancellation, storage and delivery gates. Production
    // always uses the cloud wrapper, with its allowlisted endpoint and a source-bound wait budget.
    #[cfg(test)]
    async fn finish_rewrite_with<F>(self:&Arc<Self>,id:String,generation:u64,
        generate:impl FnOnce(ProviderConfig,ModeSnapshot,SourceRevision,CancellationToken,rewrite::execution::RewriteExecution)->F)
        where F:std::future::Future<Output=std::result::Result<rewrite::RewriteResult,CloudError>> {
        self.finish_rewrite_with_policy(id,generation,generate,None::<RewriteWaitPolicy>,|hub,id,generation| {
            let hub=Arc::clone(hub);let id=id.to_owned();
            tokio::task::spawn_blocking(move||hub.initial_delivery(&id,generation));
        }).await;
    }
    // The delivery seam permits real SQLite/coordinator tests without host input or clipboard IO.
    // None selects production policy from the same frozen snapshot sent to the provider;
    // tests can inject a fixed budget without changing the production selection path.
    #[cfg(test)]
    async fn finish_rewrite_with_policy<F>(self:&Arc<Self>,id:String,generation:u64,
        generate:impl FnOnce(ProviderConfig,ModeSnapshot,SourceRevision,CancellationToken,rewrite::execution::RewriteExecution)->F,
        policy:impl Into<Option<RewriteWaitPolicy>>,deliver:impl FnOnce(&Arc<Self>,&str,u64))
        where F:std::future::Future<Output=std::result::Result<rewrite::RewriteResult,CloudError>> {
        self.finish_rewrite_with_context_policy(id,generation,|config,mode,source,cancel,execution,_context|
            generate(config,mode,source,cancel,execution),policy,deliver).await;
    }
    async fn finish_rewrite_with_context_policy<F>(self:&Arc<Self>,id:String,generation:u64,
        generate:impl FnOnce(ProviderConfig,ModeSnapshot,SourceRevision,CancellationToken,rewrite::execution::RewriteExecution,Option<WritingContext>)->F,
        policy:impl Into<Option<RewriteWaitPolicy>>,deliver:impl FnOnce(&Arc<Self>,&str,u64))
        where F:std::future::Future<Output=std::result::Result<rewrite::RewriteResult,CloudError>> {
        let trace=self.trace(&id,generation);
        let policy={let i=self.inner.lock().unwrap();let Some(s)=i.active.as_ref().filter(|s|s.id==id&&s.generation==generation&&!s.cancel.is_cancelled())else{return;};
            policy.into().unwrap_or_else(||RewriteWaitPolicy::for_source(s.reason,s.source.as_ref().expect("checkpoint before rewrite")))};
        let rewrite_deadline=tokio::time::Instant::now()+policy.deadline;
        // ASR finalization overlaps stop-time capture. Give it a bounded grace
        // inside the existing total rewrite deadline, never an extra budget.
        let context_deadline=(tokio::time::Instant::now()+Duration::from_millis(600)).min(rewrite_deadline);
        loop {
            let pending={let i=self.inner.lock().unwrap();i.active.as_ref().is_some_and(|s|s.id==id&&s.generation==generation
                &&!s.cancel.is_cancelled()&&s.target_capture_pending&&(s.prefs.app_writing.input_context_enabled
                    || (s.prefs.app_writing.enabled&&s.mode.mode_id=="auto"&&s.mode.rules.is_empty())))};
            if !pending || tokio::time::Instant::now()>=context_deadline {break;}
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        // Freeze the scene before generation. A late target inspection may still
        // authorize delivery, but cannot change this draft's presentation policy.
        let (config,mode,source,cancel,reason,writing_context)={let mut i=self.inner.lock().unwrap();let Some(s)=i.active.as_mut().filter(|s|s.id==id&&s.generation==generation&&!s.cancel.is_cancelled())else{return;};
            let source=s.source.clone().unwrap();let frozen=app_writing::freeze(s);
            s.writing_scene=frozen.as_ref().map(|context|context.scene);
            s.input_context_status=if !s.prefs.app_writing.input_context_enabled {InputContextStatus::Disabled}
                else if frozen.as_ref().is_some_and(|context|context.input.is_some()){InputContextStatus::Available}else{InputContextStatus::Unavailable};
            s.input_context=None;s.input_context_frozen=true;
            (s.config.clone(),s.mode.clone(),source,s.cancel.clone(),s.reason,frozen)};
        self.publish_session();
        self.diagnostics.rewrite_started(trace.as_ref(),if writing_context.is_some(){rewrite::writing_context::VERSION}else{rewrite::production_protocol_version(&config,&mode)},policy.deadline.as_millis().min(u64::MAX as u128) as u64);
        let rewrite_cancel=cancel.child_token();
        let observer_hub=Arc::downgrade(self);let observer_trace=trace.clone();
        let execution=rewrite::execution::RewriteExecution::observed(rewrite_deadline,move|event|{
            if let Some(hub)=observer_hub.upgrade(){hub.diagnostics.rewrite_request(observer_trace.as_ref(),event);}
        });
        let generated=tokio::select! {
            biased;
            _=cancel.cancelled()=>Err(CloudError::new("cancelled","rewrite","已取消整理")),
            result=tokio::time::timeout_at(rewrite_deadline,generate(config,mode.clone(),source.clone(),rewrite_cancel.clone(),execution,writing_context.clone()))=>match result {
                Ok(result)=>result,
                Err(_)=>{rewrite_cancel.cancel();Err(CloudError::new("rewrite_timeout","rewrite","整理等待已结束，使用完整识别稿"))},
            },
        };
        // A completed parser/transport response cannot revive a cancelled or
        // replaced session, even if its future returned success after cutoff.
        if !self.valid_active(&id,generation){
            self.diagnostics.rewrite_finished(trace.as_ref(),RewriteOutcome::Cancelled,None,Some("cancelled"));
            return;
        }
        // Synchronous decoding/validation can finish without yielding to the
        // timer. It must not turn an already expired request into a late paste.
        let generated=if tokio::time::Instant::now()>=rewrite_deadline && !generated.as_ref().err().is_some_and(|error|error.code=="cancelled") {
            rewrite_cancel.cancel();
            Err(CloudError::new("rewrite_timeout","rewrite","整理等待已结束，使用完整识别稿"))
        } else { generated };
        let (generated,rewrite_fallback)=match generated {
            Ok(value)=>{
                self.diagnostics.mark(trace.as_ref(),Stage::RewriteValidated);
                self.diagnostics.rewrite_finished(trace.as_ref(),RewriteOutcome::Validated,None,None);
                (value,None)
            },
            Err(error)=>{
                let fallback=rewrite_fallback::fallback_reason(&error);
                let eligible=reason==CreationReason::NormalInitial && fallback.is_some() && rewrite::validate_source(&source).is_ok()
                    && source.session_id==id && source.generation==generation
                    && self.inner.lock().unwrap().active.as_ref().is_some_and(|s|s.id==id&&s.generation==generation
                        && s.phase==SessionPhase::Rewriting && s.pending.is_none() && s.db_pending==0 && !s.original_delivery_revoked
                        && s.source.as_ref().is_some_and(|saved|saved==&source));
                if !eligible {
                    self.diagnostics.rewrite_finished(trace.as_ref(),if error.code=="cancelled" {RewriteOutcome::Cancelled} else {RewriteOutcome::Failed},None,Some(&error.code));
                    self.fail(&id,generation,error.into(),true);return;
                }
                self.diagnostics.mark(trace.as_ref(),Stage::RewriteFallbackSelected);
                self.diagnostics.rewrite_finished(trace.as_ref(),RewriteOutcome::Fallback,fallback,Some(&error.code));
                (rewrite_fallback::source_fallback(&source),fallback)
            },
        };
        if rewrite_fallback.is_some(){let mut i=self.inner.lock().unwrap();if let Some(s)=i.active.as_mut().filter(|s|s.id==id&&s.generation==generation){s.writing_scene=None;s.input_context_status=if s.prefs.app_writing.input_context_enabled {InputContextStatus::Unavailable}else{InputContextStatus::Disabled};}}
        let receipt=writing_context.as_ref().filter(|_|rewrite_fallback.is_none()).map(|context|crate::core::writing_context::WritingReceipt{scene:context.scene,language:context.language,context_used:context.input.is_some(),contract_version:rewrite::writing_context::VERSION.into()});
        let output=OutputRevision{writing_receipt:receipt, confirmed_context:None, id:Uuid::new_v4().to_string(),session_id:id.clone(),parent_revision_id:None,base_revision_id:None,source_revision_id:source.id,mode_snapshot:mode,resolved_mode:generated.resolved_mode,rewrite_fallback,blocks:generated.blocks,kind:RevisionKind::Initial,creation_reason:reason,created_at:now()};
        if let Err(e)=self.commit_output_async(&id,generation,output).await{self.fail(&id,generation,e,true);return;}
        self.publish_session();self.changed();deliver(self,&id,generation);
    }
    fn prepare_output(&self,id:&str,generation:u64,output:OutputRevision)->Result<(String,Arc<CommitGuard>)> {
        let guard=self.record_guard(id,generation);let mut i=self.inner.lock().unwrap();let s=i.active.as_mut().filter(|s|s.id==id&&s.generation==generation&&(!s.cancel.is_cancelled()||s.phase==SessionPhase::Ready)).ok_or_else(||ApiError::new("cancelled","session","口述已取消").for_session(id,generation))?;
        if guard.is_cancelled(){return Err(ApiError::new("cancelled","session","口述已取消").for_session(id,generation));}
        let pending_id=Uuid::new_v4().to_string();s.pending=Some((pending_id.clone(),PendingCommit::Ready(output)));s.db_pending+=1;Ok((pending_id,guard))
    }
    fn output_receipt(&self,id:&str,generation:u64,pending_id:String,output:OutputRevision,guard:Arc<CommitGuard>,result:Result<DbValue>)->Result<()> {
        let mut i=self.inner.lock().unwrap();if let Some(s)=i.active.as_mut().filter(|s|s.id==id&&s.generation==generation){s.db_pending=s.db_pending.saturating_sub(1);
            if result.is_ok()&&!guard.is_cancelled(){s.pending=None;s.output=Some(output);s.phase=SessionPhase::Ready;s.cache=None;s.cache_complete=false;s.error=None;
                self.diagnostics.mark(s.diagnostic_trace.as_ref(),Stage::ReadyCommitted);self.diagnostics.outcome(s.diagnostic_trace.as_ref(),"ready",None);
                s.delivery_worker_released=!(s.reason==CreationReason::NormalInitial&&s.prefs.delivery_mode==DeliveryMode::Direct&&!s.cancel.is_cancelled());}}
        result.map_err(|mut error|{error=error.for_session(id,generation);error.pending_commit_id=Some(pending_id);error.commit_stage=Some("ready".into());error.can_retry_save=true;error.retry_kind=Some("save".into());error})?;Ok(())
    }
    fn commit_output(&self,id:&str,generation:u64,output:OutputRevision)->Result<()> {
        let(pending,guard)=self.prepare_output(id,generation,output.clone())?;let result=self.db_blocking(DbCommand::Ready{output:output.clone()},Some(Arc::clone(&guard)),DbPriority::Critical);self.output_receipt(id,generation,pending,output,guard,result)
    }
    async fn commit_output_async(self:&Arc<Self>,id:&str,generation:u64,output:OutputRevision)->Result<()> {
        let(pending,guard)=self.prepare_output(id,generation,output.clone())?;let hub=Arc::clone(self);let id=id.to_owned();self.db_command_workers.fetch_add(1,Ordering::SeqCst);
        tokio::spawn(async move{let _lease=CommandLease(Arc::clone(&hub));let result=hub.db_write(DbCommand::Ready{output:output.clone()},Arc::clone(&guard),DbPriority::Critical).await;hub.output_receipt(&id,generation,pending,output,guard,result)}).await.map_err(|_|ApiError::new("save_interrupted","storage","结果保存未确认，请核对本机记录"))?
    }
    fn initial_delivery(&self,id:&str,generation:u64) {
        self.initial_delivery_with(id,generation,|target|platform::delivery::validate_target(target).is_ok(),
            |permit,target,text,gate|platform::delivery::dispatch_once(permit,target,text,gate));
    }
    fn initial_delivery_with(&self,id:&str,generation:u64,
        validate:impl Fn(&platform::delivery::TargetSnapshot)->bool,
        dispatch:impl Fn(&platform::delivery::DeliveryPermit,&platform::delivery::TargetSnapshot,&str,&dyn Fn()->bool)->platform::delivery::DispatchReceipt) {
        let trace=self.trace(id,generation);
        struct Finished<'a>{hub:&'a AppHub,id:&'a str,generation:u64}
        impl Drop for Finished<'_>{fn drop(&mut self){{let mut i=self.hub.inner.lock().unwrap();if let Some(s)=i.active.as_mut().filter(|s|s.id==self.id&&s.generation==self.generation){s.delivery_worker_released=true;s.delivery_permit=None;}}self.hub.publish_session();}}
        let _finished=Finished{hub:self,id,generation};
        // Failed/terminal delivery decisions are never reset by a duplicate
        // completion callback, including an unavailable ledger write.
        if self.inner.lock().unwrap().active.as_ref().is_some_and(|s|s.id==id&&s.generation==generation&&(s.delivery.is_some()||s.delivery_failure_reason.is_some())){return;}
        // Stop already cut audio. Resolve its bounded native inspection before
        // making any delivery decision, including the no-target fallback.
        let deadline=Instant::now()+Duration::from_millis(4300);
        let target_timed_out=loop {
            let pending={let i=self.inner.lock().unwrap();i.active.as_ref().is_some_and(|s|s.id==id&&s.generation==generation&&!s.cancel.is_cancelled()&&!s.original_delivery_revoked&&s.target_capture_pending)};
            if !pending {break false;}
            if Instant::now()>=deadline {break true;}
            std::thread::sleep(Duration::from_millis(10));
        };
        let eligible={let i=self.inner.lock().unwrap();i.active.as_ref().filter(|s|s.id==id&&s.generation==generation&&!s.cancel.is_cancelled()&&!self.quitting.load(Ordering::SeqCst)&&!s.original_delivery_revoked&&s.phase==SessionPhase::Ready&&s.prefs.delivery_mode==DeliveryMode::Direct&&s.reason==CreationReason::NormalInitial).and_then(|s|s.output.clone().map(|output|(if target_timed_out{None}else{s.target.clone()},output,s.target_capture_failure_reason.clone())))};
        let Some((target,output,capture_failure))=eligible else{let _=self.native_action("recording",false);if self.inner.lock().unwrap().active.as_ref().is_some_and(|s|s.id==id&&s.generation==generation&&s.phase==SessionPhase::Ready&&!s.cancel.is_cancelled()&&s.prefs.delivery_mode==DeliveryMode::Preview){let _=self.native_action("showMain",true);}return;};
        let rejection=match &target{None=>Some(if target_timed_out{"targetInspectionTimeout"}else{capture_failure.as_deref().unwrap_or("targetUnavailableAtStop")}),Some(target) if !validate(target)=>Some("targetChangedBeforeDelivery"),_=>None};
        // A skipped automatic delivery is still a once-only decision. Preserve
        // an opaque ledger receipt even when no input target could be captured.
        let fingerprint=target.as_ref().map(|target|format!("{}:{}:{}",target.hwnd,target.process_id,target.process_created_at)).unwrap_or_else(||"unavailableAtStop".into());
        let attempt=DeliveryAttempt{id:Uuid::new_v4().to_string(),session_id:id.into(),generation,output_revision_id:output.id.clone(),target_fingerprint:fingerprint,claimed_at:now(),state:DeliveryState::Prepared,reason:None};
        let guard=self.record_guard(id,generation).operation();
        let attempt=match self.db_blocking(DbCommand::PrepareDelivery{attempt},Some(guard),DbPriority::Critical){Ok(DbValue::Delivery(value))=>value,_=>{
            let show={let mut i=self.inner.lock().unwrap();if let Some(s)=i.active.as_mut().filter(|s|s.id==id&&s.generation==generation&&!s.cancel.is_cancelled()&&!s.original_delivery_revoked&&s.delivery.is_none()){s.delivery_failure_reason=Some("deliveryLedgerUnavailable".into());true}else{false}};
            self.publish_session();if show{let _=self.native_action("recording",true);}return;
        }};
        self.diagnostics.mark(trace.as_ref(),Stage::DeliveryClaim);
        let permit=Arc::new(platform::delivery::DeliveryPermit::from_committed_attempt(attempt.id.clone(),generation));
        {let mut i=self.inner.lock().unwrap();if let Some(s)=i.active.as_mut().filter(|s|s.id==id&&s.generation==generation){s.delivery=Some(attempt.clone());s.delivery_permit=Some(Arc::clone(&permit));if s.cancel.is_cancelled()||s.original_delivery_revoked||s.output.as_ref().is_none_or(|current|current.id!=output.id){permit.cancel_before_injection();}}else{permit.cancel_before_injection();}}
        // COMMIT of Prepared is acknowledged before the native gate is entered.
        // Native control is released before any ledger persistence wait.
        let receipt=if let Some(reason)=rejection {
            permit.cancel_before_injection();
            platform::delivery::DispatchReceipt{outcome:platform::delivery::DispatchOutcome::Skipped,clipboard_changed:false,input_events:0,reason:Some(reason.into()),reason_code:Some(reason.into())}
        }else{let _native=self.native_io.lock().unwrap();dispatch(&permit,target.as_ref().unwrap(),&output.text(),&||{
            let i=self.inner.lock().unwrap();!self.quitting.load(Ordering::SeqCst)&&self.capture_available.load(Ordering::SeqCst)&&i.active.as_ref().is_some_and(|s|s.id==id&&s.generation==generation&&!s.cancel.is_cancelled()&&!s.original_delivery_revoked&&s.output.as_ref().is_some_and(|current|current.id==output.id)&&s.delivery.as_ref().is_some_and(|d|d.id==attempt.id&&d.output_revision_id==output.id&&d.state==DeliveryState::Prepared))
        })};
        self.diagnostics.mark(trace.as_ref(),Stage::NativeDeliveryReceipt);
        let state=match receipt.outcome{platform::delivery::DispatchOutcome::Skipped=>DeliveryState::Skipped,platform::delivery::DispatchOutcome::Failed=>DeliveryState::Failed,platform::delivery::DispatchOutcome::Unconfirmed=>DeliveryState::Unconfirmed};
        let reason=if receipt.clipboard_changed&&matches!(state,DeliveryState::Skipped)&&receipt.reason_code.as_deref()==Some("cancelledBeforeInjection"){"clipboardChangedInjectionSkipped".to_owned()}
            else if receipt.clipboard_changed&&matches!(state,DeliveryState::Failed)&&receipt.reason_code.as_deref()==Some("nativeDispatchFailed"){"clipboardChangedDispatchFailed".to_owned()}
            else{receipt.reason_code.unwrap_or_else(||match state{DeliveryState::Skipped=>"targetChangedBeforeDelivery",DeliveryState::Failed=>"nativeDispatchFailed",_=>if receipt.input_events==4{"inputDispatchedUnconfirmed"}else{"partialInputUnconfirmed"}}.to_owned())};
        let persisted=if receipt.input_events>0{
            self.db_blocking(DbCommand::TransitionDelivery{attempt_id:attempt.id.clone(),expected:DeliveryState::Prepared,next:DeliveryState::Dispatched,reason:None},None,DbPriority::Critical)
                .and_then(|_|self.db_blocking(DbCommand::TransitionDelivery{attempt_id:attempt.id.clone(),expected:DeliveryState::Dispatched,next:state,reason:Some(reason.clone())},None,DbPriority::Critical))
        }else{self.db_blocking(DbCommand::TransitionDelivery{attempt_id:attempt.id.clone(),expected:DeliveryState::Prepared,next:state,reason:Some(reason.clone())},None,DbPriority::Critical)};
        {let mut i=self.inner.lock().unwrap();if let Some(s)=i.active.as_mut().filter(|s|s.id==id&&s.generation==generation){if let Some(d)=s.delivery.as_mut(){d.state=state;d.reason=Some(reason);}if persisted.is_err(){s.error=Some(ApiError::new("delivery_receipt_save_failed","storage","本机输入回执未保存；不会再次自动输入，请核对目标后手动复制").for_session(id,generation));}}}
        self.publish_session();self.changed();
        let current=self.inner.lock().unwrap().active.as_ref().is_some_and(|s|s.id==id&&s.generation==generation&&!s.cancel.is_cancelled()&&!s.original_delivery_revoked);
        if current{let _=self.native_action("recording",matches!(state,DeliveryState::Skipped|DeliveryState::Failed));}
    }

    pub fn copy_recoverable(&self,p:&Value)->Result<Value> {
        let content={let mut i=self.inner.lock().unwrap();let s=matching(&mut i,p)?;
            if p.get("recoverableContentId").and_then(Value::as_str).is_some_and(|id|id!=s.id){return Err(ApiError::new("stale_session","recovery","来源已过期"));}
            if let Some(source)=&s.source{source.text()}else{s.captions.iter().map(|c|c.text.as_str()).collect::<Vec<_>>().join("\n")}};
        if content.trim().is_empty(){return Err(ApiError::new("empty_recovery","recovery","没有可复制的识别内容"));}
        let _native=self.native_io.lock().unwrap();platform::delivery::copy_text(&content)?;Ok(json!({"copied":true,"adopted":false}))
    }
    fn prepare_use_source(&self,p:&Value,saved:&HistoryDetail)->Result<(String,u64,Arc<CommitGuard>)> {
        let id=saved.session.seed.id.clone();let guard=self.record_guard(&id,saved.session.seed.generation);
        if p.get("sourceRevisionId").and_then(Value::as_str).is_some_and(|s|s!=saved.source.id){return Err(ApiError::new("revisionConflict","recovery","来源已经变化"));}
        let generation=p.get("expectedGeneration").and_then(Value::as_u64).unwrap_or(saved.session.seed.generation);
        {let mut i=self.inner.lock().unwrap();if let Some(s)=i.active.as_mut().filter(|s|s.id==id){if s.busy()||s.pending.is_some(){return Err(ApiError::new("session_busy","recovery","请等待处理释放或先重试保存"));}s.db_pending+=1;}}
        Ok((id,generation,guard))
    }
    fn use_source_receipt(&self,id:&str,generation:u64,result:Result<DbValue>)->Result<Value> {
        {let mut i=self.inner.lock().unwrap();if let Some(s)=i.active.as_mut().filter(|s|s.id==id&&s.generation==generation){s.db_pending=s.db_pending.saturating_sub(1);}}
        let detail=db_detail(result?)?;
        {let mut i=self.inner.lock().unwrap();let deleted=i.guards.get(id).is_some_and(|g|g.is_cancelled());if !deleted{if let Some(s)=i.active.as_mut().filter(|s|s.id==id&&s.generation==generation){s.adopt_first_history_result(&detail);s.reason=CreationReason::UseSource;}}}
        self.publish_session();wire(detail)
    }
    pub fn use_source(&self,p:&Value)->Result<Value> {
        let id=p.get("sessionId").or_else(||p.get("resultId")).and_then(Value::as_str).ok_or_else(||ApiError::new("invalid_payload","recovery","缺少来源身份"))?;
        let saved=db_detail(self.db_blocking(DbCommand::GetResult{session_id:id.into()},None,DbPriority::Normal)?)?;let(id,generation,guard)=self.prepare_use_source(p,&saved)?;
        let result=self.db_blocking(DbCommand::UseSource{session_id:id.clone(),expected_generation:generation,created_at:now()},Some(guard),DbPriority::Critical);self.use_source_receipt(&id,generation,result)
    }
    pub async fn use_source_async(&self,p:&Value)->Result<Value> {
        let id=p.get("sessionId").or_else(||p.get("resultId")).and_then(Value::as_str).ok_or_else(||ApiError::new("invalid_payload","recovery","缺少来源身份"))?;
        let saved=db_detail(self.db_read(DbCommand::GetResult{session_id:id.into()}).await?)?;let(id,generation,guard)=self.prepare_use_source(p,&saved)?;
        let result=self.db_write(DbCommand::UseSource{session_id:id.clone(),expected_generation:generation,created_at:now()},guard,DbPriority::Critical).await;self.use_source_receipt(&id,generation,result)
    }
    pub async fn retry_save(self:&Arc<Self>,p:&Value)->Result<Value> {
        let retry_intent=Instant::now();
        let (id,generation,pending_id,pending,config)={let mut i=self.inner.lock().unwrap();let s=matching(&mut i,p)?;
            if s.busy(){return Err(ApiError::new("session_busy","recovery","正在释放上一条操作"));}
            let (pending_id,pending)=s.pending.clone().ok_or_else(||ApiError::new("save_expired","recovery","没有可重试的保存"))?;
            if text(p,"pendingCommitId")?!=pending_id{return Err(ApiError::new("save_expired","recovery","保存身份已过期"));}
            (s.id.clone(),s.generation,pending_id,pending,s.config.clone())};
        match pending {
            PendingCommit::Source(_seed,source)=>{cloud::endpoints(&config)?;let secret=credentials::read(&config)?;
                {let mut i=self.inner.lock().unwrap();let s=matching(&mut i,p)?;
                    if s.id!=id||s.generation!=generation||s.busy()||s.pending.as_ref().is_none_or(|(key,_)|key!=&pending_id){return Err(ApiError::new("save_expired","recovery","保存请求已过期"));}
                    s.cancel=CancellationToken::new();s.reason=CreationReason::Retry;s.processing_worker_released=false;s.error=None;s.caption_render_token=None;
                    s.diagnostic_trace=self.diagnostics.begin(&s.id,s.generation,Origin::SaveRetry,retry_intent,&s.config,&s.mode);self.diagnostics.attempt(s.diagnostic_trace.as_ref(),s.asr_attempt_id.as_deref().unwrap_or(""));
                }
                let processing=self.processing_lease(&id,generation);
                if let Err(error)=self.checkpoint_async(&id,generation,source).await{self.fail(&id,generation,error.clone(),true);return Err(error);}
                self.finish_rewrite(id.clone(),generation,secret).await;
                drop(processing);
            }
            PendingCommit::Ready(mut output)=>{
                {let mut i=self.inner.lock().unwrap();let s=matching(&mut i,p)?;
                    if s.id!=id||s.generation!=generation||s.busy()||s.pending.as_ref().is_none_or(|(key,_)|key!=&pending_id){return Err(ApiError::new("save_expired","recovery","保存请求已过期"));}
                    s.cancel=CancellationToken::new();s.reason=CreationReason::Retry;s.processing_worker_released=false;s.caption_render_token=None;
                    s.diagnostic_trace=self.diagnostics.begin(&s.id,s.generation,Origin::SaveRetry,retry_intent,&s.config,&s.mode);self.diagnostics.attempt(s.diagnostic_trace.as_ref(),s.asr_attempt_id.as_deref().unwrap_or(""));
                }
                let processing=self.processing_lease(&id,generation);
                output.creation_reason=CreationReason::Retry;
                if let Err(error)=self.commit_output_async(&id,generation,output).await{self.fail(&id,generation,error.clone(),true);return Err(error);}
                drop(processing);
                self.publish_session();
            }
        }
        self.scoped_view(&id,generation)
    }
    pub async fn retry_rewrite(self:&Arc<Self>,p:&Value)->Result<Value> {
        let retry_intent=Instant::now();
        let id=text(p,"sessionId")?.to_owned();
        let (config,generation)={let mut i=self.inner.lock().unwrap();let s=matching(&mut i,p)?;
            if s.id!=id||!s.view().can_retry_rewrite{return Err(ApiError::new("rewrite_unavailable","recovery","这条口述不能直接重试整理，请到记录中操作"));}
            (s.config.clone(),s.generation)};
        cloud::endpoints(&config)?;
        let secret=credentials::read(&config)?;
        {let mut i=self.inner.lock().unwrap();let s=matching(&mut i,p)?;
            if s.id!=id||s.generation!=generation||!s.view().can_retry_rewrite{return Err(ApiError::new("stale_session","recovery","整理请求已过期"));}
            s.cancel=CancellationToken::new();s.phase=SessionPhase::Rewriting;s.reason=CreationReason::Retry;s.error=None;s.processing_worker_released=false;s.caption_render_token=None;
            s.diagnostic_trace=self.diagnostics.begin(&s.id,s.generation,Origin::RewriteRetry,retry_intent,&s.config,&s.mode);self.diagnostics.attempt(s.diagnostic_trace.as_ref(),s.asr_attempt_id.as_deref().unwrap_or(""));
        }
        let processing=self.processing_lease(&id,generation);
        self.finish_rewrite(id.clone(),generation,secret).await;drop(processing);
        self.scoped_view(&id,generation)
    }
    pub async fn retry_asr(self:&Arc<Self>,p:&Value)->Result<Value> {
        let retry_intent=Instant::now();
        let (id,old_generation,config)={let mut i=self.inner.lock().unwrap();let s=matching(&mut i,p)?;
            if !s.view().can_retry_asr{return Err(ApiError::new("asr_retry_unavailable","recovery","完整音频缓存不可用，请重新口述"));}
            if p.get("failedAttemptId").and_then(Value::as_str)!=s.asr_attempt_id.as_deref(){return Err(ApiError::new("stale_attempt","recovery","这次识别尝试已经变化"));}
            (s.id.clone(),s.generation,s.config.clone())};
        cloud::endpoints(&config)?;
        let secret=credentials::read(&config)?;
        let (generation,language,dictionary,cache,cancel,attempt)={let mut i=self.inner.lock().unwrap();let s=matching(&mut i,p)?;
            if s.id!=id||s.generation!=old_generation||!s.view().can_retry_asr||p.get("failedAttemptId").and_then(Value::as_str)!=s.asr_attempt_id.as_deref(){return Err(ApiError::new("stale_attempt","recovery","识别请求已过期"));}
            s.generation+=1;s.cancel=CancellationToken::new();s.phase=SessionPhase::FinalizingAsr;s.reason=CreationReason::Retry;s.error=None;s.captions.clear();s.processing_worker_released=false;
            let attempt=Uuid::new_v4().to_string();s.asr_attempt_id=Some(attempt.clone());
            s.caption_render_token=None;s.provider_ready=false;
            s.diagnostic_trace=self.diagnostics.begin(&s.id,s.generation,Origin::AsrRetry,retry_intent,&s.config,&s.mode);self.diagnostics.attempt(s.diagnostic_trace.as_ref(),&attempt);
            let values=(s.generation,s.prefs.language.clone(),s.dictionary.clone(),s.cache.clone().unwrap(),s.cancel.clone(),attempt);
            i.guards.insert(id.clone(),Arc::new(CommitGuard::new(id.clone(),values.0)));values};
        let processing=self.processing_lease(&id,generation);
        let trace=self.trace(&id,generation);
        let (tx,rx)=mpsc::channel(5);let producer_cancel=cancel.child_token();let producer_stop=producer_cancel.clone();let producer_hub=Arc::clone(self);let producer_trace=trace.clone();
        let configured=Arc::new(AtomicBool::new(false));let configured_notify=Arc::new(Notify::new());
        let producer_ready=configured.clone();let producer_notify=configured_notify.clone();
        let producer=tokio::spawn(async move{
            retry_audio::replay(cache,tx,producer_cancel,producer_ready,producer_notify,
                &producer_hub.diagnostics,producer_trace.as_ref()).await;
        });
        let hub=Arc::clone(self);let event_id=id.clone();let event_attempt=attempt.clone();let callback_trace=trace.clone();
        let callback=Arc::new(move|progress:AsrProgress|match progress {
            AsrProgress::Configured=>{configured.store(true,Ordering::Release);configured_notify.notify_waiters();hub.accept_asr_progress(&event_id,generation,&event_attempt,AsrProgress::Configured);},
            AsrProgress::Diagnostics(metadata)=>hub.diagnostics.asr(callback_trace.as_ref(),&metadata),
            progress=>hub.accept_asr_progress(&event_id,generation,&event_attempt,progress),
        });
        let result=asr::run(config.clone(),secret.clone(),language,dictionary,attempt.clone(),rx,cancel,callback).await;
        if result.is_ok(){self.diagnostics.mark(trace.as_ref(),Stage::FinalAsrReceived);}producer_stop.cancel();let _=producer.await;
        let rows=match result{Ok(v)=>v,Err(e)=>{self.fail(&id,generation,e.into(),true);drop(processing);return self.scoped_view(&id,generation);}};
        if rows.is_empty(){self.fail(&id,generation,ApiError::new("empty_recording","asr","未识别到有效语音"),true);drop(processing);return self.scoped_view(&id,generation);}
        let source=SourceRevision{id:Uuid::new_v4().to_string(),session_id:id.clone(),asr_attempt_id:attempt,generation,provider:config.provider.clone(),model:config.asr_model.clone(),adapter_version:config.adapter_version.clone(),complete:true,items:rows,created_at:now()};
        let duration=self.inner.lock().unwrap().active.as_ref().filter(|s|s.id==id&&s.generation==generation).map(|s|s.capture_duration_ms).unwrap_or(0);
        self.diagnostics.lengths(trace.as_ref(),duration,Some(source.text().chars().count()));
        if let Err(error)=self.checkpoint_async(&id,generation,source).await{self.fail(&id,generation,error.clone(),true);return Err(error);}
        self.finish_rewrite(id.clone(),generation,secret).await;drop(processing);
        self.scoped_view(&id,generation)
    }
    fn scoped_view(&self,id:&str,generation:u64)->Result<Value> {
        let i=self.inner.lock().unwrap();wire(i.active.as_ref().filter(|s|s.id==id&&s.generation==generation).ok_or_else(||ApiError::new("stale_session","session","这次口述已变化"))?.view())
    }
}

fn pump_audio(hub:&Arc<AppHub>,id:&str,generation:u64,device:Option<&str>,soft_voice_enabled:bool,tx:mpsc::Sender<AsrInput>,cancel:CancellationToken,stop:Arc<AtomicBool>)->Result<(CaptureSummary,Vec<i16>)> {
    let trace=hub.trace(id,generation);let mut bridge_peak=0;let mut bridge=BridgeTelemetry::default();
    if cancel.is_cancelled(){return Err(ApiError::new("cancelled","audio","采音开始前已取消"));}
    use platform::audio_conditioner::ConditioningMode;
    let profile=if soft_voice_enabled {ConditioningMode::SoftVoice} else {ConditioningMode::Bypass};
    let mut capture=CaptureHandle::start_with_conditioning(device,profile)?;
    let mut cache=Vec::new();let mut stop_started=None;let mut network_closed=false;let mut pending_frame=None;let mut limit_reached=false;
    loop {
        if cancel.is_cancelled(){capture.cancel();hub.diagnostics.mark(trace.as_ref(),Stage::CaptureCutoff);stop_started.get_or_insert(Instant::now());}
        else if stop.load(Ordering::SeqCst){capture.request_stop();hub.diagnostics.mark(trace.as_ref(),Stage::CaptureCutoff);stop_started.get_or_insert(Instant::now());}
        else if stop_started.is_none()&&hub.recording_progress(id,generation,capture.duration_ms()) {limit_reached=true;capture.cancel();stop_started=Some(Instant::now());}
        while let Ok(event)=capture.events.try_recv(){match event{
            AudioEvent::FirstSamples{at}=>{hub.diagnostics.mark_at(trace.as_ref(),Stage::CaptureFirstAccepted,at);{let mut i=hub.inner.lock().unwrap();if let Some(s)=i.active.as_mut().filter(|s|s.id==id&&s.generation==generation&&!s.cancel.is_cancelled()){s.first_samples=true;if s.provider_ready&&s.phase==SessionPhase::Starting{s.phase=SessionPhase::Recording;}}}hub.publish_session();}
            AudioEvent::Level(level)=>{hub.accept_audio_level(id,generation,level);}
            AudioEvent::Error(_)=>{capture.request_stop();hub.diagnostics.mark(trace.as_ref(),Stage::CaptureCutoff);stop_started.get_or_insert(Instant::now());}
        }}
        if let Some(frame)=pending_frame.take(){match enqueue_observed(&tx,frame,&hub.diagnostics,trace.as_ref(),&mut bridge){Ok(())=>{},Err(mpsc::error::TrySendError::Closed(_))=>{network_closed=true;capture.request_stop();stop_started.get_or_insert(Instant::now());},Err(mpsc::error::TrySendError::Full(frame))=>pending_frame=Some(frame)}}
        while pending_frame.is_none(){let Ok(frame)=capture.frames.try_recv()else{break;};
            bridge.observed.add_frame(frame.pcm16.len().saturating_sub(frame.padding_samples),frame.padding_samples);hub.diagnostics.bridge(trace.as_ref(),&bridge);
            if !cancel.is_cancelled(){let real=frame.pcm16.len().saturating_sub(frame.padding_samples);if cache.len()+real>50*1024*1024{capture.cancel();return Err(ApiError::new("cache_overflow","audio","音频缓存已达到上限"));}cache.extend_from_slice(&frame.pcm16[..real]);}
            if !network_closed&&!cancel.is_cancelled(){match enqueue_observed(&tx,cloud_frame(frame),&hub.diagnostics,trace.as_ref(),&mut bridge){Ok(())=>{},Err(mpsc::error::TrySendError::Closed(_))=>{network_closed=true;capture.request_stop();stop_started.get_or_insert(Instant::now());},Err(mpsc::error::TrySendError::Full(frame))=>pending_frame=Some(frame)}}
        }
        let peak=tx.max_capacity()-tx.capacity()+usize::from(pending_frame.is_some());
        if peak>bridge_peak{bridge_peak=peak;hub.diagnostics.bridge_peak(trace.as_ref(),peak);}
        if let Some(at)=stop_started {
            match capture.wait_drained(Duration::from_millis(5)){
                Ok(summary)=>{
                    hub.diagnostics.mark(trace.as_ref(),Stage::CaptureDrained);
                    hub.diagnostics.capture(trace.as_ref(),summary.captured_samples,summary.input_sample_rate,summary.emitted_frames,summary.complete,summary.cancelled);
                    if let Some(metadata)=summary.conditioning.as_ref(){hub.diagnostics.conditioning(trace.as_ref(),metadata);}
                    if limit_reached {return Err(ApiError::new("recording_limit","audio","已到本次录音时长上限。已有字幕可复制；本次不会自动整理或输入，请重新口述剩余内容。"));}
                    let deadline=at+Duration::from_secs(5);
                    if let Some(frame)=pending_frame.take(){if !network_closed&&!cancel.is_cancelled(){send_bounded(&tx,frame,&cancel,deadline,&hub.diagnostics,trace.as_ref(),&mut bridge)?;}}
                    while let Ok(frame)=capture.frames.try_recv(){let real=frame.pcm16.len().saturating_sub(frame.padding_samples);if !cancel.is_cancelled(){if cache.len()+real>50*1024*1024{return Err(ApiError::new("cache_overflow","audio","音频缓存已达到上限"));}cache.extend_from_slice(&frame.pcm16[..real]);}
                        bridge.observed.add_frame(real,frame.padding_samples);hub.diagnostics.bridge(trace.as_ref(),&bridge);
                        if !network_closed&&!cancel.is_cancelled(){send_bounded(&tx,cloud_frame(frame),&cancel,deadline,&hub.diagnostics,trace.as_ref(),&mut bridge)?;}}
                    if summary.complete&&!network_closed&&!cancel.is_cancelled(){send_bounded(&tx,AsrInput::Finish{next_sequence:summary.emitted_frames},&cancel,deadline,&hub.diagnostics,trace.as_ref(),&mut bridge)?;}
                    return Ok((summary,cache));
                }
                Err(platform::PlatformError::Timeout(_)) if at.elapsed()<Duration::from_secs(5)=>{},
                Err(_)=>return Err(ApiError::new("capture_cleanup_failed","audio","采音资源未确认释放")),
            }
        }else if capture.frames.is_empty(){std::thread::sleep(Duration::from_millis(5));}
    }
}

fn cloud_frame(frame:AudioFrame)->AsrInput {
    AsrInput::Frame{sequence:frame.sequence,pcm16:frame.pcm16,padding_samples:frame.padding_samples}
}
fn enqueue_observed(tx:&mpsc::Sender<AsrInput>,input:AsrInput,diagnostics:&DiagnosticsRecorder,trace:Option<&TraceHandle>,bridge:&mut BridgeTelemetry)->std::result::Result<(),mpsc::error::TrySendError<AsrInput>> {
    let counts=input.frame_counts();let result=tx.try_send(input);
    if result.is_ok(){bridge.accepted(counts);diagnostics.bridge(trace,bridge);}result
}

/// Native supply failure is more specific than the ASR receiver closing as a
/// consequence. Cancellation remains authoritative, and no raw driver text is
/// put into product messages or diagnostics.
fn capture_failure(summary:&CaptureSummary)->Option<ApiError> {
    if summary.cancelled || summary.complete {return None;}
    let (code,message)=match classify_capture_error(summary.error.as_deref())? {
        CaptureFailureReason::PcmStalled=>("audio_capture_stalled","麦克风已连续 5 秒没有提供音频，本次录音已停止。已有字幕可复制，请检查设备后重新录音。"),
        CaptureFailureReason::FirstSampleTimeout=>("audio_first_sample_timeout","麦克风启动后没有提供音频，请检查设备连接和 Windows 麦克风权限。"),
        CaptureFailureReason::DeviceError=>("audio_capture_device_error","麦克风连接或音频流中断，本次录音未完整。已有字幕可复制，请重新连接设备。"),
        CaptureFailureReason::QueueOverflow=>("audio_capture_queue_overflow","音频处理队列未能及时接收，本次录音未完整。已有字幕可复制，请稍后重新录音。"),
        CaptureFailureReason::MalformedFrame=>("audio_capture_malformed","麦克风提供的音频格式异常，本次录音未完整。请更换设备后重新录音。"),
    };
    Some(ApiError::new(code,"audio",message))
}
fn send_bounded(tx:&mpsc::Sender<AsrInput>,mut input:AsrInput,cancel:&CancellationToken,deadline:Instant,diagnostics:&DiagnosticsRecorder,trace:Option<&TraceHandle>,bridge:&mut BridgeTelemetry)->Result<()> {
    loop {
        if cancel.is_cancelled(){return Err(ApiError::new("cancelled","audio","口述已取消"));}
        match enqueue_observed(tx,input,diagnostics,trace,bridge){
            Ok(())=>return Ok(()),
            Err(mpsc::error::TrySendError::Closed(_))=>return Err(ApiError::new("asr_closed","audio","语音连接已结束")),
            Err(mpsc::error::TrySendError::Full(value))=>input=value,
        }
        if Instant::now()>=deadline{return Err(ApiError::new("asr_send_timeout","audio","语音尾段未能及时发送；内容未完整"));}
        std::thread::sleep(Duration::from_millis(5));
    }
}
