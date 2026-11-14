//! Durable input orchestration; native checks belong to platform::delivery.
use super::*;

impl AppHub {
    pub(super) fn initial_delivery(&self,id:&str,generation:u64) {
        self.deliver_once(id,generation,
            |permit,target,text,gate|platform::delivery::dispatch_once(permit,target,text,gate));
    }
    #[cfg(test)]
    pub(super) fn initial_delivery_with(&self,id:&str,generation:u64,
        validate:impl Fn(&platform::delivery::TargetSnapshot)->bool,
        dispatch:impl Fn(&platform::delivery::DeliveryPermit,&platform::delivery::TargetSnapshot,&str,&dyn Fn()->bool)->platform::delivery::DispatchReceipt) {
        self.initial_delivery_with_validation(id,generation,|target|if validate(target){Ok(())}else{Err(platform::PlatformError::Unavailable("fixture target changed".into()))},dispatch);
    }
    #[cfg(test)]
    pub(super) fn initial_delivery_with_validation(&self,id:&str,generation:u64,
        validate:impl Fn(&platform::delivery::TargetSnapshot)->platform::Result<()>,
        dispatch:impl Fn(&platform::delivery::DeliveryPermit,&platform::delivery::TargetSnapshot,&str,&dyn Fn()->bool)->platform::delivery::DispatchReceipt) {
        self.deliver_once(id,generation,|permit,target,text,gate|{
            if let Err(error)=validate(target) {return platform::delivery::DispatchReceipt{
                outcome:platform::delivery::DispatchOutcome::Skipped,clipboard_changed:false,input_events:0,
                reason:Some(error.to_string()),reason_code:Some(platform::delivery::validation_failure_reason(target,&error).into())};}
            dispatch(permit,target,text,gate)
        });
    }
    /// Native delivery owns the two checks surrounding clipboard access.
    /// This coordinator owns only the durable once-only decision.
    pub(super) fn deliver_once(&self,id:&str,generation:u64,
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
        let rejection=target.is_none().then(||if target_timed_out{"targetInspectionTimeout"}else{capture_failure.as_deref().unwrap_or("targetUnavailableAtStop")});
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
        {let mut i=self.inner.lock().unwrap();if let Some(s)=i.active.as_mut().filter(|s|s.id==id&&s.generation==generation){if let Some(d)=s.delivery.as_mut(){d.state=state;d.reason=Some(reason.clone());}if persisted.is_err(){s.error=Some(ApiError::new("delivery_receipt_save_failed","storage","本机输入回执未保存；不会再次自动输入，请核对目标后手动复制").for_session(id,generation));}}}
        self.publish_session();self.changed();
        let current=self.inner.lock().unwrap().active.as_ref().is_some_and(|s|s.id==id&&s.generation==generation&&!s.cancel.is_cancelled()&&!s.original_delivery_revoked);
        // A saved/raw fallback is not a polished result. Keep its recovery
        // notice visible even when the one allowed paste was dispatched.
        if current{let _=self.native_action("recording",output.rewrite_fallback.is_some()||output.rewrite_recovery.is_some()||persisted.is_err()||reason=="partialInputUnconfirmed"||matches!(state,DeliveryState::Skipped|DeliveryState::Failed));}
    }

    pub fn copy_recoverable(&self,p:&Value)->Result<Value> {
        self.copy_recoverable_with(p,|content|platform::delivery::copy_text(content).map_err(Into::into))
    }
    pub(crate) fn copy_recoverable_with(&self,p:&Value,copy:impl FnOnce(&str)->Result<()>)->Result<Value> {
        // Main commands may contain recovery action selectors. Reduce them to
        // the exact live identity; the common native gate rechecks after waiting.
        let payload={let mut inner=self.inner.lock().unwrap();let s=matching(&mut inner,p)?;
            json!({"sessionId":s.id,"expectedGeneration":s.generation,
                "recoverableContentId":p.get("recoverableContentId").and_then(Value::as_str).map(|id|json!(id)).unwrap_or_else(||json!(s.id))})};
        self.copy_current_recorder_with("recoverable.copy",&payload,copy)
    }
}
