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

pub(super) async fn request_content(client: &Client, config: &ProviderConfig, secret: &str,
    endpoint: &str, body: &Value, cancel: &CancellationToken, deadline: Instant) -> Result<String, CloudError> {
    checkpoint(cancel, deadline)?;
    // The caller supplies its existing no-redirect client and allowlisted endpoint.
    let mut request = client.post(endpoint).timeout(deadline.saturating_duration_since(Instant::now()))
        .bearer_auth(secret).json(body);
    if let Some(workspace) = config.workspace.as_deref().filter(|value| !value.is_empty()) {
        request = request.header("X-DashScope-WorkSpace", workspace);
    }
    let mut response = request.send().await.map_err(transport_error)?;
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
    data.pointer("/choices/0/message/content").and_then(Value::as_str).map(str::to_owned)
        .ok_or_else(|| CloudError::new("empty_output", "rewrite", "整理响应为空"))
}

pub(super) async fn run(client: &Client, config: &ProviderConfig, secret: &str, mode: &ModeSnapshot,
    source: &SourceRevision, dictionary: &DictionarySnapshot, cancel: CancellationToken,
    endpoint: &str, timeout: Duration) -> Result<RewriteResult, CloudError> {
    let deadline = Instant::now() + timeout;
    let job = async {
        checkpoint(&cancel, deadline)?;
        let protected = if uses_guarded_long(source) { Some(prepare_guarded_long(config, mode, source, dictionary)?) } else { None };
        let body = match &protected { Some(request) => request.body.clone(), None => fluent_request_body(config, mode, source, dictionary)? };
        let content = request_content(client, config, secret, endpoint, &body, &cancel, deadline).await?;
        let mut draft = match &protected { Some(request) => parse_protected_long(&content, mode, source, request)?, None => parse_fluent_draft(&content, mode, source)? };
        super::normalize_draft_dictionary(source, dictionary, &mut draft);
        checkpoint(&cancel, deadline)?;
        review_admission(source, &draft).map_err(admission_error)?;
        checkpoint(&cancel, deadline)?;
        let review_request = semantic_review::prepare(config, mode, source, &draft).map_err(review_error)?;
        checkpoint(&cancel, deadline)?;
        let content = request_content(client, config, secret, endpoint, &review_request.body, &cancel, deadline).await?;
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
