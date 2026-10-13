//! No provider calls: protocol/provenance and meaningful safety regressions.
use super::*;
use crate::core::SourceItem;
use serde_json::json;

fn fixture(texts: &[&str]) -> (ModeSnapshot, SourceRevision) {
    let mode = ModeSnapshot {
        mode_id: "auto".into(), name: "自动".into(), base_mode: BaseMode::Auto,
        rules: String::new(), revision: 1,
    };
    let source = SourceRevision {
        id: "automatic-contract-source".into(), session_id: "automatic-contract-session".into(),
        asr_attempt_id: "automatic-contract-attempt".into(), generation: 1,
        provider: "synthetic".into(), model: "fixed-text".into(),
        adapter_version: "protocol-test-only".into(), complete: true,
        created_at: "2026-10-10T00:00:00Z".into(),
        items: texts.iter().enumerate().map(|(index, text)| SourceItem {
            id: format!("automatic-contract-attempt:{index}"),
            provider_item_id: index.to_string(), capture_order: index as u64,
            text: (*text).into(), is_final: true, start_ms: None, end_ms: None,
        }).collect(),
    };
    (mode, source)
}

fn wire(mode: BaseMode, text: &str) -> String {
    json!({"resolvedMode":mode,"text":text}).to_string()
}

#[test]
fn explicit_whole_document_keeps_every_source_id_and_immutable_source() {
    let (mode, source) = fixture(&["嗯，今天测试麦克风。", "呃，明天测试快捷键。"]);
    let original = source.clone();
    let text = "今天测试麦克风。\n\n明天测试快捷键。";
    let result = parse(&wire(BaseMode::Light, text), &mode, &source).unwrap();
    assert_eq!(result.blocks.len(), 1);
    assert_eq!(result.blocks[0].text, text);
    assert_eq!(result.blocks[0].source_refs, source.items.iter().map(|i| i.id.clone()).collect::<Vec<_>>());
    assert_eq!(source, original);
}

#[test]
fn explicit_whole_document_structure_has_one_provenance_block() {
    let (mode, source) = fixture(&["先测试麦克风。", "然后测试快捷键。"]);
    let text = "• 先测试麦克风。\n• 然后测试快捷键。";
    let result = parse(&wire(BaseMode::Structure, text), &mode, &source).unwrap();
    assert_eq!(result.resolved_mode, BaseMode::Structure);
    assert_eq!(result.blocks.len(), 1);
    assert_eq!(result.blocks[0].text, text);
    assert_eq!(result.blocks[0].source_refs.len(), 2);
}

#[test]
fn old_mapping_schema_and_extra_fields_are_not_repaired() {
    let (mode, source) = fixture(&["测试麦克风。"]);
    for content in [
        r#"{"resolvedMode":"light","blocks":[{"text":"测试麦克风。","sourceRange":[0,0]}]}"#,
        r#"{"resolvedMode":"light","text":"测试麦克风。","sourceRange":[0,0]}"#,
        r#"{"resolvedMode":"light","text":"测试麦克风。","sourceRefs":["guessed"]}"#,
        r#"{"resolvedMode":"light","text":"测试麦克风。","reason":"quality"}"#,
        r#"{"resolvedMode":"light","text":"测试麦克风。","text":"改写"}"#,
        r#"{"resolvedMode":"light","text":null}"#,
        r#"{"resolvedMode":"light","text":["测试麦克风。"]}"#,
        r#"{"text":"测试麦克风。"}"#,
        r#"{"resolvedMode":"unknown","text":"测试麦克风。"}"#,
        "```json\n{\"resolvedMode\":\"light\",\"text\":\"测试麦克风。\"}\n```",
    ] {
        assert_eq!(parse(content, &mode, &source).unwrap_err().code, "invalid_output", "{content}");
    }
}

#[test]
fn unsupported_mode_and_personal_rules_cannot_silently_use_this_contract() {
    let (mut mode, source) = fixture(&["测试麦克风。"]);
    let content = wire(BaseMode::Light, "测试麦克风。");
    mode.base_mode = BaseMode::Light;
    assert_eq!(parse(&content, &mode, &source).unwrap_err().code, "invalid_mode");
    mode.base_mode = BaseMode::Structure;
    assert_eq!(parse(&content, &mode, &source).unwrap_err().code, "invalid_mode");
    mode.base_mode = BaseMode::Auto;
    mode.rules = "使用邮件格式".into();
    assert_eq!(parse(&content, &mode, &source).unwrap_err().code, "invalid_mode");
}

#[test]
fn empty_unresolved_or_oversized_output_is_rejected() {
    let (mode, source) = fixture(&["测试麦克风。"]);
    assert_eq!(parse(" \n ", &mode, &source).unwrap_err().code, "empty_output");
    for content in [wire(BaseMode::Light, " \n "), wire(BaseMode::Auto, "测试麦克风。"), wire(BaseMode::Light, &"字".repeat(80_001))] {
        assert_eq!(parse(&content, &mode, &source).unwrap_err().code, "invalid_output");
    }
    assert_eq!(parse(&"x".repeat(1024 * 1024 + 1), &mode, &source).unwrap_err().code, "response_too_large");
}

#[test]
fn incomplete_or_invalid_source_cannot_receive_document_provenance() {
    let (mode, original) = fixture(&["测试麦克风。", "测试快捷键。"]);
    let content = wire(BaseMode::Light, "测试麦克风。测试快捷键。");
    let mut source = original.clone();
    source.complete = false;
    assert_eq!(parse(&content, &mode, &source).unwrap_err().code, "source_incomplete");
    source = original.clone(); source.items[1].is_final = false;
    assert_eq!(parse(&content, &mode, &source).unwrap_err().code, "source_incomplete");
    source = original.clone(); source.items[1].id = source.items[0].id.clone();
    assert_eq!(parse(&content, &mode, &source).unwrap_err().code, "invalid_source");
    source = original; source.items[1].capture_order = 0;
    assert_eq!(parse(&content, &mode, &source).unwrap_err().code, "invalid_source");
}

#[test]
fn qualifier_and_numeric_guards_are_not_bypassed_by_whole_source_mapping() {
    let (mode, source) = fixture(&["如果时间还没确认，先不要发邀请。", "预算是三千五百元。"]);
    let good = "如果时间还没确认，先不要发邀请。预算是3500元。";
    assert!(parse(&wire(BaseMode::Light, good), &mode, &source).is_ok());
    for bad in [
        "时间已经确认，可以发邀请。预算是3500元。",
        "如果时间还没确认，先不要发邀请。预算是5000元。",
        "如果时间还没确认，先不要发邀请。预算是3500元，另加运费500元。",
    ] {
        assert_eq!(parse(&wire(BaseMode::Light, bad), &mode, &source).unwrap_err().code, "rewrite_meaning_risk", "{bad}");
    }
}

#[test]
fn ambiguous_target_correction_is_not_automatically_approved() {
    let (mode, source) = fixture(&["小王周五来，小李也周五来。", "不对，改成周六。"]);
    let bad = "小王周六来，小李也周六来。";
    assert_eq!(parse(&wire(BaseMode::Light, bad), &mode, &source).unwrap_err().code, "rewrite_meaning_risk");
}
