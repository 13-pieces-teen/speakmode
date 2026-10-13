use super::*;
use crate::core::SourceItem;

fn source(texts: &[&str]) -> SourceRevision {
    SourceRevision { id: "source".into(), session_id: "session".into(), asr_attempt_id: "attempt".into(), generation: 1, provider: "aliyun".into(), model: "candidate".into(), adapter_version: "v1".into(), complete: true, created_at: "now".into(), items: texts.iter().enumerate().map(|(index, text)| SourceItem { id: format!("item-{index}"), provider_item_id: format!("{index}"), capture_order: index as u64, text: (*text).into(), is_final: true, start_ms: None, end_ms: None }).collect() }
}
fn block(text: &str, refs: &[usize]) -> OutputBlock { OutputBlock { id: "output".into(), text: text.into(), source_refs: refs.iter().map(|index| format!("item-{index}")).collect() } }
fn accepted(input: &[&str], output: &str) -> bool { validate_meaning(&source(input), &[block(output, &(0..input.len()).collect::<Vec<_>>())]).is_ok() }

#[test]
fn source_item_boundary_cannot_detach_not_saying_from_its_action() {
    let input = ["讲座十一点开始，不是", "说十一点签到，签到是十点半。"];
    assert!(accepted(&input, "讲座十一点开始，不是说十一点签到，签到是十点半。"));
    assert!(!accepted(&input, "讲座十一点开始不是。说十一点签到，签到是十点半。"));
    assert!(!accepted(&input, "讲座十一点开始不是，说十一点签到，签到是十点半。"));
    assert!(!accepted(&input, "讲座十一点开始，说十一点签到，签到是十点半。"));
}

#[test]
fn original_experiment_fragment_does_not_pass_by_copying_broken_boundary() {
    let input = ["那个演示从九点半开始不是", "说九点半签到签到还是九点"];
    assert!(accepted(&input, "演示从九点半开始，不是说九点半签到，签到还是九点。"));
    assert!(!accepted(&input, "那个演示从九点半开始不是。说九点半签到，签到还是九点。"));
}

#[test]
fn joined_analysis_requires_both_source_references() {
    let input = source(&["演示从九点半开始不是", "说九点半签到，签到还是九点。"]);
    assert!(validate_meaning(&input, &[block("演示从九点半开始。", &[0]), block("不是说九点半签到，签到还是九点。", &[1])]).is_err());
    assert!(validate_meaning(&input, &[block("演示从九点半开始，不是说九点半签到，签到还是九点。", &[0, 1])]).is_ok());
}

#[test]
fn source_analysis_keeps_exact_byte_ranges_and_never_mutates_items() {
    let input = source(&["演示从九点半开始不是", "说九点半签到，签到还是九点。"]);
    let before = input.items.iter().map(|item| item.text.clone()).collect::<Vec<_>>();
    let view = effective_spans(&input);
    let joined = view.iter().find(|span| span.text.starts_with("不是说")).unwrap();
    assert_eq!(joined.origins.len(), 2);
    assert_eq!(&input.items[0].text[joined.origins[0].start..joined.origins[0].end], "不是");
    assert_eq!(&input.items[1].text[joined.origins[1].start..joined.origins[1].end], "说九点半签到");
    assert_eq!(before, input.items.iter().map(|item| item.text.clone()).collect::<Vec<_>>());
}

#[test]
fn explicit_punctuation_is_not_silently_repaired_in_source_analysis() {
    let input = source(&["演示从九点半开始不是。", "说九点半签到。"]);
    assert!(!effective_spans(&input).iter().any(|span| span.text.starts_with("不是说")));
    let input = source(&["演示从九点半开始不是\n", "说九点半签到。"]);
    assert!(!effective_spans(&input).iter().any(|span| span.text.starts_with("不是说")));
    let input = source(&["演示从九点半开始不是", "。", "说九点半签到。"]);
    assert!(!effective_spans(&input).iter().any(|span| span.text.starts_with("不是说")));
}

#[test]
fn date_repair_does_not_retract_intervening_location_or_restriction() {
    let input = ["预约周二下午见面。", "地点在南门咖啡厅，地点不要改。", "我说错了，是周三下午。"];
    assert!(accepted(&input, "预约周三下午在南门咖啡厅见面，地点不要改。"));
    assert!(!accepted(&input, "预约周二下午在南门咖啡厅见面，地点不要改。"));
    assert!(!accepted(&input, "预约周三下午在南门咖啡厅见面，地点可以改。"));
}

#[test]
fn compound_weekday_correction_retains_week_and_day() {
    let input = ["把预约改到下周二上午十点，地点是东楼204。", "我说错了，是下周三上午十点，不是改成下午。"];
    assert!(accepted(&input, "预约下周三上午十点，地点是东楼204，不是改成下午。"));
    assert!(!accepted(&input, "预约下周二上午十点，地点是东楼204，不是改成下午。"));
    assert!(!accepted(&input, "预约本周三上午十点，地点是东楼204，不是改成下午。"));
    assert!(accepted(&["下周二检查麦克风。"], "下星期二检查麦克风。"));
    assert!(!accepted(&["下周二检查麦克风。"], "这周二检查麦克风。"));
    assert!(!accepted(&["上个星期二检查麦克风。"], "上个星期三检查麦克风。"));
}

#[test]
fn ambiguous_date_correction_does_not_guess_the_most_recent_event() {
    let input = ["会议周二，展会周四。", "我说错了，是周三。"];
    let hints = preservation_hints(&source(&input));
    assert!(hints.iter().any(|hint| hint == "周二"));
    assert!(hints.iter().any(|hint| hint == "周四"));
    assert!(!accepted(&input, "会议周二，展会周三。"));
}

#[test]
fn explicit_new_subject_does_not_retract_only_old_date() {
    let input = ["会议周二。", "我说错了，展会周三。"];
    assert!(!accepted(&input, "会议周三，展会周三。"));
    assert!(preservation_hints(&source(&input)).iter().any(|hint| hint == "周二"));
}

#[test]
fn adjacent_bare_date_can_be_completed_by_an_explicit_correction() {
    assert!(accepted(&["今天，不对，明天开会。"], "明天开会。"));
    assert!(accepted(&["今天。", "嗯。", "不对。", "明天开会。"], "明天开会。"));
    assert!(!accepted(&["今天。", "地点先不要改。", "我说错了，明天开会。"], "明天开会，地点先不要改。"));
    assert!(!accepted(&["今天面试。", "我说错了，明天开会。"], "明天面试，明天开会。"));
}

#[test]
fn explicit_old_value_with_multiple_targets_is_not_guessed() {
    let input = ["会议周二，展会周二。", "把周二改成周三。"];
    assert!(!accepted(&input, "会议周二，展会周三。"));
    assert!(!accepted(&input, "会议周三，展会周二。"));
    let input = ["甲客户周二交付，乙客户周二交付。", "把周二改成周三。"];
    assert!(!accepted(&input, "甲客户周三交付，乙客户周二交付。"));
    assert!(!accepted(&input, "甲客户周二交付，乙客户周三交付。"));
    assert!(effective_spans(&source(&input)).iter().any(|span| span.unresolved_correction));
    assert!(accepted(&input, &input.join("")));
    assert!(!accepted(&input, "甲客户周二交付，乙客户周二交付。周三交付。"));
    // One identifiable target remains correctable; shared dates are not a
    // blanket ban on all explicit old/new replacement syntax.
    assert!(accepted(&["甲客户周二交付，乙客户周四交付。", "把周二改成周三。"], "甲客户周三交付，乙客户周四交付。"));
}

#[test]
fn adjacent_numeric_correction_cannot_change_the_quantity_dimension() {
    let input = ["预算3000元。", "我说错了，是3人。"];
    assert!(!accepted(&input, "预算3人。"));
    assert!(!accepted(&input, "预算3000元，参会3人。"));
    assert!(effective_spans(&source(&input)).iter().any(|span| span.unresolved_correction));
    assert!(accepted(&input, &input.join("")));
    assert!(accepted(&["预算3000元。", "我说错了，是3500元。"], "预算3500元。"));
    assert!(accepted(&["等待3秒。", "我说错了，是3分钟。"], "等待3分钟。"));
    assert!(accepted(&["预算五千，不对，三千五百元。"], "预算3500元。"));
}

#[test]
fn ambiguity_identity_exemption_requires_exact_text_in_each_cited_block() {
    let input = source(&["甲客户周二交付。", "乙客户周二交付。", "把周二改成周三。"]);
    assert!(validate_meaning(&input, &[block("甲客户周二交付。", &[0]), block("乙客户周二交付。把周二改成周三。", &[1, 2])]).is_ok());
    // Entire output and ordered coverage are unchanged, but local ownership is
    // shifted: restoring block 0 would otherwise lose the second customer's text.
    assert!(validate_meaning(&input, &[block("甲客户周二交付。乙客户周二交付。", &[0]), block("把周二改成周三。", &[1, 2])]).is_err());
    assert!(validate_meaning(&input, &[block("甲客户周二交付。", &[1]), block("乙客户周二交付。把周二改成周三。", &[0, 2])]).is_err());
}

#[test]
fn correction_does_not_turn_a_negative_or_uncertain_value_into_a_positive_one() {
    for input in [["预算3000元。", "我说错了，不是3500元。"], ["预算3000元。", "我说错了，可能3500元。"]] {
        assert!(!accepted(&input, "预算3500元。"));
        assert!(effective_spans(&source(&input)).iter().any(|span| span.unresolved_correction));
        assert!(accepted(&input, &input.join("")));
    }
    let input = ["周二开会。", "我说错了，不是周三开会。"];
    assert!(!accepted(&input, "周三开会，不是周三开会。"));
    assert!(accepted(&input, &input.join("")));
}

#[test]
fn ability_questions_remain_questions_instead_of_negative_assertions() {
    let input = ["确认一下现在能不能上传附件。"];
    for output in ["确认一下现在能否上传附件。", "确认一下现在是否能上传附件。", "确认一下现在是否能够上传附件。"] {
        assert!(accepted(&input, output), "{output}");
    }
    assert!(!accepted(&input, "现在不能上传附件。"));
    assert!(!accepted(&input, "现在能上传附件。"));
    assert!(!accepted(&input, "确认一下现在能否删除附件。"));
    assert!(!accepted(&["现在不能上传附件。"], "现在能否上传附件。"));
}

#[test]
fn questions_cannot_borrow_scope_from_another_action_or_source() {
    assert!(!accepted(&["检查能不能提交，检查附件。"], "检查可以提交，检查能不能上传附件。"));
    let input = source(&["检查能不能提交。", "检查能不能上传附件。"]);
    assert!(validate_meaning(&input, &[block("检查能不能上传附件。", &[0]), block("检查能不能提交。", &[1])]).is_err());
}

#[test]
fn quantitative_only_is_distinct_from_a_necessary_condition() {
    assert!(accepted(&["备用电池只有三块。"], "备用电池仅三块。"));
    assert!(accepted(&["备用电池只有三块。"], "备用电池仅有3块。"));
    assert!(!accepted(&["备用电池只有三块。"], "备用电池仅五块。"));
    assert!(!accepted(&["备用电池只有三块。"], "备用电池至少三块。"));
    assert!(!accepted(&["备用电池只有三块。"], "备用电池三块以上。"));
    assert!(!accepted(&["只有确认电池充满，我们才出发。"], "我们出发，再确认电池充满。"));
    assert!(!accepted(&["只有1人同意才出发。"], "仅有1人同意，我们出发。"));
}

#[test]
fn no_change_paraphrase_does_not_allow_reversal_or_borrowed_negation() {
    let input = ["标题先不改，正文今天更新。"];
    assert!(accepted(&input, "标题暂不更改，正文今天更新。"));
    assert!(!accepted(&input, "标题先改，正文今天更新。"));
    assert!(!accepted(&input, "标题先改，正文今天暂不更改。"));
}

#[test]
fn cross_item_condition_preserves_prohibition_and_independent_next_statement() {
    let input = ["如果审批没有通过，", "先不要发合同，周五仍然要交预算。"];
    assert!(accepted(&input, "如果审批没有通过，先不要发合同。周五仍然要交预算。"));
    assert!(!accepted(&input, "如果审批没有通过，先发合同。周五仍然要交预算。"));
    assert!(!accepted(&input, "如果审批没有通过，先不要交预算。周五仍然要发合同。"));
}
