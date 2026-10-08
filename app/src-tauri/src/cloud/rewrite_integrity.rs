//! A deliberately finite, deterministic rewrite risk check.
//!
//! This is not a semantic equivalence proof or a general Chinese parser. It
//! protects a small set of high-value qualifiers and checks their local scope
//! inside blocks citing the same immutable source. It never repairs text,
//! invents references, or includes dictated content in its static error.

use super::CloudError;
use crate::core::{OutputBlock, SourceRevision};
use std::collections::HashSet;

#[path = "rewrite_facts.rs"]
mod facts;

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
    ("如果", Kind::Conditional), ("假如", Kind::Conditional), ("假若", Kind::Conditional), ("倘若", Kind::Conditional), ("要是", Kind::Conditional), ("万一", Kind::Conditional), ("若", Kind::Conditional), ("情况下", Kind::Conditional),
    ("只有", Kind::NecessaryCondition), ("仅当", Kind::NecessaryCondition),
    ("除非", Kind::Exception), ("否则", Kind::Otherwise), ("不然", Kind::Otherwise),
    ("可能", Kind::Possibility), ("也许", Kind::Possibility), ("或许", Kind::Possibility), ("大概", Kind::Possibility), ("不一定", Kind::Possibility), ("未必", Kind::Possibility), ("有可能", Kind::Possibility),
    ("还没有确定", Kind::Pending), ("还没有确认", Kind::Pending), ("还没确定", Kind::Pending), ("还没确认", Kind::Pending),
    ("尚未确定", Kind::Pending), ("尚未确认", Kind::Pending), ("未确定", Kind::Pending), ("未确认", Kind::Pending),
    ("没有确定", Kind::Pending), ("没有确认", Kind::Pending), ("没确定", Kind::Pending), ("没确认", Kind::Pending),
    ("不能确定", Kind::Pending), ("不确定", Kind::Pending), ("待确认", Kind::Pending), ("待确定", Kind::Pending), ("待定", Kind::Pending), ("未定", Kind::Pending),
    ("不要", Kind::Prohibition), ("请勿", Kind::Prohibition), ("切勿", Kind::Prohibition), ("禁止", Kind::Prohibition), ("不得", Kind::Prohibition), ("暂不", Kind::Prohibition), ("暂时不", Kind::Prohibition), ("暂勿", Kind::Prohibition), ("别", Kind::Prohibition),
    ("并不是", Kind::NegativeFact), ("不是", Kind::NegativeFact), ("并非", Kind::NegativeFact), ("不等于", Kind::NegativeFact), ("≠", Kind::NegativeFact),
    ("尚未", Kind::NegativeStatus), ("还没有", Kind::NegativeStatus), ("还没", Kind::NegativeStatus), ("没有", Kind::NegativeStatus), ("并未", Kind::NegativeStatus), ("未曾", Kind::NegativeStatus), ("没", Kind::NegativeStatus),
    ("不会", Kind::Negative), ("不能", Kind::Negative), ("无法", Kind::Negative), ("不可能", Kind::Negative), ("不可以", Kind::Negative), ("不", Kind::Negative),
    ("unless", Kind::Exception), ("if", Kind::Conditional),
    ("might", Kind::Possibility), ("may", Kind::Possibility), ("could", Kind::Possibility),
    ("not", Kind::Negative), ("never", Kind::Negative), ("cannot", Kind::Negative),
    ("can't", Kind::Negative), ("don't", Kind::Negative), ("doesn't", Kind::Negative), ("didn't", Kind::Negative),
];

// Lexical idioms are not the negative assertions covered by this guard. This
// list also prevents "别" inside "特别" and "不" inside "不得不".
const NON_ANCHORS: &[&str] = &[
    "不仅", "不但", "不错", "不少", "不妨", "不如", "不止", "不得不", "不约而同", "不久", "不管", "无论",
    "特别", "分别", "个别", "别的", "别人", "别处", "别名", "没错", "没关系", "没事儿", "not only",
];
const CORRECTIONS: &[&str] = &["不对", "说错了", "我说错了", "更正一下", "更正为", "改口", "准确地说"];
const FILLERS: &[&str] = &["嗯", "呃", "啊", "那个", "这个", "就是说", "就是", "然后", "我说"];

#[derive(Debug)]
struct Span<'a> { source_id: &'a str, text: String }
#[derive(Debug)]
struct Anchor<'a> { text: &'a str, kind: Kind, start: usize, end: usize }

fn is_separator(c: char) -> bool {
    matches!(c, '，' | ',' | '。' | '.' | '！' | '!' | '？' | '?' | '；' | ';' | '\n' | '\r')
}

fn effective_spans(source: &SourceRevision) -> Vec<Span<'_>> {
    let mut spans: Vec<Span<'_>> = Vec::new();
    for item in &source.items {
        for raw in facts::clauses(&item.text) {
            // Only explicit single-target replacements retract part of a clause.
            // Unrelated locations, conditions and qualifiers remain protected.
            if let Some((old, replacement)) = partial_correction(raw) {
                for span in spans.iter_mut().rev() {
                    if let Some((start, end)) = correction_target(&span.text, old) {
                        span.text.replace_range(start..end, replacement);
                        break;
                    }
                }
                spans.push(Span { source_id: &item.id, text: replacement.to_owned() });
                continue;
            }
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

fn correction_target(text: &str, old: &str) -> Option<(usize, usize)> {
    if let Some(expected) = anchors(old).first() {
        return anchors(text).into_iter().rev().find(|anchor| anchor.kind == expected.kind).map(|anchor| (anchor.start, anchor.end));
    }
    let expected = facts::extract(old).into_iter().next()?;
    facts::extract(text).into_iter().rev().find(|fact| fact.key == expected.key).map(|fact| (fact.start, fact.end))
}

fn partial_correction(raw: &str) -> Option<(&str, &str)> {
    let raw = CORRECTIONS.iter().filter(|marker| raw.starts_with(**marker)).max_by_key(|marker| marker.len()).map_or(raw, |marker| raw[marker.len()..].trim());
    let rest = raw.strip_prefix('把').or_else(|| raw.strip_prefix('将'))?;
    let (old, replacement) = ["改成", "改为", "改到"].iter().find_map(|marker| rest.split_once(marker))?;
    let old = old.trim(); let replacement = replacement.trim();
    let single_fact = |text: &str| {
        let anchors = anchors(text);
        (anchors.len() == 1 && anchors[0].text.len() == text.len() && matches!(anchors[0].kind, Kind::Time(_)))
            || { let numbers = facts::extract(text); numbers.len() == 1 && numbers[0].start == 0 && numbers[0].end == text.len() }
    };
    (single_fact(old) && single_fact(replacement)).then_some((old, replacement))
}

fn phrase_at(text: &str, offset: usize, phrase: &str) -> bool {
    if !phrase.is_ascii() { return text[offset..].starts_with(phrase); }
    text.get(offset..offset + phrase.len()).is_some_and(|candidate| candidate.eq_ignore_ascii_case(phrase))
        && text[..offset].chars().next_back().is_none_or(|c| !c.is_ascii_alphanumeric() && c != '\'')
        && text[offset + phrase.len()..].chars().next().is_none_or(|c| !c.is_ascii_alphanumeric() && c != '\'')
}

fn anchors(text: &str) -> Vec<Anchor<'_>> {
    let mut found = Vec::new();
    let mut offset = 0;
    while offset < text.len() {
        let remainder = &text[offset..];
        let protected = PHRASES.iter().filter(|entry| phrase_at(text, offset, entry.0)).max_by_key(|entry| entry.0.len());
        let neutral = NON_ANCHORS.iter().filter(|phrase| phrase_at(text, offset, phrase)).max_by_key(|phrase| phrase.len());
        if neutral.is_some_and(|phrase| protected.is_none_or(|(candidate, _)| phrase.len() >= candidate.len())) {
            offset += neutral.unwrap().len();
        } else if let Some((phrase, kind)) = protected {
            found.push(Anchor { text: &text[offset..offset + phrase.len()], kind: *kind, start: offset, end: offset + phrase.len() });
            offset += phrase.len();
        } else {
            offset += remainder.chars().next().unwrap().len_utf8();
        }
    }
    found
}

fn without_anchors(text: &str) -> String {
    let mut result = String::new(); let mut start = 0;
    for anchor in anchors(text) {
        result.push_str(&text[start..anchor.start]); result.push(' '); start = anchor.end;
    }
    result.push_str(&text[start..]); result
}

/// Guidance for the existing rewrite request; no revoked qualifier is demanded.
/// The immutable source still remains the sole authority for the output text.
pub(super) fn preservation_hints(source: &SourceRevision) -> Vec<String> {
    let mut seen = HashSet::new();
    effective_spans(source).iter().flat_map(|span| {
        let mut hints = anchors(&span.text).into_iter().map(|anchor| (anchor.start, anchor.text.to_owned())).collect::<Vec<_>>();
        hints.extend(facts::extract(&span.text).into_iter().map(|fact| (fact.start, span.text[fact.start..fact.end].to_owned())));
        hints.sort_by_key(|(start, _)| *start);
        hints.into_iter().map(|(_, text)| text)
    }).filter(|text| seen.insert(text.clone())).collect()
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
    for c in without_anchors(text).chars() {
        if c.is_whitespace() { gap.push(c); continue; }
        if !(previous_ascii_letter && c.is_ascii_alphabetic()) { result.push_str(&gap); }
        gap.clear(); result.extend(c.to_lowercase()); previous_ascii_letter = c.is_ascii_alphabetic();
    }
    result.push_str(&gap);
    // Remove qualifiers themselves before comparing local subjects/actions. A
    // qualifier on an unrelated bullet cannot satisfy the check by itself.
    for (from, to) in [("发送", "发"), ("发出", "发"), ("移除", "删除"), ("检测", "检查"), ("对比", "比较"), ("使用", "用"), ("试一下", "试用")] { result = result.replace(from, to); }
    // Actual quantities were canonicalized before this scope-only cleanup;
    // remaining 千万/十足 are the finite prose expressions skipped by extract.
    for filler in ["千万", "十足", "首先", "然后", "但是", "不过", "我们", "你们", "他们", "同样", "这个", "那个", "就是说", "就是", "已经", "暂时", "具体", "确认", "确定", "需要", "进行", "可以", "请", "先", "再", "还", "也", "是", "的", "了"] { result = result.replace(filler, " "); }
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
        else if unit == 10_000 { section = section.checked_add(number)?.max(1).checked_mul(unit)?; }
        else { total = total.checked_add(section)?.checked_add(number)?.max(1).checked_mul(unit)?; section = 0; }
        number = 0;
    }
    total.checked_add(section)?.checked_add(number)
}

fn fingerprints(text: &str) -> (HashSet<String>, HashSet<String>) {
    let canonical = facts::canonical_text(text);
    let text = canonical.as_str();
    if text.chars().any(|c| c.is_ascii_alphabetic()) && !text.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)) {
        // English words retain boundaries. Compacting them like spaced product
        // names would turn "do not send" into an unmatchable identifier.
        let stripped = without_anchors(text).to_ascii_lowercase();
        let strong = stripped.split(|c: char| !c.is_ascii_alphanumeric()).filter(|token| !token.is_empty())
            .filter(|token| !["a", "an", "the", "we", "you", "they", "it", "is", "are", "be", "do", "does", "did", "to", "will", "would", "should", "must", "please"].contains(token))
            .map(|token| match token { "invitation" => "invite", "sending" => "send", "deleting" => "delete", "confirmed" => "confirm", other => other }.to_owned()).collect();
        return (strong, HashSet::new());
    }
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
        for clause in facts::clauses(plain) {
            candidates.push(if is_bullet && !header.is_empty() { format!("{header} {clause}") } else { clause.to_owned() });
        }
    }
    candidates
}

fn english_negative_scope<'a>(text: &'a str, anchor: &Anchor<'_>) -> &'a str {
    let end = anchors(text).into_iter().find(|next| next.start >= anchor.end).map_or(text.len(), |next| next.start);
    &text[anchor.end..end]
}

fn number_is_negative(text: &str, numeric: &[facts::Fact], index: usize) -> bool {
    anchors(facts::local_label(text, numeric, index)).iter().any(|anchor| matches!(anchor.kind, Kind::Negative | Kind::NegativeFact | Kind::NegativeStatus | Kind::Prohibition))
}

pub(super) fn validate_meaning(source: &SourceRevision, blocks: &[OutputBlock]) -> Result<(), CloudError> {
    // Source completeness and strict citation coverage are checked by the caller
    // before this guard. References here are never modified or guessed.
    // Parse each output clause once. Repeated ASR fragments must not repeatedly
    // tokenize all output text on the UI's request completion path.
    let candidates = blocks.iter().map(|block| (block, scoped_candidates(&block.text).into_iter().map(|clause| {
        let kinds = anchors(&clause).into_iter().map(|anchor| anchor.kind).collect::<HashSet<_>>();
        let numeric = facts::extract(&clause);
        let numeric_scopes = numeric.iter().enumerate().map(|(index, fact)| (fact.key.clone(), fingerprints(&facts::local_scope(&clause, &numeric, index)), number_is_negative(&clause, &numeric, index))).collect::<Vec<_>>();
        let english_negatives = anchors(&clause).into_iter().filter(|anchor| anchor.text.is_ascii() && anchor.kind == Kind::Negative)
            .map(|anchor| fingerprints(english_negative_scope(&clause, &anchor))).collect::<Vec<_>>();
        (kinds, fingerprints(&clause), numeric_scopes, english_negatives)
    }).collect::<Vec<_>>())).collect::<Vec<_>>();
    let spans = effective_spans(source);
    for span in &spans {
        let protected = anchors(&span.text);
        let scope = fingerprints(&span.text);
        let english_negative_count = protected.iter().filter(|anchor| anchor.text.is_ascii() && anchor.kind == Kind::Negative).count();
        for anchor in protected {
            let negative_scope = (anchor.text.is_ascii() && anchor.kind == Kind::Negative).then(|| fingerprints(english_negative_scope(&span.text, &anchor)));
            let retained = candidates.iter().filter(|(block, _)| block.source_refs.iter().any(|id| id == span.source_id)).flat_map(|(_, clauses)| clauses).any(|(kinds, candidate_scope, _, english_negatives)| {
                kinds.iter().any(|candidate| same_kind(anchor.kind, *candidate)) && scope_matches(&scope, candidate_scope)
                    && negative_scope.as_ref().is_none_or(|expected| english_negatives.len() == english_negative_count && english_negatives.iter().any(|candidate| scope_matches(expected, candidate)))
            });
            if !retained { return Err(CloudError::new("rewrite_meaning_risk", "rewrite", "整理结果可能遗漏时间、条件或否定等限定，请核对识别稿或重新整理")); }
        }
        let numeric = facts::extract(&span.text);
        for (index, fact) in numeric.iter().enumerate() {
            let scope = fingerprints(&facts::local_scope(&span.text, &numeric, index));
            let negative = number_is_negative(&span.text, &numeric, index);
            let retained = candidates.iter().filter(|(block, _)| block.source_refs.iter().any(|id| id == span.source_id)).flat_map(|(_, clauses)| clauses).flat_map(|(_, _, numbers, _)| numbers)
                .any(|(key, candidate_scope, candidate_negative)| *key == fact.key && negative == *candidate_negative && scope_matches(&scope, candidate_scope));
            if !retained { return Err(CloudError::new("rewrite_meaning_risk", "rewrite", "整理结果可能改变数值、单位或版本，请核对识别稿或重新整理")); }
        }
    }
    validate_added_facts(&spans, blocks)
}

// The forward check above cannot detect "budget 3500, tax 500" when only the
// budget was dictated. Check a deliberately bounded set of additions too. Bare
// scalars are excluded: numbering a list or normalizing prose like "一、" is
// presentation, and this finite parser cannot distinguish every such case.
// This is still NOT semantic equivalence: facts without these numeric units,
// unsupported notations, paraphrases and new unqualified prose are unverified.
fn addition_checked_key(key: &str) -> bool { !key.starts_with("scalar:") }

fn adjacent_equivalence(text: &str, numeric: &[facts::Fact], index: usize) -> bool {
    if index == 0 { return false; }
    let previous = &numeric[index - 1]; let current = &numeric[index];
    if previous.key != current.key || number_is_negative(text, numeric, index - 1) != number_is_negative(text, numeric, index) { return false; }
    // Permit only a directly adjacent parenthetical equivalent representation,
    // such as 3秒（即3000毫秒）. A new label (税款3500元), changed value, changed
    // polarity or separated assertion must still find its own source support.
    let gap = text[previous.end..current.start].trim();
    let Some((opening, mut inner)) = gap.strip_prefix('（').map(|inner| ('（',inner))
        .or_else(||gap.strip_prefix('(').map(|inner| ('(',inner))) else { return false; };
    inner = inner.trim();
    for cue in ["也就是", "等于", "即"] {
        if let Some(rest) = inner.strip_prefix(cue) { inner = rest.trim(); break; }
    }
    if !inner.is_empty() && !["不是", "并非", "不等于", "≠"].contains(&inner) { return false; }
    text[current.end..].trim_start().starts_with(if opening == '（' { '）' } else { ')' })
}

fn validate_added_facts(spans: &[Span<'_>], blocks: &[OutputBlock]) -> Result<(), CloudError> {
    let source_facts = spans.iter().flat_map(|span| {
        let numeric = facts::extract(&span.text);
        numeric.iter().enumerate().filter(|(_, fact)| addition_checked_key(&fact.key)).map(|(index, fact)|
            (span.source_id, fact.key.clone(), fingerprints(&facts::local_scope(&span.text, &numeric, index)), number_is_negative(&span.text, &numeric, index)))
            .collect::<Vec<_>>()
    }).collect::<Vec<_>>();
    for block in blocks {
        for clause in scoped_candidates(&block.text) {
            let numeric = facts::extract(&clause);
            let mut directly_supported = vec![false; numeric.len()];
            for (index, fact) in numeric.iter().enumerate().filter(|(_, fact)| addition_checked_key(&fact.key)) {
                let scope = fingerprints(&facts::local_scope(&clause, &numeric, index));
                let negative = number_is_negative(&clause, &numeric, index);
                let supported = source_facts.iter().any(|(source_id, key, source_scope, source_negative)|
                    block.source_refs.iter().any(|id| id == source_id) && *key == fact.key
                        && *source_negative == negative && scope_matches(source_scope, &scope));
                directly_supported[index] = supported;
                if !supported && !(index > 0 && directly_supported[index - 1] && adjacent_equivalence(&clause, &numeric, index)) {
                    return Err(CloudError::new("rewrite_meaning_risk", "rewrite", "整理结果可能新增数值、单位或版本，请核对识别稿或重新整理"));
                }
            }
            // Do not infer that a qualifier was invented merely because the
            // source lexicon did not find it. Spoken "等确认了再发" can legally
            // become "如果确认了，再发". Existing forward qualifier checks stay;
            // arbitrary new conditions/prose remain outside this finite guard.
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
    #[test]
    fn amounts_units_percentages_and_versions_are_not_interchangeable() {
        for (input, legal, changed) in [
            ("预算三千五百元。", "预算3500元。", "预算35000元。"),
            ("预算3.5万元。", "预算35000元。", "预算3.5元。"),
            ("预算三点五万元。", "预算35000元。", "预算3.5元。"),
            ("今天预算3.5万元。", "今日预算35000元。", "今日预算3500元。"),
            ("降低3%。", "降低3.0％。", "降低3个百分点。"),
            ("降低百分之三。", "降低3%。", "降低30%。"),
            ("等待3秒。", "等待3000毫秒。", "等待3分钟。"),
            ("版本1.7。", "版本 1.7。", "版本1.70。"),
            ("升级到版本0.1.7。", "升级到版本 0.1.7。", "升级到版本0.7。"),
            ("预算1,500元。", "预算1500元。", "预算500元。"),
        ] {
            assert!(accepted(&[input], legal), "legal: {input} / {legal}");
            assert!(!accepted(&[input], changed), "changed: {input} / {changed}");
        }
    }
    #[test]
    fn scalar_facts_keep_their_local_label_and_source_reference() {
        assert!(!accepted(&["预算5000元，已付3500元。"], "预算3500元，已付5000元。"));
        assert!(!accepted(&["预算5000元和已付3500元。"], "预算3500元和已付5000元。"));
        assert!(accepted(&["预算5000元和已付3500元。"], "预算为5000元，已付3500元。"));
        let source = source(&["预算5000元。", "已付3500元。"]);
        assert!(validate_meaning(&source, &[block("预算5000元。", &[1]), block("已付3500元。", &[0])]).is_err());
    }
    #[test]
    fn corrected_amount_keeps_the_final_value_and_meaningful_contrast() {
        let input = ["预算五千，不对，三千五百元，不是五千元。"];
        assert!(accepted(&input, "预算3500元，不是5000元。"));
        assert!(!accepted(&input, "预算5000元，不是3500元。"));
        assert!(!accepted(&input, "预算3500元。"));
        assert!(!accepted(&input, "预算不是3500元，也不是5000元。"));
        assert!(!accepted(&["预算不是5000元而是3500元。"], "预算是5000元而不是3500元。"));
        assert!(accepted(&["预算不是五千元而是三千五百元。"], "预算不是5000元而是3500元。"));
    }
    #[test]
    fn explicit_partial_day_correction_retains_the_other_clause() {
        let input = ["周五在实验室开会，地点不要改。", "把周五改成周六。"];
        assert!(accepted(&input, "周六在实验室开会，地点不要改。"));
        assert!(!accepted(&input, "周五在实验室开会，地点不要改。"));
        assert!(!accepted(&input, "周六在实验室开会，地点可以改。"));
        // Even after an explicit correction marker, a targeted date repair must
        // not pop the immediately preceding unrelated location restriction.
        let input = ["周五开会，地点不要改。", "更正一下把周五改成周六。"];
        assert!(accepted(&input, "周六开会，地点不要改。"));
        assert!(!accepted(&input, "周六开会，地点可以改。"));
    }
    #[test]
    fn explicit_partial_amount_correction_retains_other_qualifiers() {
        let input = ["预算5000元，地点不要改。把5000元改成3500元。"];
        assert!(accepted(&input, "预算3500元，地点不要改。"));
        assert!(!accepted(&input, "预算5000元，地点不要改。"));
        assert!(!accepted(&input, "预算3500元，地点可以改。"));
        assert_eq!(preservation_hints(&source(&input)), ["3500元", "不要"]);
        assert!(accepted(&["预算五千元，地点不要改。把5000元改成3500元。"], "预算3500元，地点不要改。"));
        assert!(accepted(&["星期五开会。把周五改成周六。"], "星期六开会。"));
    }
    #[test]
    fn english_negation_exception_and_uncertainty_keep_their_action() {
        for (input, legal, changed) in [
            ("Do not send the invitation unless approved.", "Never send the invite unless approved.", "Send the invite unless approved."),
            ("Do not send the invitation unless approved.", "Do not send the invite unless approved.", "Do not send the invite."),
            ("We might ship Friday.", "We may ship Friday.", "We will ship Friday."),
            ("Don't delete recordings.", "Do not delete recordings.", "Delete recordings."),
        ] {
            assert!(accepted(&[input], legal), "legal: {input} / {legal}");
            assert!(!accepted(&[input], changed), "changed: {input} / {changed}");
        }
        assert!(!accepted(&["Do not delete recordings, send invitations."], "Delete recordings, do not send invitations."));
        assert!(!accepted(&["Budget is not 5000 dollars. Budget is 3500 dollars."], "Budget is 5000 dollars. Budget is not 3500 dollars."));
        assert!(!accepted(&["Do not ship unless approved."], "Ship unless not approved."));
        assert!(!accepted(&["Do not ship unless approved."], "Do not ship unless not approved."));
    }
    #[test]
    fn english_qualifiers_have_case_insensitive_word_boundaries() {
        assert!(accepted(&["We MIGHT ship Friday."], "We may ship Friday."));
        assert!(accepted(&["Open the Nottingham notebook."], "Open the Nottingham notebook."));
        assert!(accepted(&["Not only fast, but stable."], "Fast and stable."));
        assert!(preservation_hints(&source(&["Open the Nottingham notebook."])).is_empty());
    }
    #[test]
    fn decimals_and_versions_do_not_create_false_clause_boundaries() {
        assert_eq!(facts::clauses("预算3.5万元。版本0.1.7。"), ["预算3.5万元", "版本0.1.7"]);
        assert_eq!(facts::clauses("预算1,500元，人数3人。"), ["预算1,500元", "人数3人"]);
        assert!(facts::extract("使用Qwen3.8和SpeakMode。").is_empty());
        assert_eq!(facts::extract("3千克")[0].key, "kilogram:3");
        assert_eq!(facts::extract("三千克")[0].key, "kilogram:3");
    }
    #[test]
    fn numeric_boundaries_prose_idioms_are_not_scalar_facts() {
        for input in ["千万不要发邀请。", "万一有问题就不要发邀请。", "我有十足的把握。"] {
            assert!(facts::extract(input).is_empty(), "prose must not become a quantity: {input}");
        }
        assert!(accepted(&["千万不要发邀请。"], "不要发邀请。"));
        assert!(accepted(&["万一有问题就不要发邀请。"], "如果有问题就不要发邀请。"));
        assert!(!accepted(&["万一有问题就不要发邀请。"], "有问题就不要发邀请。"));
        assert_eq!(facts::extract("预算千万元。")[0].key, "cny:10000000");
    }
    #[test]
    fn numeric_boundaries_ordinals_allow_normal_rephrasing() {
        for input in ["第一项检查麦克风。", "第一条检查麦克风。", "第1条检查麦克风。"] {
            assert!(facts::extract(input).is_empty(), "ordinal must not become a quantity: {input}");
        }
        assert!(accepted(&["第一条检查麦克风。"], "第1条检查麦克风。"));
        assert!(accepted(&["第1条检查麦克风。"], "第一条检查麦克风。"));
        assert!(accepted(&["第一条检查麦克风。"], "首先检查麦克风。"));
    }
    #[test]
    fn numeric_boundaries_nested_large_units_are_exact() {
        assert_eq!(facts::extract("预算一万亿元。")[0].key, "cny:1000000000000");
        assert!(accepted(&["预算一万亿元。"], "预算1000000000000元。"));
        assert!(!accepted(&["预算一万亿元。"], "预算100010000元。"));
        assert!(accepted(&["预算1万亿元。"], "预算1000000000000元。"));
        assert!(accepted(&["预算三点五万亿元。"], "预算3500000000000元。"));
    }
}
