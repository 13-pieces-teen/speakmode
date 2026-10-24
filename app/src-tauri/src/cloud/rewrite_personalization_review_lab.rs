//! Explicit fixed-draft research only: source fidelity and preference applicability
//! in one request. No production selection, generation, repair, persistence or
//! delivery. Model verdicts are evidence, not a proof of semantic equivalence.
use super::{Checks, CloudError, Decision, Issue, MinimumReviewFormat, MinimumReviewRequest, SemanticReview};
use super::super::{execution::{RequestOutcome, RequestStage, RewriteExecution},
    minimum_pipeline::PersonalizationLabEvent, RewriteResult};
use crate::core::{writing_context::WritingContext, ModeSnapshot, ProviderConfig, SourceRevision};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{collections::HashSet, sync::{Arc, Mutex, atomic::{AtomicBool, AtomicUsize, Ordering}}, time::Duration};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

pub const VERSION: &str = "personalization-separated-review-lab-v1";
pub const MAX_TOKENS: u64 = 512;
// Exact replacement is deliberately fail-closed. A changed upstream contract
// needs explicit offline review; appending a second JSON schema is not allowed.
const BASE_RESPONSE: &str = r#"只返回严格 JSON，结构 {"decision":"accept或reject","checks":{"coverage":"pass/fail/uncertain","facts":"pass/fail/uncertain","scope":"pass/fail/uncertain","intent":"pass/fail/uncertain","cleanup":"pass/fail/uncertain"},"issues":[]}。通过时 issues=[]。只有 cleanup=fail 且其他四项均 pass 时，issues 给最多两处精确连续原文/草稿摘录，每段最多80字：{"dimension":"cleanup","sourceQuote":"精确来源片段","draftQuote":"足以局部修复的草稿片段","reason":"简短语病说明，不写替换稿"}。不能唯一定位或超过两处时仍 reject，不猜摘录；其他失败可 issues=[]。不输出推理或多余字段。"#;
const BASE_PREFERENCE: &str = "兼容的个人排版规则必须满足（light 也可按用户规则原顺序列点），保真冲突部分忽略。";
const BASE_PERSONAL_REVIEW: &str = "Review cleanup checks only the remaining compatible preferences; it must not penalize refusing an excluded instruction.";
const SPLIT_RESPONSE: &str = r#"仅此固定稿实验，返回严格 JSON，结构 {"decision":"accept或reject","contentChecks":{"coverage":"pass/fail/uncertain","facts":"pass/fail/uncertain","scope":"pass/fail/uncertain","intent":"pass/fail/uncertain","cleanup":"pass/fail/uncertain"},"preferenceChecks":[{"key":"实际规则key","applicability":"apply/ignore_conflict/irrelevant/uncertain","status":"pass/fail/uncertain/not_applicable"}],"issues":[]}。
contentChecks 是先独立完成的五项来源、场景和基础格式检查：sourceText 是唯一意思来源，personalization 不是对照事实；contentChecks.cleanup 检查自然度、明确场景、baseMode/formatRules，不检查个人偏好是否被满足。decision 仅表示 contentChecks 的结论：五项全 pass 才 accept，否则 reject；个人偏好结论不能改变任何内容分项。
然后独立填写 preferenceChecks：personalization.rules 每个 key 恰有一行，不增加、不遗漏、不重复；没有规则时返回空数组。先排除与来源事实、条件、不确定性、字面量、明确场景/语言或不补写身份约束冲突的偏好部分。剩余部分适用时 applicability=apply，再评其 status 为 pass/fail/uncertain；整条有效偏好都冲突时 ignore_conflict，不相关时 irrelevant，这两种 status 只能是 not_applicable；不能确定适用性时 applicability=uncertain 且 status=uncertain。忽略越权偏好不构成 scope、facts、cleanup 失败。背景不需要返回规则行，不能成为正文事实依据。
保留原来的 issues 定位规则：通过时 issues=[]；仅 contentChecks.cleanup=fail 且其他四项 pass 时最多两处精确连续来源/草稿摘录，每段最多80字，结构 {"dimension":"cleanup","sourceQuote":"精确来源片段","draftQuote":"足以局部修复的草稿片段","reason":"简短语病说明，不写替换稿"}。不能唯一定位时仍拒绝，不猜摘录；其他失败可 issues=[]。不返回正文、修改建议、推理或额外字段。"#;

fn error(code: &str, message: &str) -> CloudError { CloudError::new(code, "preference_lab", message) }
fn replace_exact(prompt: &str, old: &str, new: &str) -> Result<String, CloudError> {
    if prompt.matches(old).count()!=1 { return Err(error("unsupported_lab_contract", "实验基准合同已变化，未授权请求")); }
    Ok(prompt.replacen(old,new,1))
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all="camelCase")]
pub struct LabMetadata {
    pub protocol_version: &'static str,
    pub baseline_review_version: &'static str,
    pub arm: &'static str,
    pub max_tokens: u64,
    pub maximum_client_started_requests: usize,
    pub latency_scope: &'static str,
    pub semantic_evidence: &'static str,
    pub content_check_boundary: &'static str,
    pub preference_check_boundary: &'static str,
    pub request_body_sha256: String,
}

/// Contains private source/draft/profile data. No Debug/Serialize implementation.
pub struct Prepared {
    pub body: Value,
    pub metadata: LabMetadata,
    base: MinimumReviewRequest,
    body_hash: String,
    context_hash: String,
    expected_keys: Vec<String>,
    separated: bool,
    native_admission_error: Option<CloudError>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all="snake_case")]
pub enum Applicability { Apply, IgnoreConflict, Irrelevant, Uncertain }
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all="snake_case")]
pub enum PreferenceStatus { Pass, Fail, Uncertain, NotApplicable }
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
pub struct PreferenceCheck { pub key:String, pub applicability:Applicability, pub status:PreferenceStatus }
#[derive(Deserialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
struct WireSeparated {
    decision:Decision, content_checks:Checks, preference_checks:Vec<PreferenceCheck>, issues:Vec<Issue>,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all="camelCase")]
pub struct LabRequestTiming { pub stage:&'static str,pub duration_ms:f64,pub remaining_budget_ms:u64,pub outcome:&'static str }
#[derive(Serialize)]
#[serde(rename_all="camelCase")]
pub struct LabOutcome {
    pub content_accepted:bool,
    /// The combined baseline cannot provide an independent preference verdict.
    pub preferences_accepted:Option<bool>,
    pub accepted:bool,
    pub review:SemanticReview,
    pub preference_checks:Option<Vec<PreferenceCheck>>,
    pub native_admission_accepted:bool,
    pub native_admission_error:Option<CloudError>,
    pub elapsed_ms:u64,
    pub requests:Vec<LabRequestTiming>,
    pub metadata:LabMetadata,
}

/// Preparation has no credentials/network/persistence side effects. The named
/// control is explicit: it does not claim to be Aliyun's default ordered wire.
pub fn prepare(config:&ProviderConfig,mode:&ModeSnapshot,source:&SourceRevision,draft:&RewriteResult,
    context:Option<&WritingContext>,separated:bool)->Result<Prepared,CloudError> {
    if let Some(context)=context {context.validate().map_err(|_|error("invalid_writing_context","实验上下文无效"))?;}
    let base=super::prepare_minimum_with_writing_context_and_format(config,mode,source,draft,context,MinimumReviewFormat::Named)?;
    let expected_keys=context.and_then(|c|c.personalization.as_ref()).map(|p|p.rules.iter().map(|r|r.key.clone()).collect()).unwrap_or_default();
    let mut body=base.body.clone();
    if separated {
        let original=body["messages"][0]["content"].as_str().ok_or_else(||error("invalid_lab_request","实验提示格式无效"))?;
        let prompt=replace_exact(original,BASE_RESPONSE,SPLIT_RESPONSE)?;
        let mut prompt=replace_exact(&prompt,BASE_PREFERENCE,"个人表达偏好只在 preferenceChecks 核对，不参与 contentChecks 的五项判断。")?;
        if context.is_some_and(|c|c.personalization.is_some()) {
            prompt=replace_exact(&prompt,BASE_PERSONAL_REVIEW,"Review personal preferences only in preferenceChecks. contentChecks, including cleanup, must not penalize an unmet or excluded personal preference.")?;
        }
        body["messages"][0]["content"]=json!(format!("{prompt}\n实验输出协议：{VERSION}。此协议不授权改稿、放行、保存或交付。"));
    }
    if body["max_tokens"]!=json!(MAX_TOKENS) {return Err(error("unsupported_lab_contract","实验输出预算已变化"));}
    let native_admission_error=super::super::review_admission(source,draft).err()
        .or_else(||context.and_then(|_|super::super::writing_context::validate_tense(source,draft).err()));
    let metadata=LabMetadata {protocol_version:VERSION,baseline_review_version:base.protocol_version,
        arm:if separated{"separatedNamed"}else{"combinedNamedControl"},max_tokens:MAX_TOKENS,maximum_client_started_requests:1,
        latency_scope:"FIXED_DRAFT_REVIEW_ONLY_NOT_NORMAL_LATENCY",semantic_evidence:"MODEL_VERDICT_NOT_SEMANTIC_PROOF",
        content_check_boundary:if separated{"SOURCE_SCENE_BASE_FORMAT_FIVE_CHECKS"}else{"BASELINE_FIVE_CHECKS_WITH_COMBINED_PERSONAL_CLEANUP"},
        preference_check_boundary:if separated{"SEPARATE_MODEL_APPLICABILITY_AND_ADHERENCE"}else{"NOT_SEPARATELY_OBSERVED"},
        request_body_sha256:super::fingerprint(&body)};
    let body_hash=metadata.request_body_sha256.clone();
    Ok(Prepared{body,metadata,base,body_hash,context_hash:super::fingerprint(&context),expected_keys,separated,native_admission_error})
}

fn validate_preferences(rows:&[PreferenceCheck],keys:&[String])->Result<bool,CloudError> {
    if rows.len()!=keys.len() || rows.len()>7 {return Err(error("invalid_preference_review","实验偏好行缺失或超出数量限制"));}
    let expected:HashSet<_>=keys.iter().map(String::as_str).collect();let mut seen=HashSet::new();let mut accepted=true;
    for row in rows {
        if !expected.contains(row.key.as_str()) || !seen.insert(row.key.as_str()) {return Err(error("invalid_preference_review","实验偏好key无效或重复"));}
        match (row.applicability,row.status) {
            (Applicability::Apply,PreferenceStatus::Pass)=>{},
            (Applicability::Apply,PreferenceStatus::Fail|PreferenceStatus::Uncertain)=>accepted=false,
            (Applicability::IgnoreConflict|Applicability::Irrelevant,PreferenceStatus::NotApplicable)=>{},
            (Applicability::Uncertain,PreferenceStatus::Uncertain)=>accepted=false,
            _=>return Err(error("invalid_preference_review","实验偏好适用性与结论不一致")),
        }
    }
    Ok(accepted)
}

fn parse(content:&str,request:&Prepared,mode:&ModeSnapshot,source:&SourceRevision,draft:&RewriteResult,
    context:Option<&WritingContext>)->Result<(SemanticReview,Option<Vec<PreferenceCheck>>,Option<bool>),CloudError> {
    if request.body_hash!=super::fingerprint(&request.body) || request.context_hash!=super::fingerprint(&context) {
        return Err(error("stale_semantic_review","实验请求或上下文已变化"));
    }
    request.base.validate_personalization(context)?;
    if !request.separated {return Ok((super::parse_minimum(content,&request.base,mode,source,draft)?,None,None));}
    if content.trim().is_empty() || content.len()>64*1024 {return Err(error("invalid_preference_review","实验响应为空或超过容量限制"));}
    // Typed deserialization rejects duplicate keys before constructing a Value.
    let wire:WireSeparated=serde_json::from_str(content).map_err(|_|error("invalid_preference_review","实验响应结构无效"))?;
    let preferences_accepted=validate_preferences(&wire.preference_checks,&request.expected_keys)?;
    let projected=json!({"decision":wire.decision,"checks":wire.content_checks,"issues":wire.issues}).to_string();
    // Reuse exact source/draft/mode/profile bindings and native letter guards.
    // This projection is local validation, not a claim of receiving base wire.
    let mut review=super::parse_minimum(&projected,&request.base,mode,source,draft)?;
    review.protocol_version=VERSION;review.reviewer_wire_protocol_version=VERSION;
    Ok((review,Some(wire.preference_checks),Some(preferences_accepted)))
}

/// One request per arm. A failed ledger callback cancels before dispatch where
/// possible and always prevents an accepted result. No automatic retry.
pub async fn run(config:&ProviderConfig,secret:&str,mode:&ModeSnapshot,source:&SourceRevision,draft:&RewriteResult,
    context:Option<&WritingContext>,separated:bool,wait:Duration,
    observer:impl Fn(PersonalizationLabEvent)->bool+Send+Sync+'static)->Result<LabOutcome,CloudError> {
    let started=Instant::now();let request=prepare(config,mode,source,draft,context,separated)?;
    let endpoint=super::super::rewrite_endpoint(config)?;
    let client=reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).retry(reqwest::retry::never()).build()
        .map_err(|_|error("lab_client_failed","实验客户端创建失败"))?;
    let cancel=CancellationToken::new();let observed_cancel=cancel.clone();
    let failed=Arc::new(AtomicBool::new(false));let observed_failed=failed.clone();
    let starts=Arc::new(AtomicUsize::new(0));let observed_starts=starts.clone();
    let timings=Arc::new(Mutex::new(Vec::new()));let observed_timings=timings.clone();
    let execution=RewriteExecution::observed(started+wait,move|event|{
        let outcome=event.outcome.map(|value|match value {RequestOutcome::ResponseReceived=>"responseReceived",RequestOutcome::Failed=>"failed",
            RequestOutcome::Cancelled=>"cancelled",RequestOutcome::TimedOut=>"timedOut",RequestOutcome::Interrupted=>"interrupted"});
        let duration_ms=event.finished_at.map(|end|end.duration_since(event.started_at).as_secs_f64()*1000.);
        if event.finished_at.is_none() && observed_starts.fetch_add(1,Ordering::SeqCst)>=1 {observed_failed.store(true,Ordering::SeqCst);observed_cancel.cancel();return;}
        if let (Some(duration_ms),Some(outcome))=(duration_ms,outcome) {
            match observed_timings.lock() {Ok(mut rows)=>rows.push(LabRequestTiming{stage:"review",duration_ms,remaining_budget_ms:event.remaining_budget_ms,outcome}),
                Err(_)=>{observed_failed.store(true,Ordering::SeqCst);observed_cancel.cancel();return;}}
        }
        if !observer(PersonalizationLabEvent{stage:"review",event:if event.finished_at.is_some(){"finished"}else{"started"},duration_ms,
            remaining_budget_ms:event.remaining_budget_ms,outcome}) {observed_failed.store(true,Ordering::SeqCst);observed_cancel.cancel();}
    });
    let job=async {
        execution.checkpoint(&cancel)?;
        let content=super::super::fluent_pipeline::request_observed(&client,config,secret,&endpoint,&request.body,&cancel,&execution,RequestStage::Review).await?;
        execution.checkpoint(&cancel)?;
        let parsed=parse(&content,&request,mode,source,draft,context)?;execution.checkpoint(&cancel)?;Ok(parsed)
    };
    let result=tokio::select! {biased;
        _=cancel.cancelled()=>Err(CloudError::new("cancelled","rewrite","实验复核已取消")),
        result=tokio::time::timeout_at(execution.deadline,job)=>result.unwrap_or_else(|_|Err(CloudError::new("rewrite_timeout","rewrite","实验复核期限已结束"))),
    };
    if failed.load(Ordering::SeqCst) {return Err(error("lab_ledger_failed","实验账本写入失败，复核未获授权"));}
    execution.checkpoint(&cancel)?;
    let (review,preference_checks,preferences_accepted)=result?;
    let content_accepted=review.accepted();let native_admission_accepted=request.native_admission_error.is_none();
    let requests=std::mem::take(&mut *timings.lock().map_err(|_|error("lab_ledger_failed","实验请求记录无效"))?);
    Ok(LabOutcome{content_accepted,preferences_accepted,accepted:content_accepted&&native_admission_accepted&&preferences_accepted.unwrap_or(true),
        review,preference_checks,native_admission_accepted,native_admission_error:request.native_admission_error,
        elapsed_ms:started.elapsed().as_millis().min(u64::MAX as u128) as u64,requests,metadata:request.metadata})
}

#[cfg(test)]
#[path="rewrite_personalization_review_lab_tests.rs"]
mod tests;
