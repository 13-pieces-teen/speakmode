//! Numeric interpretation across soft ASR item boundaries.
use super::{integrity, literals};
use crate::core::SourceRevision;
use std::ops::Range;

const WINDOW_BYTES: usize = 192;
const LIST_CUES: &[&str] = &["分别", "两个", "两组", "两条", "多个", "各为", "范围"];

fn horizontal(text: &str) -> &str {
    text.trim_start_matches([' ', '\t', '\u{3000}'])
}

fn hard_edge(c: char) -> bool {
    matches!(c, '，' | ',' | '、' | '。' | '.' | '！' | '!' | '？' | '?' | '；' | ';' | '\n' | '\r' | '\\')
}

fn digit(c: char) -> bool {
    integrity::fragment_digit(c).is_some()
}

fn clock_digit(c: char) -> bool {
    digit(c) || matches!(c, '十' | '两')
}

fn numeric_run(text: &str) -> (usize, usize) {
    text.chars().take(25).take_while(|&c| digit(c))
        .fold((0, 0), |(bytes, chars), c| (bytes + c.len_utf8(), chars + 1))
}

fn nearby(text: &str, start: usize) -> (usize, &str) {
    let left = text.ceil_char_boundary(start.saturating_sub(WINDOW_BYTES));
    let right = text.floor_char_boundary((start + WINDOW_BYTES).min(text.len()));
    (left, &text[left..right])
}

#[derive(Clone, Copy)]
struct Seam {
    byte: usize,
    after_item: usize,
}

struct Preview {
    text: String,
    seams: Vec<Seam>,
    literals: Vec<Range<usize>>,
}

fn preview(source: &SourceRevision) -> Preview {
    let original = source.text();
    let protected = literals::spans(&original);
    let mut protected_at = protected.iter().peekable();
    let mut text = String::with_capacity(original.len());
    let mut seams = Vec::new();
    let mut removed = Vec::new();
    let mut original_pos = 0;
    for (index, item) in source.items.iter().enumerate() {
        text.push_str(&item.text);
        original_pos += item.text.len();
        let Some(next) = source.items.get(index + 1) else { break; };
        while protected_at.peek().is_some_and(|literal| literal.end <= original_pos) {
            protected_at.next();
        }
        let left = item.text.trim_end_matches([' ', '\t', '\u{3000}']).chars().next_back();
        let right = horizontal(&next.text).chars().next();
        let valid_edge = left.is_some_and(|c| !hard_edge(c)) && right.is_some_and(|c| !hard_edge(c));
        let quoted = protected_at.peek().is_some_and(|literal| literal.start <= original_pos);
        if item.is_final && next.is_final && valid_edge && !quoted {
            seams.push(Seam { byte: text.len(), after_item: index });
            removed.push(original_pos);
        } else {
            text.push('\n');
        }
        original_pos += 1;
    }
    let mapped = |at| at - removed.partition_point(|&seam| seam < at);
    let literals = protected.iter().map(|literal| mapped(literal.start)..mapped(literal.end)).collect();
    Preview { text, seams, literals }
}

/// Mark only seams strictly inside the already recognized fact or its cue.
/// In particular, an explicit punctuation mark stays in the analysis text.
fn mark(
    prep: &Preview,
    selected: &mut [bool],
    range: Range<usize>,
    allowed: impl Fn(usize) -> bool,
) {
    let preceding_literal = prep.literals.partition_point(|literal| literal.end <= range.start);
    if prep.literals.get(preceding_literal).is_some_and(|literal| literal.start < range.end) {
        return;
    }
    let first = prep.seams.partition_point(|seam| seam.byte <= range.start);
    for seam in prep.seams[first..].iter().take_while(|seam| seam.byte < range.end) {
        if allowed(seam.byte) {
            selected[seam.after_item] = true;
        }
    }
}

fn identifier_end(prep: &Preview, left: usize, text: &str, at: usize, digits: &str) -> Option<usize> {
    let (length, count) = numeric_run(digits);
    let end = text.len() - digits.len() + length;
    if !(2..=24).contains(&count) {
        return None;
    }
    let tail = &text[end..];
    let soft_end = prep.seams.binary_search_by_key(&(left + end), |seam| seam.byte).is_ok();
    let terminal = horizontal(tail).chars().next().is_none_or(|c| hard_edge(c) && c != '\\');
    if !terminal && !soft_end {
        return None;
    }
    let before = text[..at].rsplit(hard_edge).next().unwrap_or("");
    let after = tail.split(['。', '.', '！', '!', '？', '?', '；', ';', '\n', '\r']).next().unwrap_or("");
    (!LIST_CUES.iter().any(|cue| before.contains(cue) || after.contains(cue))).then_some(end)
}

fn correction_digits(text: &str) -> Option<&str> {
    let marker = horizontal(text).strip_prefix(['，', ','])?;
    let tail = ["不对", "我说错了", "说错了"].iter().find_map(|cue| horizontal(marker).strip_prefix(cue))?;
    let tail = horizontal(tail);
    let tail = tail.strip_prefix(['，', ',']).map(horizontal).unwrap_or(tail);
    Some(tail.strip_prefix('是').map(horizontal).unwrap_or(tail))
}

pub(super) fn joined_boundaries(source: &SourceRevision) -> Vec<usize> {
    if !source.complete || source.items.len() < 2 {
        return Vec::new();
    }
    let prep = preview(source);
    if prep.seams.is_empty() {
        return Vec::new();
    }
    let mut selected = vec![false; source.items.len() - 1];
    let mut seam_cursor = 0;
    let mut literal_cursor = 0;
    let mut consumed = 0;
    let mut previous = None;
    for (start, c) in prep.text.char_indices() {
        let precedes_clock_digit = previous.is_some_and(clock_digit);
        previous = Some(c);
        if start < consumed {
            continue;
        }
        while prep.seams.get(seam_cursor).is_some_and(|seam| seam.byte <= start) {
            seam_cursor += 1;
        }
        let close_to_seam = |seam: &Seam| seam.byte.abs_diff(start) <= WINDOW_BYTES;
        if !prep.seams.get(seam_cursor).is_some_and(close_to_seam)
            && !(seam_cursor > 0 && close_to_seam(&prep.seams[seam_cursor - 1]))
        {
            continue;
        }
        while prep.literals.get(literal_cursor).is_some_and(|literal| literal.end <= start) {
            literal_cursor += 1;
        }
        if prep.literals.get(literal_cursor).is_some_and(|literal| literal.start <= start) {
            continue;
        }
        let (left, window) = nearby(&prep.text, start);
        let at = start - left;
        let input = &window[at..];
        if let Some((label, digits)) = integrity::fragment_identifier_prefix(input) {
            if let Some(end) = identifier_end(&prep, left, window, at, digits) {
                let digits_start = left + window.len() - digits.len();
                let end = left + end;
                let generic = matches!(label, "编号" | "号码");
                mark(&prep, &mut selected, start..end, |seam| {
                    !generic || seam <= digits_start
                        || prep.text[digits_start..seam].chars().count() < 2
                        || prep.text[seam..end].chars().count() < 2
                });
                consumed = end;
                if let Some(replacement) = correction_digits(&window[end - left..]) {
                    let replacement_start = window.len() - replacement.len();
                    if let Some(replacement_end) = identifier_end(&prep, left, window, replacement_start, replacement) {
                        mark(&prep, &mut selected, left + replacement_start..left + replacement_end, |_| true);
                        consumed = left + replacement_end;
                    }
                }
                continue;
            }
        }
        if clock_digit(c) && !precedes_clock_digit {
            if let Some((begin, end)) = integrity::fragment_clock_range(window, at) {
                mark(&prep, &mut selected, left + begin..left + end, |_| true);
                consumed = left + end;
            }
        }
    }
    selected.iter().enumerate().filter_map(|(index, joined)| joined.then_some(index)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::SourceItem;

    fn source(items: &[&str]) -> SourceRevision {
        SourceRevision {
            id: "source".into(),
            session_id: "test".into(),
            asr_attempt_id: "asr".into(),
            generation: 1,
            provider: "test".into(),
            model: "test".into(),
            adapter_version: "1".into(),
            complete: true,
            created_at: "now".into(),
            items: items.iter().enumerate().map(|(index, text)| SourceItem {
                id: format!("item-{index}"),
                provider_item_id: index.to_string(),
                capture_order: index as u64,
                text: (*text).into(),
                is_final: true,
                start_ms: None,
                end_ms: None,
            }).collect(),
        }
    }

    #[test]
    fn joins_explicit_times_and_identifier_values_across_multiple_items() {
        for (items, expected) in [
            (&["下", "午五点", "零", "三开", "始。"][..], "下午五点零三开始。"),
            (&["约在", "凌晨零点", "〇五起飞。"][..], "约在凌晨零点〇五起飞。"),
            (&["验证", "码是０", "０", "７。"][..], "验证码是００７。"),
            (&["验证码零零", "七，不对，是零零", "八。"][..], "验证码零零七，不对，是零零八。"),
            (&["验证码是零零", "七", "请登记。"][..], "验证码是零零七\n请登记。"),
        ] {
            let original = source(items);
            let before = original.clone();
            assert_eq!(projected_text(&original).as_deref(), Some(expected), "{items:?}");
            assert_eq!(original, before);
        }
    }

    #[test]
    fn protects_literals_punctuation_and_ambiguous_groups() {
        for items in [
            ["原话是 '下午五点", "零三开会。'"],
            ["原话是「下午五点", "零三开会。」"],
            ["字段是 `验证码零零", "七`。"],
            ["编号一二三", "四五六，分别登记。"],
            ["编号一二三", "四五六。"],
            ["两组编号分别为一二", "三和四五六。"],
            ["验证码零零，", "七。"],
            ["下午五点。", "零三开会。"],
            ["下午三点", "五分钟后开会。"],
        ] {
            assert!(joined_boundaries(&source(&items)).is_empty(), "{items:?}");
        }
        let mut partial = source(&["验证码是零零", "七。"]);
        partial.items[1].is_final = false;
        assert!(joined_boundaries(&partial).is_empty());
    }

    #[test]
    fn relative_day_scope_survives_each_cut_inside_a_clock() {
        let value = "今天晚上十一点五十九分";
        let text = format!("{value}结束。");
        for (cut, _) in value.char_indices().skip(1) {
            let original = source(&[&text[..cut], &text[cut..]]);
            assert_eq!(projected_text(&original).as_deref(), Some(text.as_str()), "{cut}");
        }
        assert!(joined_boundaries(&source(&["今天。", "晚上十一点五十九分结束。"])).is_empty());
    }
}

/// Model and analysis consume the same approved item seams. Original item
/// positions and stored text stay with the caller's source revision.
pub(super) fn projected_text(source: &SourceRevision) -> Option<String> {
    let boundaries = joined_boundaries(source);
    if boundaries.is_empty() {
        return None;
    }
    let mut boundaries = boundaries.into_iter().peekable();
    let mut text = String::new();
    for (index, item) in source.items.iter().enumerate() {
        if index > 0 && boundaries.next_if_eq(&(index - 1)).is_none() {
            text.push('\n');
        }
        text.push_str(&item.text);
    }
    Some(text)
}
