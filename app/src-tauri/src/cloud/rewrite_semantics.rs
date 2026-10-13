//! Bounded analysis view over immutable ASR items. Not a sentence parser.
//!
//! Every joined or corrected span retains original item IDs and byte ranges.
//! This view is never persisted as an edited source or used for source restore.
use super::{anchors, facts, fingerprints, partial_correction, Kind, CORRECTIONS, FILLERS};
use crate::core::{OutputBlock, SourceRevision};
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SourceSlice<'a> {
    pub(super) source_id: &'a str,
    pub(super) start: usize,
    pub(super) end: usize,
}

#[derive(Clone, Debug)]
pub(super) struct Span<'a> {
    pub(super) origins: Vec<SourceSlice<'a>>,
    pub(super) text: String,
    pub(super) unresolved_correction: bool,
}

impl Span<'_> {
    pub(super) fn is_cited_by(&self, block: &OutputBlock) -> bool {
        self.origins.iter().all(|origin| block.source_refs.iter().any(|id| id == origin.source_id))
    }
}

pub(super) fn compound_weekday(text: &str) -> Option<(usize, Kind)> {
    for (prefix, week) in [("下个星期", 2), ("上个星期", 0), ("这个星期", 1), ("下星期", 2), ("本星期", 1), ("上星期", 0), ("下周", 2), ("本周", 1), ("这周", 1), ("上周", 0)] {
        if let Some(rest) = text.strip_prefix(prefix) {
            let day = match rest.chars().next()? { '一' => 0, '二' => 1, '三' => 2, '四' => 3, '五' => 4, '六' => 5, '日' | '天' => 6, _ => continue };
            return Some((prefix.len() + rest.chars().next()?.len_utf8(), Kind::Time(20 + week * 7 + day)));
        }
    }
    None
}

pub(super) fn quantitative_only(text: &str) -> Option<usize> {
    let (prefix, rest) = ["只有", "仅有", "仅"].iter().find_map(|prefix| text.strip_prefix(prefix).map(|rest| (*prefix, rest)))?;
    // A numeric antecedent can still introduce a necessary condition. Never
    // erase that relation merely because its first word is a quantity.
    if rest.contains('才') { return None; }
    rest.trim_start().chars().next().is_some_and(|c| c.is_ascii_digit() || matches!(c, '零' | '一' | '二' | '两' | '三' | '四' | '五' | '六' | '七' | '八' | '九' | '十' | '百' | '千')).then_some(prefix.len())
}

fn date_class(kind: Kind) -> Option<u8> {
    match kind {
        Kind::Time(0..=4) => Some(0), Kind::Time(5..=7) => Some(1),
        Kind::Time(8..=14) => Some(2), Kind::Time(20..=40) => Some(3),
        Kind::Time(15..=18) => Some(4), _ => None,
    }
}

fn extend_origins<'a>(target: &mut Span<'a>, origins: &[SourceSlice<'a>]) {
    for origin in origins { if !target.origins.contains(origin) { target.origins.push(origin.clone()); } }
}

// Only an entire standalone clause made of pause syllables plus a known
// correction marker gets this extension. Never strip fillers from arbitrary
// prose (e.g. "啊不对的情况很多") or from quoted speech.
fn paused_marker(text: &str) -> bool {
    let rest = text.trim_start_matches(['嗯', '呃', '啊']);
    rest.len() < text.len() && CORRECTIONS.contains(&rest)
}

fn quote_masks(source: &SourceRevision) -> HashMap<&str, Vec<bool>> {
    let mut masks = HashMap::new();
    let mut end_quote = None;
    let mut escaped = false;
    let mut previous = None;
    for (item_index, item) in source.items.iter().enumerate() {
        let mut mask = vec![false; item.text.len()];
        for (offset, c) in item.text.char_indices() {
            mask[offset..offset + c.len_utf8()].fill(end_quote.is_some());
            let following = item.text[offset + c.len_utf8()..].chars().next()
                .or_else(|| source.items[item_index + 1..].iter().find_map(|item| item.text.chars().next()));
            // don't / don't-like contractions and unquoted users' possessives
            // must not open a quote that swallows a subsequent Chinese repair.
            let apostrophe = matches!(c, '\'' | '’') && previous.is_some_and(|c: char| c.is_ascii_alphanumeric())
                && (following.is_some_and(|c| c.is_ascii_alphanumeric()) || end_quote.is_none());
            if apostrophe { previous = Some(c); escaped = false; continue; }
            if let Some(end) = end_quote {
                if c == end && !escaped { end_quote = None; }
                escaped = c == '\\' && !escaped;
            } else {
                end_quote = match c {
                    '“' => Some('”'), '‘' => Some('’'), '「' => Some('」'), '『' => Some('』'),
                    '"' | '\'' | '`' => Some(c), _ => None,
                };
                escaped = false;
            }
            previous = Some(c);
        }
        masks.insert(item.id.as_str(), mask);
    }
    masks
}

fn quoted_origin(masks: &HashMap<&str, Vec<bool>>, span: &Span<'_>) -> bool {
    span.origins.first().and_then(|origin| masks.get(origin.source_id)
        .and_then(|mask| mask.get(origin.start))).copied().unwrap_or(true)
}

struct NumericRepair {
    target: usize,
    replacement: usize,
    old_key: String,
    expected: String,
}

fn numeric_repair(spans: &[Span<'_>], replacement: &Span<'_>) -> Option<NumericRepair> {
    let target = spans.len().checked_sub(1)?;
    let old = &spans[target].text;
    // An actual change/history is not an abandoned spoken value. This narrow
    // receipt does not reinterpret it, even if a nearby correction is present.
    if ["原来", "原先", "之前", "后来", "追加", "加订", "增加", "调整", "改为"]
        .iter().any(|cue| old.contains(cue)) { return None; }
    let before = facts::extract(old);
    let after = facts::extract(&replacement.text);
    if before.len() != 1 || after.len() != 1 || !anchors(&replacement.text).is_empty() { return None; }
    let first = &before[0]; let next = &after[0];
    if !matches!(replacement.text[..next.start].trim(), "" | "是")
        || !replacement.text[next.end..].trim().is_empty() || first.key == next.key { return None; }
    let unit = first.key.split(':').next()?;
    if unit != next.key.split(':').next()? || matches!(unit, "scalar" | "version" | "clock") { return None; }
    let targets = spans.iter().flat_map(|span| facts::extract(&span.text))
        .filter(|fact| fact.key.split(':').next() == Some(unit)).count();
    if targets != 1 { return None; }
    let mut expected = old.clone();
    expected.replace_range(first.start..first.end, &replacement.text[next.start..next.end]);
    Some(NumericRepair { target, replacement: spans.len(), old_key: first.key.clone(), expected })
}

fn acknowledges_retired_value(text: &str, repair: &NumericRepair) -> bool {
    let Some(value) = text.strip_prefix("前面").and_then(|text| text.strip_suffix("是我说错了")) else { return false; };
    let numeric = facts::extract(value);
    numeric.len() == 1 && numeric[0].start == 0 && numeric[0].end == value.len()
        && numeric[0].key == repair.old_key
}

fn matching_targets(spans: &[Span<'_>], old: &str) -> Vec<(usize, usize, usize)> {
    let expected_anchor = anchors(old).into_iter().next();
    let expected_fact = facts::extract(old).into_iter().next();
    let mut matches = Vec::new();
    for (index, span) in spans.iter().enumerate() {
        if let Some(expected) = &expected_anchor {
            matches.extend(anchors(&span.text).into_iter().filter(|anchor| anchor.kind == expected.kind).map(|anchor| (index, anchor.start, anchor.end)));
        } else if let Some(expected) = &expected_fact {
            matches.extend(facts::extract(&span.text).into_iter().filter(|fact| fact.key == expected.key).map(|fact| (index, fact.start, fact.end)));
        }
    }
    matches
}

// True means an explicit repair is ambiguous. Do not let the downstream local
// scope matcher accidentally satisfy two different owners using one old value.
fn resolve_correction<'a>(spans: &mut [Span<'a>], replacement: &Span<'a>) -> bool {
    // Explicit old value is authoritative only if it identifies one target.
    if let Some((old, new)) = partial_correction(&replacement.text) {
        let matches = matching_targets(spans, old);
        if matches.len() == 1 {
            let (index, start, end) = matches[0];
            spans[index].text.replace_range(start..end, new);
            extend_origins(&mut spans[index], &replacement.origins);
        }
        return matches.len() > 1;
    }
    let dates = anchors(&replacement.text).into_iter().filter(|anchor| date_class(anchor.kind).is_some()).collect::<Vec<_>>();
    // Implicit date repair is intentionally limited to a single typed date.
    // A place/amount clause is never popped to reach it. Multiple dates remain
    // unresolved instead of guessing which appointment the speaker meant.
    if dates.len() == 1 {
        if anchors(&replacement.text).iter().any(|anchor| !matches!(anchor.kind, Kind::Time(_))) {
            return true;
        }
        let date = &dates[0];
        let matches = spans.iter().enumerate().flat_map(|(index, span)| anchors(&span.text).into_iter()
            .filter(|anchor| date_class(anchor.kind) == date_class(date.kind))
            .map(move |anchor| (index, anchor.start, anchor.end))).collect::<Vec<_>>();
        if matches.len() == 1 {
            let (index, start, end) = matches[0];
            let mut remainder = replacement.text.clone(); remainder.replace_range(date.start..date.end, "");
            for word in ["上午", "下午", "凌晨", "中午", "晚上", "早上", "是", "改到", "改成", "改为", "点半", "点", "时", "分"] { remainder = remainder.replace(word, ""); }
            let remainder = remainder.chars().filter(|c| !c.is_whitespace() && !c.is_ascii_digit() && !"零一二两三四五六七八九十".contains(*c)).collect::<String>();
            let previous = &spans[index].text;
            let old_scope = fingerprints(&format!("{}{}", &previous[..start], &previous[end..]));
            let same_action = old_scope == fingerprints(&replacement.text);
            // “今天，不对，明天开会” has no old subject/action to match.
            // Permit this only for a bare date immediately before the repair
            // (ignoring fillers), never across an intervening content clause.
            let adjacent_bare_date = previous[..start].trim().is_empty() && previous[end..].trim().is_empty()
                && spans.iter().rposition(|span| !FILLERS.contains(&span.text.trim()) && !span.text.trim().is_empty()) == Some(index);
            if remainder.is_empty() || same_action || adjacent_bare_date {
                spans[index].text.replace_range(start..end, date.text);
                extend_origins(&mut spans[index], &replacement.origins);
            }
        }
        return matches.len() > 1;
    }
    // Local explicit repairs of one adjacent amount or the same named action
    // remain supported. No intervening content may be discarded.
    let Some(index) = spans.iter().rposition(|span| !FILLERS.contains(&span.text.trim()) && !span.text.trim().is_empty()) else { return false; };
    let old = &spans[index].text;
    let old_facts = facts::extract(old); let new_facts = facts::extract(&replacement.text);
    let replacement_label = new_facts.first().map(|fact| super::without_anchors(&replacement.text[..fact.start])).unwrap_or_default();
    if old_facts.len() == 1 && new_facts.len() == 1 && matches!(replacement_label.trim(), "" | "是") {
        // “不是3500元” / “可能3500元” is not a final positive value. The
        // qualifier must not disappear when selecting a replacement amount.
        if !anchors(&replacement.text).is_empty() { return true; }
        let old_fact = &old_facts[0]; let new_fact = &new_facts[0];
        let old_unit = old_fact.key.split(':').next().unwrap_or("");
        let new_unit = new_fact.key.split(':').next().unwrap_or("");
        // A bare replacement cannot identify one of several prior owners with
        // the same quantity dimension, even if the last one is adjacent.
        if spans.iter().flat_map(|span| facts::extract(&span.text))
            .filter(|fact| fact.key.split(':').next() == Some(old_unit)).count() != 1 { return true; }
        // Omitted currency units in a local budget repair are the one bounded
        // exception. An unrelated person count must not replace an amount.
        let amount_label = super::without_anchors(&old[..old_fact.start]);
        let implicit_cny = old_unit == "scalar" && new_unit == "cny"
            && ["预算", "预算先", "预算是", "金额", "金额是", "费用", "费用是", "价格", "价格是"].contains(&amount_label.trim());
        if old_unit != new_unit && !implicit_cny { return true; }
        spans[index].text.replace_range(old_fact.start..old_fact.end, &replacement.text[new_fact.start..new_fact.end]);
        extend_origins(&mut spans[index], &replacement.origins);
    } else {
        let old_scope = fingerprints(old); let new_scope = fingerprints(&replacement.text);
        if (!old_scope.0.is_empty() || !old_scope.1.is_empty()) && old_scope == new_scope {
            spans[index].text = replacement.text.clone();
            extend_origins(&mut spans[index], &replacement.origins);
        }
    }
    false
}

pub(super) fn analysis_spans(source: &SourceRevision) -> Vec<Span<'_>> {
    let quotes = quote_masks(source);
    let mut raw = Vec::<Span<'_>>::new();
    for item in &source.items {
        for clause in facts::clauses(&item.text) {
            let start = clause.as_ptr() as usize - item.text.as_ptr() as usize;
            raw.push(Span { origins: vec![SourceSlice { source_id: &item.id, start, end: start + clause.len() }], text: clause.to_owned(), unresolved_correction: false });
        }
    }
    // A provider item boundary is not a sentence boundary. Move only the
    // hanging 不是 before 说, and only without explicit intervening punctuation.
    // Output text never undergoes this normalization: “不是。说…” must fail.
    for index in 1..raw.len() {
        let before = &raw[index - 1]; let after = &raw[index];
        let left = before.origins.last().unwrap(); let right = after.origins.first().unwrap();
        if left.source_id == right.source_id || !before.text.ends_with("不是") || !after.text.starts_with('说') { continue; }
        let left_index = source.items.iter().position(|item| item.id == left.source_id).unwrap();
        let right_index = source.items.iter().position(|item| item.id == right.source_id).unwrap();
        if right_index != left_index + 1 { continue; }
        let original = &source.items[left_index];
        let following = &source.items[right_index];
        let horizontal_gap = |text: &str| text.chars().all(|c| matches!(c, ' ' | '\t' | '\u{3000}'));
        if !horizontal_gap(&original.text[left.end..]) || !horizontal_gap(&following.text[..right.start]) { continue; }
        let moved = SourceSlice { source_id: left.source_id, start: left.end - "不是".len(), end: left.end };
        let new_len = raw[index - 1].text.len() - "不是".len();
        raw[index - 1].text.truncate(new_len);
        raw[index - 1].origins.last_mut().unwrap().end -= "不是".len();
        raw[index].text.insert_str(0, "不是");
        raw[index].origins.insert(0, moved);
    }
    let mut result = Vec::new(); let mut pending = Vec::new();
    let mut pending_pause = false;
    let mut recent_repair: Option<NumericRepair> = None;
    for mut span in raw {
        // Consume a receipt at the very next raw clause only. An intervening
        // filler, statement, quote, or different old/unit prevents retraction.
        if let Some(repair) = recent_repair.take() {
            if !quoted_origin(&quotes, &span) && acknowledges_retired_value(&span.text, &repair) {
                extend_origins(&mut result[repair.target], &span.origins);
                extend_origins(&mut result[repair.replacement], &span.origins);
                continue;
            }
        }
        let quoted = quoted_origin(&quotes, &span);
        if !quoted && paused_marker(&span.text) {
            pending.extend(span.origins.clone()); pending_pause = true;
            continue;
        }
        if let Some(marker) = CORRECTIONS.iter().filter(|marker| !quoted && span.text.starts_with(**marker)).max_by_key(|marker| marker.len()) {
            span.text = span.text[marker.len()..].trim().to_owned();
            pending.extend(span.origins.clone());
            if span.text.is_empty() { continue; }
        }
        if !pending.is_empty() && !FILLERS.contains(&span.text.trim()) {
            let repair = (!quoted).then(|| numeric_repair(&result, &span)).flatten();
            extend_origins(&mut span, &pending);
            // New pause-prefixed numeric repairs are enabled only with a
            // positive, same-unit, uniquely identified adjacent value.
            span.unresolved_correction = if quoted || (pending_pause && !facts::extract(&span.text).is_empty() && repair.is_none()) {
                true
            } else { resolve_correction(&mut result, &span) };
            if !span.unresolved_correction {
                recent_repair = repair.filter(|repair| result[repair.target].text == repair.expected);
            }
            pending.clear();
            pending_pause = false;
        } else if !quoted && partial_correction(&span.text).is_some() {
            span.unresolved_correction = resolve_correction(&mut result, &span);
        }
        // The correction instruction contributes only the final value to the
        // semantic view; immutable source and all source references stay intact.
        if !quoted {
            if let Some((_, replacement)) = partial_correction(&span.text) { span.text = replacement.to_owned(); }
        }
        result.push(span);
    }
    // A dangling marker retracts nothing and must keep its source provenance.
    if !pending.is_empty() { result.push(Span { origins: pending, text: String::new(), unresolved_correction: false }); }
    result
}

#[cfg(test)]
#[path = "rewrite_correction_pause_tests.rs"]
mod correction_pause_tests;
