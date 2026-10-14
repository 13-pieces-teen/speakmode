//! Synthetic mapping regressions for the 0.1.6 wire contract. No inference,
//! microphone input, credentials or external services are used by these tests.
use super::*;
use crate::core::{CreationReason, OutputRevision, RestorePatch, RevisionKind,
    SessionSeed, SourceItem, SourceSelection, StoragePolicy, Store};

fn fixture(texts: &[&str], base: BaseMode) -> (ModeSnapshot, SourceRevision) {
    let mode = ModeSnapshot { mode_id: format!("{base:?}"), name: "实验 Mode".into(),
        base_mode: base, rules: String::new(), revision: 1 };
    let source = SourceRevision {
        id: "frozen-source-span-fixture".into(), session_id: "span-session".into(),
        asr_attempt_id: "frozen-attempt-00000000-0000-0000-0000-000000000000".into(),
        generation: 7, provider: "aliyun".into(), model: "synthetic-asr".into(),
        adapter_version: "test-only".into(), complete: true,
        created_at: "2026-10-08T12:00:00+08:00".into(),
        items: texts.iter().enumerate().map(|(order, text)| SourceItem {
            id: format!("frozen-attempt-00000000-0000-0000-0000-000000000000:provider-fragment-{order}"),
            provider_item_id: format!("provider-fragment-{order}"), capture_order: order as u64,
            text: (*text).into(), is_final: true, start_ms: Some(order as u64 * 500),
            end_ms: Some((order as u64 + 1) * 500),
        }).collect(),
    };
    (mode, source)
}

fn budget_fixture(base: BaseMode) -> (ModeSnapshot, SourceRevision) {
    // Reproduce the ten-fragment shape observed in the fixed B audio without
    // importing experiment receipts or changing their original failure record.
    fixture(&["文。", "下周的开发安排。", "预算先五千。", "不对。",
        "是三千五百元。", "不是五千元。", "如果时间还没确认。", "先不要发邀请。",
        "我们可能周五让同学试用。", "但是时间还没确定。"], base)
}

fn budget_text() -> &'static str {
    "下周的开发安排中，预算是三千五百元，不是五千元。如果时间还没确认，先不要发邀请。我们可能周五让同学试用，但时间还没确定。"
}

fn wire(mode: BaseMode, blocks: Value) -> String {
    json!({"resolvedMode":mode,"blocks":blocks}).to_string()
}

#[test]
fn request_uses_local_numeric_indexes_and_never_exposes_stable_source_ids() {
    let (mode, source) = budget_fixture(BaseMode::Light);
    let body = request_body(&ProviderConfig::default(), &mode, &source).unwrap();
    let content = body["messages"][1]["content"].as_str().unwrap();
    let data: Value = serde_json::from_str(content).unwrap();
    assert_eq!(data["sourceCount"], 10);
    for (index, (item, original)) in data["items"].as_array().unwrap().iter().zip(&source.items).enumerate() {
        assert_eq!(item["index"], index);
        assert_eq!(item["text"], original.text);
        assert_eq!(item.as_object().unwrap().len(), 2);
        assert!(!content.contains(&original.id));
        assert!(!content.contains(&original.provider_item_id));
    }
    let system = body["messages"][0]["content"].as_str().unwrap();
    assert!(system.contains("sourceRange"));
}

#[test]
fn ten_fragment_cleanup_maps_removed_filler_and_superseded_budget_without_printing_them() {
    let (mode, source) = budget_fixture(BaseMode::Light);
    let before = source.clone();
    let result = parse_model_output(&wire(BaseMode::Light,
        json!([{"text":budget_text(),"sourceRange":[0,9]}])), &mode, &source).unwrap();
    assert_eq!(result.blocks.len(), 1);
    assert_eq!(result.blocks[0].text, budget_text());
    assert_eq!(result.blocks[0].source_refs,
        source.items.iter().map(|item| item.id.clone()).collect::<Vec<_>>());
    assert!(!result.blocks[0].text.contains("文。"));
    assert!(!result.blocks[0].text.contains("预算先五千"));
    assert_eq!(source, before);
}

#[test]
fn ten_fragment_structure_partition_expands_refs_in_frozen_source_order() {
    let (mode, source) = budget_fixture(BaseMode::Structure);
    let result = parse_model_output(&wire(BaseMode::Structure, json!([
        {"text":"下周的开发安排\n• 预算是三千五百元，不是五千元。","sourceRange":[0,5]},
        {"text":"• 如果时间还没确认，先不要发邀请。","sourceRange":[6,7]},
        {"text":"• 我们可能周五让同学试用，但是时间还没确定。","sourceRange":[8,9]}
    ])), &mode, &source).unwrap();
    for (block, indexes) in result.blocks.iter().zip([0..6, 6..8, 8..10]) {
        assert_eq!(block.source_refs, source.items[indexes].iter().map(|item| item.id.clone()).collect::<Vec<_>>());
    }
    assert_eq!(result.blocks.iter().map(|block|block.source_refs.len()).sum::<usize>(), 10);
}

#[test]
fn six_fragment_three_step_input_supports_single_or_multiple_blocks() {
    let (mut mode, source) = fixture(&["第一项。", "先检查麦克风。", "第二项。",
        "把测试结果保存到本地。", "最后一句是。", "不要删除原始录音。"], BaseMode::Light);
    let light = parse_model_output(&wire(BaseMode::Light, json!([
        {"text":"第一项：先检查麦克风。第二项：把测试结果保存到本地。最后一句：不要删除原始录音。","sourceRange":[0,5]}
    ])), &mode, &source).unwrap();
    assert_eq!(light.blocks[0].source_refs.len(), 6);
    mode.base_mode = BaseMode::Structure;
    let structured = parse_model_output(&wire(BaseMode::Structure, json!([
        {"text":"1. 先检查麦克风。","sourceRange":[0,1]},
        {"text":"2. 把测试结果保存到本地。","sourceRange":[2,3]},
        {"text":"3. 不要删除原始录音。","sourceRange":[4,5]}
    ])), &mode, &source).unwrap();
    assert_eq!(structured.blocks.len(), 3);
    for (index, block) in structured.blocks.iter().enumerate() {
        assert_eq!(block.source_refs, source.items[index*2..index*2+2].iter().map(|item|item.id.clone()).collect::<Vec<_>>());
    }
}

#[test]
fn first_middle_and_last_range_gaps_are_rejected_and_never_filled_after_generation() {
    let (mode, source) = budget_fixture(BaseMode::Light);
    for blocks in [
        json!([{"text":budget_text(),"sourceRange":[1,9]}]),
        json!([{"text":budget_text(),"sourceRange":[0,3]},{"text":"剩余信息。","sourceRange":[5,9]}]),
        json!([{"text":budget_text(),"sourceRange":[0,8]}]),
    ] {
        let error = parse_model_output(&wire(BaseMode::Light, blocks), &mode, &source).unwrap_err();
        assert_eq!(error.code, "incomplete_source_coverage");
        assert_eq!(error.stage, "rewrite");
    }
}

#[test]
fn overlapping_reversed_out_of_order_and_out_of_bounds_ranges_are_rejected() {
    let (mode, source) = budget_fixture(BaseMode::Light);
    for blocks in [
        json!([{"text":budget_text(),"sourceRange":[0,5]},{"text":"剩余信息。","sourceRange":[5,9]}]),
        json!([{"text":budget_text(),"sourceRange":[0,9]},{"text":"重复来源。","sourceRange":[0,0]}]),
        json!([{"text":budget_text(),"sourceRange":[3,2]}]),
        json!([{"text":budget_text(),"sourceRange":[0,10]}]),
        json!([{"text":budget_text(),"sourceRange":[10,10]}]),
    ] {
        assert_eq!(parse_model_output(&wire(BaseMode::Light, blocks), &mode, &source).unwrap_err().code,
            "invalid_source_mapping");
    }
}

#[test]
fn malformed_range_values_cannot_be_coerced_to_indexes() {
    let (mode, source) = budget_fixture(BaseMode::Light);
    for range in [json!([0]), json!([0,9,9]), json!([-1,9]), json!([0,9.0]),
        json!(["0",9]), json!([false,9]), json!(null), json!({"start":0,"end":9})] {
        assert!(parse_model_output(&wire(BaseMode::Light,
            json!([{"text":budget_text(),"sourceRange":range}])), &mode, &source).is_err());
    }
}

#[test]
fn mixed_legacy_and_span_mappings_are_rejected_even_when_union_covers_all_sources() {
    let (mode, source) = budget_fixture(BaseMode::Light);
    for blocks in [
        json!([{"text":budget_text(),"sourceRange":[0,8]},
            {"text":"最后来源。","sourceRefs":[source.items[9].id]}]),
        json!([{"text":budget_text(),"sourceRange":[0,9],
            "sourceRefs":source.items.iter().map(|item|item.id.clone()).collect::<Vec<_>>()}]),
    ] {
        assert!(parse_model_output(&wire(BaseMode::Light, blocks), &mode, &source).is_err());
    }
}

#[test]
fn complete_legacy_mapping_remains_readable_with_original_ids() {
    let (mode, source) = budget_fixture(BaseMode::Light);
    let refs = source.items.iter().map(|item|item.id.clone()).collect::<Vec<_>>();
    let result = parse_model_output(&wire(BaseMode::Light,
        json!([{"text":budget_text(),"sourceRefs":refs}])), &mode, &source).unwrap();
    assert_eq!(result.blocks[0].source_refs, refs);
}

#[test]
fn complete_span_mapping_cannot_hide_a_missing_today_qualifier() {
    let (mode, source) = fixture(&["今天用 SpeakMode 比较 Typeless 和 Superwhisper。",
        "音频采集用 WASAPI。", "重采样用 Rubato。"], BaseMode::Structure);
    let result = parse_model_output(&wire(BaseMode::Structure, json!([
        {"text":"• 用 SpeakMode 比较 Typeless 和 Superwhisper。\n• 音频采集用 WASAPI。\n• 重采样用 Rubato。","sourceRange":[0,2]}
    ])), &mode, &source);
    assert_eq!(result.unwrap_err().code, "rewrite_meaning_risk");
}

fn scalar_substring_range(text: &str, needle: &str) -> (usize, usize) {
    let byte_start = text.find(needle).unwrap();
    let start = text[..byte_start].chars().count();
    (start, start + needle.chars().count())
}

#[test]
fn coarse_span_block_still_allows_partial_restore_and_undo_without_changing_other_text() {
    let (_, source) = fixture(&["今天比较 SpeakMode 的轻整理和结构模式。",
        "如果时间没确认，先不要发邀请。🙂", "测试记录保存在本地，不删除原始识别稿。"], BaseMode::Structure);
    let mut store = Store::in_memory().unwrap();
    // Unlike parser-only fixtures, a persisted session requires an actual
    // saved Mode revision, including its canonical ID and display name.
    let mode = store.mode_snapshot("structure", None).unwrap();
    let result = parse_model_output(&wire(BaseMode::Structure, json!([
        {"text":"今天比较 SpeakMode 的轻整理和结构模式。\n• 如果时间没确认，先不要发邀请。🙂","sourceRange":[0,1]},
        {"text":"• 测试记录保存在本地，不删除原始识别稿。","sourceRange":[2,2]}
    ])), &mode, &source).unwrap();
    assert_eq!(result.blocks[0].source_refs.len(), 2);
    let seed = SessionSeed { id: source.session_id.clone(), generation: source.generation,
        started_at: "2026-10-08T11:59:55+08:00".into(), ended_at: source.created_at.clone(),
        capture_duration_ms: 5_000, end_local_date: "2026-10-08".into(), time_zone: "Asia/Shanghai".into(),
        storage_policy: StoragePolicy::History, mode_snapshot: mode.clone() };
    store.checkpoint_source(seed, source.clone()).unwrap();
    let initial = store.commit_ready(OutputRevision { confirmed_context:None, id: "initial-span-result".into(),
        session_id: source.session_id.clone(), parent_revision_id: None, base_revision_id: None,
        source_revision_id: source.id.clone(), mode_snapshot: mode, resolved_mode: result.resolved_mode,
        blocks: result.blocks, kind: RevisionKind::Initial, creation_reason: CreationReason::NormalInitial,
        created_at: source.created_at.clone(), rewrite_fallback: None }).unwrap().output.unwrap();
    let mut edited_blocks = initial.blocks.clone();
    edited_blocks[0].text = edited_blocks[0].text.replace("先不要发邀请", "先发邀请");
    let edited = store.edit_output(&source.session_id, &initial.id, edited_blocks, "edited").unwrap().output.unwrap();
    let (start, end) = scalar_substring_range(&edited.blocks[0].text, "先发邀请");
    let (source_start, source_end) = scalar_substring_range(&source.items[1].text, "先不要发邀请");
    let restored = store.restore_output(&source.session_id, &edited.id, RestorePatch {
        block_id: edited.blocks[0].id.clone(), start, end,
        source_selections: vec![SourceSelection { item_id: source.items[1].id.clone(),
            start: source_start, end: source_end }],
    }, "restored").unwrap();
    let restored_output = restored.output.unwrap();
    assert_eq!(restored_output.blocks, initial.blocks);
    assert_eq!(restored.source, source);
    assert_eq!(restored_output.kind, RevisionKind::Restore);
    let undone = store.undo_output(&source.session_id, &restored_output.id, "undo").unwrap();
    assert_eq!(undone.output.unwrap().blocks, edited.blocks);
    assert_eq!(undone.source, source);
    assert_eq!(store.usage_today("2026-10-08").unwrap().count, 1);
}
