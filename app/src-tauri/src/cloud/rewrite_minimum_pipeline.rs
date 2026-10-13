//! All stages share one absolute deadline. At most four calls, one repair.
use super::{minimum,local_repair,semantic_review,CloudError,RewriteResult};
use crate::core::{DictionarySnapshot,ModeSnapshot,ProviderConfig,SourceRevision};
use serde::Serialize;
use std::time::Duration;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

const REPAIR_RESERVE_MS:u64=1400;
#[derive(Default,Serialize)]#[serde(rename_all="camelCase")]
pub struct Trace {
    pub calls:usize,pub elapsed_ms:u64,pub draft:Option<RewriteResult>,
    pub review:Option<semantic_review::SemanticReview>,pub repair_attempted:bool,
    pub repaired_draft:Option<RewriteResult>,pub final_review:Option<semantic_review::SemanticReview>,
    pub native_deletion_verified:bool,
}
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
    let started=Instant::now();let deadline=started+wait;let mut trace=Trace::default();
    let job=async {
        checkpoint(&cancel,deadline)?;
        let mut draft=if let Some(draft)=fixed {draft}else{
            let request=minimum::prepare(config,mode,source,dictionary,true)?;
            checkpoint(&cancel,deadline)?;trace.calls+=1;
            let content=super::fluent_pipeline::request_content(client,config,secret,endpoint,&request.body,&cancel,deadline).await?;
            minimum::parse(&content,mode,source,&request)?
        };
        super::normalize_draft_dictionary(source,dictionary,&mut draft);
        trace.draft=Some(draft.clone());
        super::review_admission(source,&draft).map_err(safe_error)?;
        checkpoint(&cancel,deadline)?;
        let request=semantic_review::prepare_minimum(config,mode,source,&draft).map_err(safe_error)?;
        checkpoint(&cancel,deadline)?;trace.calls+=1;
        let content=super::fluent_pipeline::request_content(client,config,secret,endpoint,&request.body,&cancel,deadline).await?;
        let review=semantic_review::parse_minimum(&content,&request,mode,source,&draft).map_err(safe_error)?;
        trace.review=Some(review.clone());checkpoint(&cancel,deadline)?;
        if review.accepted() && local_repair::residue_hint(source,&draft).is_none(){return Ok(draft);}
        if deadline.saturating_duration_since(Instant::now())<Duration::from_millis(REPAIR_RESERVE_MS){return Err(rejected());}
        let Some(request)=local_repair::prepare(config,mode,source,&draft,&review)? else{return Err(rejected());};
        checkpoint(&cancel,deadline)?;trace.calls+=1;trace.repair_attempted=true;
        let content=super::fluent_pipeline::request_content(client,config,secret,endpoint,&request.body,&cancel,deadline).await?;
        let repaired=local_repair::apply(&content,&request,mode,source,&draft).map_err(safe_error)?;
        trace.repaired_draft=Some(repaired.clone());checkpoint(&cancel,deadline)?;
        if repaired.blocks[0].text==draft.blocks[0].text{return Err(rejected());}
        if local_repair::native_deletion_verified(source,&draft,&repaired,&review){
            trace.native_deletion_verified=true;checkpoint(&cancel,deadline)?;return Ok(repaired);
        }
        // No second repair and no reuse of the pre-repair verdict.
        let request=semantic_review::prepare_minimum(config,mode,source,&repaired).map_err(safe_error)?;
        checkpoint(&cancel,deadline)?;trace.calls+=1;
        let content=super::fluent_pipeline::request_content(client,config,secret,endpoint,&request.body,&cancel,deadline).await?;
        let review=semantic_review::parse_minimum(&content,&request,mode,source,&repaired).map_err(safe_error)?;
        trace.final_review=Some(review.clone());checkpoint(&cancel,deadline)?;
        if !review.accepted() || local_repair::residue_hint(source,&repaired).is_some(){return Err(rejected());}
        Ok(repaired)
    };
    let result=tokio::select!{biased;_=cancel.cancelled()=>Err(cancelled()),result=tokio::time::timeout_at(deadline,job)=>result.unwrap_or_else(|_|Err(timeout()))};
    let result=checkpoint(&cancel,deadline).and(result);
    trace.elapsed_ms=started.elapsed().as_millis() as u64;
    Outcome{result,trace}
}
pub(super) async fn run(client:&reqwest::Client,config:&ProviderConfig,secret:&str,mode:&ModeSnapshot,source:&SourceRevision,
    dictionary:&DictionarySnapshot,cancel:CancellationToken,endpoint:&str,wait:Duration)->Result<RewriteResult,CloudError>{
    execute(client,config,secret,mode,source,dictionary,cancel,endpoint,wait).await.result
}
/// Explicit diagnostic entry point; it does not read credentials, persist or input.
pub async fn experiment(config:&ProviderConfig,secret:&str,mode:&ModeSnapshot,source:&SourceRevision,wait:Duration)->Result<Outcome,CloudError>{
    let endpoint=super::endpoints(config)?.rewrite;
    Ok(execute(super::shared_client()?,config,secret,mode,source,&DictionarySnapshot::default(),CancellationToken::new(),&endpoint,wait).await)
}
/// Fixed admissible rough draft for explicitly invoked repair diagnostics only.
/// Production never bypasses generation through this entry point.
pub async fn experiment_fixed(config:&ProviderConfig,secret:&str,mode:&ModeSnapshot,source:&SourceRevision,content:&str,wait:Duration)->Result<Outcome,CloudError>{
    let endpoint=super::endpoints(config)?.rewrite;
    let request=minimum::prepare(config,mode,source,&DictionarySnapshot::default(),false)?;
    let draft=minimum::parse(content,mode,source,&request)?;
    Ok(execute_from(super::shared_client()?,config,secret,mode,source,&DictionarySnapshot::default(),CancellationToken::new(),&endpoint,wait,Some(draft)).await)
}
/// Frozen actual 0.2.18 path, never influenced by a future promotion flag.
pub async fn frozen_018(config:&ProviderConfig,secret:&str,mode:&ModeSnapshot,source:&SourceRevision,wait:Duration)->Result<RewriteResult,CloudError>{
    let endpoint=super::endpoints(config)?.rewrite;let cancel=CancellationToken::new();let client=super::shared_client()?;
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

#[cfg(test)]#[path="rewrite_minimum_tests.rs"]mod tests;
