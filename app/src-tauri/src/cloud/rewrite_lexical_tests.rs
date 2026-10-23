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

#[test]
fn finite_absence_forms_keep_status_but_never_create_a_generic_wu_anchor() {
    for phrase in ["暂无", "尚无", "无明确"] {
        let input=format!("Alpha{phrase}完成日期。");let found=anchors(&input);
        assert_eq!(found.len(),1,"{input}");assert_eq!(found[0].text,phrase,"{input}");
        assert_eq!(found[0].kind,Kind::NegativeStatus,"{input}");
        // The scoped guard still rejects changed objects and deleted status.
        assert!(check(&[&input],&input.replace("Alpha","Beta")).is_err(),"{input}");
        assert!(check(&[&input],"Alpha已有完成日期。").is_err(),"{input}");
    }
    for input in ["无线连接正常。", "无锡供应商返回结果。", "无疑已经完成。", "无穷多个取值。", "这是无损记录。"] {
        assert!(anchors(input).iter().all(|anchor|anchor.kind!=Kind::NegativeStatus),"{input}");
    }
    assert!(!PHRASES.iter().any(|(phrase,_)|*phrase=="无"));
    for excluded in ["并无", "仍无", "无确定", "无确切", "无具体"] {
        assert!(!PHRASES.iter().any(|(phrase,_)|*phrase==excluded));
    }
}

#[test]
fn inability_longest_matches_do_not_become_absent_statuses() {
    for phrase in ["暂无法", "尚无法"] {
        let input=format!("Alpha{phrase}提交申请。");let found=anchors(&input);
        assert_eq!(found.len(),1,"{input}");assert_eq!(found[0].text,phrase,"{input}");
        assert_eq!(found[0].kind,Kind::Negative,"{input}");
        assert!(check(&[&input],"Alpha无法提交申请。").is_ok(),"{input}");
        assert!(check(&[&input],"Alpha提交申请。").is_err(),"{input}");
        assert!(!same_kind(Kind::NegativeStatus,found[0].kind));
    }
    for input in ["仍无法提交申请。", "并无法提交申请。"] {
        let found=anchors(input);assert_eq!(found.len(),1,"{input}");
        assert_eq!(found[0].text,"无法");assert_eq!(found[0].kind,Kind::Negative);
    }
}

#[test]
fn erfei_contrast_is_protected_but_the_specious_idiom_is_not_a_false_contrast() {
    assert!(anchors("这个说法似是而非。").is_empty());
    assert!(check(&["这个说法似是而非。"],"这个说法似乎有理但不可靠。").is_ok());
    for (input,expected) in [
        ("这个说法似是而非，不要直接采用。",Kind::Prohibition),
        ("不要给出似是而非的解释。",Kind::Prohibition),
        ("这个说法并非似是而非。",Kind::NegativeFact),
        ("这个说法似是而非，而非已经验证。",Kind::NegativeFact),
    ] {
        let found=anchors(input);assert_eq!(found.len(),1,"{input}");assert_eq!(found[0].kind,expected,"{input}");
    }
    assert!(check(&["不是提供审批结果。"],"而非提供审批结果。").is_ok());
    assert!(check(&["而非提供审批结果。"],"提供审批结果。").is_err());
    assert!(check(&["而非提供审批结果。"],"而非提交合同。").is_err());
}

#[test]
fn complete_conjunctions_do_not_turn_feichang_into_a_false_erfei_contrast() {
    for (input,output) in [
        ("然而非常顺利。","但很顺利。"),
        ("反而非常清楚。","却很清楚。"),
        ("因而非常满意。","因此很满意。"),
        ("从而非常稳定。","由此变得很稳定。"),
        ("进而非常好。","随后变得很好。"),
    ] {
        assert!(anchors(input).is_empty(),"{input}");
        assert!(check(&[input],output).is_ok(),"{input} -> {output}");
    }
    // There is deliberately no broad 而非常/非常 exclusion. This unambiguous
    // contrast must remain protected; plain 而非常顺利 stays a finite ambiguity.
    let contrast=anchors("采用异常流程，而非常规流程。");
    assert_eq!(contrast.len(),1);assert_eq!(contrast[0].text,"而非");
    assert_eq!(contrast[0].kind,Kind::NegativeFact);
    assert!(check(&["采用异常流程，而非常规流程。"],"采用异常流程和常规流程。").is_err());
}

#[test]
fn conjunction_boundaries_preserve_adjacent_actual_negation_and_later_contrasts() {
    for (input,kind,output) in [
        ("然而不是最终结果。",Kind::NegativeFact,"但这是最终结果。"),
        ("反而无法继续处理。",Kind::Negative,"却可以继续处理。"),
        ("因而并非最终结果。",Kind::NegativeFact,"因此这是最终结果。"),
        ("从而不能继续处理。",Kind::Negative,"由此可以继续处理。"),
        ("进而不会提供结果。",Kind::Negative,"随后提供结果。"),
        ("然而这是更新进度，而非提供审批结果。",Kind::NegativeFact,"但这是更新进度，也提供审批结果。"),
    ] {
        let found=anchors(input);assert_eq!(found.len(),1,"{input}");assert_eq!(found[0].kind,kind,"{input}");
        assert!(check(&[input],output).is_err(),"real negation must remain: {input}");
    }
}

#[test]
fn observed_absence_spellings_respect_finite_complete_word_boundaries() {
    for input in ["请合并无线网络配置。", "归并无效规则。", "兼并无线设备业务。", "吞并无线设备业务。",
        "她品格高尚无私。", "穿搭时尚无比。", "我崇尚无为而治。", "这种风尚无比自然。", "这次停顿短暂无比。"] {
        assert!(anchors(input).is_empty(),"complete-word boundary: {input}");
    }
    for (input,output) in [
        ("请合并无线网络配置。","请把无线网络配置合并。"),
        ("请归并无效规则。","请把无效规则归并。"),
        ("公司准备兼并无线设备业务。","公司准备将无线设备业务兼并。"),
        ("集团吞并无线设备业务。","无线设备业务被集团吞并。"),
        ("她品格高尚无私。","她的品格崇高而无私。"),
        ("穿搭时尚无比。","穿搭十分时髦。"),
        ("我崇尚无为而治。","我提倡顺其自然的治理。"),
        ("这种风尚无比自然。","这种风气十分自然。"),
        ("这次停顿短暂无比。","这次停顿十分短暂。"),
    ] {assert!(check(&[input],output).is_ok(),"{input} -> {output}");}
}

#[test]
fn actual_absence_or_inability_after_complete_words_still_cannot_disappear() {
    for (input,kind,output) in [
        ("请合并配置，但尚无备份。",Kind::NegativeStatus,"请合并配置，有备份。"),
        ("归并后尚无结果。",Kind::NegativeStatus,"归并后已有结果。"),
        ("兼并暂无进展。",Kind::NegativeStatus,"兼并已有进展。"),
        ("吞并仍无法完成。",Kind::Negative,"吞并可以完成。"),
        ("想法高尚，但尚无证据。",Kind::NegativeStatus,"想法高尚，已有证据。"),
        ("穿搭时尚，但尚无成片。",Kind::NegativeStatus,"穿搭时尚，已有成片。"),
        ("大家崇尚自然，但尚无定论。",Kind::NegativeStatus,"大家崇尚自然，已有定论。"),
        ("这种风尚流行，但尚无样本。",Kind::NegativeStatus,"这种风尚流行，已有样本。"),
        ("这种风尚无法改变。",Kind::Negative,"这种风尚可以改变。"),
        ("停顿短暂，但暂无结果。",Kind::NegativeStatus,"停顿短暂，已有结果。"),
        ("合并未完成。",Kind::NegativeStatus,"合并已经完成。"),
        ("品格高尚未改变。",Kind::NegativeStatus,"品格高尚已经改变。"),
    ] {
        let found=anchors(input);assert_eq!(found.len(),1,"{input}");assert_eq!(found[0].kind,kind,"{input}");
        assert!(check(&[input],output).is_err(),"actual qualifier must remain: {input}");
    }
    for input in ["暂无明确完成日期。", "尚无明确完成日期。", "无明确完成日期。"] {
        let found=anchors(input);assert_eq!(found.len(),1,"{input}");assert_eq!(found[0].kind,Kind::NegativeStatus);
        assert!(check(&[input],"已有明确完成日期。").is_err(),"{input}");
    }
}
