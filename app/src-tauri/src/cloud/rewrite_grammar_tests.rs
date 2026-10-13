//! Finite grammar regressions; these prechecks do not replace semantic review.
use super::*;
use crate::core::SourceItem;

fn admitted(input: &str, output: &str) -> bool {
    let source = SourceRevision {
        id: "grammar-source".into(), session_id: "grammar-session".into(),
        asr_attempt_id: "attempt".into(), generation: 1, provider: "synthetic".into(),
        model: "fixed".into(), adapter_version: "test".into(), complete: true, created_at: "now".into(),
        items: vec![SourceItem { id: "item-0".into(), provider_item_id: "0".into(), capture_order: 0,
            text: input.into(), is_final: true, start_ms: None, end_ms: None }],
    };
    review_admission(&source, &[OutputBlock { id: "candidate".into(), text: output.into(),
        source_refs: vec!["item-0".into()] }]).is_ok()
}

#[test]
fn per_occurrence_spellings_keep_one_frequency_fact() {
    for phrase in ["每一次", "每1次", "每次"] {
        let input = format!("{phrase}请求都检查证书。");
        let numeric = facts::extract(&input);
        assert_eq!(numeric.len(), 1, "{input}");
        assert_eq!(numeric[0].key, "frequency:eachTime");
        assert_eq!(&input[numeric[0].start..numeric[0].end], phrase);
        assert!(admitted(&input, "每次请求都检查证书。"));
        assert!(admitted("每次请求都检查证书。", &input));
    }
    assert!(admitted("不需要每一次请求都检查证书。", "不需要每次请求都检查证书。"));
    assert_eq!(facts::extract("每一次检查三人。").iter().map(|fact| fact.key.as_str()).collect::<Vec<_>>(),
        ["frequency:eachTime", "person:3"]);
}

#[test]
fn per_occurrence_normalization_does_not_erase_counts_or_frequency() {
    for (input, output) in [
        ("每次请求都检查证书。", "请求时检查证书。"),
        ("每一次请求都检查证书。", "检查一次证书。"),
        ("每次请求都检查证书。", "每两次请求检查证书。"),
        ("每两次请求检查证书。", "每次请求检查证书。"),
        ("仅检查一次证书。", "每次检查证书。"),
        ("检查一次证书。", "检查两次证书。"),
        ("每次检查三人。", "每次检查四人。"),
        ("不需要每次检查证书。", "每次检查证书。"),
    ] { assert!(!admitted(input, output), "{input} -> {output}"); }
    assert_eq!(facts::extract("每两次请求检查证书。")[0].key, "times:2");
    assert_eq!(facts::extract("仅检查一次证书。")[0].key, "times:1");
}

#[test]
fn polar_question_markers_do_not_assert_their_internal_negative_syllables() {
    for input in ["检查有没有被替换。", "检查是不是被替换。", "检查是否被替换。", "检查是否有没有被替换。"] {
        let found = anchors(input);
        assert!(found.iter().any(|anchor| anchor.kind == Kind::PolarQuestion), "{input}");
        assert!(found.iter().all(|anchor| anchor.kind == Kind::PolarQuestion), "{input}");
        assert!(admitted(input, "检查是否被替换。"), "{input}");
        assert!(!admitted(input, "检查后确认被替换。"), "must preserve question: {input}");
    }
}

#[test]
fn explicit_negation_inside_or_outside_questions_remains_protected() {
    for input in ["检查是否没有被替换。", "检查是不是没有被替换。", "检查有没有尚未被替换的实例。"] {
        let found = anchors(input);
        assert!(found.iter().any(|anchor| anchor.kind == Kind::PolarQuestion), "{input}");
        assert!(found.iter().any(|anchor| anchor.kind == Kind::NegativeStatus), "{input}");
        assert!(!admitted(input, "检查是否被替换。"), "negative must survive: {input}");
    }
    for (input, output) in [
        ("没有被替换。", "被替换。"),
        ("并不是定稿。", "这是定稿。"),
        ("检查是否不是定稿。", "检查是否是定稿。"),
        ("如果没有通过，不要开放入口。", "如果通过，开放入口。"),
    ] { assert!(!admitted(input, output), "{input} -> {output}"); }
}

#[test]
fn longer_ability_questions_still_win_and_do_not_become_plain_questions() {
    for input in ["检查是不是能够恢复。", "检查是不是可以恢复。", "检查是否能够恢复。", "检查是否能恢复。", "检查能不能恢复。"] {
        let found = anchors(input);
        assert_eq!(found.len(), 1, "{input}");
        assert_eq!(found[0].kind, Kind::AbilityQuestion, "{input}");
        assert!(admitted(input, "检查能否恢复。"), "{input}");
        assert!(!admitted(input, "检查是否恢复。"), "must preserve ability: {input}");
    }
}
