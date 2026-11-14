//! Finished failures are independent of capture admission. Moving entries never
//! extends their original 30-minute lease or resumes cloud/input work.
use super::*;

impl ActiveSession {
    fn needs_recovery(&self)->bool {
        matches!(self.phase,SessionPhase::Failed|SessionPhase::Incomplete)&&
            (self.pending.is_some()||self.source.is_some()||!self.captions.is_empty()||self.view().can_retry_asr)
    }
    fn park(&mut self) {
        self.expire_audio_cache();
        if self.cache.is_none()&&self.cache_created.is_some_and(|created|created.elapsed()>=AUDIO_CACHE_RETENTION){self.cache_created=None;}
        self.cache_created.get_or_insert_with(Instant::now);
        self.original_delivery_revoked=true;self.target=None;
        if let Some(permit)=self.delivery_permit.take(){permit.cancel_before_injection();}
        self.discard_input_context();
    }
    pub(crate) fn recovery_view(&self)->Option<Value> {
        let remaining=AUDIO_CACHE_RETENTION.checked_sub(self.cache_created?.elapsed())?;
        let expires=chrono::Utc::now()+chrono::Duration::from_std(remaining).ok()?;
        Some(json!({"session":self.view(),"expiresAt":expires.to_rfc3339()}))
    }
    pub(super) fn release_recovery_content(&mut self,guards:&HashMap<String,Arc<CommitGuard>>,db:&DbClient) {
        // Persistent checkpoints remain available from history. Private failed
        // checkpoints have no independent UI owner after this recovery disappears.
        if self.source.is_none()||!self.prefs.keep_history {
            if let Some(guard)=guards.get(&self.id){guard.cancel();}
        }
        if !self.prefs.keep_history{db.release_memory_checkpoint(&self.id,self.generation);}
        self.revoke_for_deletion();self.discard_deleted_content();
    }
}

impl Inner {
    pub(crate) fn expire_audio_caches(&mut self,db:&DbClient)->bool {
        let active=self.active.as_mut().is_some_and(ActiveSession::expire_audio_cache);
        let before=self.recoveries.len();
        self.recoveries.retain_mut(|s|{
            let keep=s.cache_created.is_some_and(|created|created.elapsed()<AUDIO_CACHE_RETENTION);
            if !keep{s.release_recovery_content(&self.guards,db);}
            keep
        });
        active||before!=self.recoveries.len()
    }

    pub(super) fn park_retryable_failure(&mut self,db:&DbClient) {
        self.expire_audio_caches(db);
        if self.active.as_ref().is_some_and(ActiveSession::needs_recovery){
            let mut session=self.active.take().unwrap();session.park();self.recoveries.push(session);
        }
    }

    pub(crate) fn discard_retained_recovery(&mut self,db:&DbClient) {
        for mut session in self.recoveries.drain(..){
            session.release_recovery_content(&self.guards,db);
        }
    }
}

impl AppHub {
    pub(super) fn retry_configuration(&self,p:&Value,session:&ActiveSession)->Result<ProviderConfig> {
        self.auxiliary_available()?;
        match p.get("retryConfiguration").map(Value::as_str) {
            None|Some(Some("original"))=>Ok(session.original_config.clone()),
            Some(Some("current"))=>{
                let current=self.db.snapshot().provider_config;
                if p.get("expectedProviderVersion").is_some_and(|version|version.as_u64()!=Some(current.version)) {
                    return Err(ApiError::new("revisionConflict","recovery","当前服务设置已变化，请刷新后重新选择重试配置"));
                }
                Ok(current)
            },
            _=>Err(ApiError::new("invalid_retry_configuration","recovery","请选择本次原配置或设置中的当前配置")),
        }
    }
    pub(crate) fn activate_recovery(self:&Arc<Self>,p:&Value)->Result<Value> {
        let id=text(p,"sessionId")?;let expected=number(p,"expectedGeneration")?;
        let result={
            let _native=self.native_io.try_lock().map_err(|_|ApiError::new("native_busy","recovery","上一条复制或输入操作尚未结束"))?;
            let mut inner=self.inner.lock().unwrap();inner.expire_audio_caches(&self.db);
            let index=inner.recoveries.iter().position(|s|s.id==id).ok_or_else(||ApiError::new("recovery_expired","recovery","这条口述已恢复、放弃或到期"))?;
            let retained=&inner.recoveries[index];
            if retained.generation!=expected{return Err(ApiError::new("stale_recovery","recovery","暂存口述已变化，请刷新后再打开"));}
            if self.quitting.load(Ordering::SeqCst)||self.db.owner_released()||!self.capture_available.load(Ordering::SeqCst){return Err(ApiError::new("system_unavailable","recovery","系统尚未恢复，请稍候"));}
            if self.db.outstanding()>0||self.db_command_workers.load(Ordering::SeqCst)>0{return Err(ApiError::new("storage_busy","recovery","本机处理尚未结束，请稍候"));}
            if inner.settings_pending.is_some(){return Err(ApiError::new("settings_busy","recovery","设置正在保存，请稍候"));}
            if inner.shortcut_capture.is_some()||!platform::audio::capture_resources_released(){return Err(ApiError::new("capture_busy","recovery","录音或快捷键录制尚未释放，请稍候"));}
            if !inner.auxiliary_workers.is_empty(){return Err(ApiError::new("processing_busy","recovery","文字处理任务尚未释放，请稍候"));}
            if inner.active.as_ref().is_some_and(ActiveSession::busy){return Err(ApiError::new("session_busy","recovery","请等待当前口述处理结束后再打开恢复"));}
            let generation=inner.active.as_ref().map(|s|s.generation).unwrap_or(0).max(retained.generation)+1;
            // No moves until all checks succeed. Preserve the current failure
            // too, including partial captions and unsaved checkpoints/drafts.
            let mut restored=inner.recoveries.remove(index);inner.park_retryable_failure(&self.db);
            // Source/checkpoint generations are immutable in the store. An
            // audio-only entry instead gets a fresh ASR attempt generation.
            if restored.source.is_none(){restored.generation=generation;}
            restored.cancel=CancellationToken::new();restored.park();
            restored.reason=CreationReason::Retry;restored.original_delivery_revoked=true;
            restored.target=None;restored.target_capture_failure_reason=None;restored.delivery=None;
            restored.delivery_permit=None;restored.delivery_failure_reason=None;
            restored.diagnostic_trace=None;restored.caption_render_token=None;
            if restored.source.is_none(){
                if let Some(old)=inner.guards.get(id){old.cancel();}
                inner.guards.insert(id.into(),Arc::new(CommitGuard::new(id.into(),restored.generation)));
            }
            let result=wire(restored.view())?;inner.active=Some(restored);result
        };
        self.schedule_retained_expiry();
        self.publish_session();self.publish("recovery.changed",Value::Null);Ok(result)
    }

    pub(super) fn schedule_retained_expiry(self:&Arc<Self>) {
        let entry=self.inner.lock().unwrap().recoveries.last().and_then(|s|s.cache_created.map(|created|(s.id.clone(),created)));
        if let Some((id,created))=entry{self.schedule_audio_cache_expiry(id,created);}
    }

    pub(crate) fn discard_recovery(&self,p:&Value)->Result<Value> {
        let id=text(p,"sessionId")?;let expected=number(p,"expectedGeneration")?;
        {
            let mut inner=self.inner.lock().unwrap();inner.expire_audio_caches(&self.db);
            let index=inner.recoveries.iter().position(|s|s.id==id).ok_or_else(||ApiError::new("recovery_expired","recovery","这条口述已恢复、放弃或到期"))?;
            if inner.recoveries[index].generation!=expected{return Err(ApiError::new("stale_recovery","recovery","暂存口述已变化，未放弃任何内容"));}
            let mut session=inner.recoveries.remove(index);
            session.release_recovery_content(&inner.guards,&self.db);
        }
        self.publish("recovery.changed",Value::Null);Ok(json!({"discarded":true}))
    }
}
