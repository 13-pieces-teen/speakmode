//! Single-owner SQLite actor. No caller waits while holding the engine control lock.
use super::*;
use serde::Serialize;
use std::{path::{Path,PathBuf},sync::{Arc,Mutex,Condvar,atomic::{AtomicBool,AtomicUsize,Ordering}},time::Duration};
use crossbeam_channel::{Sender,Receiver,bounded,TrySendError};
use tokio::sync::Notify;

#[derive(Debug,Clone)]
pub struct DbSnapshot {pub revision:u64,pub onboarding:OnboardingProgress,pub personalization:PersonalizationConfig,pub preferences:Preferences,pub provider_config:ProviderConfig,pub modes:Vec<ModeRecord>,pub dictionary:DictionarySnapshot,pub usage:DailyUsage,pub temporary_result:Option<HistoryDetail>}
#[derive(Debug,Clone,Serialize)]
pub enum DbCommand {
 GetPersonalization,SavePersonalization{config:PersonalizationConfig,expected_version:u64},RecordPersonalizationEvent{event:PersonalizationEvent},GetPersonalizationEvents{since:Option<String>},SetPersonalizationFeedback{task_id:String,feedback:PersonalizationFeedback},
 ReadSnapshot{date:String},Onboarding,SaveOnboarding{progress:OnboardingProgress,expected_version:u64},Preferences,ProviderConfig,ListModes,Dictionary,SaveDictionary{draft:DictionaryDraft,expected_version:u64},ImportDictionary{terms:Vec<String>,expected_version:u64},DeleteDictionary{entry_id:String,expected_version:u64},ModeSnapshot{id:String,revision:Option<u64>},
 SavePreferences{next:Preferences,expected_version:u64},SaveProviderConfig{config:ProviderConfig,expected_version:u64},SaveMode{draft:ModeDraft},DeleteMode{id:String,expected_revision:u64},SetDefaultMode{id:String,expected_version:u64},
 GetResult{session_id:String},GetSource{source_id:String},GetOutput{revision_id:String},HistoryList{query:String,cursor:u64,limit:u64},UsageToday{date:String},UsageTotal,UsageEvent{session_id:String},
 Checkpoint{seed:SessionSeed,source:SourceRevision},Ready{output:OutputRevision},UseSource{session_id:String,expected_generation:u64,created_at:String},Adopt{session_id:String,expected_current_revision_id:String,candidate:OutputRevision},Edit{session_id:String,expected_current_revision_id:String,blocks:Vec<OutputBlock>,created_at:String},Restore{session_id:String,expected_current_revision_id:String,patch:RestorePatch,created_at:String},Undo{session_id:String,expected_current_revision_id:String,created_at:String},CancelUnfinished{session_id:String},DeleteHistory{session_id:String},PrepareDelivery{attempt:DeliveryAttempt},TransitionDelivery{attempt_id:String,expected:DeliveryState,next:DeliveryState,reason:Option<String>},DeliveryAttempt{attempt_id:String},ClearTemporary,Shutdown,
 #[cfg(test)] TestExecuteBatch{sql:String},
 CorrectTerm{session_id:String,expected_current_revision_id:String,patch:CorrectionPatch,expected_dictionary_version:Option<u64>,created_at:String},
 #[cfg(test)] TestQueryCount{sql:String},
}
#[derive(Debug,Clone)]
pub enum DbValue {Snapshot(DbSnapshot),Personalization(PersonalizationConfig),PersonalizationEvents(Vec<PersonalizationEvent>),Onboarding(OnboardingProgress),Preferences(Preferences),ProviderConfig(ProviderConfig),Modes(Vec<ModeRecord>),Dictionary(DictionarySnapshot),Correction(CorrectionReceipt),Mode(ModeRecord),ModeSnapshot(ModeSnapshot),Detail(HistoryDetail),Source(SourceRevision),Output(OutputRevision),HistoryPage(HistoryPage),Usage(DailyUsage),UsageTotal(UsageTotals),UsageEvent(Option<UsageEvent>),Delivery(DeliveryAttempt),Unit,#[cfg(test)] Count(u64)}
#[derive(Debug,Clone,Copy,PartialEq,Eq)]
pub enum DbPriority {Normal,Critical}
#[derive(Debug,Clone)]
pub struct DbOptions {pub normal_capacity:usize,pub critical_capacity:usize,pub byte_budget:usize,pub idle_refresh:Duration}
impl Default for DbOptions {fn default()->Self {Self{normal_capacity:32,critical_capacity:8,byte_budget:8*1024*1024,idle_refresh:Duration::from_millis(250)}}}

#[derive(Default)]
struct Outcome {result:Mutex<Option<StoreResult<DbValue>>>,cv:Condvar,notify:Notify}
impl Outcome {
 fn complete(&self,value:StoreResult<DbValue>){*self.result.lock().unwrap_or_else(|p|p.into_inner())=Some(value);self.cv.notify_all();self.notify.notify_waiters();}
}
/// Observers do not own/cancel the operation. Dropping or aborting a waiter leaves it running.
#[derive(Clone)]
pub struct DbTicket {outcome:Arc<Outcome>}
impl DbTicket {
 pub async fn wait(&self)->StoreResult<DbValue> {loop {let notified=self.outcome.notify.notified();tokio::pin!(notified);notified.as_mut().enable();if let Some(result)=self.outcome.result.lock().unwrap_or_else(|p|p.into_inner()).clone(){return result;}notified.await;}}
 pub fn wait_blocking(&self)->StoreResult<DbValue> {let mut result=self.outcome.result.lock().unwrap_or_else(|p|p.into_inner());while result.is_none(){result=self.outcome.cv.wait(result).unwrap_or_else(|p|p.into_inner());}result.as_ref().unwrap().clone()}
 pub fn is_complete(&self)->bool {self.outcome.result.lock().unwrap_or_else(|p|p.into_inner()).is_some()}
}
struct Job {command:DbCommand,guard:Option<Arc<CommitGuard>>,outcome:Arc<Outcome>,bytes:usize,cleanup_key:Option<String>,statistics_event:bool}
struct Shared {snapshot:Mutex<Option<DbSnapshot>>,cache_error:Mutex<Option<StoreError>>,admission:Mutex<bool>,outstanding:AtomicUsize,bytes:AtomicUsize,owner_released:AtomicBool,stopping:AtomicBool,shutdown:Arc<Outcome>,cleanup:Mutex<Option<(String,Arc<Outcome>)>>,statistics_incomplete:AtomicBool}
pub struct DbClient {wake:Sender<()>,normal:Sender<Job>,critical:Sender<Job>,cleanup:Sender<Job>,shared:Arc<Shared>,options:DbOptions}
impl Clone for DbClient {fn clone(&self)->Self {Self{wake:self.wake.clone(),normal:self.normal.clone(),critical:self.critical.clone(),cleanup:self.cleanup.clone(),shared:self.shared.clone(),options:self.options.clone()}}}
impl DbClient {
 pub fn open(path:impl AsRef<Path>)->StoreResult<Self> {Self::start(Some(path.as_ref().to_path_buf()),DbOptions::default())}
 pub fn in_memory()->StoreResult<Self> {Self::start(None,DbOptions::default())}
 pub fn open_with_options(path:impl AsRef<Path>,options:DbOptions)->StoreResult<Self> {Self::start(Some(path.as_ref().to_path_buf()),options)}
 pub fn in_memory_with_options(options:DbOptions)->StoreResult<Self> {Self::start(None,options)}
 fn start(path:Option<PathBuf>,options:DbOptions)->StoreResult<Self> {
  if options.normal_capacity==0||options.critical_capacity==0||options.byte_budget==0||options.idle_refresh.is_zero(){return Err(StoreError::Invalid("DB worker bounds must be positive".into()));}
  let (wake,wrx)=bounded(1);let (normal,nrx)=bounded(options.normal_capacity);let (critical,crx)=bounded(options.critical_capacity);let (cleanup,rrx)=bounded(1);let (boot,brecv)=bounded(1);
  let shared=Arc::new(Shared{snapshot:Mutex::new(None),cache_error:Mutex::new(None),admission:Mutex::new(true),outstanding:AtomicUsize::new(0),bytes:AtomicUsize::new(0),owner_released:AtomicBool::new(false),stopping:AtomicBool::new(false),shutdown:Arc::new(Outcome::default()),cleanup:Mutex::new(None),statistics_incomplete:AtomicBool::new(false)});
  let state=shared.clone();let opts=options.clone();
  std::thread::Builder::new().name("speakmode-db".into()).spawn(move||worker(path,opts,state,nrx,crx,rrx,wrx,boot)).map_err(|e|StoreError::Database(format!("cannot start DB owner: {e}")))?;
  brecv.recv().map_err(|_|StoreError::WorkerStopped)??;
  Ok(Self{wake,normal,critical,cleanup,shared,options})
 }
 pub fn snapshot(&self)->DbSnapshot {self.shared.snapshot.lock().unwrap_or_else(|p|p.into_inner()).as_ref().expect("bootstrap installs a snapshot").clone()}
 pub fn cache_error(&self)->Option<StoreError> {self.shared.cache_error.lock().unwrap_or_else(|p|p.into_inner()).clone()}
 pub fn outstanding(&self)->usize {self.shared.outstanding.load(Ordering::SeqCst)}
 pub fn queued_bytes(&self)->usize {self.shared.bytes.load(Ordering::SeqCst)}
 pub fn owner_released(&self)->bool {self.shared.owner_released.load(Ordering::SeqCst)}
 pub fn statistics_incomplete(&self)->bool {self.shared.statistics_incomplete.load(Ordering::SeqCst)}
 pub fn request_shutdown(&self) {*self.shared.admission.lock().unwrap_or_else(|p|p.into_inner())=false;self.shared.stopping.store(true,Ordering::SeqCst);let _=self.wake.try_send(());}
 pub async fn shutdown(&self)->StoreResult<()> {self.request_shutdown();DbTicket{outcome:self.shared.shutdown.clone()}.wait().await.map(|_|())}
 pub fn submit(&self,command:DbCommand,guard:Option<Arc<CommitGuard>>,priority:DbPriority)->StoreResult<DbTicket> {
  let statistics_event=matches!(command,DbCommand::RecordPersonalizationEvent{..});
  let result=self.enqueue(command,guard,priority,statistics_event);
  if statistics_event&&result.is_err(){self.shared.statistics_incomplete.store(true,Ordering::SeqCst);}
  result
 }
 fn enqueue(&self,command:DbCommand,guard:Option<Arc<CommitGuard>>,priority:DbPriority,statistics_event:bool)->StoreResult<DbTicket> {
  if matches!(command,DbCommand::Shutdown){self.request_shutdown();return Ok(DbTicket{outcome:self.shared.shutdown.clone()});}
  let bytes=command.payload_bytes()?;
  let admission=self.shared.admission.lock().map_err(|_|StoreError::WorkerStopped)?;
  if !*admission||self.owner_released(){return Err(StoreError::WorkerStopped);}
  let cleanup_key=if let DbCommand::CancelUnfinished{session_id}=&command {Some(session_id.clone())}else{None};
  // This reserved lane is never occupied by history/settings/read traffic. Duplicate
  // cancellation for the one active session shares its actual cleanup receipt.
  let mut cleanup_state=self.shared.cleanup.lock().map_err(|_|StoreError::WorkerStopped)?;
  if let Some(key)=&cleanup_key {if let Some((active,outcome))=&*cleanup_state {return if active==key {Ok(DbTicket{outcome:outcome.clone()})}else{Err(StoreError::Busy)};}}
  let used=self.queued_bytes();
  // Reserved cleanup has bounded opaque IDs and does not compete for body budget.
  if cleanup_key.is_none() && (bytes>self.options.byte_budget||used>self.options.byte_budget-bytes){return Err(StoreError::Busy);}
  if cleanup_key.is_some()&&bytes>2048{return Err(StoreError::Invalid("cleanup identity too large".into()));}
  let reserved_bytes=if cleanup_key.is_some(){0}else{bytes};let outcome=Arc::new(Outcome::default());let ticket=DbTicket{outcome:outcome.clone()};
  self.shared.bytes.fetch_add(reserved_bytes,Ordering::SeqCst);self.shared.outstanding.fetch_add(1,Ordering::SeqCst);
  if let Some(key)=&cleanup_key {*cleanup_state=Some((key.clone(),outcome.clone()));}
  let job=Job{command,guard,outcome,bytes:reserved_bytes,cleanup_key:cleanup_key.clone(),statistics_event};
  let sender=if cleanup_key.is_some(){&self.cleanup}else if priority==DbPriority::Critical{&self.critical}else{&self.normal};
  match sender.try_send(job) {Ok(())=>Ok(ticket),Err(error)=>{self.shared.bytes.fetch_sub(reserved_bytes,Ordering::SeqCst);self.shared.outstanding.fetch_sub(1,Ordering::SeqCst);if cleanup_key.is_some(){*cleanup_state=None;}Err(match error{TrySendError::Full(_)=>StoreError::Busy,TrySendError::Disconnected(_)=>StoreError::WorkerStopped})}}
 }
}
impl DbCommand {
 fn payload_bytes(&self)->StoreResult<usize>{struct Count(usize);impl std::io::Write for Count{fn write(&mut self,b:&[u8])->std::io::Result<usize>{self.0=self.0.saturating_add(b.len());Ok(b.len())}fn flush(&mut self)->std::io::Result<()>{Ok(())}}let mut count=Count(0);serde_json::to_writer(&mut count,self)?;Ok(count.0)}
 fn writes(&self)->bool {#[cfg(test)] if matches!(self,Self::TestQueryCount{..}){return false;} !matches!(self,Self::GetPersonalization|Self::GetPersonalizationEvents{..}|Self::ReadSnapshot{..}|Self::Onboarding|Self::Preferences|Self::ProviderConfig|Self::ListModes|Self::Dictionary|Self::ModeSnapshot{..}|Self::GetResult{..}|Self::GetSource{..}|Self::GetOutput{..}|Self::HistoryList{..}|Self::UsageToday{..}|Self::UsageTotal|Self::UsageEvent{..}|Self::DeliveryAttempt{..})}
}
// Implementation is kept below the DTOs so engine callers can migrate independently.

fn today()->String {chrono::Local::now().format("%Y-%m-%d").to_string()}
fn build_snapshot(store:&Store,date:&str,revision:u64)->StoreResult<DbSnapshot>{store.read_consistent(|store|Ok(DbSnapshot{revision,onboarding:store.onboarding()?,personalization:store.personalization()?,preferences:store.preferences()?,provider_config:store.provider_config()?,modes:store.list_modes()?,dictionary:store.dictionary()?,usage:store.usage_today(date)?,temporary_result:store.latest_temporary_result()?}))}
fn refresh(store:&Store,shared:&Shared,date:&str)->StoreResult<DbSnapshot>{
 let revision=shared.snapshot.lock().unwrap_or_else(|p|p.into_inner()).as_ref().map_or(1,|s|s.revision.saturating_add(1));
 match build_snapshot(store,date,revision){Ok(snapshot)=>{*shared.snapshot.lock().unwrap_or_else(|p|p.into_inner())=Some(snapshot.clone());*shared.cache_error.lock().unwrap_or_else(|p|p.into_inner())=None;Ok(snapshot)},Err(error)=>{*shared.cache_error.lock().unwrap_or_else(|p|p.into_inner())=Some(error.clone());Err(error)}}
}
fn validate_guard(store:&Store,command:&DbCommand,guard:Option<&CommitGuard>)->StoreResult<()> {
 let identity:Option<(&str,Option<u64>)>=match command{
  DbCommand::Checkpoint{seed,..}=>Some((&seed.id,Some(seed.generation))),DbCommand::Ready{output}=>Some((&output.session_id,None)),
  DbCommand::UseSource{session_id,expected_generation,..}=>Some((session_id,Some(*expected_generation))),
  DbCommand::Adopt{session_id,..}|DbCommand::Edit{session_id,..}|DbCommand::CorrectTerm{session_id,..}|DbCommand::Restore{session_id,..}|DbCommand::Undo{session_id,..}|DbCommand::CancelUnfinished{session_id}|DbCommand::DeleteHistory{session_id}=>Some((session_id,None)),
  DbCommand::PrepareDelivery{attempt}=>Some((&attempt.session_id,Some(attempt.generation))),_=>None,
 };
 let requires=matches!(command,DbCommand::SaveDictionary{..}|DbCommand::ImportDictionary{..}|DbCommand::DeleteDictionary{..}|DbCommand::Checkpoint{..}|DbCommand::Ready{..}|DbCommand::UseSource{..}|DbCommand::Adopt{..}|DbCommand::Edit{..}|DbCommand::CorrectTerm{..}|DbCommand::Restore{..}|DbCommand::Undo{..}|DbCommand::PrepareDelivery{..});
 if requires&&guard.is_none(){return Err(StoreError::Invalid("session content writes require a commit guard".into()));}
 if let (Some((id,generation)),Some(guard))=(identity,guard){
  if id!=guard.session_id{return Err(StoreError::Invalid("commit guard session identity mismatch".into()));}
  if generation.is_some_and(|value|value!=guard.generation){return Err(StoreError::Conflict{current:"commit guard generation mismatch".into()});}
  match store.get_result(id){Ok(detail)=>{if detail.session.seed.generation!=guard.generation{return Err(StoreError::Conflict{current:"stored generation differs from guard".into()});}if detail.session.status==StoredSessionStatus::Ready{guard.observe_ready()?;}},Err(StoreError::NotFound)=>{},Err(error)=>return Err(error)}
 }
 Ok(())
}
fn execute(store:&mut Store,command:DbCommand,guard:Option<Arc<CommitGuard>>)->StoreResult<DbValue>{
 validate_guard(store,&command,guard.as_deref())?;
 if let Some(guard)=guard {store.with_commit_guard(guard,|store|execute_inner(store,command))}else{execute_inner(store,command)}
}
fn execute_inner(store:&mut Store,command:DbCommand)->StoreResult<DbValue>{
 use DbCommand as C;use DbValue as V;
 match command{
  C::GetPersonalization=>Ok(V::Personalization(store.personalization()?)),C::SavePersonalization{config,expected_version}=>Ok(V::Personalization(store.save_personalization(config,expected_version)?)),C::GetPersonalizationEvents{since}=>Ok(V::PersonalizationEvents(store.personalization_events(since.as_deref())?)),C::RecordPersonalizationEvent{event}=>{store.record_personalization_event(event)?;Ok(V::Unit)},C::SetPersonalizationFeedback{task_id,feedback}=>{store.set_personalization_feedback(&task_id,feedback)?;Ok(V::Unit)},
  C::Onboarding=>Ok(V::Onboarding(store.onboarding()?)),C::SaveOnboarding{progress,expected_version}=>Ok(V::Onboarding(store.save_onboarding(progress,expected_version)?)),
  C::Preferences=>Ok(V::Preferences(store.preferences()?)),C::ProviderConfig=>Ok(V::ProviderConfig(store.provider_config()?)),C::ListModes=>Ok(V::Modes(store.list_modes()?)),C::Dictionary=>Ok(V::Dictionary(store.dictionary()?)),C::SaveDictionary{draft,expected_version}=>Ok(V::Dictionary(store.save_dictionary_entry(draft,expected_version)?)),C::ImportDictionary{terms,expected_version}=>Ok(V::Dictionary(store.import_dictionary(terms,expected_version)?)),C::DeleteDictionary{entry_id,expected_version}=>Ok(V::Dictionary(store.delete_dictionary_entry(&entry_id,expected_version)?)),C::ModeSnapshot{id,revision}=>Ok(V::ModeSnapshot(store.mode_snapshot(&id,revision)?)),
  C::SavePreferences{next,expected_version}=>Ok(V::Preferences(store.save_preferences(next,expected_version)?)),C::SaveProviderConfig{config,expected_version}=>Ok(V::ProviderConfig(store.save_provider_config(config,expected_version)?)),C::SaveMode{draft}=>Ok(V::Mode(store.save_mode(draft)?)),C::DeleteMode{id,expected_revision}=>{store.delete_mode(&id,expected_revision)?;Ok(V::Unit)},C::SetDefaultMode{id,expected_version}=>Ok(V::Preferences(store.set_default_mode(&id,expected_version)?)),
  C::GetResult{session_id}=>Ok(V::Detail(store.get_result(&session_id)?)),C::GetSource{source_id}=>Ok(V::Source(store.get_source(&source_id)?)),C::GetOutput{revision_id}=>Ok(V::Output(store.get_output(&revision_id)?)),C::HistoryList{query,cursor,limit}=>Ok(V::HistoryPage(store.history_list(&query,cursor,limit)?)),C::UsageToday{date}=>Ok(V::Usage(store.usage_today(&date)?)),C::UsageTotal=>Ok(V::UsageTotal(store.usage_totals()?)),C::UsageEvent{session_id}=>Ok(V::UsageEvent(store.usage_event(&session_id)?)),
  C::Checkpoint{seed,source}=>Ok(V::Detail(store.checkpoint_source(seed,source)?)),C::Ready{output}=>Ok(V::Detail(store.commit_ready(output)?)),C::UseSource{session_id,expected_generation,created_at}=>Ok(V::Detail(store.use_source(&session_id,expected_generation,&created_at)?)),C::Adopt{session_id,expected_current_revision_id,candidate}=>Ok(V::Detail(store.adopt_candidate(&session_id,&expected_current_revision_id,candidate)?)),C::Edit{session_id,expected_current_revision_id,blocks,created_at}=>Ok(V::Detail(store.edit_output(&session_id,&expected_current_revision_id,blocks,&created_at)?)),C::Restore{session_id,expected_current_revision_id,patch,created_at}=>Ok(V::Detail(store.restore_output(&session_id,&expected_current_revision_id,patch,&created_at)?)),C::Undo{session_id,expected_current_revision_id,created_at}=>Ok(V::Detail(store.undo_output(&session_id,&expected_current_revision_id,&created_at)?)),C::CancelUnfinished{session_id}=>{store.cancel_unfinished(&session_id)?;Ok(V::Unit)},C::DeleteHistory{session_id}=>{store.delete_history(&session_id)?;Ok(V::Unit)},
  C::PrepareDelivery{attempt}=>Ok(V::Delivery(store.prepare_delivery(attempt)?)),C::TransitionDelivery{attempt_id,expected,next,reason}=>Ok(V::Delivery(store.transition_delivery(&attempt_id,expected,next,reason)?)),C::DeliveryAttempt{attempt_id}=>Ok(V::Delivery(store.delivery_attempt(&attempt_id)?)),C::ClearTemporary=>{store.clear_temporary_results();Ok(V::Unit)},
  C::ReadSnapshot{..}|C::Shutdown=>Err(StoreError::Invalid("worker control reached SQL dispatcher".into())),
  #[cfg(test)] C::TestQueryCount{sql}=>Ok(V::Count(store.test_connection().query_row(&sql,[],|r|r.get(0))?)),
  #[cfg(test)] C::TestExecuteBatch{sql}=>{store.test_connection().execute_batch(&sql)?;Ok(V::Unit)},
  C::CorrectTerm{session_id,expected_current_revision_id,patch,expected_dictionary_version,created_at}=>Ok(V::Correction(store.correct_term(&session_id,&expected_current_revision_id,patch,expected_dictionary_version,&created_at)?)),
 }
}
// Queries that assemble multiple tables execute in a consistent read transaction.
fn execute_read(store:&Store,command:DbCommand)->StoreResult<DbValue>{store.read_consistent(|s|{
 use DbCommand as C;use DbValue as V;match command{
 C::GetPersonalization=>Ok(V::Personalization(s.personalization()?)),C::GetPersonalizationEvents{since}=>Ok(V::PersonalizationEvents(s.personalization_events(since.as_deref())?)),
 C::Onboarding=>Ok(V::Onboarding(s.onboarding()?)),C::Preferences=>Ok(V::Preferences(s.preferences()?)),C::ProviderConfig=>Ok(V::ProviderConfig(s.provider_config()?)),C::ListModes=>Ok(V::Modes(s.list_modes()?)),C::Dictionary=>Ok(V::Dictionary(s.dictionary()?)),C::ModeSnapshot{id,revision}=>Ok(V::ModeSnapshot(s.mode_snapshot(&id,revision)?)),C::GetResult{session_id}=>Ok(V::Detail(s.get_result(&session_id)?)),C::GetSource{source_id}=>Ok(V::Source(s.get_source(&source_id)?)),C::GetOutput{revision_id}=>Ok(V::Output(s.get_output(&revision_id)?)),C::HistoryList{query,cursor,limit}=>Ok(V::HistoryPage(s.history_list(&query,cursor,limit)?)),C::UsageToday{date}=>Ok(V::Usage(s.usage_today(&date)?)),C::UsageTotal=>Ok(V::UsageTotal(s.usage_totals()?)),C::UsageEvent{session_id}=>Ok(V::UsageEvent(s.usage_event(&session_id)?)),C::DeliveryAttempt{attempt_id}=>Ok(V::Delivery(s.delivery_attempt(&attempt_id)?)),
 #[cfg(test)] C::TestQueryCount{sql}=>Ok(V::Count(s.test_connection().query_row(&sql,[],|r|r.get(0))?)),_=>Err(StoreError::Invalid("write reached read dispatcher".into()))}
})}
fn finish_job(shared:&Shared,job:Job,result:StoreResult<DbValue>){
 if job.statistics_event&&result.is_err(){shared.statistics_incomplete.store(true,Ordering::SeqCst);}
 shared.bytes.fetch_sub(job.bytes,Ordering::SeqCst);shared.outstanding.fetch_sub(1,Ordering::SeqCst);
 if job.cleanup_key.is_some(){*shared.cleanup.lock().unwrap_or_else(|p|p.into_inner())=None;}
 job.outcome.complete(result);
}
fn worker(path:Option<PathBuf>,options:DbOptions,shared:Arc<Shared>,normal:Receiver<Job>,critical:Receiver<Job>,cleanup:Receiver<Job>,wake:Receiver<()>,boot:Sender<StoreResult<()>>){
 let opened=(||{let mut store=if let Some(path)=path{Store::open(path)?}else{Store::in_memory()?};store.apply_app_upgrades()?;store.recover_after_restart()?;refresh(&store,&shared,&today())?;
  shared.statistics_incomplete.store(store.begin_statistics_run()?,Ordering::SeqCst);Ok::<_,StoreError>(store)})();
 let mut store=match opened{Ok(store)=>{let _=boot.send(Ok(()));store},Err(error)=>{let _=boot.send(Err(error.clone()));shared.owner_released.store(true,Ordering::SeqCst);shared.shutdown.complete(Err(error));return;}};
 let mut critical_streak=0;let mut fatal=None;
 loop{
  if shared.stopping.load(Ordering::SeqCst)&&shared.outstanding.load(Ordering::SeqCst)==0{break;}
  let mut job=None;
  if critical_streak>=4{job=normal.try_recv().ok();if job.is_some(){critical_streak=0;}}
  if job.is_none(){job=cleanup.try_recv().ok();if job.is_some(){critical_streak+=1;}}
  if job.is_none(){job=critical.try_recv().ok();if job.is_some(){critical_streak+=1;}}
  if job.is_none(){job=normal.try_recv().ok();if job.is_some(){critical_streak=0;}}
  if job.is_none(){
   crossbeam_channel::select_biased!{
    recv(wake)->value=>{if value.is_err(){shared.stopping.store(true,Ordering::SeqCst);}},
    recv(cleanup)->value=>match value{Ok(value)=>{job=Some(value);critical_streak+=1;},Err(_)=>shared.stopping.store(true,Ordering::SeqCst)},
    recv(critical)->value=>match value{Ok(value)=>{job=Some(value);critical_streak+=1;},Err(_)=>shared.stopping.store(true,Ordering::SeqCst)},
    recv(normal)->value=>match value{Ok(value)=>{job=Some(value);critical_streak=0;},Err(_)=>shared.stopping.store(true,Ordering::SeqCst)},
    default(options.idle_refresh)=>{}
   }
  }
  let Some(mut job)=job else{
   if shared.stopping.load(Ordering::SeqCst){continue;}
   let date=today();let stale=shared.snapshot.lock().unwrap_or_else(|p|p.into_inner()).as_ref().is_some_and(|s|s.usage.date!=date);
   if fatal.is_none()&&(stale||shared.cache_error.lock().unwrap_or_else(|p|p.into_inner()).is_some()){let _=refresh(&store,&shared,&date);}continue;
  };
  if let Some(error)=fatal.clone(){finish_job(&shared,job,Err(error));continue;}
  let writes=job.command.writes();
  // Event rows do not change the configuration/result snapshot. Avoid a full
  // preferences/history/usage read after every optional telemetry write.
  let refresh_snapshot=writes&&!matches!(job.command,DbCommand::RecordPersonalizationEvent{..}|DbCommand::SetPersonalizationFeedback{..});
  let command=std::mem::replace(&mut job.command,DbCommand::Preferences);
  let clears_temporary=matches!(command,DbCommand::ClearTemporary);
  // The actor owns the accepted command until its actual completion; receiver drop
  // does not affect execution, cache publication, or reservation lifetime.
  let result=std::panic::catch_unwind(std::panic::AssertUnwindSafe(||{
   if let Some(error)=store.health_error(){return Err(error);}
   let result=if let DbCommand::ReadSnapshot{date}=&command{refresh(&store,&shared,date).map(DbValue::Snapshot)}else if writes{execute(&mut store,command,job.guard.clone())}else{execute_read(&store,command)};
   if let Some(error)=store.health_error(){return Err(error);}
   // A successfully committed write is never relabelled failed because this read fails.
   if result.is_ok()&&writes{
    // The next session must freeze the committed model, region, and version even
    // if an unrelated snapshot query fails after the provider update commits.
    if let Ok(DbValue::ProviderConfig(config))=&result {if let Some(snapshot)=shared.snapshot.lock().unwrap_or_else(|p|p.into_inner()).as_mut(){snapshot.provider_config=config.clone();snapshot.revision=snapshot.revision.saturating_add(1);}}
    // Publish the committed vocabulary even if an unrelated full-cache read fails.
    // Deleted terms cannot survive in the session-start cache after a successful delete.
    if let Ok(DbValue::Dictionary(dictionary))=&result {if let Some(snapshot)=shared.snapshot.lock().unwrap_or_else(|p|p.into_inner()).as_mut(){snapshot.dictionary=dictionary.clone();snapshot.revision=snapshot.revision.saturating_add(1);}}
    // A committed disable/delete is visible to the next task even if another cache query fails.
    if let Ok(DbValue::Personalization(config))=&result {if let Some(snapshot)=shared.snapshot.lock().unwrap_or_else(|p|p.into_inner()).as_mut(){snapshot.personalization=config.clone();snapshot.temporary_result=None;snapshot.revision=snapshot.revision.saturating_add(1);}}
    // Never keep removed/replaced MemoryOnly bodies merely because an unrelated
    // preferences/Mode query prevents a complete cache refresh. Old counters remain
    // explicitly stale under cache_error; they are never replaced by invented zeroes.
    let replaces_temporary=matches!(&result,Ok(DbValue::Detail(detail)) if detail.session.seed.storage_policy==StoragePolicy::MemoryOnly&&detail.output.is_some());
    if clears_temporary||replaces_temporary{if let Some(snapshot)=shared.snapshot.lock().unwrap_or_else(|p|p.into_inner()).as_mut(){if snapshot.temporary_result.take().is_some(){snapshot.revision=snapshot.revision.saturating_add(1);}}}
    if refresh_snapshot {let _=refresh(&store,&shared,&today());}
   }result
  }));
  let result=match result{Ok(result)=>result,Err(_)=>Err(StoreError::Unhealthy("DB owner panicked while executing accepted request".into()))};
  if let Err(StoreError::Unhealthy(_))=&result{fatal=result.as_ref().err().cloned();*shared.cache_error.lock().unwrap_or_else(|p|p.into_inner())=fatal.clone();*shared.admission.lock().unwrap_or_else(|p|p.into_inner())=false;shared.stopping.store(true,Ordering::SeqCst);}
  finish_job(&shared,job,result);
 }
 store.clear_temporary_results();
 if let Some(snapshot)=shared.snapshot.lock().unwrap_or_else(|p|p.into_inner()).as_mut(){snapshot.temporary_result=None;}
 let mut health=fatal.or_else(||store.health_error());
 if health.is_none(){if let Err(error)=store.finish_statistics_run(shared.statistics_incomplete.load(Ordering::SeqCst)){health=Some(error);}}
 drop(store); // The connection and all MemoryOnly bodies are gone BEFORE the owner ACK.
 shared.owner_released.store(true,Ordering::SeqCst);
 shared.shutdown.complete(health.map_or(Ok(DbValue::Unit),Err));
}
