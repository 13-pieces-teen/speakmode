//! Experimental semantic-review admission, never an alternative acceptance guard.
//!
//! The caller must first validate JSON, Mode, source coverage and block references.
//! Compare global canonical fact sets after explicit spoken repairs. This deliberately
//! does not reuse prefix/bigram scope matching, and cannot prove who owns a value,
//! how often it occurs, or whether prose retains its meaning. Count/scalar facts stay
//! for the independent reviewer: their determiners are the false-rejection problem.
use super::{anchors, effective_spans, facts, same_kind, CloudError, Kind};
use crate::core::{OutputBlock, SourceRevision};
use std::collections::HashSet;
use super::super::english_years;

fn reject(message: &'static str) -> CloudError {
    CloudError::new("rewrite_meaning_risk", "rewrite", message)
}

fn numeric_set<'a>(texts: impl Iterator<Item = &'a str>) -> HashSet<String> {
    texts.flat_map(facts::extract).map(|fact| fact.key)
        .filter(|key| !key.starts_with("count:") && !key.starts_with("scalar:"))
        .collect()
}

fn numeric_sets(source: &SourceRevision, blocks: &[OutputBlock], spans: &[super::Span<'_>]) -> (HashSet<String>, HashSet<String>) {
    let mut source_numbers = numeric_set(spans.iter().map(|span| span.text.as_str()));
    let mut output_numbers = numeric_set(blocks.iter().map(|block| block.text.as_str()));
    // Whole-source extraction is essential here: clause splitting would turn
    // "in twenty thirty.\nTwo." into a false 2030 before seeing its suffix.
    let text=source.text();
    let expected=english_years::source_keys(&text);
    output_numbers.extend(blocks.iter().flat_map(|block| english_years::retained_keys(&block.text))
        .filter(|key|expected.contains(key)));
    source_numbers.extend(expected);
    // A date's optional comma must not hide its year from clause-based
    // extraction. Use the effective (already corrected) spans in source order,
    // rather than reintroducing a date withdrawn by an explicit spoken repair.
    let effective=spans.iter().map(|span|span.text.as_str()).collect::<Vec<_>>().join(", ");
    source_numbers.extend(super::super::dates::fact_keys(&effective));
    output_numbers.extend(blocks.iter().flat_map(|block|super::super::dates::fact_keys(&block.text)));
    (source_numbers, output_numbers)
}

fn kinds<'a>(texts: impl Iterator<Item = &'a str>) -> HashSet<Kind> {
    texts.flat_map(anchors).map(|anchor| anchor.kind).collect()
}

/// Diagnostic labels describe only the finite classifier; no source excerpts.
fn kind_label(kind: Kind) -> String {
    match kind {
        Kind::Time(code) => format!("time:{code}"),
        Kind::Conditional => "conditional".into(),
        Kind::NecessaryCondition => "necessary_condition".into(),
        Kind::Exception => "exception".into(),
        Kind::Otherwise => "otherwise".into(),
        Kind::Possibility => "possibility".into(),
        Kind::Pending => "pending".into(),
        Kind::Prohibition => "prohibition".into(),
        Kind::NegativeFact => "negative_fact".into(),
        Kind::NegativeStatus => "negative_status".into(),
        Kind::Negative => "negative".into(),
        Kind::AbilityQuestion => "ability_question".into(),
        Kind::PolarQuestion => "polar_question".into(),
        Kind::QuantityOnly => "quantity_only".into(),
    }
}

fn sorted(values: impl Iterator<Item = String>) -> Vec<String> {
    values.collect::<std::collections::BTreeSet<_>>().into_iter().collect()
}

/// Explicit offline lab output. Numeric values are private experimental data;
/// callers must not put this object into ordinary application diagnostics.
/// It observes the same effective spans and helpers as validate, without
/// changing validation, repairing a draft, or asserting semantic equivalence.
pub(super) fn evidence(source: &SourceRevision, blocks: &[OutputBlock]) -> serde_json::Value {
    let spans = effective_spans(source);
    let (source_numbers, output_numbers) = numeric_sets(source, blocks, &spans);
    let source_kinds = kinds(spans.iter().map(|span| span.text.as_str()));
    let output_kinds = kinds(blocks.iter().map(|block| block.text.as_str()));
    let time_labels = |values: &HashSet<Kind>| sorted(values.iter().copied()
        .filter(|kind| matches!(kind, Kind::Time(_))).map(kind_label));
    let missing_kinds = sorted(source_kinds.iter().copied()
        .filter(|kind| !matches!(kind, Kind::Time(_)))
        .filter(|expected| !output_kinds.iter().any(|actual| same_kind(*expected, *actual)))
        .map(kind_label));
    let result = validate(source, blocks);
    serde_json::json!({
        "schemaVersion": 1,
        "scope": "OFFLINE_FINITE_ADMISSION_DIAGNOSTICS_NOT_SEMANTIC_PROOF",
        "source": {
            "numericKeys": sorted(source_numbers.iter().cloned()),
            "allFactKeys": sorted(spans.iter().flat_map(|span| facts::extract(&span.text)).map(|fact| fact.key)),
            "timeKinds": time_labels(&source_kinds),
            "anchorKinds": sorted(source_kinds.iter().copied().map(kind_label)),
            "effectiveSpanCount": spans.len(),
            "unresolvedCorrectionCount": spans.iter().filter(|span| span.unresolved_correction).count()
        },
        "output": {
            "numericKeys": sorted(output_numbers.iter().cloned()),
            "allFactKeys": sorted(blocks.iter().flat_map(|block| facts::extract(&block.text)).map(|fact| fact.key)),
            "timeKinds": time_labels(&output_kinds),
            "anchorKinds": sorted(output_kinds.iter().copied().map(kind_label)),
            "blockCount": blocks.len()
        },
        "differences": {
            "numericRemoved": sorted(source_numbers.difference(&output_numbers).cloned()),
            "numericAdded": sorted(output_numbers.difference(&source_numbers).cloned()),
            "timeRemoved": sorted(source_kinds.difference(&output_kinds).copied().filter(|kind| matches!(kind, Kind::Time(_))).map(kind_label)),
            "timeAdded": sorted(output_kinds.difference(&source_kinds).copied().filter(|kind| matches!(kind, Kind::Time(_))).map(kind_label)),
            "missingQualifierKindsUnderSameKindRule": missing_kinds
        },
        "admissionPassed": result.is_ok(),
        "admissionErrorCode": result.err().map(|error| error.code),
        "limitations": "Set comparison ignores multiplicity, ownership and textual scope; count/scalar keys are excluded only from numericKeys, not allFactKeys. No raw transcript or draft excerpts are included."
    })
}

/// Detect only known dimensions, not names or arbitrary semantic facts. Counts
/// require reviewer evidence even though admission intentionally does not compare
/// them. Inspect both texts so newly introduced risk cannot be marked absent.
pub(super) fn required_evidence(source: &SourceRevision, blocks: &[OutputBlock]) -> (bool, bool) {
    let spans = effective_spans(source);
    let (mut facts_required, mut scope_required) = (false, false);
    for text in spans.iter().map(|span| span.text.as_str()).chain(blocks.iter().map(|block| block.text.as_str())) {
        facts_required |= facts::extract(text).iter().any(|fact| !fact.key.starts_with("scalar:"));
        for anchor in anchors(text) {
            if matches!(anchor.kind, Kind::Time(_)) { facts_required = true; }
            else { scope_required = true; }
        }
    }
    facts_required |= !english_years::keys(&source.text()).is_empty() || blocks.iter().any(|block| !english_years::keys(&block.text).is_empty());
    facts_required |= !super::super::dates::fact_keys(&spans.iter().map(|span|span.text.as_str()).collect::<Vec<_>>().join(", ")).is_empty()
        || blocks.iter().any(|block|!super::super::dates::fact_keys(&block.text).is_empty());
    (facts_required, scope_required)
}

pub(super) fn validate(source: &SourceRevision, blocks: &[OutputBlock]) -> Result<(), CloudError> {
    let spans = effective_spans(source);
    if spans.is_empty() || blocks.is_empty() || spans.iter().any(|span| span.unresolved_correction) {
        return Err(reject("语义复核的来源或改口含义尚不明确"));
    }
    let (source_numbers, output_numbers) = numeric_sets(source, blocks, &spans);
    if source_numbers != output_numbers {
        return Err(reject("语义复核前发现明确数值、单位或版本变化"));
    }
    let source_kinds = kinds(spans.iter().map(|span| span.text.as_str()));
    let output_kinds = kinds(blocks.iter().map(|block| block.text.as_str()));
    let times = |values: &HashSet<Kind>| values.iter().copied()
        .filter(|kind| matches!(kind, Kind::Time(_))).collect::<HashSet<_>>();
    if times(&source_kinds) != times(&output_kinds) {
        return Err(reject("语义复核前发现明确时间变化"));
    }
    // All remaining finite anchor kinds protect negation, conditions, uncertainty,
    // ability questions or quantity-only limits. They need not remain verbatim.
    if source_kinds.iter().filter(|kind| !matches!(kind, Kind::Time(_)))
        .any(|expected| !output_kinds.iter().any(|actual| same_kind(*expected, *actual))) {
        return Err(reject("语义复核前发现否定、条件或不确定性类别消失"));
    }
    Ok(())
}

#[cfg(test)]
#[path = "rewrite_review_admission_tests.rs"]
mod tests;
