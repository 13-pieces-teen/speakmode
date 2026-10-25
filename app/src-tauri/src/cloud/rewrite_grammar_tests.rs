//! Finite grammar regressions; these prechecks do not replace semantic review.
use super::*;
use crate::core::SourceItem;

fn source(input: &str) -> SourceRevision {
    SourceRevision {
        id: "grammar-source".into(), session_id: "grammar-session".into(),
        asr_attempt_id: "attempt".into(), generation: 1, provider: "synthetic".into(),
        model: "fixed".into(), adapter_version: "test".into(), complete: true, created_at: "now".into(),
        items: vec![SourceItem { id: "item-0".into(), provider_item_id: "0".into(), capture_order: 0,
            text: input.into(), is_final: true, start_ms: None, end_ms: None }],
    }
}

fn admitted(input: &str, output: &str) -> bool {
    let source = source(input);
    review_admission(&source, &[OutputBlock { id: "candidate".into(), text: output.into(),
        source_refs: vec!["item-0".into()] }]).is_ok()
}

#[test]
fn person_classifier_spellings_keep_one_typed_value_and_source_range() {
    for phrase in ["两个人", "2个人", "两人", "2人"] {
        let input = format!("仪器演示由{phrase}值守。");
        let numeric = facts::extract(&input);
        assert_eq!(numeric.len(), 1, "{input}");
        assert_eq!(numeric[0].key, "person:2", "{input}");
        assert_eq!(&input[numeric[0].start..numeric[0].end], phrase, "{input}");
        assert!(admitted(&input, "仪器演示由2人值守。"), "{input}");
        assert!(admitted("仪器演示由两人值守。", &input), "{input}");
        assert!(!admitted(&input, "仪器演示由三人值守。"), "{input}");
        assert!(!admitted("仪器演示由三个人值守。", &input), "{input}");
    }
}

#[test]
fn person_classifier_does_not_promote_personal_prose_or_explicit_noun_compounds() {
    for input in ["这是个人观点。", "个人观点仅供参考。", "记录一段个人观点。"] {
        assert!(facts::extract(input).is_empty(), "{input}");
    }
    for (input, key) in [
        ("新增七个人工审核步骤。", "count:7"),
        ("展示1个人机交互示例。", "count:1"),
        ("比较四个人脸识别模型。", "count:4"),
        ("更换两个电池。", "count:2"),
    ] {
        let numeric = facts::extract(input);
        assert_eq!(numeric.len(), 1, "{input}");
        assert_eq!(numeric[0].key, key, "{input}");
    }
}

#[test]
fn explicit_degree_and_temperature_units_keep_distinct_dimensions() {
    for (phrase, key) in [
        ("四十二度", "degree:42"), ("42度", "degree:42"), ("42°", "degree:42"),
        ("三十二摄氏度", "temperature-c:32"), ("32℃", "temperature-c:32"), ("32°C", "temperature-c:32"),
        ("七十华氏度", "temperature-f:70"), ("70℉", "temperature-f:70"), ("70°F", "temperature-f:70"),
        ("三百开尔文", "temperature-k:300"), ("300开尔文", "temperature-k:300"), ("300 kelvin", "temperature-k:300"),
        ("三十二点五摄氏度", "temperature-c:32.5"), ("32.5℃", "temperature-c:32.5"),
    ] {
        let input = format!("读数为{phrase}。");
        let numeric = facts::extract(&input);
        assert_eq!(numeric.len(), 1, "{input}");
        assert_eq!(numeric[0].key, key, "{input}");
        assert_eq!(&input[numeric[0].start..numeric[0].end], phrase, "{input}");
    }
    assert_eq!(facts::extract("记录负七点二五摄氏度。")[0].key, "temperature-c:-7.25");
    assert_eq!(facts::extract("记录-7.25℃。")[0].key, "temperature-c:-7.25");
}

#[test]
fn bare_degree_requires_a_local_measurement_cue_and_is_not_an_occurrence_or_electricity() {
    for input in ["一度认为方案可行。", "三度提出改进建议。", "二度创作这幅作品。", "这个月用电最高四十二度电。", "记录四十二度。", "温度相关的讨论中一度认为方案可行。"] {
        assert!(facts::extract(input).iter().all(|fact| !fact.key.starts_with("degree:")), "{input}");
    }
    for input in ["外壳最高四十二度，内部没有测量。", "转台角度为四十二度。", "传感器读数约为42度。", "旋转42度。"] {
        let numeric = facts::extract(input);
        assert_eq!(numeric.len(), 1, "{input}");
        assert_eq!(numeric[0].key, "degree:42", "{input}");
    }
    assert!(admitted("外壳最高四十二度，内部没有测量。", "外壳最高42度，内部没有测量。"));
    assert!(!admitted("外壳最高四十二度，内部没有测量。", "外壳最高42℃，内部没有测量。"));
}

#[test]
fn degree_and_temperature_normalization_never_selects_or_converts_a_scale() {
    for (input, output) in [
        ("转台停在四十二度。", "转台停在42度。"),
        ("转台停在42度。", "转台停在42°。"),
        ("培养箱设为三十二摄氏度。", "培养箱设为32℃。"),
        ("培养箱设为32℃。", "培养箱设为32°C。"),
        ("传感器显示七十华氏度。", "传感器显示70℉。"),
        ("传感器显示70℉。", "传感器显示70°F。"),
        ("标准源为三百开尔文。", "标准源为300 kelvin。"),
        ("记录负七点二五摄氏度。", "记录-7.25℃。"),
    ] {
        assert!(admitted(input, output), "{input} -> {output}");
        assert!(admitted(output, input), "{output} -> {input}");
    }
    for (input, output) in [
        ("转台停在四十二度。", "转台停在42℃。"),
        ("培养箱设为三十二摄氏度。", "培养箱设为32℉。"),
        ("培养箱设为32℃。", "培养箱设为度数32。"),
        ("培养箱设为32℃。", "培养箱设为33℃。"),
        ("读数为零摄氏度。", "读数为32°F。"),
        ("读数为273.15 kelvin。", "读数为0℃。"),
        ("记录负七点二五摄氏度。", "记录7.25℃。"),
    ] {
        assert!(!admitted(input, output), "{input} -> {output}");
        assert!(!admitted(output, input), "{output} -> {input}");
    }
}

#[test]
fn new_temperature_symbols_do_not_consume_identifier_prefixes_or_ambiguous_k() {
    for input in ["使用8K视频。", "编号为300k。", "编号为300K2。", "编号为300K_id。", "编号为32°Cpu。", "编号为70°F_id。", "编号为32℃foo。", "编号为70℉_id。"] {
        assert!(facts::extract(input).iter().all(|fact|
            !fact.key.starts_with("temperature-") && !fact.key.starts_with("degree:")), "{input}");
    }
}

#[test]
fn typed_fact_aliases_cannot_change_exact_quoted_or_code_spelling() {
    use crate::{cloud::rewrite::{review_admission, RewriteResult}, core::BaseMode};
    for (input, changed) in [
        ("原句是‘两个人’。", "原句是‘2人’。"),
        ("标签为‘四十二度’。", "标签为‘42度’。"),
        ("配置为 `三十二摄氏度`。", "配置为 `32℃`。"),
    ] {
        let source = source(input);
        let draft = |text: &str| RewriteResult { resolved_mode: BaseMode::Light,
            blocks: vec![OutputBlock { id: "candidate".into(), text: text.into(), source_refs: vec!["item-0".into()] }] };
        assert!(review_admission(&source, &draft(input)).is_ok(), "{input}");
        assert_eq!(review_admission(&source, &draft(changed)).unwrap_err().code, "rewrite_literal_changed", "{input} -> {changed}");
    }
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
