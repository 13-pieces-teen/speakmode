//! One caller-owned deadline and bounded, content-free request observations.
use super::{CloudError, RewriteResult, semantic_review};
use crate::core::{ModeSnapshot, SourceRevision};
use serde::Serialize;
use std::{sync::{Arc, Mutex}, time::{Duration, Instant as WallInstant}};
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

/// The client has built a valid request and is entering its HTTP send future.
/// This is not evidence of provider receipt, billing, or semantic acceptance.
/// No request contents or profile identifiers are exposed to observers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DispatchEvent {
    pub stage: RequestStage,
    /// Only whether a top-level profile object exists in the adapted wire data.
    pub personalization: bool,
}

#[derive(Clone)]
pub(crate) struct RewriteExecution {
    pub deadline: Instant,
    observer: Option<Arc<dyn Fn(RequestEvent) + Send + Sync>>,
    dispatch_observer: Option<Arc<dyn Fn(DispatchEvent) + Send + Sync>>,
    reviewed: Arc<Mutex<Option<ReviewedDraft>>>,
}
struct ReviewedDraft { source_hash: String, mode_hash: String, draft: RewriteResult }
impl RewriteExecution {
    pub fn until(deadline: Instant) -> Self { Self { deadline, observer: None, dispatch_observer: None, reviewed: Arc::new(Mutex::new(None)) } }
    pub fn for_duration(wait: Duration) -> Self { Self::until(Instant::now() + wait) }
    /// A separate recovery attempt shares observers, but never inherits an expired
    /// clock or a draft reviewed under the original mode/context.
    pub(crate) fn recovery(&self, deadline: Instant) -> Self {
        Self { deadline, reviewed: Arc::new(Mutex::new(None)), ..self.clone() }
    }
    pub fn observed(deadline: Instant, observer: impl Fn(RequestEvent) + Send + Sync + 'static) -> Self {
        Self { observer: Some(Arc::new(observer)), ..Self::until(deadline) }
    }
    /// Keep the exact draft whose content checks passed before further polishing.
    pub(super) fn retain_reviewed(&self, source: &SourceRevision, mode: &ModeSnapshot,
        draft: &RewriteResult, review: &semantic_review::SemanticReview, cancel: &CancellationToken) {
        if self.checkpoint(cancel).is_err() || !review.fidelity_passed()
            || !semantic_review::matches_draft(review, source, draft) { return; }
        *self.reviewed.lock().unwrap() = Some(ReviewedDraft {
            source_hash: super::minimum::hash(source), mode_hash: super::minimum::hash(mode), draft: draft.clone(),
        });
    }
    pub(crate) fn take_reviewed(&self, source: &SourceRevision, mode: &ModeSnapshot) -> Option<RewriteResult> {
        self.reviewed.lock().unwrap().take()
            .filter(|saved| saved.source_hash == super::minimum::hash(source) && saved.mode_hash == super::minimum::hash(mode))
            .map(|saved| saved.draft)
    }
    /// Add task-level dispatch tracking without replacing existing timing or
    /// dispatch observers, and without changing the caller-owned deadline.
    pub fn with_dispatch_observer(mut self, observer: impl Fn(DispatchEvent) + Send + Sync + 'static) -> Self {
        let previous = self.dispatch_observer.take();
        self.dispatch_observer = Some(Arc::new(move |event| {
            if let Some(previous) = &previous { previous(event); }
            observer(event);
        }));
        self
    }
    pub(super) fn dispatch_started(&self, stage: RequestStage, personalization: bool) {
        if let Some(observer) = &self.dispatch_observer { observer(DispatchEvent { stage, personalization }); }
    }
    pub(super) fn has_dispatch_observer(&self) -> bool { self.dispatch_observer.is_some() }
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
