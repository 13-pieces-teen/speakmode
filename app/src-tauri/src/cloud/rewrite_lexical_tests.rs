//! Finite lexical regressions from a synthetic-text cloud receipt. These tests
//! replay text only: no new inference, microphone, credentials or user history.
use super::*;
use crate::core::SourceItem;

fn source(texts: &[&str]) -> SourceRevision {
    SourceRevision {
        id: "lexical-source".into(), session_id: "lexical-session".into(),
        asr_attempt_id: "lexical-attempt".into(), generation: 1,
        provider: "synthetic".into(), model: "fixed-text".into(),
        adapter_version: "lexical-guard-regression".into(), complete: true,
        created_at: "2026-10-10T00:00:00Z".into(),
        items: texts.iter().enumerate().map(|(index, text)| SourceItem {
            id: format!("lexical-item-{index}"), provider_item_id: index.to_string(),
            capture_order: index as u64, text: (*text).into(), is_final: true,
            start_ms: None, end_ms: None,
        }).collect(),
    }
}

fn check(input: &[&str], output: &str) -> Result<(), CloudError> {
    let source = source(input);
    let block = OutputBlock {
        id: "lexical-output".into(), text: output.into(),
        source_refs: source.items.iter().map(|item| item.id.clone()).collect(),
    };
    validate_meaning(&source, &[block])
}

#[test]
fn lexical_bie_is_not_a_prohibition_or_a_preservation_hint() {
    for input in ["看一下两者的区别。", "比较两种模式的差别。", "区别和差别是两个词。"] {
        assert!(anchors(input).iter().all(|anchor| anchor.kind != Kind::Prohibition), "{input}");
        assert!(!preservation_hints(&source(&[input])).iter().any(|hint| hint == "别"), "{input}");
    }
    assert!(check(&["看一下两者的区别。"], "看看两者的不同。").is_ok());
    assert!(check(&["比较两种模式的差别。"], "比较两种模式的差异。").is_ok());
}

#[test]
fn an_adjacent_real_negation_is_not_consumed_with_the_lexical_word() {
    for (input, protected) in [
        ("别区别对待用户。", "别"),
        ("不要区别对待用户。", "不要"),
        ("这个区别不影响录音。", "不"),
        ("别忽略差别。", "别"),
    ] {
        let actual = anchors(input);
        assert!(actual.iter().any(|anchor| anchor.text == protected), "{input}");
        assert_eq!(actual.iter().filter(|anchor| anchor.text == "别").count(), usize::from(protected == "别"), "{input}");
    }
}

#[test]
fn deleting_real_prohibitions_still_fails_next_to_difference_words() {
    for (input, output) in [
        ("请区别这两种模式，别删除录音。", "请区别这两种模式，删除录音。"),
        ("这两种模式存在差别，不要删除原稿。", "这两种模式存在差别，可以删除原稿。"),
        ("不要区别对待用户。", "区别对待用户。"),
        ("这个区别不影响录音。", "这个区别影响录音。"),
    ] {
        assert_eq!(check(&[input], output).unwrap_err().code, "rewrite_meaning_risk", "{input} => {output}");
    }
}

#[test]
fn lexical_fix_does_not_allow_a_prohibition_to_move_to_another_action() {
    assert!(check(
        &["请说明差别，别删除录音。打开设置。"],
        "请说明差别，删除录音。别打开设置。",
    ).is_err());
    assert!(check(
        &["请说明区别，不要删除原稿。整理词典。"],
        "请说明区别，删除原稿。不要整理词典。",
    ).is_err());
}

#[test]
fn lexical_fix_does_not_weaken_quantity_or_owner_matching() {
    let input = "这两种模式有区别，预算3500元，运费500元。";
    assert!(check(&[input], "这两种模式有差别，预算3500元，运费500元。").is_ok());
    assert!(check(&[input], "这两种模式有差别，预算500元，运费3500元。").is_err());
    assert!(check(&[input], "这两种模式有差别，预算3500元。").is_err());
}

#[test]
fn p01_literal_cloud_outputs_no_longer_report_a_false_prohibition() {
    // Captured in automatic-dictation-0.2.10/dev-cloud-r1.json, P01.
    // A numeric-prefix-only fallback now also recognizes the finite neutral
    // collective reprise. It still requires the stated count in each output.
    let input = "你好啊，就是呃刚刚同样的一段话，我现在用这个 typeless 还有这个呃我们的 swing mode 一起用，然后嗯看一下两者它的一个区别。";
    assert!(anchors(input).is_empty());
    for output in [
        "你好，就是刚刚那段话，我现在用这个 typeless 和我们的 swing mode 一起用，然后看一下两者的一个区别。",
        "你好，就是刚刚同样的一段话，我现在用这个 Typeless 和我们的 Swing Mode 一起用，然后看一下两者的一个区别。",
    ] {
        assert!(check(&[input], output).is_ok());
    }
}

#[test]
fn pure_pause_removal_does_not_create_a_numeric_scope_requirement() {
    for pause in ["嗯", "呃", "啊"] {
        let input = format!("然后{pause}看一下差别。");
        assert!(check(&[&input], "然后看一下差别。").is_ok());
        let input = format!("给小王{pause}留三个，给小李留五个。");
        assert!(check(&[&input], "给小王留三个，给小李留五个。").is_ok());
        assert!(check(&[&input], "给小王留五个，给小李留三个。").is_err());
    }
}

#[test]
fn numeric_scope_pause_fallback_requires_nonempty_full_local_equality() {
    let matches = |source: &str, candidate: &str| numeric_scope_matches(source, &fingerprints(source), candidate, &fingerprints(candidate));
    assert!(!scope_matches(&fingerprints("嗯看"), &fingerprints("看")));
    assert!(matches("嗯看", "看"));
    assert!(matches("呃看", "看"));
    assert!(matches("啊看", "看"));
    assert!(!matches("嗯看", "发"));
    assert!(!matches("嗯呃", ""));
    assert!(!matches("给小王嗯留", "给小李留"));
    // Isolate this fallback from the pre-existing fuzzy matching path.
    assert!(!scope_matches(&fingerprints("王嗯留"), &fingerprints("王送")));
    assert!(!matches("王嗯留", "王送"));
}

#[test]
fn numeric_pause_fallback_still_requires_value_unit_and_polarity() {
    let input = "嗯看一个区别。";
    assert!(check(&[input], "看一个区别。").is_ok());
    assert!(check(&[input], "看两个区别。").is_err());
    assert!(check(&[input], "看一次区别。").is_err());
    assert!(check(&["嗯预算不是3500元。"], "预算是3500元。").is_err());
}

#[test]
fn exact_collective_reprise_is_a_finite_numeric_prefix_rule() {
    for owner in ["二者", "两者", "双方"] {
        for pronoun in ["它", "它们"] {
            let repeated = format!("然后嗯看一下{owner}{pronoun}的");
            let plain = format!("然后看一下{owner}的");
            assert!(exact_numeric_scope_matches(&repeated, &plain));
            assert!(exact_numeric_scope_matches(&plain, &repeated));
        }
    }
    for (source, output) in [
        ("它的", "她的"),
        ("他的", "它的"),
        ("两者他的", "两者的"),
        ("两者她的", "两者的"),
        ("两者它", "两者"),
        ("两者它们", "两者"),
        ("两者它的确不同", "两者的确不同"),
        ("双方它的", "甲方的"),
        ("双方它们的", "乙方的"),
        ("老板它的", "老板的"),
        ("看一下两者它的", "删除两者的"),
        ("给小王嗯留", "给小李留"),
    ] {
        assert!(!exact_numeric_scope_matches(source, output), "{source} => {output}");
    }
}

#[test]
fn p01_v2_candidate_keeps_quantity_but_the_baseline_drops_it() {
    // Exact recorded responses from dev-cloud-r2-flash.json. Replaying this
    // response proves only parser behavior, not a new model or native run.
    let input = "你好啊，就是呃刚刚同样的一段话，我现在用这个 typeless 还有这个呃我们的 swing mode 一起用，然后嗯看一下两者它的一个区别。";
    let candidate = "你好，就是刚刚那段话，我现在用这个 Typeless 和我们的 Swing Mode 一起用，然后看一下两者的一个区别。";
    let baseline = "你好，就是刚刚那段话，我现在用 typeless 和我们的 swing mode 一起测试，然后看一下两者之间的区别。";
    assert!(check(&[input], candidate).is_ok());
    assert_eq!(check(&[input], baseline).unwrap_err().code, "rewrite_meaning_risk");
    assert!(check(&[input], &candidate.replace("一个区别", "两个区别")).is_err());
    assert!(check(&[input], &candidate.replace("一个区别", "一次区别")).is_err());
    assert!(check(&[input], &candidate.replace("两者的一个区别", "甲方的一个区别")).is_err());
}

#[test]
fn d03_explicit_prefixed_correction_no_longer_requires_the_retired_value() {
    let input = [
        "帮我记一下，明天给会议室订三个麦克风，啊不对，是五个，前面三个是我说错了。",
        "下午两点前送到就行。",
    ];
    let source = source(&input);
    let spans = effective_spans(&source);
    assert!(!spans.iter().any(|span| span.text == "啊不对"));
    assert!(!spans.iter().any(|span| span.text == "前面三个是我说错了"));
    assert!(spans.iter().any(|span| span.text.contains("订五个麦克风")));
    assert!(anchors("啊不对").iter().any(|anchor| anchor.kind == Kind::Negative));
    for output in [
        "帮我记一下，明天给会议室订五个麦克风，下午两点前送到就行。",
        "明天给会议室订五个麦克风，下午两点前送到就行。",
    ] {
        assert!(check(&input, output).is_ok());
    }
}
