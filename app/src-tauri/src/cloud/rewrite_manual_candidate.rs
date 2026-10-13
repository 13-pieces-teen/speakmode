//! Human-reviewed history candidates. This entry point never authorizes paste
//! or persistence. Shape/source checks remain mandatory; fidelity findings are
//! returned with the draft so the user can see and correct the actual changes.
use super::*;
use tokio::time::Instant;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all="camelCase")]
pub enum ManualReviewIssue { ValuesChanged, MeaningChanged, LiteralsChanged }

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
    let mut review_issues = Vec::new();
    if literals::validate(&source.text(),&draft.blocks.iter().map(|b|b.text.as_str()).collect::<Vec<_>>().join("\n")).is_err() {
        review_issues.push(ManualReviewIssue::LiteralsChanged);
    }
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
    let endpoint=endpoints(config)?.rewrite;
    run(shared_client()?,config,secret,mode,source,dictionary,cancel,&endpoint,Duration::from_secs(20)).await
}

async fn run(client:&reqwest::Client, config:&ProviderConfig, secret:&str,
    mode:&ModeSnapshot, source:&SourceRevision, dictionary:&DictionarySnapshot,
    cancel:CancellationToken, endpoint:&str, timeout:Duration)->Result<ManualCandidate,CloudError> {
    let deadline=Instant::now()+timeout;
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
        let content=fluent_pipeline::request_content(client,config,secret,endpoint,&body,&cancel,deadline).await?;
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
