//! Nonblocking cancellation arbitration at the actual final COMMIT.
use std::sync::{Arc,Mutex,atomic::{AtomicBool,Ordering}};
use super::{StoreError,StoreResult};
#[derive(Debug,Clone,Copy,PartialEq,Eq)] pub enum CancelDecision {Accepted,Pending,AlreadyCompleted}
#[derive(Debug,Clone,Copy,PartialEq,Eq)] pub(crate) enum CommitKind {Checkpoint,Ready,Revision,Cleanup,Delivery}
#[derive(Debug,Default)] struct Intent {requested:AtomicBool,accepted:AtomicBool,completed:AtomicBool,finalizing:AtomicBool}
#[derive(Debug,Default)] struct Shared {gate:Mutex<()>,deleted:AtomicBool,ready:AtomicBool,finalizing:AtomicBool,record:Arc<Intent>}
#[derive(Debug)] pub struct CommitGuard {pub session_id:String,pub generation:u64,shared:Arc<Shared>,intent:Arc<Intent>,operation:bool}
impl CommitGuard {
 pub fn new(session_id:String,generation:u64)->Self {let shared=Arc::new(Shared::default());Self{session_id,generation,intent:shared.record.clone(),shared,operation:false}}
 /// A run-specific child shares record deletion/COMMIT ordering, never another run's cancel.
 pub fn operation(self:&Arc<Self>)->Arc<Self> {Arc::new(Self{session_id:self.session_id.clone(),generation:self.generation,shared:self.shared.clone(),intent:Arc::new(Intent::default()),operation:true})}
 fn completed(&self)->bool {if self.operation {self.intent.completed.load(Ordering::SeqCst)}else{self.ready_committed()}}
 pub fn cancel(&self)->CancelDecision {
  self.intent.requested.store(true,Ordering::SeqCst);
  match self.shared.gate.try_lock() {
   Ok(_gate)=>if self.completed(){CancelDecision::AlreadyCompleted}else{self.intent.accepted.store(true,Ordering::SeqCst);CancelDecision::Accepted},
   Err(std::sync::TryLockError::WouldBlock)=>{
    if self.completed(){CancelDecision::AlreadyCompleted}
    else if self.operation&&!self.intent.finalizing.load(Ordering::SeqCst){self.intent.accepted.store(true,Ordering::SeqCst);CancelDecision::Accepted}
    else{CancelDecision::Pending}
   },
   Err(std::sync::TryLockError::Poisoned(_))=>{self.intent.accepted.store(true,Ordering::SeqCst);CancelDecision::Accepted},
  }
 }
 pub fn delete(&self){self.shared.deleted.store(true,Ordering::SeqCst);}
 pub fn is_cancelled(&self)->bool {self.intent.accepted.load(Ordering::SeqCst)||self.shared.deleted.load(Ordering::SeqCst)||(!self.ready_committed()&&self.shared.record.accepted.load(Ordering::SeqCst))}
 pub fn ready_committed(&self)->bool {self.shared.ready.load(Ordering::SeqCst)}
 pub fn pending(&self)->bool {self.intent.requested.load(Ordering::SeqCst)&&!self.intent.accepted.load(Ordering::SeqCst)&&!self.completed()}
 pub fn finalizing(&self)->bool {self.shared.finalizing.load(Ordering::SeqCst)}
 /// Only called by the DB owner after reading an already-committed Ready record.
 pub(crate) fn observe_ready(&self)->StoreResult<()> {let _gate=self.shared.gate.lock().map_err(|_|StoreError::Unhealthy("commit gate poisoned".into()))?;self.shared.ready.store(true,Ordering::SeqCst);self.shared.record.accepted.store(false,Ordering::SeqCst);Ok(())}
 pub(crate) fn finalize<T>(&self,kind:CommitKind,commit:impl FnOnce()->StoreResult<T>)->StoreResult<T> {
  let _gate=self.shared.gate.lock().map_err(|_|StoreError::Unhealthy("commit gate poisoned".into()))?;
  // Set the per-operation phase BEFORE checking requests. A sibling can accept
  // cancellation immediately; only this operation's real finalizer makes it Pending.
  self.intent.finalizing.store(true,Ordering::SeqCst);
  struct Reset<'a>(&'a AtomicBool);impl Drop for Reset<'_>{fn drop(&mut self){self.0.store(false,Ordering::SeqCst);}}
  let _own_phase=Reset(&self.intent.finalizing);
  if kind!=CommitKind::Cleanup {
   let own_cancel=self.intent.requested.load(Ordering::SeqCst)||self.intent.accepted.load(Ordering::SeqCst);
   let record_cancel=!self.ready_committed()&&self.shared.record.requested.load(Ordering::SeqCst);
   let completed_record_revision=kind==CommitKind::Revision&&!self.operation&&self.ready_committed();
   if self.shared.deleted.load(Ordering::SeqCst)||record_cancel||(own_cancel&&!completed_record_revision) {if !self.completed(){self.intent.accepted.store(true,Ordering::SeqCst);}return Err(StoreError::Cancelled);}
  }
  self.shared.finalizing.store(true,Ordering::SeqCst);
  let result=commit();
  if result.is_ok(){if kind==CommitKind::Ready{self.shared.ready.store(true,Ordering::SeqCst);}if self.operation{self.intent.completed.store(true,Ordering::SeqCst);}}
  if self.intent.requested.load(Ordering::SeqCst)&&!self.completed(){self.intent.accepted.store(true,Ordering::SeqCst);}
  if self.shared.record.requested.load(Ordering::SeqCst)&&!self.ready_committed(){self.shared.record.accepted.store(true,Ordering::SeqCst);}
  self.shared.finalizing.store(false,Ordering::SeqCst);
  result
 }
}
