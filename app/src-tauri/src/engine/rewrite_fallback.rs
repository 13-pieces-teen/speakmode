//! Recovery policy for completing editing after a cloud failure.
use super::*;
use crate::core::writing_context::WritingContext;

/// Bound cloud editing before selecting the best available complete result. The short-text
/// budget is an engineering margin over synthetic measurements, not a long-text or
/// weak-network latency guarantee. Explicit retries keep the existing cloud ceiling.
#[derive(Clone, Copy)]
pub(super) struct RewriteWaitPolicy { pub deadline: Duration }
impl Default for RewriteWaitPolicy {
    fn default() -> Self { Self { deadline: Duration::from_secs(20) } }
}
impl RewriteWaitPolicy {
    pub fn for_source(reason: CreationReason, source: &SourceRevision) -> Self {
        // Count the exact frozen body, including item separators, by Unicode scalar
        // values. UTF-8 byte length would give Chinese dictation a smaller budget.
        if reason == CreationReason::NormalInitial {
            Self { deadline: Duration::from_millis(rewrite::initial_wait_ms(source)) }
        } else {
            Self::default()
        }
    }
}

pub(super) fn fallback_reason(error: &CloudError) -> Option<RewriteFallbackReason> {
    if matches!(error.code.as_str(),"cancelled"|"source_incomplete"|"invalid_source") || error.code.starts_with("stale_") {return None;}
    if matches!(error.stage.as_str(),"credentials"|"configuration") {return Some(RewriteFallbackReason::ServiceUnavailable);}
    if !matches!(error.stage.as_str(),"rewrite"|"semantic_review") { return None; }
    Some(match error.code.as_str() {
        "rewrite_timeout" => RewriteFallbackReason::Timeout,
        "rewrite_network" | "network_setup" | "rewrite_service_error" | "authentication_failed" | "rate_limited" => RewriteFallbackReason::ServiceUnavailable,
        "invalid_response" | "invalid_output" | "invalid_local_repair" | "empty_output" | "response_too_large" | "truncated_output" | "invalid_source_mapping" | "incomplete_source_coverage" => RewriteFallbackReason::InvalidOutput,
        "rewrite_meaning_risk" | "rewrite_admission_rejected" | "rewrite_semantic_rejected" | "rewrite_literal_changed"
        | "protected_terms_missing" | "protected_terms_new_latin" | "protected_terms_marker_unknown" | "protected_terms_marker_invalid"
        | "protected_terms_limit" | "protected_terms_empty" | "protected_terms_nonce_invalid" | "protected_terms_nonce_collision" => RewriteFallbackReason::MeaningRisk,
        _ => RewriteFallbackReason::InvalidOutput,
    })
}

pub(super) fn automatic_mode() -> ModeSnapshot {
    ModeSnapshot { mode_id: "auto".into(), name: "自动".into(), base_mode: BaseMode::Auto, rules: String::new(), revision: 1 }
}

pub(super) fn automatic_context(context: Option<&WritingContext>) -> WritingContext {
    WritingContext { scene: AppWritingScene::General,
        language: context.map_or(DraftLanguage::AsSpoken, |context| context.language),
        input: None, personalization: None }
}

pub(super) fn automatic_wait(source: &SourceRevision) -> Duration {
    Duration::from_secs(match source.text().chars().count() { 0..=800 => 20, 801..=3000 => 30, _ => 60 })
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
