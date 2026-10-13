use super::*;
use crate::core::SourceItem;

fn source(texts: &[&str]) -> SourceRevision {
    SourceRevision { id: "admission-source".into(), session_id: "admission-session".into(),
        asr_attempt_id: "attempt".into(), generation: 1, provider: "synthetic".into(),
        model: "fixed".into(), adapter_version: "test".into(), complete: true, created_at: "now".into(),
        items: texts.iter().enumerate().map(|(index, text)| SourceItem {
            id: format!("item-{index}"), provider_item_id: index.to_string(), capture_order: index as u64,
            text: (*text).into(), is_final: true, start_ms: None, end_ms: None,
        }).collect() }
}

fn admitted(texts: &[&str], output: &str) -> bool {
    let source = source(texts);
    let original = source.clone();
    let blocks = vec![OutputBlock { id: "candidate".into(), text: output.into(),
        source_refs: source.items.iter().map(|item| item.id.clone()).collect() }];
    let before = blocks.clone();
    let result = super::super::review_admission(&source, &blocks);
    assert_eq!(source, original);
    assert_eq!(blocks, before);
    if let Err(error) = &result { assert_eq!(error.code, "rewrite_meaning_risk"); }
    result.is_ok()
}

#[test]
fn review_admission_allows_cleanup_and_exact_unit_normalization() {
    assert!(admitted(&["嗯，测试一个这样的一个工具的运行形式吧。"], "测试工具的运行形式。"));
    assert!(admitted(&["预算三千元，等待一分钟，版本 v0.2.15，明天上午九点发布。"],
        "明天上午9点发布 v0.2.15；预算3000元，等待60秒。"));
    assert!(admitted(&["可能需要等待，如果确认就发，先不要删除。"], "如果确认就发，可能需要等待，不删除。"));
}

#[test]
fn review_admission_rejects_explicit_value_unit_version_or_time_changes() {
    for (input, output) in [
        ("预算3000元。", "预算3500元。"),
        ("等待3秒。", "等待3分钟。"),
        ("版本 v0.2.15。", "版本 v0.2.16。"),
        ("下午两点开会。", "下午三点开会。"),
        ("明天发布。", "今天发布。"),
        ("三人参加。", "参加。"),
        ("安排讨论。", "安排讨论，预算500元。"),
    ] { assert!(!admitted(&[input], output), "{input} -> {output}"); }
}

#[test]
fn review_admission_rejects_whole_qualifier_category_disappearance() {
    for (input, output) in [
        ("不要发送邀请。", "发送邀请。"),
        ("这不是定稿。", "这是定稿。"),
        ("如果学校同意，再进行测试。", "进行测试。"),
        ("只有确认了才发出。", "发出。"),
        ("除非确认，否则不要发出。", "不要发出。"),
        ("可能延后。", "延后。"),
        ("地点尚未确定。", "地点确定。"),
        ("测试能不能达到预期。", "测试达到预期。"),
    ] { assert!(!admitted(&[input], output), "{input} -> {output}"); }
}

#[test]
fn review_admission_uses_effective_repairs_and_rejects_ambiguous_repairs() {
    let input = ["预算3000元。", "我说错了，是3500元。"];
    assert!(admitted(&input, "预算3500元。"));
    assert!(!admitted(&input, "预算3000元。"));
    assert!(admitted(&["今天，不对，明天开会。"], "明天开会。"));
    assert!(!admitted(&["会议周二，展会周四。", "我说错了，是周三。"], "会议周三，展会周四。"));
}

#[test]
fn review_admission_is_not_semantic_acceptance_or_owner_proof() {
    // These deliberately pass this precheck and MUST still go to the reviewer.
    assert!(admitted(&["甲预算3000元，乙预算5000元。"], "甲预算5000元，乙预算3000元。"));
    assert!(admitted(&["做一个工具。"], "做两个工具。"));
    assert!(admitted(&["请不要删除甲文件，也不要发送乙文件。"], "删除甲文件，不要发送乙文件。"));
    assert!(admitted(&["我觉得运行不够流畅。"], "运行不够流畅。"));
}

#[test]
fn review_evidence_requires_known_dimensions_from_either_side() {
    for (input, output, expected) in [
        ("测试麦克风。", "测试麦克风。", (false, false)),
        ("做一个工具。", "做工具。", (true, false)),
        ("安排讨论。", "预算3000元。", (true, false)),
        ("明天发布。", "发布。", (true, false)),
        ("请不要删除文件。", "删除文件。", (false, true)),
        ("进行测试。", "如果允许，可能测试。", (false, true)),
    ] {
        let source = source(&[input]);
        let blocks = vec![OutputBlock { id: "candidate".into(), text: output.into(), source_refs: vec!["item-0".into()] }];
        assert_eq!(super::super::review_required_evidence(&source, &blocks), expected, "{input} -> {output}");
    }
}
