//! One caller-owned deadline and bounded, content-free request observations.
use super::CloudError;
use serde::Serialize;
use std::{sync::Arc, time::{Duration, Instant as WallInstant}};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all="camelCase")]
pub(crate) enum RequestStage { Generation, Review, Repair, FinalReview }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all="camelCase")]
pub(crate) enum RequestOutcome { ResponseReceived, Failed, Cancelled, TimedOut, Interrupted }

#[derive(Clone)]
pub(crate) struct RequestEvent {
    pub stage: RequestStage, pub started_at: WallInstant,
    pub finished_at: Option<WallInstant>, pub remaining_budget_ms: u64,
    pub outcome: Option<RequestOutcome>,
}

#[derive(Clone)]
pub(crate) struct RewriteExecution {
    pub deadline: Instant,
    observer: Option<Arc<dyn Fn(RequestEvent) + Send + Sync>>,
}
impl RewriteExecution {
    pub fn until(deadline: Instant) -> Self { Self { deadline, observer: None } }
    pub fn for_duration(wait: Duration) -> Self { Self::until(Instant::now() + wait) }
    pub fn observed(deadline: Instant, observer: impl Fn(RequestEvent) + Send + Sync + 'static) -> Self {
        Self { deadline, observer: Some(Arc::new(observer)) }
    }
    pub fn checkpoint(&self, cancel: &CancellationToken) -> Result<(), CloudError> {
        if cancel.is_cancelled() { Err(CloudError::new("cancelled", "rewrite", "已取消整理")) }
        else if Instant::now() >= self.deadline { Err(CloudError::new("rewrite_timeout", "rewrite", "整理等待已结束，识别稿仍保留")) }
        else { Ok(()) }
    }
    pub fn start(&self, stage: RequestStage, cancel: &CancellationToken) -> RequestGuard {
        let event = RequestEvent { stage, started_at: WallInstant::now(), finished_at: None,
            remaining_budget_ms: self.deadline.saturating_duration_since(Instant::now()).as_millis().min(u64::MAX as u128) as u64,
            outcome: None };
        if let Some(observer) = &self.observer { observer(event.clone()); }
        RequestGuard { execution: self.clone(), cancel: cancel.clone(), event }
    }
}

// Dropping a request at the outer timeout/cancellation still closes its timing.
// A received response is a transport result, never a semantic acceptance verdict.
pub(crate) struct RequestGuard {
    execution: RewriteExecution, cancel: CancellationToken, event: RequestEvent,
}
impl RequestGuard {
    pub fn finish<T>(&mut self, result: &Result<T, CloudError>) {
        self.event.outcome = Some(match result {
            Ok(_) => RequestOutcome::ResponseReceived,
            Err(error) if error.code == "cancelled" => RequestOutcome::Cancelled,
            Err(error) if error.code == "rewrite_timeout" => RequestOutcome::TimedOut,
            Err(_) => RequestOutcome::Failed,
        });
    }
}
impl Drop for RequestGuard {
    fn drop(&mut self) {
        self.event.finished_at = Some(WallInstant::now());
        if self.event.outcome.is_none() {
            self.event.outcome = Some(if self.cancel.is_cancelled() { RequestOutcome::Cancelled }
                else if Instant::now() >= self.execution.deadline { RequestOutcome::TimedOut }
                else { RequestOutcome::Interrupted });
        }
        if let Some(observer) = &self.execution.observer { observer(self.event.clone()); }
    }
}
