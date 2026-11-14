use super::*;
use crate::core::{DictionaryEntry, DictionarySnapshot, ProviderConfig, SourceItem};
use serde_json::{Value, json};

fn fixture(texts: &[&str]) -> (ModeSnapshot, SourceRevision) {
    let mode = ModeSnapshot { mode_id: "auto".into(), name: "自动".into(), base_mode: BaseMode::Auto, rules: String::new(), revision: 1 };
    let source = SourceRevision {
        id: "natural-source".into(), session_id: "natural-session".into(), asr_attempt_id: "natural-attempt".into(), generation: 1,
        provider: "synthetic".into(), model: "fixed-text".into(), adapter_version: "protocol-test-only".into(),
        complete: true, created_at: "2026-10-11T00:00:00Z".into(),
        items: texts.iter().enumerate().map(|(index, text)| SourceItem {
            id: format!("natural-attempt:{index}"), provider_item_id: index.to_string(), capture_order: index as u64,
            text: (*text).into(), is_final: true, start_ms: None, end_ms: None,
        }).collect(),
    };
    (mode, source)
}
fn wire(text: &str) -> String { json!({"resolvedMode":"light","text":text}).to_string() }

#[test]
fn document_cleanup_keeps_all_provenance_and_immutable_source() {
    let (mode, source) = fixture(&["嗯，这个页面的。", "呃，页面的字体需要统一。", "然后保留之前的配色。"]);
    let original = source.clone();
    let result = parse(&wire("这个页面的字体需要统一。保留之前的配色。"), &mode, &source).unwrap();
    assert_eq!(source, original);
    assert_eq!(result.blocks.len(), 1);
    assert_eq!(result.blocks[0].source_refs, source.items.iter().map(|item|item.id.clone()).collect::<Vec<_>>());
}

#[test]
fn whole_mapping_does_not_bypass_conditional_uncertainty_or_numeric_checks() {
    let (mode, source) = fixture(&["如果时间还没确认，先不要发邀请。", "预算是三千五百元。", "我们可能周五让同学试用。"]);
    assert!(parse(&wire("如果时间还没确认，先不要发邀请。预算是3500元。我们可能周五让同学试用。"), &mode, &source).is_ok());
    for bad in [
        "时间已经确认，可以发邀请。预算是3500元。我们可能周五让同学试用。",
        "如果时间还没确认，先不要发邀请。预算是5000元。我们可能周五让同学试用。",
        "如果时间还没确认，先不要发邀请。预算是3500元。我们周五让同学试用。",
    ] { assert_eq!(parse(&wire(bad), &mode, &source).unwrap_err().code, "rewrite_meaning_risk"); }
}

#[test]
fn ambiguous_correction_does_not_authorize_multiple_replacements() {
    let (mode, source) = fixture(&["小王周五来，小李也周五来。", "不对，改成周六。"]);
    assert_eq!(parse(&wire("小王周六来，小李也周六来。"), &mode, &source).unwrap_err().code, "rewrite_meaning_risk");
}

#[test]
fn only_declared_schema_is_accepted_and_empty_or_large_output_fails() {
    let (mode, source) = fixture(&["测试麦克风。"]);
    for content in [
        r#"{"resolvedMode":"light","blocks":[{"text":"测试麦克风。","sourceRange":[0,0]}]}"#.to_owned(),
        r#"{"resolvedMode":"light","text":"测试麦克风。","sourceRefs":["guess"]}"#.to_owned(),
        r#"{"resolvedMode":"light","text":"测试麦克风。","text":"测试"}"#.to_owned(),
        r#"{"resolvedMode":"auto","text":"测试麦克风。"}"#.to_owned(),
        wire("  "), wire(&"字".repeat(80_001)),
    ] { assert_eq!(parse(&content, &mode, &source).unwrap_err().code, "invalid_output"); }
    assert_eq!(parse("", &mode, &source).unwrap_err().code, "empty_output");
    assert_eq!(parse(&"x".repeat(1024 * 1024 + 1), &mode, &source).unwrap_err().code, "response_too_large");
}

#[test]
fn incomplete_or_duplicated_sources_cannot_gain_document_provenance() {
    let (mode, mut source) = fixture(&["测试麦克风。", "测试快捷键。"]);
    source.complete = false;
    assert_eq!(parse(&wire("测试麦克风。测试快捷键。"), &mode, &source).unwrap_err().code, "source_incomplete");
    source.complete = true; source.items[1].id = source.items[0].id.clone();
    assert_eq!(parse(&wire("测试麦克风。测试快捷键。"), &mode, &source).unwrap_err().code, "invalid_source");
}

#[test]
fn selected_modes_and_personal_rules_never_silently_enter_candidate() {
    let (mut mode, source) = fixture(&["测试麦克风。"]);
    for base in [BaseMode::Light, BaseMode::Structure] {
        mode.base_mode = base;
        assert_eq!(parse(&wire("测试麦克风。"), &mode, &source).unwrap_err().code, "invalid_mode");
        assert!(super::super::natural_request_body(&ProviderConfig::default(), &mode, &source, &DictionarySnapshot::default()).is_err());
    }
    mode.base_mode = BaseMode::Auto; mode.rules = "写一封邮件".into();
    assert_eq!(parse(&wire("测试麦克风。"), &mode, &source).unwrap_err().code, "invalid_mode");
}

#[test]
fn source_and_exact_case_hints_are_data_without_fuzzy_name_replacement() {
    let (mode, source) = fixture(&["用 typeless 和 github 看一下 Aria 的输出。"]);
    let dictionary = DictionarySnapshot { version: 1, entries: ["Typeless", "GitHub", "Arena"].iter().enumerate().map(|(index,term)|DictionaryEntry {
        id:index.to_string(), term:term.to_string(), created_at:"now".into(), updated_at:"now".into(),
    }).collect() };
    let body = super::super::natural_request_body(&ProviderConfig::default(), &mode, &source, &dictionary).unwrap();
    let data:Value = serde_json::from_str(body["messages"][1]["content"].as_str().unwrap()).unwrap();
    assert_eq!(data["sourceText"], source.text());
    assert_eq!(data["canonicalTerms"], json!(["Typeless", "GitHub"]));
    assert!(data.get("items").is_none()); assert!(body.get("tools").is_none());
}

#[test]
fn adding_candidate_does_not_change_default_request_protocol() {
    let (mode, source) = fixture(&["嗯，测试麦克风。"]);
    let config = ProviderConfig::default();
    assert_ne!(super::super::request_body(&config, &mode, &source).unwrap(), super::super::baseline_request_body(&config, &mode, &source).unwrap());
    assert_ne!(super::super::request_body(&config, &mode, &source).unwrap(), super::super::natural_request_body(&config, &mode, &source, &DictionarySnapshot::default()).unwrap());
}
