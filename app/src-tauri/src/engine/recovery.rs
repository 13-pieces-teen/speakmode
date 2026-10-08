//! One explicitly managed, memory-only recovery slot. Finished capture owns its
//! original 30-minute deadline; moving between slots never extends that lease.
use super::*;

impl ActiveSession {
    pub(crate) fn recovery_view(&self)->Option<Value> {
        let remaining=AUDIO_CACHE_RETENTION.checked_sub(self.cache_created?.elapsed())?;
        if !self.view().can_retry_asr{return None;}
        let expires=chrono::Utc::now()+chrono::Duration::from_std(remaining).ok()?;
        Some(json!({"session":self.view(),"expiresAt":expires.to_rfc3339()}))
    }
}

impl Inner {
    pub(crate) fn expire_audio_caches(&mut self)->bool {
        let active=self.active.as_mut().is_some_and(ActiveSession::expire_audio_cache);
        let retained=self.recovery.as_mut().is_some_and(ActiveSession::expire_audio_cache);
        if retained{self.recovery=None;}
        active||retained
    }

    pub(super) fn park_retryable_failure(&mut self)->Result<()> {
        self.expire_audio_caches();
        if self.active.as_ref().is_some_and(|s|s.pending.is_some()){
            return Err(ApiError::new("save_recovery_pending","recovery","当前口述尚未保存，请先重试保存或明确放弃，再开始下一条。"));
        }
        if self.active.as_ref().is_some_and(|s|s.view().can_retry_asr){
            if self.recovery.is_some(){
                return Err(ApiError::new("recovery_capacity","recovery","已有一条暂存录音。请先恢复或放弃它，再开始下一条；当前两条内容均已保留。"));
            }
            self.recovery=self.active.take();
        }
        Ok(())
    }

    pub(crate) fn discard_retained_recovery(&mut self) {
        if let Some(mut session)=self.recovery.take(){
            session.revoke_for_deletion();session.discard_deleted_content();
            if let Some(guard)=self.guards.get(&session.id){guard.cancel();}
        }
    }
}

impl AppHub {
    pub(crate) fn activate_recovery(&self,p:&Value)->Result<Value> {
        let id=text(p,"sessionId")?;let expected=number(p,"expectedGeneration")?;
        let result={
            let _native=self.native_io.try_lock().map_err(|_|ApiError::new("native_busy","recovery","上一条复制或输入操作尚未结束"))?;
            let mut inner=self.inner.lock().unwrap();inner.expire_audio_caches();
            let retained=inner.recovery.as_ref().ok_or_else(||ApiError::new("recovery_expired","recovery","这条录音已恢复、放弃或到期"))?;
            if retained.id!=id||retained.generation!=expected{return Err(ApiError::new("stale_recovery","recovery","暂存录音已变化，请刷新后再打开"));}
            if !retained.view().can_retry_asr{return Err(ApiError::new("asr_retry_unavailable","recovery","完整音频缓存不可用，请重新口述"));}
            if self.quitting.load(Ordering::SeqCst)||self.db.owner_released()||!self.capture_available.load(Ordering::SeqCst){return Err(ApiError::new("system_unavailable","recovery","系统尚未恢复，请稍候"));}
            if self.db.outstanding()>0||self.db_command_workers.load(Ordering::SeqCst)>0{return Err(ApiError::new("storage_busy","recovery","本机处理尚未结束，请稍候"));}
            if inner.settings_pending.is_some(){return Err(ApiError::new("settings_busy","recovery","设置正在保存，请稍候"));}
            if inner.shortcut_capture.is_some()||!platform::audio::capture_resources_released(){return Err(ApiError::new("capture_busy","recovery","录音或快捷键录制尚未释放，请稍候"));}
            if !inner.auxiliary_workers.is_empty(){return Err(ApiError::new("processing_busy","recovery","文字处理任务尚未释放，请稍候"));}
            if inner.active.as_ref().is_some_and(ActiveSession::busy){return Err(ApiError::new("session_busy","recovery","请等待当前口述处理结束后再打开恢复"));}
            if inner.active.as_ref().is_some_and(|s|s.pending.is_some()||
                (matches!(s.phase,SessionPhase::Failed|SessionPhase::Incomplete)&&
                    (s.source.is_some()||(!s.captions.is_empty()&&!s.view().can_retry_asr)))){
                return Err(ApiError::new("recovery_conflict","recovery","当前口述仍有待恢复内容，请先保存、复制或明确放弃"));
            }
            let generation=inner.active.as_ref().map(|s|s.generation).unwrap_or(0).max(retained.generation)+1;
            // No state moves until every identity, deadline and admission check
            // has succeeded. A second failed ASR swaps into the one spare slot.
            let mut restored=inner.recovery.take().unwrap();
            if inner.active.as_ref().is_some_and(|s|s.view().can_retry_asr){inner.recovery=inner.active.take();}
            restored.generation=generation;restored.cancel=CancellationToken::new();
            restored.reason=CreationReason::Retry;restored.original_delivery_revoked=true;
            restored.target=None;restored.target_capture_failure_reason=None;restored.delivery=None;
            restored.delivery_permit=None;restored.delivery_failure_reason=None;
            restored.diagnostic_trace=None;restored.caption_render_token=None;
            if let Some(old)=inner.guards.get(id){old.cancel();}
            inner.guards.insert(id.into(),Arc::new(CommitGuard::new(id.into(),generation)));
            let result=wire(restored.view())?;inner.active=Some(restored);result
        };
        self.publish_session();self.publish("recovery.changed",Value::Null);Ok(result)
    }

    pub(crate) fn discard_recovery(&self,p:&Value)->Result<Value> {
        let id=text(p,"sessionId")?;let expected=number(p,"expectedGeneration")?;
        {
            let mut inner=self.inner.lock().unwrap();inner.expire_audio_caches();
            let retained=inner.recovery.as_ref().ok_or_else(||ApiError::new("recovery_expired","recovery","这条录音已恢复、放弃或到期"))?;
            if retained.id!=id||retained.generation!=expected{return Err(ApiError::new("stale_recovery","recovery","暂存录音已变化，未放弃任何内容"));}
            inner.discard_retained_recovery();
        }
        self.publish("recovery.changed",Value::Null);Ok(json!({"discarded":true}))
    }
}
