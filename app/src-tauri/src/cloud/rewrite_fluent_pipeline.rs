//! Candidate transport: generate once, then review every admissible frozen draft.
//! Selection belongs to the caller; this module does not enable itself, persist
//! text, retry, fall back, or deliver. Both requests share one absolute deadline.
use super::{fluent_request_body, parse_fluent_draft, parse_protected_long, prepare_guarded_long, uses_guarded_long, review_admission, semantic_review, CloudError, RewriteResult};
use crate::core::{DictionarySnapshot, ModeSnapshot, ProviderConfig, SourceRevision};
use reqwest::Client;
use serde_json::Value;
use std::time::Duration;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use super::execution::{RequestStage, RewriteExecution};

fn cancelled() -> CloudError { CloudError::new("cancelled", "rewrite", "已取消整理") }
fn timed_out() -> CloudError { CloudError::new("rewrite_timeout", "rewrite", "整理与核对等待已结束，识别稿仍保留") }
fn checkpoint(cancel: &CancellationToken, deadline: Instant) -> Result<(), CloudError> {
    if cancel.is_cancelled() { Err(cancelled()) }
    else if Instant::now() >= deadline { Err(timed_out()) }
    else { Ok(()) }
}
fn transport_error(error: reqwest::Error) -> CloudError {
    if error.is_timeout() { timed_out() }
    else { CloudError::new("rewrite_network", "rewrite", "整理服务连接中断，识别稿仍保留") }
}
fn review_error(error: CloudError) -> CloudError {
    if error.stage == "semantic_review" {
        // Never expose reviewer evidence or an arbitrary response through errors.
        CloudError::new("invalid_output", "rewrite", "语义核对响应无效，识别稿仍保留")
    } else { error }
}
fn admission_error(error: CloudError) -> CloudError {
    // A fixed stage label is enough to diagnose a fallback. Numeric keys and
    // source/draft excerpts belong only in explicitly invoked offline tools.
    if error.stage == "rewrite" && error.code == "rewrite_meaning_risk" {
        CloudError::new("rewrite_admission_rejected", "rewrite", "整理稿未通过复核前检查，识别稿仍保留")
    } else { error }
}

fn wire_has_personalization(body: &Value) -> bool {
    body["messages"].as_array().is_some_and(|messages| messages.iter().any(|message| {
        message["role"].as_str() == Some("user") && message["content"].as_str()
            .and_then(|content| serde_json::from_str::<Value>(content).ok())
            .is_some_and(|data| data.get("personalization").is_some_and(Value::is_object))
    }))
}

pub(super) async fn request_content(client: &Client, config: &ProviderConfig, secret: &str,
    endpoint: &str, body: &Value, cancel: &CancellationToken, deadline: Instant) -> Result<String, CloudError> {
    request_content_with_execution(client,config,secret,endpoint,body,cancel,
        &RewriteExecution::until(deadline),RequestStage::Generation).await
}

/// Transport-only entry point for callers that maintain their own timing trace.
/// It participates in dispatch tracking but does not add request timing events.
pub(super) async fn request_content_with_execution(client: &Client, config: &ProviderConfig, secret: &str,
    endpoint: &str, body: &Value, cancel: &CancellationToken, execution: &RewriteExecution,
    stage: RequestStage) -> Result<String, CloudError> {
    let deadline = execution.deadline;
    checkpoint(cancel, deadline)?;
    let body = super::chat_request::for_provider(config, body)?;
    // Unobserved/disabled-profile requests keep their existing transport cost;
    // parsing the JSON data envelope is needed only for dispatch evidence.
    let personalization = execution.has_dispatch_observer() && wire_has_personalization(&body);
    // The caller supplies its existing no-redirect client and allowlisted endpoint.
    let mut request = client.post(endpoint).timeout(deadline.saturating_duration_since(Instant::now()))
        .bearer_auth(secret).json(&body);
    if let Some(workspace) = config.workspace.as_deref().filter(|value| config.rewrite_provider == "aliyun" && !value.is_empty()) {
        request = request.header("X-DashScope-WorkSpace", workspace);
    }
    // Validate the URL, headers and serialization before recording any dispatch.
    // Building a request or starting a timing span is not a send attempt.
    let request = request.build().map_err(transport_error)?;
    let send = async {
        checkpoint(cancel, deadline)?;
        execution.dispatch_started(stage, personalization);
        client.execute(request).await.map_err(transport_error)
    };
    let mut response = tokio::select! {
        biased;
        _ = cancel.cancelled() => return Err(cancelled()),
        _ = tokio::time::sleep_until(deadline) => return Err(timed_out()),
        response = send => response?,
    };
    checkpoint(cancel, deadline)?;
    if !response.status().is_success() {
        let code = match response.status().as_u16() { 401 | 403 => "authentication_failed", 429 => "rate_limited", _ => "rewrite_service_error" };
        return Err(CloudError::new(code, "rewrite", "整理服务未接受请求，识别稿仍保留"));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(transport_error)? {
        checkpoint(cancel, deadline)?;
        if bytes.len() + chunk.len() > 1024 * 1024 {
            return Err(CloudError::new("response_too_large", "rewrite", "整理响应超过容量限制"));
        }
        bytes.extend_from_slice(&chunk);
    }
    let data: Value = serde_json::from_slice(&bytes)
        .map_err(|_| CloudError::new("invalid_response", "rewrite", "整理响应格式无效"))?;
    checkpoint(cancel, deadline)?;
    if data.pointer("/choices/0/finish_reason").and_then(Value::as_str) != Some("stop") {
        return Err(CloudError::new("truncated_output", "rewrite", "整理响应未完整结束"));
    }
    if data.pointer("/choices/0/message/refusal").is_some_and(|value| !value.is_null()) {
        return Err(CloudError::new("rewrite_refused", "rewrite", "整理服务未能处理此内容，识别稿仍保留"));
    }
    data.pointer("/choices/0/message/content").and_then(Value::as_str).map(str::to_owned)
        .filter(|content| !content.trim().is_empty())
        .ok_or_else(|| CloudError::new("empty_output", "rewrite", "整理响应为空"))
}

pub(super) async fn request_observed(client: &Client, config: &ProviderConfig, secret: &str,
    endpoint: &str, body: &Value, cancel: &CancellationToken, execution: &RewriteExecution,
    stage: RequestStage) -> Result<String, CloudError> {
    execution.checkpoint(cancel)?;
    let mut guard = execution.start(stage, cancel);
    let result = request_content_with_execution(client,config,secret,endpoint,body,cancel,execution,stage).await;
    guard.finish(&result);
    result
}

pub(super) async fn run(client: &Client, config: &ProviderConfig, secret: &str, mode: &ModeSnapshot,
    source: &SourceRevision, dictionary: &DictionarySnapshot, cancel: CancellationToken,
    endpoint: &str, timeout: Duration) -> Result<RewriteResult, CloudError> {
    run_until(client,config,secret,mode,source,dictionary,cancel,endpoint,RewriteExecution::for_duration(timeout)).await
}

pub(super) async fn run_until(client: &Client, config: &ProviderConfig, secret: &str, mode: &ModeSnapshot,
    source: &SourceRevision, dictionary: &DictionarySnapshot, cancel: CancellationToken,
    endpoint: &str, execution: RewriteExecution) -> Result<RewriteResult, CloudError> {
    let deadline = execution.deadline;
    let job = async {
        checkpoint(&cancel, deadline)?;
        let protected = if uses_guarded_long(source) { Some(prepare_guarded_long(config, mode, source, dictionary)?) } else { None };
        let body = match &protected { Some(request) => request.body.clone(), None => fluent_request_body(config, mode, source, dictionary)? };
        let content = request_observed(client, config, secret, endpoint, &body, &cancel, &execution, RequestStage::Generation).await?;
        let mut draft = match &protected { Some(request) => parse_protected_long(&content, mode, source, request)?, None => parse_fluent_draft(&content, mode, source)? };
        super::normalize_draft_dictionary(source, dictionary, &mut draft);
        checkpoint(&cancel, deadline)?;
        review_admission(source, &draft).map_err(admission_error)?;
        checkpoint(&cancel, deadline)?;
        let review_request = semantic_review::prepare(config, mode, source, &draft).map_err(review_error)?;
        checkpoint(&cancel, deadline)?;
        let content = request_observed(client, config, secret, endpoint, &review_request.body, &cancel, &execution, RequestStage::Review).await?;
        // Reuse this exact draft: reparsing would change its bound block identity.
        let review = semantic_review::parse(&content, &review_request.binding, source, &draft).map_err(review_error)?;
        checkpoint(&cancel, deadline)?;
        if !review.accepted() {
            return Err(CloudError::new("rewrite_semantic_rejected", "rewrite", "语义核对未通过，识别稿仍保留"));
        }
        Ok(draft)
    };
    let result = tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(cancelled()),
        result = tokio::time::timeout_at(deadline, job) => result.unwrap_or_else(|_| Err(timed_out())),
    };
    // Synchronous parsing/validation may not yield to timeout_at. Never admit a
    // result after the absolute cutoff, or let a simultaneous success beat cancel.
    checkpoint(&cancel, deadline)?;
    result
}

#[cfg(test)]
#[path = "rewrite_fluent_pipeline_tests.rs"]
mod tests;
