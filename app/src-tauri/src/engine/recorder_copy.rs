//! Current-session clipboard access for the restricted recording window.
use super::*;

impl AppHub {
    // Production passes only the native clipboard writer. Tests inject a writer
    // to exercise the same locks and authorization without touching a clipboard.
    pub(crate) fn copy_current_recorder_with(&self,name:&str,p:&Value,copy:impl FnOnce(&str)->Result<()>)->Result<Value> {
        let recovery=match name {"result.copy"=>false,"recoverable.copy"=>true,_=>return Err(ApiError::new("forbidden","ipc","录音窗口不能执行此复制操作"))};
        let object=p.as_object().ok_or_else(||ApiError::new("invalid_payload","ipc","复制请求无效"))?;
        if object.keys().any(|key|!matches!(key.as_str(),"sessionId"|"expectedGeneration"|"requestId")&&!(recovery&&key=="recoverableContentId")) {
            return Err(ApiError::new("forbidden","ipc","录音窗口只能复制当前会话内容"));
        }
        let id=text(p,"sessionId")?;let generation=number(p,"expectedGeneration")?;
        if recovery&&p.get("recoverableContentId").is_some_and(|value|value.as_str()!=Some(id)) {
            return Err(ApiError::new("stale_session","recovery","可复制内容已经变化").for_session(id,generation));
        }
        // Use the same lock order as native delivery. Validate AFTER acquiring
        // native IO so a delayed copy cannot outlive session replacement/cancel.
        // Holding the live-session lock through the bounded clipboard operation
        // makes this explicit user action atomic with a new recording/dismissal.
        let _native=self.native_io.lock().unwrap();let inner=self.inner.lock().unwrap();
        let s=inner.active.as_ref().filter(|s|s.id==id&&s.generation==generation)
            .ok_or_else(||ApiError::new("stale_session","result","当前口述已经变化").for_session(id,generation))?;
        let cache_expired=recovery&&s.error.as_ref().is_some_and(|error|error.code=="asr_retry_expired");
        if s.cancel_pending||(s.cancel.is_cancelled()&&!cache_expired) {
            return Err(ApiError::new("cancelled","result","本次口述已取消").for_session(id,generation));
        }
        if s.delivery.as_ref().is_some_and(|attempt|matches!(attempt.state,DeliveryState::Prepared|DeliveryState::Dispatched)) {
            return Err(ApiError::new("delivery_busy","result","正在发送，请等待发送结束后复制").for_session(id,generation));
        }
        let content=if recovery {
            if !matches!(s.phase,SessionPhase::Failed|SessionPhase::Incomplete)||s.error.as_ref().is_some_and(|error|error.code=="cancel_cleanup_failed") {
                return Err(ApiError::new("recovery_unavailable","recovery","当前没有可复制的恢复内容").for_session(id,generation));
            }
            s.source.as_ref().map(SourceRevision::text).unwrap_or_else(||s.captions.iter().map(|item|item.text.as_str()).collect::<Vec<_>>().join("\n"))
        } else {
            if s.phase!=SessionPhase::Ready||!s.view().worker_released {
                return Err(ApiError::new("result_not_ready","result","请等待本次成稿和发送流程结束").for_session(id,generation));
            }
            let output=s.output.as_ref().ok_or_else(||ApiError::new("no_output","result","当前没有可复制的成稿").for_session(id,generation))?;
            if output.session_id!=id||s.source.as_ref().is_none_or(|source|source.session_id!=id||source.generation!=generation||source.id!=output.source_revision_id) {
                return Err(ApiError::new("stale_session","result","当前成稿来源已经变化").for_session(id,generation));
            }
            output.text()
        };
        if content.trim().is_empty(){return Err(ApiError::new("empty_recovery","result","没有可复制的文字").for_session(id,generation));}
        copy(&content)?;
        Ok(if recovery{json!({"copied":true,"adopted":false})}else{json!({"copied":true})})
    }
}
