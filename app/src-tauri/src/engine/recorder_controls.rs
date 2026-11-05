use super::*;

impl AppHub {
    // Rechecked on the UI thread immediately before hiding the native window.
    // Dismissal never changes the session, delivery eligibility or recovery cache.
    pub fn validate_recorder_dismiss(&self, id: &str, generation: u64) -> Result<()> {
        let inner = self.inner.lock().unwrap();
        let view = inner.active.as_ref().filter(|s| s.id == id && s.generation == generation)
            .ok_or_else(|| ApiError::new("stale_session", "window", "当前口述已变化"))?.view();
        if !matches!(view.state, SessionPhase::Ready | SessionPhase::Failed | SessionPhase::Incomplete | SessionPhase::Cancelled)
            || matches!(view.delivery_state, Some(DeliveryState::Prepared | DeliveryState::Dispatched))
            || (view.state == SessionPhase::Ready && !view.worker_released) {
            return Err(ApiError::new("session_busy", "window", "口述仍在处理中").for_session(id, generation));
        }
        Ok(())
    }

    pub(super) fn request_current_result(&self, payload: &Value) -> Result<()> {
        let id = text(payload, "sessionId")?;
        let generation = number(payload, "expectedGeneration")?;
        let mut inner = self.inner.lock().unwrap();
        let view = inner.active.as_ref().filter(|s| s.id == id && s.generation == generation)
            .ok_or_else(|| ApiError::new("stale_session", "window", "当前口述已变化"))?.view();
        if !matches!(view.state, SessionPhase::Ready | SessionPhase::Failed | SessionPhase::Incomplete) {
            return Err(ApiError::new("session_busy", "window", "本次结果尚未就绪").for_session(id, generation));
        }
        // Retained until acknowledged: a hidden/reloading main webview must not
        // miss this intent. Acknowledging an older request cannot clear a new one.
        inner.main_result_request = Some(json!({"requestId":Uuid::new_v4().to_string(),"sessionId":id,
            "generation":generation,"target":if view.state == SessionPhase::Ready {"result"} else {"recovery"}}));
        Ok(())
    }
}
