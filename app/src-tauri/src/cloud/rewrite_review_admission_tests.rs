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
fn explicit_unitless_amounts_cannot_change_even_before_an_all_pass_reviewer() {
    for (input,good,bad) in [
        ("预算三千，暂时不发布。","预算3000，暂时不发布。","预算999，暂时不发布。"),
        ("金额是三千。","金额是3000。","金额是999。"),
        ("价格为3000。","价格为3000。","价格为999。"),
        ("The budget is 3000.","The budget is 3000.","The budget is 999."),
        ("The amount is 3000.","The amount is 3000.","The amount is 999."),
    ] {
        assert!(admitted(&[input],good),"{input} -> {good}");assert!(!admitted(&[input],bad),"{input} -> {bad}");
    }
    assert!(!admitted(&["预算三千，暂时不发布。"],"暂时不发布。"));
    assert!(!admitted(&["暂时不发布。"],"预算999，暂时不发布。"));
}

#[test]
fn unitless_amount_checks_resolve_only_existing_explicit_correction_scope() {
    for (input,good,bad) in [
        ("预算3000，不对，是3500。","预算3500。","预算3000。"),
        ("预算3000，不对，是3500元。","预算3500元。","预算3000元。"),
        ("价格三千，我说错了，是3500。","价格3500。","价格3000。"),
    ] {
        assert!(admitted(&[input],good),"{input} -> {good}");assert!(!admitted(&[input],bad),"{input} -> {bad}");
    }
    assert!(!admitted(&["甲预算3000，乙预算5000，我说错了，是3500。"],"甲预算3000，乙预算3500。"));
    // Generic scalars/counts deliberately remain outside this finite admission
    // check. Its pass is still not a semantic equivalence or owner proof.
    assert!(admitted(&["这个编号3000。"],"这个编号999。"));
    assert!(admitted(&["做一个工具。"],"做两个工具。"));
}

#[test]
fn english_measurements_after_words_keep_value_and_unit_checks() {
    for (source, changed) in [
        ("The budget is 30 dollars.", "The budget is 40 dollars."),
        ("We wait 3 seconds.", "We wait 3 minutes."),
        ("The limit is\n30元.", "The limit is\n40元."),
    ] { assert!(!admitted(&[source],changed),"{source} -> {changed}"); }
    assert!(admitted(&["We wait 60 seconds."],"We wait 1 minute."));
    assert!(admitted(&["The budget is 30 dollars."],"The budget is 30 USD."));
    let keys:Vec<_>=facts::extract("Qwen3.8, v0.2.1, budget 30 dollars.").into_iter().map(|fact|fact.key).collect();
    assert_eq!(keys,["version:0.2.1","usd:30"]);
}

#[test]
fn spoken_english_measurements_normalize_without_allowing_value_changes() {
    for (raw, good, bad) in [
        ("I have two years of experience.", "I have 2 years of experience.", "I have 3 years of experience."),
        ("The budget is twenty seven hundred dollars.", "The budget is 2700 USD.", "The budget is 1700 USD."),
        ("Wait for sixty seconds.", "Wait for 1 minute.", "Wait for 2 minutes.")
    ] {
        assert!(admitted(&[raw], good), "{raw} => {good}");
        assert!(!admitted(&[raw], bad), "{raw} => {bad}");
    }
}

#[test]
fn interrupted_english_year_cannot_pass_as_a_shorter_year_and_orphan_suffix() {
    let input=["I expect to graduate in twenty thirty.", "Two. I am still learning."];
    assert!(admitted(&input,"I expect to graduate in 2032. I am still learning."));
    assert!(admitted(&input,"I expect to graduate in twenty thirty two. I am still learning."));
    assert!(!admitted(&input,"I expect to graduate in 2030.\nTwo. I am still learning."));
    assert!(!admitted(&input,"I expect to graduate in 2033. I am still learning."));
    assert!(admitted(&["I graduated in two thousand and twenty seven."],"I graduated in 2027."));
    assert!(admitted(&["I graduated in twenty twenty.\nTwo teams joined."],"I graduated in 2020. Two teams joined."));
    assert!(admitted(&["I graduated in 2027, sorry, in 2028."],"I graduated in 2028."));
}

#[test]
fn unrelated_correction_words_do_not_waive_source_year_preservation() {
    use crate::{cloud::rewrite::{review_admission, RewriteResult}, core::BaseMode};
    for aside in ["I actually prefer a short summary.", "Sorry, I lost my place."] {
        let source = source(&[&format!("The archive opened in twenty forty one. {aside}")]);
        let original = source.clone();
        let draft = |text: String| RewriteResult { resolved_mode: BaseMode::Light,
            blocks: vec![OutputBlock { id: "candidate".into(), text, source_refs: vec!["item-0".into()] }] };
        assert!(review_admission(&source, &draft(format!("The archive opened in 2041. {aside}"))).is_ok(), "{aside}");
        for changed in [format!("The archive opened in 2042. {aside}"), format!("The archive opened. {aside}")] {
            let result = review_admission(&source, &draft(changed.clone()));
            assert!(result.is_err(), "unrelated aside must not waive the year: {changed}");
            assert_eq!(result.unwrap_err().code, "rewrite_meaning_risk");
        }
        assert_eq!(source, original);
    }
}

#[test]
fn only_a_direct_year_correction_supersedes_the_abandoned_year() {
    use crate::{cloud::rewrite::{review_admission, RewriteResult}, core::BaseMode};
    for (input, output, expected) in [
        ("The archive opened in twenty forty one, sorry, in twenty forty two.", "The archive opened in 2042.", true),
        ("The archive opened in twenty forty one, sorry, in twenty forty two.", "The archive opened in 2041.", false),
        ("The archive opened in twenty forty one. Sorry, the newsletter started in twenty forty two.", "The archive opened in 2041. The newsletter started in 2042.", true),
        ("The archive opened in twenty forty one. Sorry, the newsletter started in twenty forty two.", "The archive opened. The newsletter started in 2042.", false),
        ("The archive opened in 2041, actually, in 2042. The newsletter started in 2041.", "The archive opened in 2042. The newsletter started in 2041.", true),
        ("The archive opened in 2041, actually, in 2042. The newsletter started in 2041.", "The archive opened in 2042. The newsletter started.", false),
    ] {
        let source = source(&[input]);
        let draft = RewriteResult { resolved_mode: BaseMode::Light, blocks: vec![OutputBlock {
            id: "candidate".into(), text: output.into(), source_refs: vec!["item-0".into()],
        }] };
        assert_eq!(review_admission(&source, &draft).is_ok(), expected, "{input} -> {output}");
    }
}

#[test]
fn year_first_followup_after_sentence_boundary_is_not_a_correction() {
    use crate::{cloud::rewrite::{review_admission, RewriteResult}, core::BaseMode};
    let source = source(&["The archive opened in 2041. Sorry. In 2042 the newsletter started."]);
    let draft = |text: &str| RewriteResult { resolved_mode: BaseMode::Light,
        blocks: vec![OutputBlock { id: "candidate".into(), text: text.into(), source_refs: vec!["item-0".into()] }] };
    assert!(review_admission(&source, &draft("The archive opened in 2041. In 2042 the newsletter started.")).is_ok());
    let result = review_admission(&source, &draft("The archive opened. In 2042 the newsletter started."));
    assert!(result.is_err(), "a new sentence's year must not erase the previous sentence's year");
    assert_eq!(result.unwrap_err().code, "rewrite_meaning_risk");
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
