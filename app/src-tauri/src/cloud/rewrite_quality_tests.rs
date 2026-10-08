//! Frozen, synthetic parser challenges. They test deterministic guard behavior,
//! not live model quality or semantic equivalence of arbitrary dictation.
use super::*;
use crate::core::SourceItem;

fn fixture(texts: &[&str], base: BaseMode) -> (ModeSnapshot, SourceRevision) {
    (ModeSnapshot { mode_id: "quality-fixture".into(), name: "质量实验".into(), base_mode: base,
        rules: String::new(), revision: 1 },
    SourceRevision { id: "quality-source".into(), session_id: "quality-session".into(),
        asr_attempt_id: "synthetic".into(), generation: 1, provider: "synthetic".into(),
        model: "fixed-text".into(), adapter_version: "quality-v1".into(), complete: true,
        created_at: "2026-10-08T00:00:00Z".into(),
        items: texts.iter().enumerate().map(|(i, text)| SourceItem {
            id: format!("synthetic:{i}"), provider_item_id: i.to_string(), capture_order: i as u64,
            text: (*text).into(), is_final: true, start_ms: None, end_ms: None }).collect() })
}

fn parse(texts: &[&str], output: &str) -> Result<RewriteResult, CloudError> {
    let (mode, source) = fixture(texts, BaseMode::Structure);
    let content = json!({"resolvedMode":"structure", "blocks":[{"text":output,
        "sourceRange":[0,texts.len()-1]}]}).to_string();
    parse_model_output(&content, &mode, &source)
}

#[test]
fn invented_amounts_and_versions_are_rejected_even_when_original_facts_are_retained() {
    for output in ["预算3500元。税款500元。", "预算3500元。升级到版本3.2。", "预算3500元。预算3500元，已付3500元。"] {
        assert_eq!(parse(&["预算三千五百元。"], output).unwrap_err().code, "rewrite_meaning_risk", "{output}");
    }
}

#[test]
fn an_existing_condition_or_uncertainty_cannot_disappear_during_regrouping() {
    for (input,output) in [("如果预算批准，发送邀请。", "预算已经批准，发送邀请。"),
        ("可能发送邀请。", "发送邀请。")] {
        assert_eq!(parse(&[input], output).unwrap_err().code, "rewrite_meaning_risk", "{output}");
    }
}

#[test]
fn regrouping_topics_inside_one_source_range_preserves_source_identity() {
    let texts = ["预算五千，不对，三千五百元。", "可能周五开会。", "刚才的预算只算设备，差旅另算。", "如果会议没确认，先不要买票。"];
    let output = "• 设备预算：3500元，差旅另算。\n• 可能周五开会。\n• 如果会议没确认，先不要买票。";
    let result = parse(&texts, output).unwrap();
    assert_eq!(result.blocks[0].text, output);
    assert_eq!(result.blocks[0].source_refs, ["synthetic:0", "synthetic:1", "synthetic:2", "synthetic:3"]);
}

#[test]
fn regrouping_never_authorizes_disjoint_or_reordered_source_ranges() {
    let (mode, source) = fixture(&["讨论预算。", "检查麦克风。", "补充预算。"], BaseMode::Structure);
    let content = json!({"resolvedMode":"structure", "blocks":[
        {"text":"预算讨论与补充。", "sourceRange":[0,2]},
        {"text":"检查麦克风。", "sourceRange":[1,1]}]}).to_string();
    assert_eq!(parse_model_output(&content, &mode, &source).unwrap_err().code, "invalid_source_mapping");
}

#[test]
fn scoped_regrouping_cannot_move_or_remove_a_condition() {
    assert_eq!(parse(&["如果会议没确认，先不要买票。预算3500元。"],
        "• 会议已经确认，可以买票。\n• 如果预算3500元，先不要发邀请。").unwrap_err().code, "rewrite_meaning_risk");
}

#[test]
fn numeric_conversion_and_layout_numbers_remain_acceptable() {
    assert!(parse(&["预算三点五万元。", "等待三秒。", "降低百分之三。"],
        "1. 预算35000元。\n2. 等待3000毫秒。\n3. 降低3%。").is_ok());
}

#[test]
fn ordinary_unparsed_conditional_and_uncertain_paraphrases_are_not_rejected_as_additions() {
    assert!(parse(&["估计他们会发送邀请。"], "他们可能会发送邀请。").is_ok());
    assert!(parse(&["只要审核通过，就发送邀请。"], "如果审核通过，就发送邀请。").is_ok());
    assert!(parse(&["审核通过后再发送邀请。"], "如果审核通过，就发送邀请。").is_ok());
}

#[test]
fn implicit_spoken_conditions_may_be_written_as_explicit_conditions() {
    assert!(parse(&["等确认了再发邀请。"], "如果确认了，再发邀请。").is_ok());
    assert!(parse(&["看情况再发邀请。"], "如果情况允许，再发邀请。").is_ok());
}

#[test]
fn an_adjacent_explicit_equivalent_unit_form_is_not_an_invented_fact() {
    assert!(parse(&["等待3秒。"], "等待3秒（即3000毫秒）。").is_ok());
    assert!(parse(&["预算3500元。"], "预算3500元（即0.35万元）。").is_ok());
    assert!(parse(&["等待3秒。"], "等待3秒 (3000毫秒)。").is_ok());
    assert!(parse(&["预算不是3500元。"], "预算不是3500元（即不是0.35万元）。").is_ok());
}

#[test]
fn parenthetical_different_values_or_other_subjects_are_still_rejected() {
    for output in ["预算3500元（即0.5万元）。", "预算3500元（即税款3500元）。",
        "预算3500元（不是3500元）。", "预算3500元。即3500元。", "预算3500元（即可能3500元）。"] {
        assert_eq!(parse(&["预算3500元。"], output).unwrap_err().code, "rewrite_meaning_risk", "{output}");
    }
}

#[test]
fn partial_correction_does_not_reintroduce_the_retracted_fact() {
    let input = ["预算5000元，地点不要改。", "把5000元改成3500元。"];
    assert!(parse(&input, "预算3500元，地点不要改。").is_ok());
    assert_eq!(parse(&input, "预算3500元，地点不要改。已付5000元。").unwrap_err().code, "rewrite_meaning_risk");
}

#[test]
fn questions_and_quoted_product_words_remain_literal_data() {
    let (mut mode, source) = fixture(&["把按钮命名为“不要发送”。为什么 Typeless 会改写这句话？", "用户问：如果预算没确认怎么办？"], BaseMode::Structure);
    mode.rules = "保持问句和引用，不回答问题。".into();
    let body = request_body(&ProviderConfig::default(), &mode, &source).unwrap();
    let data: Value = serde_json::from_str(body["messages"][1]["content"].as_str().unwrap()).unwrap();
    assert_eq!(data["items"][0]["text"], source.items[0].text);
    assert_eq!(data["items"][1]["text"], source.items[1].text);
    assert!(body.get("tools").is_none());
    assert!(parse(&source.items.iter().map(|item|item.text.as_str()).collect::<Vec<_>>(),
        "• 按钮命名为“不要发送”。\n• 为什么 Typeless 会改写这句话？\n• 用户问：如果预算没确认怎么办？").is_ok());
}

#[test]
fn production_request_contains_no_worked_example_facts_to_leak_into_other_dictation() {
    for base in [BaseMode::Light,BaseMode::Structure,BaseMode::Auto] {
        let (mode,source)=fixture(&["为什么按钮显示错误？"],base);
        let body=request_body(&ProviderConfig::default(),&mode,&source).unwrap();
        let system=body["messages"][0]["content"].as_str().unwrap();
        // Actual first-run contamination: an unrelated correction inherited
        // equipment/travel content from a system-prompt worked example.
        for leaked in ["设备预算","差旅另算","三千五百元","3500元","SpeakMode","Typeless","Superwhisper"] {
            assert!(!system.contains(leaked),"worked-example fact remained in system: {leaked}");
        }
        let data: Value=serde_json::from_str(body["messages"][1]["content"].as_str().unwrap()).unwrap();
        assert_eq!(data["items"][0]["text"],"为什么按钮显示错误？");
    }
}

#[test]
fn actual_synthetic_r3_response_preserves_prohibition_with_zanwu() {
    let fixture: Value=serde_json::from_str(include_str!("../../tests/fixtures/rewrite_prohibition_synonym_r3.json")).unwrap();
    let source: SourceRevision=serde_json::from_value(fixture["source"].clone()).unwrap();
    let (mode,_)=self::fixture(&["unused"],BaseMode::Structure);
    let content=fixture["modelContent"].as_str().unwrap();
    assert!(parse_model_output(content,&mode,&source).is_ok(),"actual r3 output uses 暂勿, preserving the source prohibition");
    let dropped=content.replace("暂勿","");
    assert_eq!(parse_model_output(&dropped,&mode,&source).unwrap_err().code,"rewrite_meaning_risk");
}
