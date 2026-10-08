//! A deliberately finite, deterministic rewrite risk check.
//!
//! This is not a semantic equivalence proof or a general Chinese parser. It
//! protects a small set of high-value qualifiers and checks their local scope
//! inside blocks citing the same immutable source. It never repairs text,
//! invents references, or includes dictated content in its static error.

use super::CloudError;
use crate::core::{OutputBlock, SourceRevision};
use std::collections::HashSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Kind {
    Time(u8),
    Conditional,
    NecessaryCondition,
    Exception,
    Otherwise,
    Possibility,
    Pending,
    Prohibition,
    NegativeFact,
    NegativeStatus,
    Negative,
}

// Longest matching phrase wins. Do not treat a syllable inside a longer
// qualifier (e.g. 不 in 不确定) as an independent negation.
const PHRASES: &[(&str, Kind)] = &[
    ("今天", Kind::Time(0)), ("今日", Kind::Time(0)),
    ("明天", Kind::Time(1)), ("明日", Kind::Time(1)),
    ("昨天", Kind::Time(2)), ("昨日", Kind::Time(2)),
    ("后天", Kind::Time(3)), ("後天", Kind::Time(3)),
    ("前天", Kind::Time(4)),
    ("本周", Kind::Time(5)), ("这周", Kind::Time(5)), ("本星期", Kind::Time(5)), ("这个星期", Kind::Time(5)),
    ("下周", Kind::Time(6)), ("下星期", Kind::Time(6)), ("下个星期", Kind::Time(6)),
    ("上周", Kind::Time(7)), ("上星期", Kind::Time(7)), ("上个星期", Kind::Time(7)),
    ("周一", Kind::Time(8)), ("星期一", Kind::Time(8)), ("礼拜一", Kind::Time(8)),
    ("周二", Kind::Time(9)), ("星期二", Kind::Time(9)), ("礼拜二", Kind::Time(9)),
    ("周三", Kind::Time(10)), ("星期三", Kind::Time(10)), ("礼拜三", Kind::Time(10)),
    ("周四", Kind::Time(11)), ("星期四", Kind::Time(11)), ("礼拜四", Kind::Time(11)),
    ("周五", Kind::Time(12)), ("星期五", Kind::Time(12)), ("礼拜五", Kind::Time(12)),
    ("周六", Kind::Time(13)), ("星期六", Kind::Time(13)), ("礼拜六", Kind::Time(13)),
    ("周日", Kind::Time(14)), ("周天", Kind::Time(14)), ("星期日", Kind::Time(14)), ("星期天", Kind::Time(14)), ("礼拜天", Kind::Time(14)),
    ("今晚", Kind::Time(15)), ("今天晚上", Kind::Time(15)), ("今夜", Kind::Time(15)),
    ("今早", Kind::Time(16)), ("今天早上", Kind::Time(16)), ("今天上午", Kind::Time(16)),
    ("明晚", Kind::Time(17)), ("明天晚上", Kind::Time(17)), ("明早", Kind::Time(18)), ("明天早上", Kind::Time(18)),
    ("如果", Kind::Conditional), ("假如", Kind::Conditional), ("假若", Kind::Conditional), ("倘若", Kind::Conditional), ("要是", Kind::Conditional), ("若", Kind::Conditional), ("情况下", Kind::Conditional),
    ("只有", Kind::NecessaryCondition), ("仅当", Kind::NecessaryCondition),
    ("除非", Kind::Exception), ("否则", Kind::Otherwise), ("不然", Kind::Otherwise),
    ("可能", Kind::Possibility), ("也许", Kind::Possibility), ("或许", Kind::Possibility), ("大概", Kind::Possibility), ("不一定", Kind::Possibility), ("未必", Kind::Possibility), ("有可能", Kind::Possibility),
    ("还没有确定", Kind::Pending), ("还没有确认", Kind::Pending), ("还没确定", Kind::Pending), ("还没确认", Kind::Pending),
    ("尚未确定", Kind::Pending), ("尚未确认", Kind::Pending), ("未确定", Kind::Pending), ("未确认", Kind::Pending),
    ("没有确定", Kind::Pending), ("没有确认", Kind::Pending), ("没确定", Kind::Pending), ("没确认", Kind::Pending),
    ("不能确定", Kind::Pending), ("不确定", Kind::Pending), ("待确认", Kind::Pending), ("待确定", Kind::Pending), ("待定", Kind::Pending), ("未定", Kind::Pending),
    ("不要", Kind::Prohibition), ("请勿", Kind::Prohibition), ("切勿", Kind::Prohibition), ("禁止", Kind::Prohibition), ("不得", Kind::Prohibition), ("暂不", Kind::Prohibition), ("暂时不", Kind::Prohibition), ("别", Kind::Prohibition),
    ("并不是", Kind::NegativeFact), ("不是", Kind::NegativeFact), ("并非", Kind::NegativeFact), ("不等于", Kind::NegativeFact), ("≠", Kind::NegativeFact),
    ("尚未", Kind::NegativeStatus), ("还没有", Kind::NegativeStatus), ("还没", Kind::NegativeStatus), ("没有", Kind::NegativeStatus), ("并未", Kind::NegativeStatus), ("未曾", Kind::NegativeStatus), ("没", Kind::NegativeStatus),
    ("不会", Kind::Negative), ("不能", Kind::Negative), ("无法", Kind::Negative), ("不可能", Kind::Negative), ("不可以", Kind::Negative), ("不", Kind::Negative),
];

// Lexical idioms are not the negative assertions covered by this guard. This
// list also prevents "别" inside "特别" and "不" inside "不得不".
const NON_ANCHORS: &[&str] = &[
    "不仅", "不但", "不错", "不少", "不妨", "不如", "不止", "不得不", "不约而同", "不久", "不管", "无论",
    "特别", "分别", "个别", "别的", "别人", "别处", "别名", "没错", "没关系", "没事儿",
];
const CORRECTIONS: &[&str] = &["不对", "说错了", "我说错了", "更正一下", "更正为", "改口", "准确地说"];
const FILLERS: &[&str] = &["嗯", "呃", "啊", "那个", "这个", "就是说", "就是", "然后", "我说"];

#[derive(Debug)]
struct Span<'a> { source_id: &'a str, text: String }
#[derive(Debug)]
struct Anchor<'a> { text: &'a str, kind: Kind }

fn is_separator(c: char) -> bool {
    matches!(c, '，' | ',' | '。' | '.' | '！' | '!' | '？' | '?' | '；' | ';' | '\n' | '\r')
}

fn effective_spans(source: &SourceRevision) -> Vec<Span<'_>> {
    let mut spans: Vec<Span<'_>> = Vec::new();
    for item in &source.items {
        for raw in item.text.split(is_separator).map(str::trim).filter(|text| !text.is_empty()) {
            // Explicit repairs retract the immediately preceding spoken clause,
            // including across ASR item boundaries. Never infer corrections from
            // a contrast such as "不是五千元" or silently delete the old item ID.
            if let Some(marker) = CORRECTIONS.iter().filter(|marker| raw.starts_with(**marker)).max_by_key(|marker| marker.len()) {
                while spans.last().is_some_and(|span| FILLERS.contains(&span.text.trim())) { spans.pop(); }
                spans.pop();
                let remainder = raw[marker.len()..].trim();
                if !remainder.is_empty() { spans.push(Span { source_id: &item.id, text: remainder.to_owned() }); }
            } else {
                spans.push(Span { source_id: &item.id, text: raw.to_owned() });
            }
        }
    }
    spans
}

fn anchors(text: &str) -> Vec<Anchor<'_>> {
    let mut found = Vec::new();
    let mut offset = 0;
    while offset < text.len() {
        let remainder = &text[offset..];
        let protected = PHRASES.iter().filter(|entry| remainder.starts_with(entry.0)).max_by_key(|entry| entry.0.len());
        let neutral = NON_ANCHORS.iter().filter(|phrase| remainder.starts_with(**phrase)).max_by_key(|phrase| phrase.len());
        if neutral.is_some_and(|phrase| protected.is_none_or(|(candidate, _)| phrase.len() >= candidate.len())) {
            offset += neutral.unwrap().len();
        } else if let Some((phrase, kind)) = protected {
            found.push(Anchor { text: &text[offset..offset + phrase.len()], kind: *kind });
            offset += phrase.len();
        } else {
            offset += remainder.chars().next().unwrap().len_utf8();
        }
    }
    found
}

/// Guidance for the existing rewrite request; no revoked qualifier is demanded.
/// The immutable source still remains the sole authority for the output text.
pub(super) fn preservation_hints(source: &SourceRevision) -> Vec<String> {
    let mut seen = HashSet::new();
    effective_spans(source).iter().flat_map(|span| anchors(&span.text).into_iter().map(|anchor| anchor.text.to_owned())).filter(|text| seen.insert(text.clone())).collect()
}

fn same_kind(expected: Kind, actual: Kind) -> bool {
    expected == actual || matches!((expected, actual), (Kind::Prohibition, Kind::Negative))
}

fn scope_text(text: &str) -> String {
    // ASR may space a mixed-language product name ("speak mode") while the
    // rewrite removes that space ("SpeakMode"). Normalize only original gaps
    // between ASCII letters, before qualifier removal inserts its own gaps.
    let mut result = String::new();
    let mut gap = String::new(); let mut previous_ascii_letter = false;
    for c in text.chars() {
        if c.is_whitespace() { gap.push(c); continue; }
        if !(previous_ascii_letter && c.is_ascii_alphabetic()) { result.push_str(&gap); }
        gap.clear(); result.extend(c.to_lowercase()); previous_ascii_letter = c.is_ascii_alphabetic();
    }
    result.push_str(&gap);
    // Remove qualifiers themselves before comparing local subjects/actions. A
    // qualifier on an unrelated bullet cannot satisfy the check by itself.
    let mut phrases = PHRASES.iter().map(|(phrase, _)| *phrase).collect::<Vec<_>>();
    phrases.sort_by_key(|phrase| std::cmp::Reverse(phrase.len()));
    for phrase in phrases { result = result.replace(phrase, " "); }
    for (from, to) in [("发送", "发"), ("发出", "发"), ("移除", "删除"), ("检测", "检查"), ("对比", "比较"), ("使用", "用"), ("试一下", "试用")] { result = result.replace(from, to); }
    for filler in ["首先", "然后", "但是", "不过", "我们", "你们", "他们", "同样", "这个", "那个", "就是说", "就是", "已经", "暂时", "具体", "确认", "确定", "需要", "进行", "可以", "请", "先", "再", "还", "也", "是", "的", "了"] { result = result.replace(filler, " "); }
    normalize_numbers(&result)
}

// Only token normalization for local matching: never alters the result text or
// checks that every dictated number was semantically preserved.
fn normalize_numbers(text: &str) -> String {
    let mut result = String::new();
    let mut run = String::new();
    for c in text.chars().chain(std::iter::once(' ')) {
        if matches!(c, '零' | '〇' | '一' | '二' | '两' | '三' | '四' | '五' | '六' | '七' | '八' | '九' | '十' | '百' | '千' | '万' | '亿') { run.push(c); }
        else {
            if !run.is_empty() { result.push_str(&chinese_number(&run).map(|number| number.to_string()).unwrap_or_else(|| run.clone())); run.clear(); }
            result.push(c);
        }
    }
    result.pop(); // Sentinel only.
    result
}

fn chinese_number(text: &str) -> Option<u64> {
    let digit = |c| match c { '零' | '〇' => Some(0u64), '一' => Some(1), '二' | '两' => Some(2), '三' => Some(3), '四' => Some(4), '五' => Some(5), '六' => Some(6), '七' => Some(7), '八' => Some(8), '九' => Some(9), _ => None };
    if !text.chars().any(|c| matches!(c, '十' | '百' | '千' | '万' | '亿')) {
        return text.chars().try_fold(0u64, |number, c| number.checked_mul(10)?.checked_add(digit(c)?));
    }
    let mut total = 0u64; let mut section = 0u64; let mut number = 0u64;
    for c in text.chars() {
        if let Some(value) = digit(c) { number = value; continue; }
        let unit = match c { '十' => 10u64, '百' => 100, '千' => 1000, '万' => 10_000, '亿' => 100_000_000, _ => return None };
        if unit < 10_000 { section = section.checked_add(number.max(1).checked_mul(unit)?)?; }
        else { total = total.checked_add(section.checked_add(number)?.checked_mul(unit)?)?; section = 0; }
        number = 0;
    }
    total.checked_add(section)?.checked_add(number)
}

fn fingerprints(text: &str) -> (HashSet<String>, HashSet<String>) {
    let text = scope_text(text);
    let mut strong = HashSet::new(); let mut chinese = HashSet::new();
    for run in text.split(|c: char| !c.is_alphanumeric()) {
        let mut ascii = String::new(); let mut chinese_run = Vec::new();
        let flush = |ascii: &mut String, chinese_run: &mut Vec<char>, strong: &mut HashSet<String>, chinese: &mut HashSet<String>| {
            if !ascii.is_empty() { strong.insert(std::mem::take(ascii)); }
            for pair in chinese_run.windows(2) { chinese.insert(pair.iter().collect()); }
            chinese_run.clear();
        };
        for c in run.chars() {
            if c.is_ascii_alphanumeric() { for pair in chinese_run.windows(2) { chinese.insert(pair.iter().collect()); } chinese_run.clear(); ascii.push(c); }
            else { if !ascii.is_empty() { strong.insert(std::mem::take(&mut ascii)); } chinese_run.push(c); }
        }
        flush(&mut ascii, &mut chinese_run, &mut strong, &mut chinese);
    }
    (strong, chinese)
}

fn scope_matches(source: &(HashSet<String>, HashSet<String>), candidate: &(HashSet<String>, HashSet<String>)) -> bool {
    let (strong, chinese) = source;
    let (output_strong, output_chinese) = candidate;
    // A single common product name is not the whole scope of a statement:
    // "today compare SpeakMode with Typeless" cannot borrow "today reinstall
    // SpeakMode". Preserve the named participants AND the local action tokens.
    if !strong.iter().all(|token| output_strong.contains(token)) { return false; }
    if chinese.is_empty() { return true; } // Standalone qualifier has no lexical scope to compare.
    let needed = if chinese.len() >= 4 { 2 } else { 1 };
    chinese.intersection(&output_chinese).count() >= needed
}

fn scoped_candidates(text: &str) -> Vec<String> {
    let mut candidates = Vec::new(); let mut header = String::new();
    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let plain = line.trim_start_matches(['•', '-', '*', ' ', '\t']);
        // A short actual heading can carry a shared qualifier to its bullets.
        // End it at another heading; do not carry it between output blocks.
        let is_bullet = line != plain || line.chars().next().is_some_and(|c| c.is_ascii_digit());
        if !is_bullet && plain.chars().count() <= 24 && !plain.chars().any(is_separator) {
            header = plain.to_owned();
        } else if !is_bullet { header.clear(); }
        for clause in plain.split(is_separator).map(str::trim).filter(|clause| !clause.is_empty()) {
            candidates.push(if is_bullet && !header.is_empty() { format!("{header} {clause}") } else { clause.to_owned() });
        }
    }
    candidates
}

pub(super) fn validate_meaning(source: &SourceRevision, blocks: &[OutputBlock]) -> Result<(), CloudError> {
    // Source completeness and strict citation coverage are checked by the caller
    // before this guard. References here are never modified or guessed.
    // Parse each output clause once. Repeated ASR fragments must not repeatedly
    // tokenize all output text on the UI's request completion path.
    let candidates = blocks.iter().map(|block| (block, scoped_candidates(&block.text).into_iter().map(|clause| {
        let kinds = anchors(&clause).into_iter().map(|anchor| anchor.kind).collect::<HashSet<_>>();
        (kinds, fingerprints(&clause))
    }).collect::<Vec<_>>())).collect::<Vec<_>>();
    for span in effective_spans(source) {
        let protected = anchors(&span.text);
        let scope = fingerprints(&span.text);
        for anchor in protected {
            let retained = candidates.iter().filter(|(block, _)| block.source_refs.iter().any(|id| id == span.source_id)).flat_map(|(_, clauses)| clauses).any(|(kinds, candidate_scope)| {
                kinds.iter().any(|candidate| same_kind(anchor.kind, *candidate)) && scope_matches(&scope, candidate_scope)
            });
            if !retained { return Err(CloudError::new("rewrite_meaning_risk", "rewrite", "整理结果可能遗漏时间、条件或否定等限定，请核对识别稿或重新整理")); }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::SourceItem;
    fn source(texts: &[&str]) -> SourceRevision {
        SourceRevision { id: "source".into(), session_id: "session".into(), asr_attempt_id: "attempt".into(), generation: 1, provider: "aliyun".into(), model: "candidate".into(), adapter_version: "v1".into(), complete: true, created_at: "now".into(), items: texts.iter().enumerate().map(|(index, text)| SourceItem { id: format!("item-{index}"), provider_item_id: format!("{index}"), capture_order: index as u64, text: (*text).into(), is_final: true, start_ms: None, end_ms: None }).collect() }
    }
    fn block(text: &str, refs: &[usize]) -> OutputBlock { OutputBlock { id: "output".into(), text: text.into(), source_refs: refs.iter().map(|index| format!("item-{index}")).collect() } }
    fn accepted(input: &[&str], output: &str) -> bool { let source = source(input); validate_meaning(&source, &[block(output, &(0..input.len()).collect::<Vec<_>>())]).is_ok() }

    #[test]
    fn measured_today_omission_is_rejected_even_with_complete_mapping() {
        let source = source(&["今天用SpeakMode比较Typeless和Superwhisper。", "音频采集用WASAPI。", "重采样用Rubato。"]);
        let result = validate_meaning(&source, &[block("• 使用SpeakMode对比Typeless和Superwhisper。\n• 音频采集采用WASAPI。\n• 重采样使用Rubato。", &[0, 1, 2])]);
        let error = result.unwrap_err(); assert_eq!(error.code, "rewrite_meaning_risk"); assert_eq!(error.stage, "rewrite"); assert!(!error.message.contains("SpeakMode"));
        assert!(accepted(&["今天用SpeakMode比较Typeless和Superwhisper。"], "今日使用SpeakMode对比Typeless和Superwhisper。"));
    }
    #[test]
    fn moving_today_to_an_unrelated_source_block_is_rejected() {
        let source = source(&["今天检查麦克风。", "整理预算。"]);
        assert!(validate_meaning(&source, &[block("检查麦克风。", &[0]), block("今天整理预算。", &[1])]).is_err());
        assert!(!accepted(&["今天检查麦克风。整理预算。"], "检查麦克风。今天整理预算。"));
    }
    #[test]
    fn a_shared_ascii_name_cannot_move_today_to_another_action_in_the_same_block() {
        assert!(!accepted(&["今天用SpeakMode比较Typeless。"], "用SpeakMode比较Typeless。今天重装SpeakMode。"));
        assert!(!accepted(&["今天用SpeakMode录音。"], "用SpeakMode录音。今天重装SpeakMode。"));
        assert!(!accepted(&["今天用SpeakMode比较Typeless。"], "用SpeakMode比较Typeless。今天比较SpeakMode和Typeoff。"));
        assert!(accepted(&["今天用SpeakMode比较Typeless。"], "今日使用SpeakMode对比Typeless。"));
        assert!(accepted(&["今天用SpeakMode录音。"], "今日使用SpeakMode录音。"));
    }
    #[test]
    fn mixed_language_name_spacing_and_numeric_formatting_remain_acceptable() {
        assert!(accepted(&["今天用 speak mode 比较 typeless 和 super whisper。"], "今日使用SpeakMode对比Typeless和Superwhisper。"));
        assert!(accepted(&["预算不是五千元。"], "预算并非5000元。"));
        assert!(accepted(&["今天检查WASAPI采音。"], "今日检测WASAPI采音。"));
    }
    #[test]
    fn negation_on_another_comma_clause_is_not_enough() {
        assert!(!accepted(&["不要删除原始录音。"], "删除原始录音，但不要发邀请。"));
        assert!(accepted(&["不要删除原始录音。"], "暂不删除原始录音。"));
    }
    #[test]
    fn condition_negation_and_pending_synonyms_are_allowed() {
        assert!(accepted(&["如果时间还没确认，先不要发邀请。"], "若时间尚未确定，暂不发送邀请。"));
        assert!(!accepted(&["如果时间还没确认，先不要发邀请。"], "时间已经确认，发送邀请。"));
        assert!(!accepted(&["如果时间还没确认，先不要发邀请。"], "若预算尚未确定，暂不发送邀请。"));
    }
    #[test]
    fn uncertainty_stays_with_the_trial_and_not_another_action() {
        assert!(accepted(&["我们可能周五让同学试用，但是时间还没确定。"], "也许星期五让同学试用，但时间尚未确定。"));
        assert!(!accepted(&["我们可能周五让同学试用，但是时间还没确定。"], "星期五让同学试用，但时间尚未确定。"));
        assert!(!accepted(&["可能周五让同学试用。周六整理预算。"], "周五让同学试用。可能周六整理预算。"));
    }
    #[test]
    fn selected_days_and_weeks_are_not_interchangeable() {
        for (input, equivalent, changed) in [("明天开会。", "明日开会。", "今天开会。"), ("昨天开会。", "昨日开会。", "明天开会。"), ("下周开发安排。", "下星期开发安排。", "本周开发安排。"), ("本周检查麦克风。", "这周检查麦克风。", "下周检查麦克风。"), ("周日开会。", "星期天开会。", "星期一开会。"), ("周一开会。", "星期一开会。", "星期二开会。")] {
            assert!(accepted(&[input], equivalent), "{input} / {equivalent}"); assert!(!accepted(&[input], changed), "{input} / {changed}");
        }
    }
    #[test]
    fn explicit_self_correction_does_not_require_revoked_date() {
        assert!(accepted(&["今天，不对，明天开会。"], "明天开会。"));
        assert!(accepted(&["今天开会。", "嗯。", "不对。", "明天开会。"], "明天开会。"));
        assert!(accepted(&["不要删除原始录音，不对，可以删除原始录音。"], "可以删除原始录音。"));
        let hints = preservation_hints(&source(&["今天，不对，明天开会。"])); assert_eq!(hints, ["明天"]);
    }
    #[test]
    fn factual_contrast_is_not_mistaken_for_self_correction() {
        assert!(accepted(&["预算先五千，不对，是三千五百元，不是五千元。"], "预算是3500元，不是5000元。"));
        assert!(!accepted(&["预算是三千五百元，不是五千元。"], "预算是3500元，是5000元。"));
        assert!(!accepted(&["不是今天开会，是明天开会。"], "今天开会，明天也开会。"));
    }
    #[test]
    fn removing_filler_and_nonnegative_idioms_is_allowed() {
        assert!(accepted(&["嗯，那个，今天检查麦克风。"], "今日检查麦克风。"));
        assert!(accepted(&["不仅快，而且稳定。"], "速度快，稳定性也好。"));
        assert!(accepted(&["这个效果不错。"], "效果很好。"));
        assert!(accepted(&["分别检查麦克风和输入框。"], "检查麦克风及输入框。"));
        assert!(accepted(&["我们不得不重新录音。"], "我们必须重新录音。"));
    }
    #[test]
    fn shared_heading_can_preserve_a_qualifier_in_its_cited_block() {
        assert!(accepted(&["今天检查麦克风，再保存录音。"], "今天\n• 检查麦克风。\n• 保存录音。"));
        assert!(accepted(&["下周的开发安排。", "先检查麦克风。"], "下周开发安排\n• 先检查麦克风。"));
    }
    #[test]
    fn protected_hints_are_unique_and_ordered_and_ignore_retracted_clutter() {
        assert_eq!(preservation_hints(&source(&["今天，不对，明天。", "明天可能周五开会，如果时间还没确认，不要发邀请。"])), ["明天", "可能", "周五", "如果", "还没确认", "不要"]);
    }
    #[test]
    fn number_normalization_is_local_only_and_checked() {
        assert_eq!(normalize_numbers("三千五百元，不是五千元"), "3500元，不是5000元");
        assert_eq!(normalize_numbers("三十五万元"), "350000元");
        assert_eq!(normalize_numbers("二零二六年"), "2026年");
        assert_eq!(normalize_numbers("5000元"), "5000元");
    }
}
