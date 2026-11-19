//! Recovery policy for completing editing after a cloud failure.
use super::*;
use crate::core::writing_context::WritingContext;

/// Allow generation, review and polishing to finish before delivering a result.
/// Length-based budgets are bounded engineering margins, not latency guarantees.
/// Retry budgets follow the same complete frozen source as initial generation.
#[derive(Clone, Copy)]
pub(super) struct RewriteWaitPolicy { pub deadline: Duration }
impl Default for RewriteWaitPolicy {
    fn default() -> Self { Self { deadline: Duration::from_secs(20) } }
}
impl RewriteWaitPolicy {
    pub fn for_source(source: &SourceRevision) -> Self {
        // Count the frozen body and separators by Unicode characters, also on retry.
        Self { deadline: Duration::from_millis(rewrite::full_pipeline_wait_ms(source)) }
    }
}

pub(super) fn failure_message(error:&CloudError,stage:Option<&str>,has_candidate:bool)->String {
    let step=match stage {
        Some("review"|"finalReview"|"recoveryReview"|"recoveryFinalReview")=>"整理稿复核",
        Some("repair"|"recoveryRepair")=>"整理稿修正",
        _=>"整理",
    };
    let reason=match error.code.as_str() {
        "rewrite_timeout"=>format!("{step}超时"),
        "rewrite_network"|"network_setup"=>"连接整理服务失败".into(),
        "rewrite_service_error"=>"整理服务暂时不可用".into(),
        "authentication_failed"=>"整理服务认证失败，请检查 API Key".into(),
        "rate_limited"=>"整理服务请求过于频繁，请稍后重试".into(),
        "rewrite_request_rejected"|"invalid_request"=>"整理请求被服务拒绝，请检查服务配置和模型".into(),
        "rewrite_refused"=>"整理服务未能处理本次内容".into(),
        _=>match fallback_reason(error) {
            Some(RewriteFallbackReason::MeaningRisk)=>format!("{step}未通过内容检查"),
            Some(RewriteFallbackReason::InvalidOutput)=>format!("{step}返回的内容格式无效"),
            _=>return error.message.clone(),
        },
    };
    let retained=if has_candidate {"已保留识别稿和待核对的整理候选。"}else{"已保留完整识别稿。"};
    format!("{reason}。{retained}")
}

pub(super) fn fallback_reason(error: &CloudError) -> Option<RewriteFallbackReason> {
    if error.forbids_automatic_rewrite_retry() {return None;}
    if !matches!(error.stage.as_str(),"rewrite"|"semantic_review") { return None; }
    Some(match error.code.as_str() {
        "rewrite_timeout" => RewriteFallbackReason::Timeout,
        "rewrite_network" | "network_setup" | "rewrite_service_error" => RewriteFallbackReason::ServiceUnavailable,
        "invalid_response" | "invalid_output" | "invalid_local_repair" | "empty_output" | "response_too_large" | "truncated_output" | "invalid_source_mapping" | "incomplete_source_coverage" => RewriteFallbackReason::InvalidOutput,
        "rewrite_meaning_risk" | "rewrite_admission_rejected" | "rewrite_semantic_rejected" | "rewrite_literal_changed"
        | "protected_terms_missing" | "protected_terms_new_latin" | "protected_terms_marker_unknown" | "protected_terms_marker_invalid"
        | "protected_terms_limit" | "protected_terms_empty" | "protected_terms_nonce_invalid" | "protected_terms_nonce_collision" => RewriteFallbackReason::MeaningRisk,
        _ => RewriteFallbackReason::InvalidOutput,
    })
}

pub(super) fn automatic_context(context: Option<&WritingContext>) -> WritingContext {
    context.cloned().unwrap_or(WritingContext { scene: AppWritingScene::General,
        language: DraftLanguage::AsSpoken, input: None, personalization: None })
}

pub(super) fn automatic_wait(source: &SourceRevision) -> Duration {
    Duration::from_secs(match source.text().chars().count() { 0..=800 => 30, 801..=3000 => 60, _ => 120 })
}

#[cfg(test)]
pub(super) fn source_fallback(source: &SourceRevision) -> rewrite::RewriteResult {
    rewrite::RewriteResult {
        resolved_mode: BaseMode::Light,
        blocks: vec![OutputBlock {
            id: Uuid::new_v4().to_string(), text: source.text(), source_refs: source.items.iter().map(|item|item.id.clone()).collect(),
        }],
    }
}
