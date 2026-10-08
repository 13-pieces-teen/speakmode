mod session;
mod diagnostics;
pub use session::{LiveSession, SessionPhase};

use crate::{cloud::{self, CloudError, credentials, rewrite}, core::*, platform};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::{collections::{HashMap,HashSet,VecDeque}, future::Future, pin::Pin, task::{Context,Poll}, path::{Path, PathBuf}, sync::{Arc, Mutex}};
use sha2::{Digest,Sha256};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all="camelCase")]
pub struct ApiError { pub code:String, pub stage:String, pub message:String, pub recoverable_content_id:Option<String>, pub pending_commit_id:Option<String>, pub commit_stage:Option<String>, pub can_retry_save:bool, pub retry_kind:Option<String>,
    #[serde(skip_serializing_if="Option::is_none")] pub scope:Option<String>,
    #[serde(skip_serializing_if="Option::is_none")] pub session_id:Option<String>,
    #[serde(skip_serializing_if="Option::is_none")] pub generation:Option<u64>,
    #[serde(skip_serializing_if="Option::is_none")] pub operation_id:Option<String>,
}
impl ApiError {
    pub fn new(code:&str,stage:&str,message:&str)->Self {Self{code:code.into(),stage:stage.into(),message:message.into(),recoverable_content_id:None,pending_commit_id:None,commit_stage:None,can_retry_save:false,retry_kind:None,scope:Some("application".into()),session_id:None,generation:None,operation_id:None}}
    pub fn for_session(mut self,id:&str,generation:u64)->Self{self.scope=Some("session".into());self.session_id=Some(id.into());self.generation=Some(generation);self}
    pub fn for_operation(mut self,scope:&str,id:&str)->Self{self.scope=Some(scope.into());self.operation_id=Some(id.into());self}
}
impl From<CloudError> for ApiError {fn from(e:CloudError)->Self {Self::new(&e.code,&e.stage,&e.message)}}
impl From<StoreError> for ApiError {
    fn from(e:StoreError)->Self {
        let message=match &e {StoreError::Conflict{..}=>"记录已变化，请核对后重试；当前草稿仍保留",StoreError::NotFound=>"记录不存在或已删除",StoreError::Invalid(_)=>"输入内容或版本无效",StoreError::AlreadyCompleted=>"该口述已经完成",StoreError::UndoUnavailable=>"没有可撤销的修改",_=>"本机保存失败，结果可暂留供复制"};
        Self::new(e.code(),"storage",message)
    }
}
impl From<platform::PlatformError> for ApiError {fn from(error:platform::PlatformError)->Self {match error {
    platform::PlatformError::TargetHigherIntegrity=>Self::new("target_higher_integrity","native","目标应用以管理员权限运行，请查看成稿后手动粘贴"),
    platform::PlatformError::TargetCapture(reason)=>Self::new(reason.api_code(),"native",reason.message()),
    _=>Self::new("native_unavailable","native","本机操作未完成，请检查设备、快捷键或目标位置"),
}}}
pub type Result<T> = std::result::Result<T,ApiError>;

#[derive(Clone, Serialize)]
#[serde(rename_all="camelCase")]
pub struct AppEvent {pub sequence:u64,pub kind:String,pub payload:Value,pub session_id:Option<String>,pub generation:Option<u64>}
pub type EventSink=Arc<dyn Fn(AppEvent)+Send+Sync>;

#[derive(Deserialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
pub struct CommandRequest {pub name:String,#[serde(default)]pub payload:Value}
#[derive(Clone,Serialize)]
#[serde(rename_all="camelCase")]
pub struct ProviderStatus {pub config:ProviderConfig,pub configured:bool,pub credential_configured:bool,pub status:String,pub message:Option<String>}

struct CandidateSlot {key:String,run:String,cancel:CancellationToken,base:String,result:Option<OutputRevision>,adopting:Option<Arc<CommitGuard>>}
#[derive(Clone)]
enum ReplayValue {Safe(Value),Session(String,u64),Mode(String,u64),Detail(String),Candidate(String,String),Capture(String),Dictionary(u64),Correction(String,String,Option<u64>,CorrectionDictionaryStatus),Acknowledged}
struct RequestRecord {fingerprint:String,outcome:Mutex<Option<Result<ReplayValue>>>,completed:tokio::sync::Notify}
struct RequestOwner(Arc<RequestRecord>);
impl Drop for RequestOwner {fn drop(&mut self){let mut outcome=self.0.outcome.lock().unwrap();if outcome.is_none(){*outcome=Some(Err(ApiError::new("request_interrupted","ipc","操作已中断，请刷新状态后核对")));}drop(outcome);self.0.completed.notify_waiters();}}
pub(crate) struct Inner {
    guards:HashMap<String,Arc<CommitGuard>>,
    sequence:u64,
    pub(crate) active:Option<session::ActiveSession>,
    recovery:Option<session::ActiveSession>,
    candidate:Option<CandidateSlot>,
    preview:Option<(String,CancellationToken)>,
    shortcut_capture:Option<String>,
    capture_commit_guard:Option<Arc<CommitGuard>>,
    used_operation_ids:HashSet<String>,
    cancelled_operation_ids:HashSet<String>,
    auxiliary_workers:HashMap<String,CancellationToken>,
    key_configured:bool,
    requests:HashMap<String,Arc<RequestRecord>>,
    request_order:VecDeque<String>,
    settings_pending:Option<String>,
}
pub struct AppHub {
    diagnostics:diagnostics::DiagnosticsRecorder,
    pub(crate) inner:Mutex<Inner>,
    pub(crate) db:DbClient,
    db_command_workers:std::sync::atomic::AtomicUsize,
    settings_io:Mutex<()>,
    subscribers:Mutex<HashMap<String,EventSink>>,
    publication:Mutex<()>,
    pub(crate) native_io:Mutex<()>,
    pub(crate) lifecycle_epoch:std::sync::atomic::AtomicU64,
    pub(crate) capture_available:std::sync::atomic::AtomicBool,
    pub(crate) quitting:std::sync::atomic::AtomicBool,
    shortcut_capture_available:std::sync::atomic::AtomicBool,
    shortcut_focus_epoch:std::sync::atomic::AtomicU64,
    shortcut_io:Mutex<()>,
    pub data_dir:PathBuf,
    pub(crate) hooks:Mutex<Option<platform::hotkeys::HotkeyService>>,
    native_actions:Mutex<Option<Arc<dyn Fn(&str,bool)->Result<()>+Send+Sync>>>,
    recorder_control:Mutex<Option<Arc<dyn Fn(f64,f64,bool)->Result<()>+Send+Sync>>>,
    stop_target_control:Mutex<Option<Arc<dyn Fn(platform::delivery::ForegroundAnchor)->Result<platform::delivery::ForegroundAnchor>+Send+Sync>>>,
}

fn wire<T:Serialize>(v:T)->Result<Value>{serde_json::to_value(v).map_err(|_|ApiError::new("serialization_failed","ipc","状态无法读取"))}
fn parse<T:DeserializeOwned>(v:Value)->Result<T>{serde_json::from_value(v).map_err(|_|ApiError::new("invalid_payload","ipc","请求字段无效"))}
fn text<'a>(v:&'a Value,key:&str)->Result<&'a str>{v.get(key).and_then(Value::as_str).filter(|s|s.len()<=4000).ok_or_else(||ApiError::new("invalid_payload","ipc","请求缺少有效字段"))}
fn number(v:&Value,key:&str)->Result<u64>{v.get(key).and_then(Value::as_u64).ok_or_else(||ApiError::new("invalid_payload","ipc","请求缺少版本信息"))}
fn operation_id<'a>(v:&'a Value,field:&str)->Result<&'a str>{let id=text(v,field)?;if id.is_empty()||id.len()>128||!id.bytes().all(|b|b.is_ascii_alphanumeric()||b"-_".contains(&b)){return Err(ApiError::new("invalid_operation_id","ipc","操作身份无效"));}Ok(id)}
const MAX_OPERATION_IDS:usize=8192;
fn claim_operation(inner:&mut Inner,key:&str)->Result<()> {
    if inner.used_operation_ids.contains(key){return Err(ApiError::new("operation_expired","operation","该操作已开始、完成或取消，请使用新的身份"));}
    if inner.used_operation_ids.len()>=MAX_OPERATION_IDS{return Err(ApiError::new("operation_limit","operation","本次运行的操作较多，请结束任务后重开应用"));}
    inner.used_operation_ids.insert(key.into());Ok(())
}
fn cancel_operation(inner:&mut Inner,key:&str)->Result<()> {
    if !inner.used_operation_ids.contains(key)&&inner.used_operation_ids.len()>=MAX_OPERATION_IDS{return Err(ApiError::new("operation_limit","operation","操作身份记录已满，请结束任务后重开应用"));}
    inner.used_operation_ids.insert(key.into());inner.cancelled_operation_ids.insert(key.into());Ok(())
}
struct AuxiliaryLease {hub:Arc<AppHub>,key:String}
impl Drop for AuxiliaryLease {fn drop(&mut self){self.hub.inner.lock().unwrap().auxiliary_workers.remove(&self.key);}}
/// Drop the actual request future before releasing admission, including task abort.
struct AuxiliaryFuture<F:Future>{future:Option<Pin<Box<F>>>,lease:Option<AuxiliaryLease>}
impl<F:Future> Future for AuxiliaryFuture<F>{type Output=F::Output;fn poll(self:Pin<&mut Self>,cx:&mut Context<'_>)->Poll<Self::Output>{let this=self.get_mut();match this.future.as_mut().unwrap().as_mut().poll(cx){Poll::Pending=>Poll::Pending,Poll::Ready(value)=>{this.future.take();this.lease.take();Poll::Ready(value)}}}}
impl<F:Future> Drop for AuxiliaryFuture<F>{fn drop(&mut self){self.future.take();self.lease.take();}}
struct CommandLease(Arc<AppHub>);
impl Drop for CommandLease {fn drop(&mut self){self.0.db_command_workers.fetch_sub(1,std::sync::atomic::Ordering::SeqCst);}}
struct RevisionIntent {session_id:String,generation:u64,sequence:u64}
struct SettingsLease {hub:Arc<AppHub>,id:String}
impl Drop for SettingsLease {fn drop(&mut self){let mut i=self.hub.inner.lock().unwrap();if i.settings_pending.as_deref()==Some(&self.id){i.settings_pending=None;}}}
fn db_wire(value:DbValue)->Result<Value>{match value{
    DbValue::Snapshot(v)=>Ok(json!({"revision":v.revision,"preferences":v.preferences,"providerConfig":v.provider_config,"modes":v.modes,"dictionary":v.dictionary,"usage":v.usage,"temporaryResult":v.temporary_result})),DbValue::Preferences(v)=>wire(v),DbValue::ProviderConfig(v)=>wire(v),
    DbValue::Modes(v)=>wire(v),DbValue::Dictionary(v)=>wire(v),DbValue::Correction(v)=>wire(v),DbValue::Mode(v)=>wire(v),DbValue::ModeSnapshot(v)=>wire(v),
    DbValue::Detail(v)=>wire(v),DbValue::Source(v)=>wire(v),DbValue::Output(v)=>wire(v),
    DbValue::HistoryPage(v)=>wire(v),DbValue::Usage(v)=>wire(v),DbValue::UsageEvent(v)=>wire(v),
    DbValue::Delivery(v)=>wire(v),DbValue::Unit=>Ok(Value::Null),
    #[cfg(test)] DbValue::Count(v)=>wire(v),
}}
fn db_detail(value:DbValue)->Result<HistoryDetail>{if let DbValue::Detail(v)=value{Ok(v)}else{Err(ApiError::new("storage_protocol","storage","数据库返回类型无效"))}}
fn db_mode(value:DbValue)->Result<ModeSnapshot>{if let DbValue::ModeSnapshot(v)=value{Ok(v)}else{Err(ApiError::new("storage_protocol","storage","数据库返回类型无效"))}}
fn dictionary_error(error:StoreError)->ApiError {
    if let StoreError::Invalid(reason)=&error {
        let message=match reason.as_str() {
            "dictionary_empty"=>"词条不能为空",
            "dictionary_control"=>"词条不能包含换行、控制字符或隐藏的格式字符",
            "dictionary_term_limit"=>"每个词条最多 80 个字符",
            "dictionary_entry_limit"=>"个人词典最多保存 200 个词条",
            "dictionary_context_limit"=>"词库超过识别上下文容量，请缩短或删除部分词条",
            "dictionary_import_limit"=>"一次导入需要 1 至 200 个词条",
            "dictionary_duplicate"=>"这个词条已存在；大小写和全半角视为同一个词条",
            _=>"词典内容无效，请检查后重试",
        };
        let code=if reason.starts_with("dictionary_"){reason.as_str()}else{"dictionary_invalid"};
        return ApiError::new(code,"dictionary",message);
    }
    let mut error=ApiError::from(error);error.stage="dictionary".into();error
}
pub(crate) fn now()->String{chrono::Utc::now().to_rfc3339()}

fn correction_error(error:StoreError)->ApiError {
    if let StoreError::Invalid(reason)=&error {
        if reason.starts_with("dictionary_"){return dictionary_error(error);}
        return ApiError::new("correction_invalid","correction","所选词条或范围无效，请重新划选；文字与词典均未保存");
    }
    let mut result=ApiError::from(error);result.stage="correction".into();result
}

impl AppHub {
    fn reserve_settings(self:&Arc<Self>)->Result<SettingsLease>{let mut i=self.inner.lock().unwrap();self.auxiliary_available()?;if i.settings_pending.is_some(){return Err(ApiError::new("settings_busy","preferences","正在保存设置，请稍候"));}let id=Uuid::new_v4().to_string();i.settings_pending=Some(id.clone());Ok(SettingsLease{hub:Arc::clone(self),id})}
    async fn db_read(&self,command:DbCommand)->Result<DbValue>{Ok(self.db.submit(command,None,DbPriority::Normal)?.wait().await?)}
    async fn db_write(&self,command:DbCommand,guard:Arc<CommitGuard>,priority:DbPriority)->Result<DbValue>{Ok(self.db.submit(command,Some(guard),priority)?.wait().await?)}
    fn db_blocking(&self,command:DbCommand,guard:Option<Arc<CommitGuard>>,priority:DbPriority)->Result<DbValue>{Ok(self.db.submit(command,guard,priority)?.wait_blocking()?)}
    fn record_guard(&self,id:&str,generation:u64)->Arc<CommitGuard>{let mut i=self.inner.lock().unwrap();i.guards.entry(id.into()).or_insert_with(||Arc::new(CommitGuard::new(id.into(),generation))).clone()}
    async fn history_guard(&self,id:&str)->Result<Arc<CommitGuard>>{
        if let Some(guard)=self.inner.lock().unwrap().guards.get(id).cloned(){return Ok(guard);}
        let detail=db_detail(self.db_read(DbCommand::GetResult{session_id:id.into()}).await?)?;Ok(self.record_guard(id,detail.session.seed.generation))
    }
    fn operation_guard(&self)->Arc<CommitGuard>{Arc::new(CommitGuard::new(Uuid::new_v4().to_string(),0))}
    fn sync_first_adoption(&self,id:&str,detail:&HistoryDetail)->bool {
        let mut i=self.inner.lock().unwrap();
        // A COMMIT receipt can be scheduled after a newer delete command.
        // Preserve the successful ledger without resurrecting the live body.
        if i.guards.get(id).is_some_and(|guard|guard.is_cancelled()){return false;}
        if let Some(s)=i.active.as_mut(){s.adopt_first_history_result(detail);}true
    }
    fn reserve_revision_intent(&self,id:&str)->Option<RevisionIntent>{
        let mut i=self.inner.lock().unwrap();let s=i.active.as_mut().filter(|s|s.id==id)?;
        // This flag also covers a Prepared COMMIT whose permit has not yet
        // reached the engine. Never re-open automatic delivery after a failed
        // or cancelled user version action; manual copy remains available.
        let sequence=s.reserve_revision_intent();
        Some(RevisionIntent{session_id:id.into(),generation:s.generation,sequence})
    }
    fn sync_revision_receipt(&self,intent:Option<RevisionIntent>,detail:&HistoryDetail){
        let Some(intent)=intent else{return;};let mut i=self.inner.lock().unwrap();
        if i.guards.get(&intent.session_id).is_some_and(|g|g.is_cancelled()){return;}
        if let Some(s)=i.active.as_mut().filter(|s|s.id==intent.session_id&&s.generation==intent.generation){s.sync_revision_receipt(detail,intent.sequence);}
    }
    pub fn open(data_dir:impl AsRef<Path>)->Result<Arc<Self>> {
        let data_dir=data_dir.as_ref().to_path_buf();
        std::fs::create_dir_all(&data_dir).map_err(|_|ApiError::new("storage_unavailable","storage","无法创建本机数据目录"))?;
        let db=DbClient::open(data_dir.join("speakmode.sqlite"))?;
        let config=db.snapshot().provider_config;
        let key_configured=credentials::configured(&config).unwrap_or(false);
        Ok(Arc::new(Self{diagnostics:diagnostics::DiagnosticsRecorder::default(),inner:Mutex::new(Inner{guards:HashMap::new(),sequence:0,active:None,recovery:None,candidate:None,preview:None,shortcut_capture:None,capture_commit_guard:None,used_operation_ids:HashSet::new(),cancelled_operation_ids:HashSet::new(),auxiliary_workers:HashMap::new(),key_configured,requests:HashMap::new(),request_order:VecDeque::new(),settings_pending:None}),db,db_command_workers:std::sync::atomic::AtomicUsize::new(0),settings_io:Mutex::new(()),subscribers:Mutex::new(HashMap::new()),publication:Mutex::new(()),native_io:Mutex::new(()),lifecycle_epoch:std::sync::atomic::AtomicU64::new(0),capture_available:std::sync::atomic::AtomicBool::new(true),quitting:std::sync::atomic::AtomicBool::new(false),shortcut_capture_available:std::sync::atomic::AtomicBool::new(true),shortcut_focus_epoch:std::sync::atomic::AtomicU64::new(0),shortcut_io:Mutex::new(()),data_dir,hooks:Mutex::new(None),native_actions:Mutex::new(None),recorder_control:Mutex::new(None),stop_target_control:Mutex::new(None)}))
    }
    pub fn set_native_actions(&self,actions:Arc<dyn Fn(&str,bool)->Result<()>+Send+Sync>){*self.native_actions.lock().unwrap()=Some(actions);}
    pub fn set_recorder_control(&self,control:Arc<dyn Fn(f64,f64,bool)->Result<()>+Send+Sync>){*self.recorder_control.lock().unwrap()=Some(control);}
    pub fn set_stop_target_control(&self,control:Arc<dyn Fn(platform::delivery::ForegroundAnchor)->Result<platform::delivery::ForegroundAnchor>+Send+Sync>){*self.stop_target_control.lock().unwrap()=Some(control);}
    fn native_action(&self,name:&str,value:bool)->Result<()> {
        let handler=self.native_actions.lock().unwrap().clone().ok_or_else(||ApiError::new("desktop_required","native","此设置需要桌面应用运行"))?;
        handler(name,value)
    }
    pub fn subscribe(&self,window:&str,sink:EventSink)->Result<Value> {
        if !matches!(window,"main"|"recording"){return Err(ApiError::new("forbidden","ipc","窗口无访问权限"));}
        let _ordered=self.publication.lock().unwrap();
        self.subscribers.lock().unwrap().insert(window.into(),sink);
        self.snapshot(window)
    }
    pub fn unsubscribe(&self,window:&str){self.subscribers.lock().unwrap().remove(window);}
    pub(crate) fn publish(&self,kind:&str,payload:Value) {
        let _ordered=self.publication.lock().unwrap();
        let (sequence,session_id,generation)={let mut i=self.inner.lock().unwrap();if kind!="level" {i.sequence+=1;}(i.sequence,i.active.as_ref().map(|s|s.id.clone()),i.active.as_ref().map(|s|s.generation))};
        let event=AppEvent{sequence:if kind=="level"{0}else{sequence},kind:kind.into(),payload,session_id,generation};
        let sinks=self.subscribers.lock().unwrap().values().cloned().collect::<Vec<_>>();
        for sink in sinks{sink(event.clone());}
    }
    pub(crate) fn changed(&self){self.publish("changed",Value::Null);}
    pub(crate) fn publish_session(&self){
        let _ordered=self.publication.lock().unwrap();
        let event={let mut i=self.inner.lock().unwrap();i.sequence+=1;AppEvent{sequence:i.sequence,kind:"session".into(),payload:i.active.as_ref().map(|s|serde_json::to_value(s.view()).unwrap()).unwrap_or(Value::Null),session_id:i.active.as_ref().map(|s|s.id.clone()),generation:i.active.as_ref().map(|s|s.generation)}};
        let sinks=self.subscribers.lock().unwrap().values().cloned().collect::<Vec<_>>();for sink in sinks{sink(event.clone());}
    }
    pub fn snapshot(&self,window:&str)->Result<Value> {
        let mut i=self.inner.lock().unwrap();
        i.expire_audio_caches();
        let cache=self.db.snapshot();
        let mut modes=cache.modes;
        if window=="recording" {for m in &mut modes{m.rules.clear();}return Ok(json!({"sequence":i.sequence,"session":i.active.as_ref().map(|s|s.view()),"modes":modes}));}
        if window!="main"{return Err(ApiError::new("forbidden","ipc","窗口无访问权限"));}
        let config=cache.provider_config;
        let configuration=cloud::endpoints(&config);let valid=configuration.is_ok();
        let mut provider=wire(ProviderStatus{config,configured:valid&&i.key_configured,credential_configured:i.key_configured,status:if valid&&i.key_configured{"configuredUnverified"}else{"unconfigured"}.into(),message:configuration.err().map(|error|error.message)})?;
        provider["modelCatalog"]=wire(cloud::models::catalog())?;
        Ok(json!({"sequence":i.sequence,"dbRevision":cache.revision,"storage":{"pending":self.db.outstanding(),"ownerReleased":self.db.owner_released(),"cacheError":self.db.cache_error().map(|e|e.code())},"preferences":cache.preferences,"modes":modes,"dictionary":cache.dictionary,"usage":cache.usage,"provider":provider,"session":i.active.as_ref().map(|s|s.view()),"recovery":i.recovery.as_ref().and_then(|s|s.recovery_view()),"temporaryResult":cache.temporary_result,"capabilities":{"recording":true,"streamingAsr":true,"customModes":true,"dictionary":true,"history":true,"nativeDelivery":true}}))
    }

    pub async fn command(self:&Arc<Self>,window:&str,request:CommandRequest)->Result<Value> {
        if window=="recording" && !matches!(request.name.as_str(),"state.snapshot"|"session.stop"|"session.cancel"|"session.selectMode"|"app.showMain"|"window.resizeRecorder"|"session.captionRendered") {return Err(ApiError::new("forbidden","ipc","录音窗口只允许操作当前会话"));}
        if !matches!(window,"main"|"recording"){return Err(ApiError::new("forbidden","ipc","窗口无访问权限"));}
        let mutation=!matches!(request.name.as_str(),"state.snapshot"|"preferences.get"|"mode.list"|"dictionary.list"|"provider.status"|"credential.status"|"audio.listDevices"|"history.list"|"history.get"|"usage.today"|"diagnostics.get"|"session.captionRendered");
        if !mutation{return self.execute_command(window,request).await;}
        let request_id=text(&request.payload,"requestId")?;
        if request_id.is_empty()||request_id.len()>128{return Err(ApiError::new("invalid_request_id","ipc","请求身份无效"));}
        let key=format!("{window}:{request_id}");
        let mut fingerprint=request.payload.clone();if let Some(object)=fingerprint.as_object_mut(){object.remove("requestId");}
        let fingerprint=format!("{:x}",Sha256::digest(format!("{}:{}",request.name,fingerprint).as_bytes()));
        let (record,owner)={let mut i=self.inner.lock().unwrap();
            if let Some(record)=i.requests.get(&key){if record.fingerprint!=fingerprint{return Err(ApiError::new("request_id_conflict","ipc","同一请求身份不能复用为不同操作"));}(record.clone(),false)}
            else {while i.requests.len()>=256 {let Some(old)=i.request_order.iter().find(|key|i.requests.get(*key).is_some_and(|r|r.outcome.lock().unwrap().is_some())).cloned() else{return Err(ApiError::new("requests_busy","ipc","正在处理的操作较多，请稍候"));};i.request_order.retain(|key|key!=&old);i.requests.remove(&old);}
                let record=Arc::new(RequestRecord{fingerprint,outcome:Mutex::new(None),completed:tokio::sync::Notify::new()});i.requests.insert(key.clone(),record.clone());i.request_order.push_back(key);(record,true)}
        };
        if !owner{loop {let notified=record.completed.notified();tokio::pin!(notified);notified.as_mut().enable();let outcome=record.outcome.lock().unwrap().clone();if let Some(outcome)=outcome{return match outcome{Ok(value)=>self.replay(value).await,Err(error)=>Err(error)};}notified.await;}}
        // The operation owns its reply and resources, not the IPC receiver.
        // Dropping a caller must not discard a late successful COMMIT.
        let track=!matches!(request.name.as_str(),"session.start"|"session.stop"|"session.cancel"|"session.selectMode"|"recovery.activate");
        let hub=Arc::clone(self);let owned_record=Arc::clone(&record);let window=window.to_owned();let (reply_tx,reply_rx)=tokio::sync::oneshot::channel();
        if track{self.db_command_workers.fetch_add(1,std::sync::atomic::Ordering::SeqCst);}
        tokio::spawn(async move{
            let _lease=track.then(||CommandLease(Arc::clone(&hub)));let _owner=RequestOwner(Arc::clone(&owned_record));
            let name=request.name.clone();
            let origin=if matches!(name.as_str(),"session.stop"|"session.cancel"|"session.selectMode"|"asr.retry"|"rewrite.retry"|"result.retrySave"){
                request.payload.get("sessionId").and_then(Value::as_str).zip(request.payload.get("expectedGeneration").and_then(Value::as_u64)).map(|(id,g)|(id.to_owned(),g))
            }else{None};
            let result=hub.execute_command(&window,request).await.map_err(|error|{
                if error.scope.as_deref()==Some("session"){return error;}
                match origin{Some((id,generation))=>error.for_session(&id,generation),None=>error}
            });
            let outcome=match &result{Ok(value)=>Ok(Self::replay_reference(&name,value)),Err(error)=>Err(error.clone())};
            *owned_record.outcome.lock().unwrap()=Some(outcome);drop(_lease);owned_record.completed.notify_waiters();let _=reply_tx.send(result);
        });
        reply_rx.await.map_err(|_|ApiError::new("request_interrupted","ipc","操作已中断，请刷新状态后核对"))?
    }
    fn replay_reference(name:&str,value:&Value)->ReplayValue {
        let id=|field:&str|value.get(field).and_then(Value::as_str).unwrap_or("").to_owned();
        match name {
            "session.start"=>ReplayValue::Safe(value.clone()),
            "session.stop"|"session.selectMode"|"asr.retry"|"rewrite.retry"|"result.retrySave"|"recovery.activate"=>ReplayValue::Session(value.get("sessionId").or_else(||value.get("id")).and_then(Value::as_str).unwrap_or("").into(),value["generation"].as_u64().unwrap_or(0)),
            "dictionary.save"|"dictionary.import"|"dictionary.delete"=>ReplayValue::Dictionary(value["version"].as_u64().unwrap_or(0)),
            "revision.correctTerm"=>ReplayValue::Correction(value.pointer("/detail/session/id").and_then(Value::as_str).unwrap_or("").into(),value.pointer("/detail/output/id").and_then(Value::as_str).unwrap_or("").into(),value.pointer("/dictionary/version").and_then(Value::as_u64),serde_json::from_value(value["dictionaryStatus"].clone()).unwrap_or(CorrectionDictionaryStatus::NotRequested)),
            "mode.save"=>ReplayValue::Mode(id("id"),value["revision"].as_u64().unwrap_or(0)),
            "revision.adopt"|"revision.edit"|"revision.restore"|"revision.undo"|"result.useSource"=>ReplayValue::Detail(value.pointer("/session/id").and_then(Value::as_str).unwrap_or("").into()),
            "history.rewrite"=>ReplayValue::Candidate(id("sessionId"),id("id")),
            "mode.preview"=>ReplayValue::Acknowledged,
            "shortcut.capture.begin"=>ReplayValue::Capture(id("captureId")),
            _=>ReplayValue::Safe(value.clone()),
        }
    }
    async fn replay(&self,value:ReplayValue)->Result<Value> {
        let already=||ApiError::new("request_already_processed","ipc","此操作已处理；内容已变化，请刷新查看，不会重复执行");
        match value {
            ReplayValue::Safe(value)=>Ok(value),
            ReplayValue::Dictionary(version)=>{let snapshot=self.db.snapshot().dictionary;if snapshot.version!=version{return Err(already());}wire(snapshot)},
            ReplayValue::Correction(id,revision,dictionary_version,dictionary_status)=>{
                let detail=db_detail(self.db_read(DbCommand::GetResult{session_id:id}).await.map_err(|_|already())?)?;
                if detail.output.as_ref().map(|output|output.id.as_str())!=Some(revision.as_str()){return Err(already());}
                let dictionary=if let Some(version)=dictionary_version{let snapshot=self.db.snapshot().dictionary;if snapshot.version!=version{return Err(already());}Some(snapshot)}else{None};
                wire(CorrectionReceipt{detail,dictionary,dictionary_status})
            },
            ReplayValue::Acknowledged=>Err(already()),
            ReplayValue::Session(id,generation)=>{let i=self.inner.lock().unwrap();wire(i.active.as_ref().filter(|s|s.id==id&&s.generation==generation).ok_or_else(already)?.view())},
            ReplayValue::Detail(id)=>db_wire(self.db_read(DbCommand::GetResult{session_id:id}).await.map_err(|_|already())?),
            ReplayValue::Mode(id,revision)=>{let snapshot=db_mode(self.db_read(DbCommand::ModeSnapshot{id:id.clone(),revision:Some(revision)}).await.map_err(|_|already())?)?;wire(ModeRecord{id,name:snapshot.name,base_mode:snapshot.base_mode,rules:snapshot.rules,revision,builtin:false})},
            ReplayValue::Candidate(id,run)=>{let i=self.inner.lock().unwrap();wire(i.candidate.as_ref().filter(|c|c.key==id&&c.run==run).and_then(|c|c.result.clone()).ok_or_else(already)?)},
            ReplayValue::Capture(id)=>{let i=self.inner.lock().unwrap();if i.shortcut_capture.as_deref()!=Some(&id){return Err(already());}Ok(json!({"captureId":id,"capturing":true}))},
        }
    }
    async fn execute_command(self:&Arc<Self>,window:&str,request:CommandRequest)->Result<Value> {
        let p=request.payload;
        if matches!(request.name.as_str(),"session.stop"|"session.cancel"|"session.selectMode"|"asr.retry"|"rewrite.retry"|"result.retrySave"){
            text(&p,"sessionId")?;number(&p,"expectedGeneration")?;
        }
        // Never retain secret/preview/transcript-bearing requests in the idempotency cache.
        let result=match request.name.as_str() {
            "state.snapshot"=>return self.snapshot(window),
            "diagnostics.get"=>return wire(self.diagnostics.snapshot().map_err(|_|ApiError::new("diagnostics_unavailable","diagnostics","诊断记录暂不可用"))?),
            "session.captionRendered"=>{
                if window!="recording"{return Err(ApiError::new("forbidden","ipc","此回执仅允许录音窗口"));}
                return self.caption_rendered(&p);
            },
            "preferences.get"=>wire(self.db.snapshot().preferences),
            "preferences.update" | "preferences.reset"=>self.update_preferences(&p,request.name.ends_with("reset")).await,
            "mode.list"=>wire(self.db.snapshot().modes),
            "dictionary.list"=>wire(self.db.snapshot().dictionary),
            "dictionary.save"=>self.dictionary_write(DbCommand::SaveDictionary{draft:parse(p["draft"].clone())?,expected_version:number(&p,"expectedVersion")?}).await,
            "dictionary.import"=>self.dictionary_write(DbCommand::ImportDictionary{terms:parse(p["terms"].clone())?,expected_version:number(&p,"expectedVersion")?}).await,
            "dictionary.delete"=>self.dictionary_write(DbCommand::DeleteDictionary{entry_id:text(&p,"entryId")?.into(),expected_version:number(&p,"expectedVersion")?}).await,
            "mode.save"=>db_wire(self.db_write(DbCommand::SaveMode{draft:parse(p.get("draft").cloned().unwrap_or(p.clone()))?},self.operation_guard(),DbPriority::Normal).await?),
            "mode.delete"=>{self.db_write(DbCommand::DeleteMode{id:text(&p,"modeId")?.into(),expected_revision:number(&p,"expectedRevision")?},self.operation_guard(),DbPriority::Normal).await?;Ok(Value::Null)},
            "mode.setDefault"=>db_wire(self.db_write(DbCommand::SetDefaultMode{id:text(&p,"modeId")?.into(),expected_version:number(&p,"expectedVersion")?},self.operation_guard(),DbPriority::Normal).await?),
            "mode.preview"=>self.preview(p).await,
            "mode.cancelPreview"=>self.cancel_preview(&p),
            "provider.status"=>Ok(self.snapshot("main")?["provider"].clone()),
            "provider.configure"=>{
                let config:ProviderConfig=parse(p.get("config").cloned().unwrap_or(p.clone()))?;cloud::endpoints(&config)?;
                let expected=p.get("expectedVersion").and_then(Value::as_u64).unwrap_or(config.version);let _reservation=self.reserve_settings()?;
                let native_config=config.clone();let key=tokio::task::spawn_blocking(move||credentials::configured(&native_config).unwrap_or(false)).await.map_err(|_|ApiError::new("credential_unavailable","credentials","服务配置未完成"))?;
                let saved=self.db_write(DbCommand::SaveProviderConfig{config,expected_version:expected},self.operation_guard(),DbPriority::Normal).await?;self.inner.lock().unwrap().key_configured=key;db_wire(saved)
            },
            "credential.status"=>Ok(json!({"configured":self.inner.lock().unwrap().key_configured})),
            "credential.set"|"credential.delete"=>{
                let _reservation=self.reserve_settings()?;let config=self.db.snapshot().provider_config;
                if number(&p,"expectedProviderVersion")?!=config.version{return Err(ApiError::new("revisionConflict","credentials","服务配置已变化，请重新核对 Key 所属地域与空间"));}
                let delete=request.name=="credential.delete";let secret=if delete{None}else{Some(text(&p,"secret")?.to_owned())};let hub=Arc::clone(self);
                tokio::task::spawn_blocking(move||{let _io=hub.settings_io.lock().unwrap();if let Some(secret)=secret{credentials::set(&config,&secret)}else{credentials::delete(&config)}}).await.map_err(|_|ApiError::new("credential_unavailable","credentials","Key 操作未完成"))??;
                self.inner.lock().unwrap().key_configured=!delete;Ok(json!({"configured":!delete}))
            },
            "audio.listDevices"=>wire(platform::audio::enumerate_inputs()?),
            "history.list"=>db_wire(self.db_read(DbCommand::HistoryList{query:p.get("query").and_then(Value::as_str).unwrap_or("").into(),cursor:p.get("cursor").and_then(Value::as_u64).unwrap_or(0),limit:p.get("limit").and_then(Value::as_u64).unwrap_or(20)}).await?),
            "history.get"=>db_wire(self.db_read(DbCommand::GetResult{session_id:text(&p,"sessionId")?.into()}).await?),
            "history.delete"=>{let id=text(&p,"sessionId")?.to_owned();let guard=self.record_guard(&id,0);guard.delete();
                {let mut i=self.inner.lock().unwrap();if let Some(s)=i.active.as_mut().filter(|s|s.id==id){s.revoke_for_deletion();}
                    if i.recovery.as_ref().is_some_and(|s|s.id==id){i.discard_retained_recovery();}
                    if i.candidate.as_ref().is_some_and(|c|c.key==id){i.candidate.take().unwrap().cancel.cancel();}}
                self.publish("recovery.changed",Value::Null);
                let result=self.db.submit(DbCommand::DeleteHistory{session_id:id.clone()},None,DbPriority::Critical).map_err(ApiError::from)?.wait().await.map_err(ApiError::from);
                if result.is_ok(){let mut i=self.inner.lock().unwrap();if let Some(s)=i.active.as_mut().filter(|s|s.id==id){s.discard_deleted_content();}}
                db_wire(result?)},
            "usage.today"=>db_wire(self.db_read(DbCommand::UsageToday{date:chrono::Local::now().format("%Y-%m-%d").to_string()}).await?),
            "history.rewrite"=>self.make_candidate(&p,false).await,
            "rewrite.retry"=>self.retry_rewrite(&p).await,
            "history.cancelCandidate"=>{let mut i=self.inner.lock().unwrap();let session=text(&p,"sessionId")?;let run=operation_id(&p,"runId")?;cancel_operation(&mut i,&format!("candidate:{session}:{run}"))?;if i.candidate.as_ref().is_some_and(|c|c.key==session&&run==c.run){if let Some(slot)=i.candidate.take(){slot.cancel.cancel();if let Some(guard)=slot.adopting{let decision=guard.cancel();return Ok(json!({"status":match decision{CancelDecision::Accepted=>"cancelled",CancelDecision::Pending=>"pending",CancelDecision::AlreadyCompleted=>"alreadyCompleted"}}));}}}Ok(Value::Null)},
            "revision.adopt"=>{
                let id=text(&p,"sessionId")?.to_owned();let expected=text(&p,"expectedCurrentRevisionId")?.to_owned();
                let candidate_id=p.get("candidateId").and_then(Value::as_str).or_else(||p.pointer("/candidate/id").and_then(Value::as_str)).ok_or_else(||ApiError::new("invalid_payload","revision","缺少候选身份"))?.to_owned();
                let record_guard=self.history_guard(&id).await?;let adopt_guard=record_guard.operation();
                let candidate={let mut i=self.inner.lock().unwrap();let slot=i.candidate.as_mut().filter(|c|c.key==id&&c.base==expected&&c.run==candidate_id&&!c.cancel.is_cancelled()).ok_or_else(||ApiError::new("candidate_expired","revision","候选已过期，请重新整理"))?;let candidate=slot.result.clone().ok_or_else(||ApiError::new("candidate_expired","revision","候选尚未完成"))?;if slot.adopting.is_some(){return Err(ApiError::new("candidate_busy","revision","正在采用这个候选，请稍候"));}slot.adopting=Some(Arc::clone(&adopt_guard));candidate};
                let intent=self.reserve_revision_intent(&id);
                let committed=self.db_write(DbCommand::Adopt{session_id:id.clone(),expected_current_revision_id:expected.clone(),candidate},adopt_guard,DbPriority::Normal).await;
                if committed.is_err(){let mut i=self.inner.lock().unwrap();if let Some(slot)=i.candidate.as_mut().filter(|c|c.key==id&&c.run==candidate_id){slot.adopting=None;}}
                let detail=db_detail(committed?)?;
                {let mut i=self.inner.lock().unwrap();if i.candidate.as_ref().is_some_and(|c|c.key==id&&c.run==candidate_id){i.candidate=None;}}
                if expected.is_empty()&&self.sync_first_adoption(&id,&detail){self.publish_session();}
                self.sync_revision_receipt(intent,&detail);wire(detail)
            },
            "revision.edit"=>{
                let id=text(&p,"sessionId")?.to_owned();let command=DbCommand::Edit{session_id:id.clone(),expected_current_revision_id:text(&p,"expectedCurrentRevisionId")?.into(),blocks:parse(p["blocks"].clone())?,created_at:now()};
                let guard=self.history_guard(&id).await?.operation();let intent=self.reserve_revision_intent(&id);
                let detail=db_detail(self.db_write(command,guard,DbPriority::Normal).await?)?;self.sync_revision_receipt(intent,&detail);wire(detail)
            },
            "revision.correctTerm"=>{
                let id=text(&p,"sessionId")?.to_owned();
                let remember=p["remember"].as_bool().ok_or_else(||ApiError::new("invalid_payload","correction","请选择是否记住词条"))?;
                let command=DbCommand::CorrectTerm{session_id:id.clone(),expected_current_revision_id:text(&p,"expectedCurrentRevisionId")?.into(),patch:parse(p["patch"].clone())?,expected_dictionary_version:if remember{Some(number(&p,"expectedDictionaryVersion")?)}else{None},created_at:now()};
                let guard=self.history_guard(&id).await?.operation();let intent=self.reserve_revision_intent(&id);
                let ticket=self.db.submit(command,Some(guard),DbPriority::Normal).map_err(correction_error)?;
                let receipt=match ticket.wait().await.map_err(correction_error)?{DbValue::Correction(receipt)=>receipt,_=>return Err(ApiError::new("storage_protocol","storage","数据库返回类型无效"))};
                self.sync_revision_receipt(intent,&receipt.detail);wire(receipt)
            },
            "revision.restore"=>{
                let id=text(&p,"sessionId")?.to_owned();let command=DbCommand::Restore{session_id:id.clone(),expected_current_revision_id:text(&p,"expectedCurrentRevisionId")?.into(),patch:parse(p["patch"].clone())?,created_at:now()};
                let guard=self.history_guard(&id).await?.operation();let intent=self.reserve_revision_intent(&id);
                let detail=db_detail(self.db_write(command,guard,DbPriority::Normal).await?)?;self.sync_revision_receipt(intent,&detail);wire(detail)
            },
            "revision.undo"=>{
                let id=text(&p,"sessionId")?.to_owned();let command=DbCommand::Undo{session_id:id.clone(),expected_current_revision_id:text(&p,"expectedCurrentRevisionId")?.into(),created_at:now()};
                let guard=self.history_guard(&id).await?.operation();let intent=self.reserve_revision_intent(&id);
                let detail=db_detail(self.db_write(command,guard,DbPriority::Normal).await?)?;self.sync_revision_receipt(intent,&detail);wire(detail)
            },
            "result.copy"=>{
                let result=db_detail(self.db_read(DbCommand::GetResult{session_id:p.get("sessionId").or_else(||p.get("resultId")).and_then(Value::as_str).ok_or_else(||ApiError::new("invalid_payload","ipc","缺少结果身份"))?.into()}).await?)?;
                let content=if p.get("sourceOnly").and_then(Value::as_bool)==Some(true){if p.get("sourceRevisionId").and_then(Value::as_str)!=Some(result.source.id.as_str()){return Err(ApiError::new("revisionConflict","result","识别来源已变化"));}result.source.text()}else{result.output.ok_or_else(||ApiError::new("no_output","result","尚未生成结果，可选择复制识别稿"))?.text()};
                let hub=Arc::clone(self);tokio::task::spawn_blocking(move||{let _native=hub.native_io.lock().unwrap();platform::delivery::copy_text(&content)}).await.map_err(|_|ApiError::new("native_unavailable","result","复制未完成"))??;Ok(json!({"copied":true}))
            },
            "recoverable.copy"=>self.copy_recoverable(&p),
            "result.useSource"=>self.use_source_async(&p).await,
            "result.retrySave"=>self.retry_save(&p).await,
            "session.start"=>self.start_session().await,
            "recovery.activate"=>self.activate_recovery(&p),
            "recovery.discard"=>self.discard_recovery(&p),
            "session.stop"=>self.stop_session(&p).await,
            "session.cancel"=>self.cancel_session_async(&p).await,
            "session.selectMode"=>self.select_session_mode_async(&p).await,
            "asr.retry"=>self.retry_asr(&p).await,
            "shortcut.capture.begin"=>{let hub=Arc::clone(self);tokio::task::spawn_blocking(move||hub.begin_shortcut_capture(&p)).await.map_err(|_|ApiError::new("native_unavailable","shortcut","快捷键操作未完成"))?},
            "shortcut.capture.cancel"=>{let hub=Arc::clone(self);tokio::task::spawn_blocking(move||hub.cancel_shortcut_capture(&p)).await.map_err(|_|ApiError::new("native_unavailable","shortcut","快捷键操作未完成"))?},
            "window.resizeRecorder"=>{
                if window!="recording"{return Err(ApiError::new("forbidden","ipc","此操作仅允许录音窗口"));}
                let width=p["width"].as_f64().filter(|n|n.is_finite()&&*n>=320.0&&*n<=640.0).ok_or_else(||ApiError::new("invalid_payload","window","浮层宽度无效"))?;
                let height=p["height"].as_f64().filter(|n|n.is_finite()&&*n>=80.0&&*n<=420.0).ok_or_else(||ApiError::new("invalid_payload","window","浮层高度无效"))?;
                let control=self.recorder_control.lock().unwrap().clone().ok_or_else(||ApiError::new("desktop_required","window","需要桌面窗口"))?;control(width,height,p["menuOpen"].as_bool().unwrap_or(false))?;Ok(Value::Null)
            },
            "app.showMain"=>{self.native_action("showMain",true)?;Ok(Value::Null)},
            "app.quit"=>{self.native_action("quit",true)?;Ok(Value::Null)},
            _=>Err(ApiError::new("unknown_command","ipc","此操作未提供")),
        };
        if result.is_ok() && !matches!(request.name.as_str(),"preferences.get"|"mode.list"|"dictionary.list"|"usage.today"|"provider.status"|"audio.listDevices"|"history.list"|"history.get"|"credential.status"|"result.copy"|"recoverable.copy"|"mode.preview"|"mode.cancelPreview"|"history.cancelCandidate"|"shortcut.capture.begin"|"shortcut.capture.cancel"|"window.resizeRecorder"|"session.cancel"){self.changed();}
        result
    }

    async fn dictionary_write(&self,command:DbCommand)->Result<Value> {
        let ticket=self.db.submit(command,Some(self.operation_guard().operation()),DbPriority::Normal).map_err(dictionary_error)?;
        db_wire(ticket.wait().await.map_err(dictionary_error)?)
    }

    async fn update_preferences(self:&Arc<Self>,p:&Value,reset:bool)->Result<Value> {
        let old=self.db.snapshot().preferences;let expected=number(p,"expectedVersion")?;
        if old.version!=expected{return Err(ApiError::new("revisionConflict","preferences","设置已变化，请重新读取"));}
        let next:Preferences=if reset{Preferences::default()}else{let mut merged=serde_json::to_value(&old).unwrap();let patch=p.get("patch").unwrap_or(p);if let Some(obj)=patch.as_object(){for(k,v)in obj{if !matches!(k.as_str(),"expectedVersion"|"requestId"|"captureId"){merged[k]=v.clone();}}}parse(merged)?};
        self.db_read(DbCommand::ModeSnapshot{id:next.default_mode_id.clone(),revision:None}).await?;
        let reservation=Uuid::new_v4().to_string();let guard;
        {let mut i=self.inner.lock().unwrap();
            if i.settings_pending.is_some(){return Err(ApiError::new("settings_busy","preferences","正在保存设置，请稍候"));}
            if self.db.snapshot().preferences.version!=expected{return Err(ApiError::new("revisionConflict","preferences","设置已变化，请重新读取"));}
            if next.shortcut!=old.shortcut {
                let supplied=p.get("captureId").and_then(Value::as_str);
                if (!reset&&supplied.is_none())||i.shortcut_capture.as_deref()!=supplied{return Err(ApiError::new("capture_expired","shortcut","快捷键录制已结束，请重新录制后保存"));}
                self.auxiliary_available()?;
                if i.active.as_ref().is_some_and(|s|s.busy()){return Err(ApiError::new("session_busy","shortcut","请结束当前口述后更换快捷键"));}
            }
            guard=if next.shortcut!=old.shortcut&&!reset{i.capture_commit_guard.clone().ok_or_else(||ApiError::new("capture_expired","shortcut","快捷键录制已结束"))?.operation()}else{self.operation_guard()};
            i.settings_pending=Some(reservation.clone());
        }
        let _reservation=SettingsLease{hub:Arc::clone(self),id:reservation.clone()};
        let hub=Arc::clone(self);let previous=old.clone();let proposed=next.clone();
        let native=tokio::task::spawn_blocking(move||hub.apply_native_preferences(&previous,&proposed)).await.map_err(|_|ApiError::new("native_unavailable","preferences","本机设置操作未完成"))?;
        if let Err(error)=native{self.inner.lock().unwrap().settings_pending=None;return Err(error);}
        let result=self.db_write(DbCommand::SavePreferences{next:next.clone(),expected_version:expected},guard,DbPriority::Normal).await;
        if result.is_err(){let hub=Arc::clone(self);let previous=next;let proposed=old;
            let rollback=tokio::task::spawn_blocking(move||hub.apply_native_preferences(&previous,&proposed)).await;
            if !matches!(rollback,Ok(Ok(()))){self.capture_available.store(false,std::sync::atomic::Ordering::SeqCst);self.inner.lock().unwrap().settings_pending=None;return Err(ApiError::new("settings_rollback_failed","native","设置未保存且本机恢复未确认，请重启后核对快捷键与自启动状态"));}
        }
        {let mut i=self.inner.lock().unwrap();if i.settings_pending.as_deref()==Some(&reservation){i.settings_pending=None;}}
        db_wire(result?)
    }
    fn apply_native_preferences(&self,old:&Preferences,next:&Preferences)->Result<()> {
        // A native ACK may take time; no session or publication lock is held.
        let _settings=self.settings_io.lock().unwrap();let _shortcut=self.shortcut_io.lock().unwrap();
        let mut hooks=self.hooks.lock().unwrap();
        if next.shortcut!=old.shortcut{hooks.as_mut().ok_or_else(||ApiError::new("shortcut_unavailable","native","快捷键监听未就绪"))?.replace_shortcut(&next.shortcut)?;}
        if next.launch_at_login!=old.launch_at_login{if let Err(error)=self.native_action("autostart",next.launch_at_login){
            if next.shortcut!=old.shortcut{if let Some(h)=hooks.as_mut(){if h.replace_shortcut(&old.shortcut).is_err(){self.capture_available.store(false,std::sync::atomic::Ordering::SeqCst);return Err(ApiError::new("settings_rollback_failed","native","快捷键恢复未确认，请重启应用"));}}}
            return Err(error);
        }}Ok(())
    }

    fn auxiliary_available(&self)->Result<()> {
        if self.quitting.load(std::sync::atomic::Ordering::SeqCst)||!self.capture_available.load(std::sync::atomic::Ordering::SeqCst){return Err(ApiError::new("system_unavailable","operation","系统正在退出或恢复，请稍候"));}Ok(())
    }
    fn cancel_preview(&self,p:&Value)->Result<Value> {
        let run=operation_id(p,"runId")?;let mut i=self.inner.lock().unwrap();
        cancel_operation(&mut i,&format!("preview:{run}"))?;
        if i.preview.as_ref().is_some_and(|(id,_)|id==run){i.preview.take().unwrap().1.cancel();}
        Ok(json!({"runId":run,"cancelled":true}))
    }
    fn begin_shortcut_capture(&self,p:&Value)->Result<Value>{self.begin_shortcut_capture_with(p,||{
        let hooks=self.hooks.lock().unwrap();hooks.as_ref().ok_or_else(||ApiError::new("shortcut_unavailable","native","快捷键监听未就绪"))?.set_capture(true)?;Ok(())
    })}
    // The injectable ACK is used only by controller tests; production always
    // waits for HotkeyService's bounded native acknowledgement.
    fn begin_shortcut_capture_with(&self,p:&Value,ack:impl FnOnce()->Result<()>)->Result<Value> {
        let id=operation_id(p,"captureId")?.to_owned();let key=format!("capture:{id}");
        let _io=self.shortcut_io.lock().unwrap();let mut i=self.inner.lock().unwrap();self.auxiliary_available()?;
        if !self.shortcut_capture_available.load(std::sync::atomic::Ordering::SeqCst){return Err(ApiError::new("capture_unavailable","shortcut","请在主窗口内录制快捷键"));}
        if i.settings_pending.is_some()||i.shortcut_capture.is_some()||i.active.as_ref().is_some_and(|s|s.busy())||!i.auxiliary_workers.is_empty()||!platform::audio::capture_resources_released(){return Err(ApiError::new("capture_busy","shortcut","请结束当前口述或快捷键录制后再试"));}
        claim_operation(&mut i,&key)?;
        // Retain ownership even if ON ACK fails: a later explicit OFF or native
        // shutdown is the only proof that capture is no longer active.
        i.shortcut_capture=Some(id.clone());i.capture_commit_guard=Some(Arc::new(CommitGuard::new(format!("capture:{id}"),0)));drop(i);ack()?;
        Ok(json!({"captureId":id,"capturing":true}))
    }
    fn cancel_shortcut_capture(&self,p:&Value)->Result<Value>{self.cancel_shortcut_capture_with(p,||{
        let hooks=self.hooks.lock().unwrap();hooks.as_ref().ok_or_else(||ApiError::new("shortcut_unavailable","native","快捷键监听未就绪"))?.set_capture(false)?;Ok(())
    })}
    fn cancel_shortcut_capture_with(&self,p:&Value,ack:impl FnOnce()->Result<()>)->Result<Value> {
        let id=operation_id(p,"captureId")?.to_owned();
        {let mut i=self.inner.lock().unwrap();cancel_operation(&mut i,&format!("capture:{id}"))?;
            if i.shortcut_capture.as_deref()!=Some(&id){return Ok(json!({"captureId":id,"capturing":false}));}
            if let Some(guard)=&i.capture_commit_guard{guard.cancel();}
        }
        let _io=self.shortcut_io.lock().unwrap();
        if self.inner.lock().unwrap().shortcut_capture.as_deref()!=Some(&id){return Ok(json!({"captureId":id,"capturing":false}));}
        ack()?;{let mut i=self.inner.lock().unwrap();if i.shortcut_capture.as_deref()==Some(&id){i.shortcut_capture=None;i.capture_commit_guard=None;}}
        self.publish("shortcut.captureCancelled",json!({"captureId":id,"capturing":false}));Ok(json!({"captureId":id,"capturing":false}))
    }
    pub fn cancel_active_shortcut_capture(&self)->Result<()> {
        let id=self.inner.lock().unwrap().shortcut_capture.clone();
        if let Some(id)=id{self.cancel_shortcut_capture(&json!({"captureId":id}))?;}Ok(())
    }
    /// Atomic-only native boundary: never block the UI on an existing ACK.
    pub fn shortcut_capture_focus_changed(&self,focused:bool)->u64 {
        self.shortcut_capture_available.store(focused,std::sync::atomic::Ordering::SeqCst);
        self.shortcut_focus_epoch.fetch_add(1,std::sync::atomic::Ordering::SeqCst)+1
    }
    pub fn cancel_shortcut_capture_id(&self,id:&str)->Result<()> {self.cancel_shortcut_capture(&json!({"captureId":id})).map(|_|())}
    pub fn cancel_shortcut_capture_at_focus_epoch(&self,epoch:u64)->Result<()> {self.cancel_shortcut_capture_at_focus_epoch_with(epoch,||{
        let hooks=self.hooks.lock().unwrap();hooks.as_ref().ok_or_else(||ApiError::new("shortcut_unavailable","native","快捷键监听未就绪"))?.set_capture(false)?;Ok(())
    })}
    fn cancel_shortcut_capture_at_focus_epoch_with(&self,epoch:u64,ack:impl FnOnce()->Result<()>)->Result<()> {
        let id={let mut i=self.inner.lock().unwrap();
            if self.shortcut_focus_epoch.load(std::sync::atomic::Ordering::SeqCst)!=epoch||self.shortcut_capture_available.load(std::sync::atomic::Ordering::SeqCst){return Ok(());}
            let Some(id)=i.shortcut_capture.clone()else{return Ok(());};cancel_operation(&mut i,&format!("capture:{id}"))?;
            if let Some(guard)=&i.capture_commit_guard{guard.cancel();}id
        };
        let _io=self.shortcut_io.lock().unwrap();
        if self.shortcut_focus_epoch.load(std::sync::atomic::Ordering::SeqCst)!=epoch||self.shortcut_capture_available.load(std::sync::atomic::Ordering::SeqCst)||self.inner.lock().unwrap().shortcut_capture.as_deref()!=Some(&id){return Ok(());}
        ack()?;{let mut i=self.inner.lock().unwrap();if i.shortcut_capture.as_deref()==Some(&id){i.shortcut_capture=None;i.capture_commit_guard=None;}}
        self.publish("shortcut.captureCancelled",json!({"captureId":id,"capturing":false}));Ok(())
    }
    pub(crate) fn cancel_auxiliary(&self) {
        let mut i=self.inner.lock().unwrap();
        Self::cancel_auxiliary_locked(&mut i);
    }
    pub(crate) fn cancel_auxiliary_at_epoch(&self,epoch:u64)->bool {
        let mut i=self.inner.lock().unwrap();if self.lifecycle_epoch.load(std::sync::atomic::Ordering::SeqCst)!=epoch{return false;}
        Self::cancel_auxiliary_locked(&mut i);true
    }
    fn cancel_auxiliary_locked(i:&mut Inner) {
        // A replaced slot may still own a live request. Tokens live in the
        // lease map until its actual future is dropped.
        for token in i.auxiliary_workers.values(){token.cancel();}
        if let Some((_,token))=i.preview.take(){token.cancel();}
        if let Some(slot)=i.candidate.take(){slot.cancel.cancel();if let Some(guard)=slot.adopting{guard.cancel();}}
    }
    async fn preview(self:&Arc<Self>,p:Value)->Result<Value> {
        let key=operation_id(&p,"runId")?.to_owned();let operation=format!("preview:{key}");
        let draft:ModeDraft=parse(p["draft"].clone())?;
        if draft.name.trim().is_empty()||draft.name.chars().count()>40||draft.rules.trim().is_empty()||draft.rules.chars().count()>4000||draft.base_mode==BaseMode::Auto{return Err(ApiError::new("invalidInput","mode","请填写名称、有效规则与基础方式"));}
        let sample=p.get("sample").or_else(||p.get("text")).and_then(Value::as_str).ok_or_else(||ApiError::new("invalidInput","mode","请填写试用文本"))?;
        if sample.trim().is_empty()||sample.chars().count()>80_000{return Err(ApiError::new("invalidInput","mode","试用文本为空或过长"));}
        let cancel=CancellationToken::new();
        let (config,epoch)={let mut i=self.inner.lock().unwrap();self.auxiliary_available()?;claim_operation(&mut i,&operation)?;(self.db.snapshot().provider_config,self.lifecycle_epoch.load(std::sync::atomic::Ordering::SeqCst))};
        cloud::endpoints(&config)?;
        let secret=credentials::read(&config)?;
        let lease={let mut i=self.inner.lock().unwrap();self.auxiliary_available()?;
            if self.lifecycle_epoch.load(std::sync::atomic::Ordering::SeqCst)!=epoch{return Err(ApiError::new("cancelled","preview","系统状态变化，试用已取消"));}
            if i.cancelled_operation_ids.contains(&operation){return Err(ApiError::new("cancelled","preview","试用已取消"));}
            if i.shortcut_capture.is_some(){return Err(ApiError::new("capture_busy","preview","请先结束快捷键录制"));}
            if let Some((_,old))=i.preview.take(){old.cancel();}i.preview=Some((key.clone(),cancel.clone()));i.auxiliary_workers.insert(operation.clone(),cancel.clone());AuxiliaryLease{hub:Arc::clone(self),key:operation}};
        let mode=ModeSnapshot{mode_id:"preview".into(),name:draft.name,base_mode:draft.base_mode,rules:draft.rules,revision:0};
        let source=SourceRevision{id:key.clone(),session_id:key.clone(),asr_attempt_id:"preview".into(),generation:0,provider:"preview".into(),model:String::new(),adapter_version:"v1".into(),complete:true,created_at:now(),items:vec![SourceItem{id:"preview:0".into(),provider_item_id:"0".into(),capture_order:0,text:sample.into(),is_final:true,start_ms:None,end_ms:None}]};
        let result=match (AuxiliaryFuture{future:Some(Box::pin(rewrite::rewrite(&config,&secret,&mode,&source,cancel.clone()))),lease:Some(lease)}).await{Ok(value)=>value,Err(error)=>{let mut i=self.inner.lock().unwrap();if i.preview.as_ref().is_some_and(|(id,_)|id==&key){i.preview=None;}return Err(ApiError::from(error).for_operation("application",&key));}};
        let mut i=self.inner.lock().unwrap();if cancel.is_cancelled()||!i.preview.as_ref().is_some_and(|(id,_)|id==&key){return Err(ApiError::new("cancelled","preview","试用已过期"));}i.preview=None;
        Ok(json!({"id":key,"runId":key,"resolvedMode":result.resolved_mode,"blocks":result.blocks,"text":result.blocks.iter().map(|b|b.text.as_str()).collect::<Vec<_>>().join("\n\n")}))
    }

    async fn make_candidate(self:&Arc<Self>,p:&Value,retry:bool)->Result<Value> {
        let id=text(p,"sessionId")?.to_owned();
        let run=operation_id(p,"runId")?.to_owned();let operation=format!("candidate:{id}:{run}");
        let epoch={let mut i=self.inner.lock().unwrap();self.auxiliary_available()?;claim_operation(&mut i,&operation)?;self.lifecycle_epoch.load(std::sync::atomic::Ordering::SeqCst)};
        let detail=db_detail(self.db_read(DbCommand::GetResult{session_id:id.clone()}).await?)?;
        if let Some(source)=p.get("sourceRevisionId").and_then(Value::as_str){if source!=detail.source.id{return Err(ApiError::new("revisionConflict","rewrite","识别来源已变化"));}}
        let mode=db_mode(self.db_read(DbCommand::ModeSnapshot{id:p.get("modeId").and_then(Value::as_str).unwrap_or(&detail.session.seed.mode_snapshot.mode_id).into(),revision:None}).await?)?;
        let base=detail.output.as_ref().map(|v|v.id.clone()).unwrap_or_default();
        if p.get("baseRevisionId").and_then(Value::as_str).is_some_and(|v|v!=base){return Err(ApiError::new("revisionConflict","rewrite","记录已变化，请重新核对"));}
        let config=self.db.snapshot().provider_config;
        cloud::endpoints(&config)?;
        let secret=credentials::read(&config)?;
        let cancel=CancellationToken::new();
        let current=db_detail(self.db_read(DbCommand::GetResult{session_id:id.clone()}).await?)?;
        let lease={let mut i=self.inner.lock().unwrap();self.auxiliary_available()?;
            if self.lifecycle_epoch.load(std::sync::atomic::Ordering::SeqCst)!=epoch{return Err(ApiError::new("cancelled","rewrite","系统状态变化，候选已取消"));}
            if i.cancelled_operation_ids.contains(&operation){return Err(ApiError::new("cancelled","rewrite","候选已取消"));}
            if i.shortcut_capture.is_some(){return Err(ApiError::new("capture_busy","rewrite","请先结束快捷键录制"));}
            if current.source.id!=detail.source.id||current.output.as_ref().map(|o|o.id.as_str()).unwrap_or("")!=base{return Err(ApiError::new("revisionConflict","rewrite","记录已变化，请重新核对"));}
            if let Some(old)=i.candidate.take(){old.cancel.cancel();if let Some(guard)=old.adopting{guard.cancel();}}
            i.candidate=Some(CandidateSlot{key:id.clone(),run:run.clone(),cancel:cancel.clone(),base,result:None,adopting:None});
            i.auxiliary_workers.insert(operation.clone(),cancel.clone());AuxiliaryLease{hub:Arc::clone(self),key:operation}
        };
        let generated=match (AuxiliaryFuture{future:Some(Box::pin(rewrite::rewrite(&config,&secret,&mode,&detail.source,cancel.clone()))),lease:Some(lease)}).await {
            Ok(value)=>value,Err(error)=>{let mut i=self.inner.lock().unwrap();if i.candidate.as_ref().is_some_and(|slot|slot.key==id&&slot.run==run){i.candidate=None;}return Err(ApiError::from(error).for_operation("application",&run));}
        };
        let candidate=OutputRevision{id:run.clone(),session_id:id.clone(),parent_revision_id:None,base_revision_id:detail.output.as_ref().map(|o|o.id.clone()),source_revision_id:detail.source.id,mode_snapshot:mode,resolved_mode:generated.resolved_mode,blocks:generated.blocks,kind:RevisionKind::Adopt,creation_reason:if retry{CreationReason::Retry}else{CreationReason::History},created_at:now()};
        let current=db_detail(self.db_read(DbCommand::GetResult{session_id:id.clone()}).await?)?;let mut i=self.inner.lock().unwrap();
        if i.guards.get(&id).is_some_and(|g|g.is_cancelled())||cancel.is_cancelled()||!i.candidate.as_ref().is_some_and(|s|s.key==id&&s.run==run&&!s.cancel.is_cancelled())||current.output.as_ref().map(|o|o.id.as_str()).unwrap_or("")!=candidate.base_revision_id.as_deref().unwrap_or(""){return Err(ApiError::new("revisionConflict","rewrite","当前记录已变化，候选不能覆盖新版本"));}
        i.candidate.as_mut().unwrap().result=Some(candidate.clone());wire(candidate)
    }
}
