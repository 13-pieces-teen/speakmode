use super::*;
use crate::core::SourceItem;

fn source(texts: &[&str]) -> SourceRevision {
    SourceRevision { id: "pause-source".into(), session_id: "pause-session".into(),
        asr_attempt_id: "pause-attempt".into(), generation: 1, provider: "synthetic".into(),
        model: "synthetic".into(), adapter_version: "test".into(), complete: true,
        created_at: "2026-10-10T00:00:00Z".into(),
        items: texts.iter().enumerate().map(|(index, text)| SourceItem {
            id: format!("item-{index}"), provider_item_id: format!("item-{index}"),
            capture_order: index as u64, text: (*text).into(), is_final: true,
            start_ms: None, end_ms: None,
        }).collect() }
}

fn accepted(texts: &[&str], output: &str) -> bool {
    let source = source(texts);
    let block = OutputBlock { id: "output".into(), text: output.into(),
        source_refs: source.items.iter().map(|item| item.id.clone()).collect() };
    super::super::validate_meaning(&source, &[block]).is_ok()
}

const ORIGINAL: &str = "明天给会议室订三个麦克风，啊不对，是五个，前面三个是我说错了。下午两点前送到就行。";
const CLEAN: &str = "明天给会议室订五个麦克风，下午两点前送到就行。";

#[test]
fn explicit_pause_repair_and_exact_retired_value_confirmation_are_accepted() {
    assert!(accepted(&[ORIGINAL], CLEAN));
    for pause in ["啊不对", "嗯不对", "呃我说错了", "嗯呃啊更正一下"] {
        let input = ORIGINAL.replace("啊不对", pause);
        assert!(accepted(&[&input], CLEAN), "{pause}");
    }
    assert!(accepted(&["明天给会议室订三个麦克风。", "啊不对。", "是五个。",
        "前面三个是我说错了。", "下午两点前送到就行。"], CLEAN));
}

#[test]
fn correction_analysis_preserves_every_original_byte_range_without_editing_source() {
    let original = source(&["明天给会议室订三个麦克风，", "啊不对，", "是五个，",
        "前面三个是我说错了。", "下午两点前送到就行。"]);
    let before = original.clone();
    let spans = analysis_spans(&original);
    let repaired = spans.iter().find(|span| span.text == "明天给会议室订五个麦克风").unwrap();
    assert_eq!(repaired.origins.len(), 4);
    assert!(!spans.iter().any(|span| span.text.contains("前面三个")));
    for origin in &repaired.origins {
        let item = original.items.iter().find(|item| item.id == origin.source_id).unwrap();
        assert!(origin.start < origin.end && origin.end <= item.text.len());
        assert!(!item.text[origin.start..origin.end].is_empty());
    }
    assert_eq!(original, before);
    let incomplete = OutputBlock { id: "out".into(), text: CLEAN.into(),
        source_refs: vec!["item-0".into(), "item-1".into(), "item-2".into(), "item-4".into()] };
    assert!(!repaired.is_cited_by(&incomplete), "confirmation origin may not be guessed or omitted");
}

#[test]
fn removed_time_or_wrong_final_quantity_still_fail() {
    assert!(!accepted(&[ORIGINAL], "明天给会议室订五个麦克风。"));
    assert!(!accepted(&[ORIGINAL], "明天给会议室订四个麦克风，下午两点前送到就行。"));
    assert!(!accepted(&[ORIGINAL], "明天给会议室订三个麦克风，下午两点前送到就行。"));
}

#[test]
fn real_quantity_change_history_is_not_collapsed_to_the_final_count() {
    let input = "原来给会议室订三个麦克风，后来又加订两个，目前一共五个，调整过程也要保留。";
    assert!(accepted(&[input], input));
    assert!(!accepted(&[input], "给会议室订五个麦克风。"));
    let history_then_repair = "原来给会议室订三个麦克风，啊不对，是五个，前面三个是我说错了。";
    assert!(!analysis_spans(&source(&[history_then_repair])).iter().any(|span| span.text == "原来给会议室订五个麦克风"));
}

#[test]
fn wrong_old_value_or_wrong_unit_confirmation_is_not_consumed() {
    for confirmation in ["前面两个是我说错了", "前面三人是我说错了", "前面三元是我说错了",
        "前面的数字是我说错了", "前面三个可能是我说错了"] {
        let input = ORIGINAL.replace("前面三个是我说错了", confirmation);
        let source = source(&[&input]);
        assert!(analysis_spans(&source).iter().any(|span| span.text == confirmation), "{confirmation}");
        if ["前面两个是我说错了", "前面三人是我说错了", "前面三元是我说错了"].contains(&confirmation) {
            assert!(!accepted(&[&input], CLEAN), "different numeric value/unit must still be checked: {confirmation}");
        }
    }
    assert!(!accepted(&[&ORIGINAL.replace("是五个", "是五人")], CLEAN));
}

#[test]
fn an_intervening_clause_expires_the_retired_value_receipt() {
    for gap in ["嗯，", "另外门不要关，", "下午两点前送到，"] {
        let input = ORIGINAL.replace("前面三个", &format!("{gap}前面三个"));
        assert!(analysis_spans(&source(&[&input])).iter().any(|span| span.text == "前面三个是我说错了"), "{gap}");
    }
}

#[test]
fn bare_repair_does_not_choose_between_multiple_quantity_owners() {
    let input = "会议室订三个麦克风，教室订两个麦克风，啊不对，是五个，前面两个是我说错了。";
    let input_source = source(&[input]);
    let spans = analysis_spans(&input_source);
    assert!(spans.iter().any(|span| span.unresolved_correction));
    assert!(!accepted(&[input], "会议室订三个麦克风，教室订五个麦克风。"));
    assert!(!accepted(&[input], "会议室订五个麦克风，教室订两个麦克风。"));
}

#[test]
fn quoted_markers_and_negative_descriptions_are_not_corrections() {
    for suffix in ["“啊不对，是五个”", "他说“啊不对，是五个”", "他说：\n“示例，啊不对，是五个”",
        "啊不对的情况很多，是五个", "不是啊不对，是五个"] {
        let input = format!("会议室订三个麦克风，{suffix}。");
        assert!(!analysis_spans(&source(&[&input])).iter().any(|span| span.text == "会议室订五个麦克风"), "{suffix}");
    }
    let input = "会议室订三个麦克风，啊不对，是五个，“前面三个是我说错了”。";
    assert!(analysis_spans(&source(&[input])).iter().any(|span| span.text.contains("前面三个")));
}

#[test]
fn a_partial_repair_inside_a_quote_neither_changes_prior_value_nor_loses_quoted_text() {
    for quote in ["“示例，把三个改成五个，然后复述”", "'example，把三个改成五个，then repeat'",
        "`example，把三个改成五个，then repeat`"] {
        let input = source(&[&format!("会议室订三个麦克风，{quote}。")]);
        let spans = analysis_spans(&input);
        assert!(spans.iter().any(|span| span.text == "会议室订三个麦克风"));
        assert!(spans.iter().any(|span| span.text == "把三个改成五个"));
        assert!(!spans.iter().any(|span| span.text == "会议室订五个麦克风"));
    }
}

#[test]
fn english_apostrophes_do_not_hide_a_later_explicit_repair() {
    for english in ["don't worry", "don’t worry", "users' feedback", "'don't worry'"] {
        let input = source(&[&format!("{english}。{ORIGINAL}")]);
        let spans = analysis_spans(&input);
        assert!(spans.iter().any(|span| span.text == "明天给会议室订五个麦克风"), "{english}");
        assert!(!spans.iter().any(|span| span.text == "前面三个是我说错了"), "{english}");
    }
}

#[test]
fn explicit_day_period_whole_hours_cannot_be_dropped_or_changed() {
    for period in ["上午", "下午"] {
        let input = format!("{period}两点前送到就行。");
        assert_eq!(facts::extract(&input)[0].key, "clock:2:00");
        assert!(accepted(&[&input], &format!("{period}2点前送到就行。")));
        assert!(!accepted(&[&input], &format!("{period}送到就行。")));
        assert!(!accepted(&[&input], &format!("{period}三点前送到就行。")));
    }
    assert_eq!(facts::extract("两点整出发。")[0].key, "clock:2:00");
}

#[test]
fn chinese_decimal_amounts_and_indefinite_prose_are_not_whole_hours() {
    assert_eq!(facts::extract("预算两点五元。")[0].key, "cny:2.5");
    assert!(accepted(&["预算两点五元。"], "预算2.5元。"));
    for text in ["我有一点意见。", "提出一点整体意见。", "下午需要一点意见。", "这一点十分自然。"] {
        assert!(!facts::extract(text).iter().any(|fact| fact.key.starts_with("clock:")), "{text}");
    }
    assert!(!facts::extract("下午两点五元。").iter().any(|fact| fact.key.starts_with("clock:")));
}
