//! A non-destructive reservation before the desktop tears resources down for an
//! installer. Refusing this gate never cancels work or expires recovery content.
use super::*;
use std::sync::atomic::Ordering;

pub(super) struct ContentOperationLease<'a>(&'a AppHub);
impl Drop for ContentOperationLease<'_> {
    fn drop(&mut self){self.0.db_command_workers.fetch_sub(1,Ordering::SeqCst);}
}
pub(super) struct UpdateReadLease<'a>(&'a AppHub);
impl Drop for UpdateReadLease<'_> {
    fn drop(&mut self){let mut inner=self.0.inner.lock().unwrap();inner.update_readers-=1;}
}

impl AppHub {
    pub(crate) fn begin_update_shutdown(&self)->Result<()> {
        self.begin_update_shutdown_with(platform::audio::capture_resources_released)
    }

    fn begin_update_shutdown_with(&self,capture_released:impl FnOnce()->bool)->Result<()> {
        let busy=||ApiError::new("update_busy","update","请先结束并处理当前口述、恢复或编辑任务，再安装更新");
        // Match native/start admission lock order. Never wait on a native owner
        // while holding the control lock, or mutate anything on a rejected gate.
        let _native=self.native_io.try_lock().map_err(|_|busy())?;
        let _settings=self.settings_io.try_lock().map_err(|_|busy())?;
        let _shortcut=self.shortcut_io.try_lock().map_err(|_|busy())?;
        let mut inner=self.inner.lock().unwrap();
        if inner.update_shutdown_started||self.quitting.load(Ordering::SeqCst)
            ||!self.capture_available.load(Ordering::SeqCst)||self.db.owner_released() {
            return Err(ApiError::new("update_unavailable","update","应用正在退出或恢复，请稍后重新打开 SpeakMode 安装更新"));
        }
        if self.db.outstanding()!=0||self.db_command_workers.load(Ordering::SeqCst)!=0||inner.update_readers!=0
            ||self.db.cache_error().is_some()||!capture_released()
            ||inner.requests.values().any(|request|request.outcome.lock().unwrap().is_none())
            ||inner.settings_pending.is_some()||inner.shortcut_capture.is_some()||inner.capture_commit_guard.is_some()
            ||!inner.auxiliary_workers.is_empty()||!inner.preview_sources.is_empty()||!inner.personalization_workers.is_empty()
            ||inner.preview.is_some()||inner.candidate.is_some()
            ||!inner.recoveries.is_empty()||inner.active.as_ref().is_some_and(session::ActiveSession::blocks_update_shutdown) {
            return Err(busy());
        }
        // History Ready survives a restart. A MemoryOnly Ready has no independent
        // durable copy, including when it is no longer the active session.
        if self.db.snapshot().temporary_result.is_some() {
            return Err(ApiError::new("update_temporary_result","update","请先复制或处理临时成稿，再重新打开 SpeakMode 安装更新"));
        }
        inner.update_shutdown_started=true;
        self.quitting.store(true,Ordering::SeqCst);
        self.capture_available.store(false,Ordering::SeqCst);
        self.lifecycle_epoch.fetch_add(1,Ordering::SeqCst);
        Ok(())
    }

    pub(super) fn update_command_available(inner:&Inner,name:&str)->Result<()> {
        if inner.update_shutdown_started&&!matches!(name,"state.snapshot"|"diagnostics.get"|"app.showMain"|"app.quit") {
            return Err(ApiError::new("update_in_progress","update","正在准备安装更新，请等待安装完成后重新打开 SpeakMode"));
        }
        Ok(())
    }

    /// Public source adoption also has native/maintenance callers that do not
    /// pass through command(). Cover the read-to-write interval for those callers.
    pub(super) fn reserve_content_operation(&self)->Result<ContentOperationLease<'_>> {
        let inner=self.inner.lock().unwrap();Self::update_command_available(&inner,"result.useSource")?;
        self.auxiliary_available()?;
        self.db_command_workers.fetch_add(1,Ordering::SeqCst);
        Ok(ContentOperationLease(self))
    }

    pub(super) fn reserve_update_read(&self,name:&str)->Result<Option<UpdateReadLease<'_>>> {
        let mut inner=self.inner.lock().unwrap();Self::update_command_available(&inner,name)?;
        if matches!(name,"state.snapshot"|"diagnostics.get"){return Ok(None);}
        inner.update_readers+=1;Ok(Some(UpdateReadLease(self)))
    }

    #[cfg(test)]
    pub(super) fn begin_update_shutdown_with_capture(&self,released:bool)->Result<()> {
        self.begin_update_shutdown_with(||released)
    }
}
