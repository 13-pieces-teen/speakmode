//! One caller-owned deadline and bounded, content-free request observations.
use super::{CloudError,ManualCandidate,ManualReviewIssue,RewriteResult};
use serde::Serialize;
use std::{sync::{Arc,Mutex}, time::{Duration, Instant as WallInstant}};
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
    attempt_budget: Duration,
    completed: Arc<Mutex<CompletedRequests>>,
    observer: Option<Arc<dyn Fn(RequestEvent) + Send + Sync>>,
    dispatch_observer: Option<Arc<dyn Fn(DispatchEvent) + Send + Sync>>,
    reviewable_observer: Option<Arc<dyn Fn(ManualCandidate) + Send + Sync>>,
}
#[derive(Default)]
struct CompletedRequests {generation:Option<Duration>,review:Option<Duration>}
impl RewriteExecution {
    pub fn until(deadline: Instant) -> Self { Self { deadline,attempt_budget:deadline.saturating_duration_since(Instant::now()),completed:Arc::new(Mutex::new(CompletedRequests::default())),observer: None, dispatch_observer: None,reviewable_observer:None } }
    pub fn for_duration(wait: Duration) -> Self { Self::until(Instant::now() + wait) }
    /// A separate recovery attempt shares observers, but never inherits an expired
    /// clock. No intermediate draft is stored in the execution context.
    pub(crate) fn recovery(&self, deadline: Instant) -> Self {
        Self { deadline,attempt_budget:deadline.saturating_duration_since(Instant::now()),completed:Arc::new(Mutex::new(CompletedRequests::default())), ..self.clone() }
    }
    pub fn observed(deadline: Instant, observer: impl Fn(RequestEvent) + Send + Sync + 'static) -> Self {
        Self { observer: Some(Arc::new(observer)), ..Self::until(deadline) }
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
    /// Replace the manual-draft observer without changing request timing or
    /// dispatch tracking. Recovery may install its own attempt-scoped owner.
    pub(crate) fn with_reviewable_observer(mut self,observer:impl Fn(ManualCandidate)+Send+Sync+'static)->Self{
        self.reviewable_observer=Some(Arc::new(observer));self
    }
    /// Publish only locally admitted content for explicit human review, never
    /// automatic acceptance or input. A deadline may leave useful content;
    /// cancellation must not publish into a retired session owner.
    pub(super) fn publish_reviewable(&self,draft:&RewriteResult,cancel:&CancellationToken,issue:ManualReviewIssue){
        if cancel.is_cancelled(){return;}
        if let Some(observer)=&self.reviewable_observer{
            observer(ManualCandidate{draft:draft.clone(),review_issues:vec![issue]});
        }
    }
    /// Budget a repair plus fresh full review using this attempt's observed
    /// request latency. Cap each estimate so one setup outlier cannot consume
    /// the whole retry decision. This never extends the caller's deadline.
    pub(super) fn repair_review_reserve(&self)->Duration{
        let completed=self.completed.lock().unwrap_or_else(|poison|poison.into_inner());
        let generation=completed.generation.unwrap_or_default();
        let review=completed.review.unwrap_or(generation/2);
        let estimated=generation.min(self.attempt_budget/4)+review.min(self.attempt_budget/6)+Duration::from_millis(400);
        estimated.max(Duration::from_millis(1400))
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
        RequestGuard { execution: self.clone(), cancel: cancel.clone(), event,started:Instant::now() }
    }
}

// Dropping a request at the outer timeout/cancellation still closes its timing.
// A received response is a transport result, never a semantic acceptance verdict.
pub(crate) struct RequestGuard {
    execution: RewriteExecution, cancel: CancellationToken, event: RequestEvent,
    started:Instant,
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
        if self.event.outcome==Some(RequestOutcome::ResponseReceived){
            let mut completed=self.execution.completed.lock().unwrap_or_else(|poison|poison.into_inner());
            match self.event.stage{
                RequestStage::Generation=>completed.generation=Some(self.started.elapsed()),
                RequestStage::Review=>completed.review=Some(self.started.elapsed()),
                _=>{},
            }
        }
        if let Some(observer) = &self.execution.observer { observer(self.event.clone()); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused=true)]
    async fn repair_reserve_uses_bounded_completed_stage_latency_and_recovery_starts_fresh(){
        let execution=RewriteExecution::for_duration(Duration::from_secs(40));let cancel=CancellationToken::new();
        assert_eq!(execution.repair_review_reserve(),Duration::from_millis(1400));
        let mut generation=execution.start(RequestStage::Generation,&cancel);
        tokio::time::advance(Duration::from_secs(2)).await;
        generation.finish(&Ok::<(),CloudError>(()));drop(generation);
        let mut review=execution.start(RequestStage::Review,&cancel);
        tokio::time::advance(Duration::from_secs(1)).await;
        review.finish(&Ok::<(),CloudError>(()));drop(review);
        assert_eq!(execution.repair_review_reserve(),Duration::from_millis(3400));
        let recovered=execution.recovery(Instant::now()+Duration::from_secs(60));
        assert_eq!(recovered.repair_review_reserve(),Duration::from_millis(1400));
        assert_eq!(execution.deadline.saturating_duration_since(Instant::now()),Duration::from_secs(37));

        let mut outlier=execution.start(RequestStage::Generation,&cancel);
        tokio::time::advance(Duration::from_secs(20)).await;
        outlier.finish(&Ok::<(),CloudError>(()));drop(outlier);
        // Generation is capped at 40/4 even if one request took twenty seconds.
        assert_eq!(execution.repair_review_reserve(),Duration::from_millis(11400));
        assert!(execution.repair_review_reserve()<Duration::from_secs(20));
    }

    #[tokio::test]
    async fn recovery_observer_replacement_preserves_timing_dispatch_and_cancel_guard(){
        use std::sync::atomic::{AtomicUsize,Ordering};
        let first=Arc::new(AtomicUsize::new(0));let old=first.clone();
        let second=Arc::new(AtomicUsize::new(0));let new=second.clone();
        let timing=Arc::new(AtomicUsize::new(0));let observed=timing.clone();
        let dispatch=Arc::new(AtomicUsize::new(0));let dispatched=dispatch.clone();
        let execution=RewriteExecution::observed(Instant::now()+Duration::from_secs(4),move|_|{observed.fetch_add(1,Ordering::SeqCst);})
            .with_dispatch_observer(move|_|{dispatched.fetch_add(1,Ordering::SeqCst);})
            .with_reviewable_observer(move|_|{old.fetch_add(1,Ordering::SeqCst);});
        let recovery=execution.recovery(Instant::now()+Duration::from_secs(6))
            .with_reviewable_observer(move|_|{new.fetch_add(1,Ordering::SeqCst);});
        let cancel=CancellationToken::new();let draft=RewriteResult{resolved_mode:crate::core::BaseMode::Light,blocks:vec![]};
        execution.publish_reviewable(&draft,&cancel,ManualReviewIssue::ReviewUnavailable);
        recovery.publish_reviewable(&draft,&cancel,ManualReviewIssue::ReviewRejected);
        assert_eq!(first.load(Ordering::SeqCst),1);assert_eq!(second.load(Ordering::SeqCst),1);
        recovery.dispatch_started(RequestStage::Generation,false);
        let mut guard=recovery.start(RequestStage::Generation,&cancel);guard.finish(&Ok::<(),CloudError>(()));drop(guard);
        assert_eq!(timing.load(Ordering::SeqCst),2);assert_eq!(dispatch.load(Ordering::SeqCst),1);
        cancel.cancel();recovery.publish_reviewable(&draft,&cancel,ManualReviewIssue::ReviewUnavailable);
        assert_eq!(second.load(Ordering::SeqCst),1);
    }
}
