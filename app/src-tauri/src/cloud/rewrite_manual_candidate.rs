//! Human-reviewed history candidates. This entry point never authorizes paste
//! or persistence. Shape/source checks remain mandatory; fidelity findings are
//! returned with the draft so the user can see and correct the actual changes.
use super::*;
use tokio::time::Instant;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all="camelCase")]
pub enum ManualReviewIssue { ValuesChanged, MeaningChanged, LiteralsChanged, ConfirmedTextChanged, TenseChanged, ReviewRejected, ReviewUnavailable }

#[derive(Serialize)]
#[serde(rename_all="camelCase")]
pub struct ManualCandidate {
    pub draft: RewriteResult,
    pub review_issues: Vec<ManualReviewIssue>,
}

fn decode(config:&ProviderConfig, mode:&ModeSnapshot, source:&SourceRevision,
    dictionary:&DictionarySnapshot, content:&str)->Result<ManualCandidate,CloudError> {
    let draft = if uses_fluent_contract(config,mode) {
        parse_fluent_draft(content,mode,source)?
    } else { parse_model_draft(content,mode,source)? };
    annotate(source,dictionary,draft)
}
fn annotate(source:&SourceRevision,dictionary:&DictionarySnapshot,mut draft:RewriteResult)->Result<ManualCandidate,CloudError> {
    normalize_draft_dictionary(source,dictionary,&mut draft);
    format_draft_spoken(&mut draft,spoken_english(source,None));
    let mut review_issues = Vec::new();
    if literals::validate(&source.text(),&draft.blocks.iter().map(|b|b.text.as_str()).collect::<Vec<_>>().join("\n")).is_err() {
        review_issues.push(ManualReviewIssue::LiteralsChanged);
    }
    if spoken_format::validate(source,&draft.blocks).is_err() {review_issues.push(ManualReviewIssue::MeaningChanged);}
    if let Err(error)=integrity::validate_meaning(source,&draft.blocks) {
        review_issues.push(if error.code=="rewrite_meaning_risk" && error.message.contains("数值") {
            ManualReviewIssue::ValuesChanged
        } else { ManualReviewIssue::MeaningChanged });
    }
    Ok(ManualCandidate{draft,review_issues})
}

pub async fn rewrite_manual_candidate(config:&ProviderConfig, secret:&str,
    mode:&ModeSnapshot, source:&SourceRevision, dictionary:&DictionarySnapshot,
    cancel:CancellationToken)->Result<ManualCandidate,CloudError> {
    let endpoint=rewrite_endpoint(config)?;
    run(shared_client()?,config,secret,mode,source,dictionary,cancel,&endpoint,Duration::from_secs(20)).await
}

/// Personalized history candidates use the same frozen guidance and independent
/// checks as live dictation. A rejected draft remains explicitly reviewable.
pub(crate) async fn rewrite_manual_candidate_with_writing_context(config:&ProviderConfig,secret:&str,
    mode:&ModeSnapshot,source:&SourceRevision,dictionary:&DictionarySnapshot,cancel:CancellationToken,
    context:Option<&crate::core::writing_context::WritingContext>)->Result<ManualCandidate,CloudError> {
    rewrite_manual_candidate_with_writing_context_execution(config,secret,mode,source,dictionary,cancel,context,
        RewriteExecution::for_duration(Duration::from_secs(20))).await
}

pub(crate) async fn rewrite_manual_candidate_with_writing_context_execution(config:&ProviderConfig,secret:&str,
    mode:&ModeSnapshot,source:&SourceRevision,dictionary:&DictionarySnapshot,cancel:CancellationToken,
    context:Option<&crate::core::writing_context::WritingContext>,execution:RewriteExecution)->Result<ManualCandidate,CloudError> {
    execution.checkpoint(&cancel)?;
    let endpoint=rewrite_endpoint(config)?;
    run_until_with_writing_context(shared_client()?,config,secret,mode,source,dictionary,cancel,&endpoint,context,execution).await
}

pub(super) async fn run_until_with_writing_context(client:&reqwest::Client,config:&ProviderConfig,secret:&str,
    mode:&ModeSnapshot,source:&SourceRevision,dictionary:&DictionarySnapshot,cancel:CancellationToken,
    endpoint:&str,context:Option<&crate::core::writing_context::WritingContext>,execution:RewriteExecution)->Result<ManualCandidate,CloudError> {
    let Some(context)=context else{return run_until(client,config,secret,mode,source,dictionary,cancel,endpoint,execution).await;};
    let outcome=minimum_pipeline::execute_until_with_writing_context(client,config,secret,mode,source,dictionary,
        cancel,endpoint,execution,None,Some(context)).await;
    match outcome.result {
        Ok(draft)=>Ok(ManualCandidate{draft,review_issues:Vec::new()}),
        Err(error)=>{
            if matches!(error.code.as_str(),"cancelled"|"rewrite_timeout"|"authentication_failed"|"rate_limited") {return Err(error);}
            let draft=outcome.trace.repaired_draft.or(outcome.trace.draft).ok_or_else(||error.clone())?;
            let issue=if review_admission_with_writing_context(mode,source,&draft,Some(context)).is_err(){ManualReviewIssue::MeaningChanged}
                else if outcome.trace.final_review.or(outcome.trace.review).is_some(){ManualReviewIssue::ReviewRejected}
                else{ManualReviewIssue::ReviewUnavailable};
            Ok(ManualCandidate{draft,review_issues:vec![issue]})
        }
    }
}

pub(super) async fn run(client:&reqwest::Client, config:&ProviderConfig, secret:&str,
    mode:&ModeSnapshot, source:&SourceRevision, dictionary:&DictionarySnapshot,
    cancel:CancellationToken, endpoint:&str, timeout:Duration)->Result<ManualCandidate,CloudError> {
    run_until(client,config,secret,mode,source,dictionary,cancel,endpoint,RewriteExecution::for_duration(timeout)).await
}

pub(super) async fn run_until(client:&reqwest::Client, config:&ProviderConfig, secret:&str,
    mode:&ModeSnapshot, source:&SourceRevision, dictionary:&DictionarySnapshot,
    cancel:CancellationToken, endpoint:&str, execution:RewriteExecution)->Result<ManualCandidate,CloudError> {
    // A valid Email wrapper exception requires the independent reviewer. Keep
    // the legacy strict annotation guard intact for generation-only candidates.
    if mode.mode_id=="email" {
        let context=writing_context::for_mode(mode).expect("explicit email scene");
        return Box::pin(run_until_with_writing_context(client,config,secret,mode,source,dictionary,cancel,endpoint,Some(&context),execution)).await;
    }
    let deadline=execution.deadline;
    let cancelled=||CloudError::new("cancelled","rewrite","候选已取消");
    let timed_out=||CloudError::new("rewrite_timeout","rewrite","生成候选超时，当前文本保持不变");
    let job=async {
        // Match the existing editing task, but do not mask literals or run the
        // automatic accept/reject reviewer: this draft is for explicit review.
        let minimum_request=if uses_minimum_contract(config,mode){Some(minimum::prepare(config,mode,source,dictionary,false)?)}else{None};
        let body=if let Some(request)=&minimum_request{request.body.clone()}
        else if uses_fluent_contract(config,mode) {
            if uses_guarded_long(source) { long_request_body(config,mode,source,dictionary)? }
            else { fluent_request_body(config,mode,source,dictionary)? }
        } else { baseline_request_body(config,mode,source)? };
        let content=fluent_pipeline::request_observed(client,config,secret,endpoint,&body,&cancel,&execution,RequestStage::Generation).await?;
        if let Some(request)=&minimum_request{annotate(source,dictionary,minimum::parse(&content,mode,source,request)?)}
        else{decode(config,mode,source,dictionary,&content)}
    };
    let result=tokio::select! { biased;
        _=cancel.cancelled()=>Err(cancelled()),
        result=tokio::time::timeout_at(deadline,job)=>result.unwrap_or_else(|_|Err(timed_out())),
    };
    if cancel.is_cancelled() { Err(cancelled()) }
    else if Instant::now()>=deadline { Err(timed_out()) }
    else { result }
}

#[cfg(test)]
#[path="rewrite_manual_candidate_tests.rs"]
mod tests;
