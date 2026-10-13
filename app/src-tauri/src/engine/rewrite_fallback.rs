//! Delivery policy for the optional rewrite, independent from audio/ASR completion.
use super::*;

/// Bound optional cleanup before delivering a complete short dictation. The short-text
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
    if error.stage != "rewrite" { return None; }
    Some(match error.code.as_str() {
        "rewrite_timeout" => RewriteFallbackReason::Timeout,
        "rewrite_network" | "network_setup" | "rewrite_service_error" | "authentication_failed" | "rate_limited" => RewriteFallbackReason::ServiceUnavailable,
        "invalid_response" | "invalid_output" | "invalid_local_repair" | "empty_output" | "response_too_large" | "truncated_output" | "invalid_source_mapping" | "incomplete_source_coverage" => RewriteFallbackReason::InvalidOutput,
        "rewrite_meaning_risk" | "rewrite_admission_rejected" | "rewrite_semantic_rejected" | "rewrite_literal_changed"
        | "protected_terms_missing" | "protected_terms_new_latin" | "protected_terms_marker_unknown" | "protected_terms_marker_invalid"
        | "protected_terms_limit" | "protected_terms_empty" | "protected_terms_nonce_invalid" | "protected_terms_nonce_collision" => RewriteFallbackReason::MeaningRisk,
        // Cancellation and invalid/incomplete sources must never become deliverable text.
        _ => return None,
    })
}

pub(super) fn source_fallback(source: &SourceRevision) -> rewrite::RewriteResult {
    rewrite::RewriteResult {
        resolved_mode: BaseMode::Light,
        blocks: vec![OutputBlock {
            id: Uuid::new_v4().to_string(), text: source.text(), source_refs: source.items.iter().map(|item|item.id.clone()).collect(),
        }],
    }
}
