//! All stages share one absolute deadline. At most four calls, one repair.
use super::{minimum,local_repair,semantic_review,CloudError,RewriteResult};
use crate::core::{DictionarySnapshot,ModeSnapshot,ProviderConfig,SourceRevision};
use serde::Serialize;
use std::time::Duration;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use super::execution::{RequestStage, RequestOutcome, RewriteExecution};

const REPAIR_RESERVE_MS:u64=1400;
#[derive(Default,Serialize)]#[serde(rename_all="camelCase")]
pub struct Trace {
    pub calls:usize,pub elapsed_ms:u64,pub draft:Option<RewriteResult>,
    pub review:Option<semantic_review::SemanticReview>,pub repair_attempted:bool,
    pub repaired_draft:Option<RewriteResult>,pub final_review:Option<semantic_review::SemanticReview>,
    pub native_deletion_verified:bool,
    pub requests:Vec<RequestTiming>,
}
#[derive(Serialize)]#[serde(rename_all="camelCase")]
pub struct RequestTiming {pub stage:&'static str,pub duration_ms:f64,pub remaining_budget_ms:u64,pub outcome:&'static str}
pub struct Outcome {pub result:Result<RewriteResult,CloudError>,pub trace:Trace}
fn cancelled()->CloudError{CloudError::new("cancelled","rewrite","已取消整理")}
fn timeout()->CloudError{CloudError::new("rewrite_timeout","rewrite","整理等待已结束，识别稿仍保留")}
fn rejected()->CloudError{CloudError::new("rewrite_semantic_rejected","rewrite","整理复核未通过，识别稿仍保留")}
fn checkpoint(cancel:&CancellationToken,deadline:Instant)->Result<(),CloudError>{
    if cancel.is_cancelled(){Err(cancelled())}else if Instant::now()>=deadline{Err(timeout())}else{Ok(())}
}
fn safe_error(error:CloudError)->CloudError{
    if error.stage=="semantic_review" {CloudError::new("invalid_output","rewrite","整理核对响应无效，识别稿仍保留")}
    else if error.code=="rewrite_meaning_risk" {CloudError::new("rewrite_admission_rejected","rewrite","整理稿未通过复核前检查，识别稿仍保留")}
    else {error}
}
async fn execute(client:&reqwest::Client,config:&ProviderConfig,secret:&str,mode:&ModeSnapshot,source:&SourceRevision,
    dictionary:&DictionarySnapshot,cancel:CancellationToken,endpoint:&str,wait:Duration)->Outcome {
    execute_from(client,config,secret,mode,source,dictionary,cancel,endpoint,wait,None).await
}
async fn execute_from(client:&reqwest::Client,config:&ProviderConfig,secret:&str,mode:&ModeSnapshot,source:&SourceRevision,
    dictionary:&DictionarySnapshot,cancel:CancellationToken,endpoint:&str,wait:Duration,fixed:Option<RewriteResult>)->Outcome {
    execute_until(client,config,secret,mode,source,dictionary,cancel,endpoint,RewriteExecution::for_duration(wait),fixed).await
}
async fn execute_until(client:&reqwest::Client,config:&ProviderConfig,secret:&str,mode:&ModeSnapshot,source:&SourceRevision,
    dictionary:&DictionarySnapshot,cancel:CancellationToken,endpoint:&str,execution:RewriteExecution,fixed:Option<RewriteResult>)->Outcome {
    let context=super::writing_context::for_mode(mode);
    execute_until_with_writing_context(client,config,secret,mode,source,dictionary,cancel,endpoint,execution,fixed,context.as_ref()).await
}
pub(super) async fn execute_until_with_writing_context(client:&reqwest::Client,config:&ProviderConfig,secret:&str,mode:&ModeSnapshot,source:&SourceRevision,
    dictionary:&DictionarySnapshot,cancel:CancellationToken,endpoint:&str,execution:RewriteExecution,fixed:Option<RewriteResult>,
    context:Option<&crate::core::writing_context::WritingContext>)->Outcome {
    execute_until_with_review_format(client,config,secret,mode,source,dictionary,cancel,endpoint,execution,fixed,context,None).await
}
async fn execute_until_with_review_format(client:&reqwest::Client,config:&ProviderConfig,secret:&str,mode:&ModeSnapshot,source:&SourceRevision,
    dictionary:&DictionarySnapshot,cancel:CancellationToken,endpoint:&str,execution:RewriteExecution,fixed:Option<RewriteResult>,
    context:Option<&crate::core::writing_context::WritingContext>,review_format:Option<semantic_review::MinimumReviewFormat>)->Outcome {
    let started=Instant::now();let deadline=execution.deadline;let mut trace=Trace::default();
    let job=async {
        checkpoint(&cancel,deadline)?;
        if let Some(context)=context {context.validate().map_err(|_|CloudError::new("invalid_writing_context","rewrite","输入位置上下文无效"))?;}
        let mut draft=if let Some(draft)=fixed {draft}else{
            let request=minimum::prepare_with_writing_context(config,mode,source,dictionary,true,context)?;
            checkpoint(&cancel,deadline)?;trace.calls+=1;
            let content=super::fluent_pipeline::request_observed(client,config,secret,endpoint,&request.body,&cancel,&execution,RequestStage::Generation).await?;
            minimum::parse(&content,mode,source,&request)?
        };
        super::normalize_draft_dictionary(source,dictionary,&mut draft);
        super::format_draft_spoken(&mut draft,super::spoken_english(source,context));
        trace.draft=Some(draft.clone());
        super::review_admission(source,&draft).map_err(safe_error)?;
        if context.is_some(){super::writing_context::validate_tense(source,&draft)?;}
        checkpoint(&cancel,deadline)?;
        let request=semantic_review::prepare_minimum_with_writing_context_and_format(config,mode,source,&draft,context,
            review_format.unwrap_or_else(||semantic_review::minimum_format(config,source))).map_err(safe_error)?;
        checkpoint(&cancel,deadline)?;trace.calls+=1;
        let content=super::fluent_pipeline::request_observed(client,config,secret,endpoint,&request.body,&cancel,&execution,RequestStage::Review).await?;
        let mut review=semantic_review::parse_minimum(&content,&request,mode,source,&draft).map_err(safe_error)?;
        if context.is_some(){review.protocol_version=super::writing_context::VERSION;}
        trace.review=Some(review.clone());checkpoint(&cancel,deadline)?;
        if review.accepted() && local_repair::residue_hint(source,&draft).is_none(){return Ok(draft);}
        if deadline.saturating_duration_since(Instant::now())<Duration::from_millis(REPAIR_RESERVE_MS){return Err(rejected());}
        let Some(mut request)=local_repair::prepare(config,mode,source,&draft,&review)? else{return Err(rejected());};
        if let Some(context)=context {super::writing_context::attach(&mut request.body,mode,source,context,super::writing_context::Stage::Repair)?;}
        request.bind_personalization(context);
        checkpoint(&cancel,deadline)?;trace.calls+=1;trace.repair_attempted=true;
        let content=super::fluent_pipeline::request_observed(client,config,secret,endpoint,&request.body,&cancel,&execution,RequestStage::Repair).await?;
        let mut repaired=local_repair::apply(&content,&request,mode,source,&draft).map_err(safe_error)?;
        super::format_draft_spoken(&mut repaired,super::spoken_english(source,context));
        super::review_admission(source,&repaired).map_err(safe_error)?;
        if context.is_some(){super::writing_context::validate_tense(source,&repaired)?;}
        trace.repaired_draft=Some(repaired.clone());checkpoint(&cancel,deadline)?;
        if repaired.blocks[0].text==draft.blocks[0].text{return Err(rejected());}
        if context.is_none() && local_repair::native_deletion_verified(source,&draft,&repaired,&review){
            trace.native_deletion_verified=true;checkpoint(&cancel,deadline)?;return Ok(repaired);
        }
        // No second repair and no reuse of the pre-repair verdict.
        let request=semantic_review::prepare_minimum_with_writing_context_and_format(config,mode,source,&repaired,context,
            review_format.unwrap_or_else(||semantic_review::minimum_format(config,source))).map_err(safe_error)?;
        checkpoint(&cancel,deadline)?;trace.calls+=1;
        let content=super::fluent_pipeline::request_observed(client,config,secret,endpoint,&request.body,&cancel,&execution,RequestStage::FinalReview).await?;
        let mut review=semantic_review::parse_minimum(&content,&request,mode,source,&repaired).map_err(safe_error)?;
        if context.is_some(){review.protocol_version=super::writing_context::VERSION;}
        trace.final_review=Some(review.clone());checkpoint(&cancel,deadline)?;
        if !review.accepted() || local_repair::residue_hint(source,&repaired).is_some(){return Err(rejected());}
        Ok(repaired)
    };
    let result=tokio::select!{biased;_=cancel.cancelled()=>Err(cancelled()),result=tokio::time::timeout_at(deadline,job)=>result.unwrap_or_else(|_|Err(timeout()))};
    let result=checkpoint(&cancel,deadline).and(result);
    trace.elapsed_ms=started.elapsed().as_millis() as u64;
    Outcome{result,trace}
}
pub(super) async fn run_until(client:&reqwest::Client,config:&ProviderConfig,secret:&str,mode:&ModeSnapshot,source:&SourceRevision,
    dictionary:&DictionarySnapshot,cancel:CancellationToken,endpoint:&str,execution:RewriteExecution)->Result<RewriteResult,CloudError>{
    execute_until(client,config,secret,mode,source,dictionary,cancel,endpoint,execution,None).await.result
}
pub(crate) async fn run_until_with_writing_context(client:&reqwest::Client,config:&ProviderConfig,secret:&str,mode:&ModeSnapshot,source:&SourceRevision,
    dictionary:&DictionarySnapshot,cancel:CancellationToken,endpoint:&str,execution:RewriteExecution,
    context:&crate::core::writing_context::WritingContext)->Result<RewriteResult,CloudError>{
    execute_until_with_writing_context(client,config,secret,mode,source,dictionary,cancel,endpoint,execution,None,Some(context)).await.result
}
/// Explicit bounded text evaluation. Caller owns the request budget/ledger;
/// this shares the production context path and never persists or delivers.
pub async fn experiment_with_writing_context(config:&ProviderConfig,secret:&str,mode:&ModeSnapshot,source:&SourceRevision,
    context:&crate::core::writing_context::WritingContext,wait:Duration)->Result<Outcome,CloudError>{
    let endpoint=super::rewrite_endpoint(config)?;
    Ok(execute_until_with_writing_context(super::shared_client()?,config,secret,mode,source,&DictionarySnapshot::default(),
        CancellationToken::new(),&endpoint,RewriteExecution::for_duration(wait),None,Some(context)).await)
}
/// Same generated draft, review, repair and absolute deadline as production.
/// Only this explicit lab entry records per-request timings and overrides the
/// reviewer wire format. It never reads history, persists, or inputs text.
pub async fn experiment_with_review_format(config:&ProviderConfig,secret:&str,mode:&ModeSnapshot,source:&SourceRevision,
    format:semantic_review::MinimumReviewFormat,wait:Duration)->Result<Outcome,CloudError>{
    use std::sync::{Arc,Mutex};
    let timings=Arc::new(Mutex::new(Vec::new()));let observed=Arc::clone(&timings);
    let execution=RewriteExecution::observed(Instant::now()+wait,move|event|{
        if let (Some(end),Some(outcome))=(event.finished_at,event.outcome){
            let mut timings=observed.lock().unwrap();if timings.len()>=4{return;}
            timings.push(RequestTiming{
                stage:match event.stage{RequestStage::Generation=>"generation",RequestStage::Review=>"review",RequestStage::Repair=>"repair",RequestStage::FinalReview=>"finalReview"},
                duration_ms:end.duration_since(event.started_at).as_secs_f64()*1000.,remaining_budget_ms:event.remaining_budget_ms,
                outcome:match outcome{RequestOutcome::ResponseReceived=>"responseReceived",RequestOutcome::Failed=>"failed",RequestOutcome::Cancelled=>"cancelled",RequestOutcome::TimedOut=>"timedOut",RequestOutcome::Interrupted=>"interrupted"},
            });
        }
    });
    let endpoint=super::rewrite_endpoint(config)?;let context=super::writing_context::for_mode(mode);
    let mut outcome=execute_until_with_review_format(super::shared_client()?,config,secret,mode,source,&DictionarySnapshot::default(),
        CancellationToken::new(),&endpoint,execution,None,context.as_ref(),Some(format)).await;
    outcome.trace.requests=std::mem::take(&mut *timings.lock().unwrap());Ok(outcome)
}
/// Explicit diagnostic entry point; it does not read credentials, persist or input.
pub async fn experiment(config:&ProviderConfig,secret:&str,mode:&ModeSnapshot,source:&SourceRevision,wait:Duration)->Result<Outcome,CloudError>{
    let endpoint=super::rewrite_endpoint(config)?;
    Ok(execute(super::shared_client()?,config,secret,mode,source,&DictionarySnapshot::default(),CancellationToken::new(),&endpoint,wait).await)
}
/// Fixed admissible rough draft for explicitly invoked repair diagnostics only.
/// Production never bypasses generation through this entry point.
pub async fn experiment_fixed(config:&ProviderConfig,secret:&str,mode:&ModeSnapshot,source:&SourceRevision,content:&str,wait:Duration)->Result<Outcome,CloudError>{
    let endpoint=super::rewrite_endpoint(config)?;
    let request=minimum::prepare(config,mode,source,&DictionarySnapshot::default(),false)?;
    let draft=minimum::parse(content,mode,source,&request)?;
    Ok(execute_from(super::shared_client()?,config,secret,mode,source,&DictionarySnapshot::default(),CancellationToken::new(),&endpoint,wait,Some(draft)).await)
}
/// Explicit one-request diagnosis of a frozen draft. Never selected by normal
/// dictation and never returns a replacement or a delivery permission.
pub async fn explain_review(config:&ProviderConfig,secret:&str,mode:&ModeSnapshot,source:&SourceRevision,content:&str)->Result<serde_json::Value,CloudError>{
    let endpoint=super::rewrite_endpoint(config)?;
    let request=minimum::prepare(config,mode,source,&DictionarySnapshot::default(),false)?;
    let draft=minimum::parse(content,mode,source,&request)?;
    let mut request=semantic_review::prepare_minimum(config,mode,source,&draft)?;
    // Diagnostics include bounded failure evidence; this larger response cap is
    // never selected by the ordinary five-check production reviewer.
    request.body["max_tokens"]=serde_json::json!(2048);
    let system=request.body["messages"][0]["content"].as_str().expect("typed system message");
    request.body["messages"][0]["content"]=serde_json::json!(format!("{system}\n仅此离线诊断：若拒绝，在 issues 中给出最主要的一处失败依据，包括 facts 失败。sourceQuote 和 draftQuote 必须是各自文本里精确连续的最多80字片段，reason最多100字明确指出哪里变化；不要因要求解释而改变判断，不猜事实。仍返回同样的严格JSON结构。"));
    let content=super::fluent_pipeline::request_content(super::shared_client()?,config,secret,&endpoint,&request.body,&CancellationToken::new(),Instant::now()+Duration::from_secs(20)).await?;
    let parsed=semantic_review::parse_minimum(&content,&request,mode,source,&draft);
    // Explicit lab output may contain private source quotes. Keep it outside
    // ordinary diagnostics/history, including when shape validation fails.
    Ok(serde_json::json!({"rawModelContent":content,"parsedReview":parsed.as_ref().ok(),
        "validationError":parsed.as_ref().err(),"deliveryAuthorized":false}))
}
/// Frozen actual 0.2.18 path, never influenced by a future promotion flag.
pub async fn frozen_018(config:&ProviderConfig,secret:&str,mode:&ModeSnapshot,source:&SourceRevision,wait:Duration)->Result<RewriteResult,CloudError>{
    let endpoint=super::rewrite_endpoint(config)?;let cancel=CancellationToken::new();let client=super::shared_client()?;
    if super::uses_fluent_contract(config,mode){
        super::fluent_pipeline::run(client,config,secret,mode,source,&DictionarySnapshot::default(),cancel,&endpoint,wait).await
    }else{
        let deadline=Instant::now()+wait;
        let job=async{
            let body=super::baseline_request_body(config,mode,source)?;
            let content=super::fluent_pipeline::request_content(client,config,secret,&endpoint,&body,&cancel,deadline).await?;
            let draft=super::parse_model_output(&content,mode,source)?;
            checkpoint(&cancel,deadline)?;Ok(draft)
        };
        tokio::time::timeout_at(deadline,job).await.unwrap_or_else(|_|Err(timeout()))
    }
}

#[cfg(test)]#[path="rewrite_minimum_tests.rs"]pub(crate) mod tests;
