//! Finite scalar facts for the rewrite guard, not a quantity or semantic parser.
//!
//! Values retain their unit and local label. Corrections are resolved by the
//! caller before extraction. Unsupported notation remains outside this guard.

#[derive(Debug)]
pub(super) struct Fact {
    pub(super) key: String,
    pub(super) start: usize,
    pub(super) end: usize,
}

fn chinese_digit(c: char) -> bool {
    matches!(c, '零' | '〇' | '一' | '二' | '两' | '三' | '四' | '五' | '六' | '七' | '八' | '九' | '十' | '百' | '千' | '万' | '亿')
}

fn prose_number_len(text: &str) -> usize {
    if text.starts_with("万一") || text.starts_with("十足") { return "万一".len(); }
    if let Some(rest) = text.strip_prefix("千万") {
        if ["不要", "别", "不能", "不可", "请勿", "要", "记得", "记住"].iter().any(|word| rest.starts_with(word)) { return "千万".len(); }
    }
    0
}

// Keep decimal/version dots and unambiguous ASCII thousands separators inside
// a clause. In particular, version 0.1.7 must never become three source spans.
pub(super) fn clauses(text: &str) -> Vec<&str> {
    let mut result = Vec::new(); let mut start = 0;
    for (index, c) in text.char_indices() {
        let previous = text[..index].chars().next_back();
        let following = text[index + c.len_utf8()..].chars().next();
        let decimal_dot = c == '.' && previous.is_some_and(|c| c.is_ascii_digit()) && following.is_some_and(|c| c.is_ascii_digit());
        let grouping = c == ',' && previous.is_some_and(|c| c.is_ascii_digit()) && grouped_suffix(&text[index + 1..]);
        if super::is_separator(c) && !decimal_dot && !grouping {
            let clause = text[start..index].trim();
            if !clause.is_empty() { result.push(clause); }
            start = index + c.len_utf8();
        }
    }
    let clause = text[start..].trim();
    if !clause.is_empty() { result.push(clause); }
    result
}

fn grouped_suffix(text: &str) -> bool {
    let digits = text.bytes().take_while(u8::is_ascii_digit).count();
    digits == 3 && text[digits..].chars().next().is_none_or(|c| !c.is_ascii_alphanumeric())
}

// Unit conversions here are deliberately small and exact. Percentage points
// and percentages differ; versions preserve component spelling, including zeros.
const UNITS: &[(&str, &str, u128)] = &[
    ("个百分点", "percentage-point", 1), ("％", "percent", 1), ("%", "percent", 1),
    ("人民币", "cny", 1), ("块钱", "cny", 1), ("元", "cny", 1), ("块", "cny", 1), ("cny", "cny", 1), ("rmb", "cny", 1),
    ("美元", "usd", 1), ("dollars", "usd", 1), ("dollar", "usd", 1), ("usd", "usd", 1),
    ("毫秒", "millisecond", 1), ("ms", "millisecond", 1),
    ("seconds", "millisecond", 1000), ("second", "millisecond", 1000), ("秒", "millisecond", 1000),
    ("minutes", "millisecond", 60_000), ("minute", "millisecond", 60_000), ("分钟", "millisecond", 60_000),
    ("小时", "millisecond", 3_600_000), ("天", "day", 1), ("个月", "month", 1), ("月", "month", 1), ("年", "year", 1), ("日", "date-day", 1),
    ("gib", "gib", 1), ("mib", "mib", 1), ("kib", "kib", 1), ("gb", "gb", 1), ("mb", "mb", 1), ("kb", "kb", 1),
    ("公斤", "kilogram", 1), ("千克", "kilogram", 1), ("kg", "kilogram", 1), ("米", "metre", 1),
    ("人", "person", 1), ("次", "times", 1), ("条", "item", 1), ("个", "count", 1), ("倍", "multiple", 1),
];

fn begins(text: &str, phrase: &str) -> bool {
    text.get(..phrase.len()).is_some_and(|prefix| prefix.eq_ignore_ascii_case(phrase))
        && (!phrase.is_ascii() || text[phrase.len()..].chars().next().is_none_or(|c| !c.is_ascii_alphabetic()))
}

fn decimal(raw: &str, multiplier: u128) -> Option<String> {
    let raw = raw.replace(',', "");
    let normalized = if raw.chars().any(chinese_digit) {
        let mut pieces = raw.split('点');
        let integer = super::chinese_number(pieces.next()?)?.to_string();
        match pieces.next() {
            None => integer,
            Some(fraction) => {
                if pieces.next().is_some() || fraction.is_empty() { return None; }
                let mut digits = String::new();
                for c in fraction.chars() {
                    let digit = super::chinese_number(&c.to_string())?;
                    if digit > 9 { return None; }
                    digits.push(char::from(b'0' + digit as u8));
                }
                format!("{integer}.{digits}")
            }
        }
    } else { raw };
    let mut parts = normalized.split('.');
    let integer = parts.next()?;
    let fraction = parts.next().unwrap_or("");
    if parts.next().is_some() || fraction.len() > 18 { return None; }
    let divisor = 10u128.checked_pow(fraction.len() as u32)?;
    let value = format!("{integer}{fraction}").parse::<u128>().ok()?.checked_mul(multiplier)?;
    let whole = value / divisor; let remainder = value % divisor;
    if remainder == 0 { Some(whole.to_string()) }
    else { Some(format!("{whole}.{remainder:0width$}", width = fraction.len()).trim_end_matches('0').to_owned()) }
}

pub(super) fn extract(text: &str) -> Vec<Fact> {
    let mut result = Vec::new(); let mut offset = 0;
    while offset < text.len() {
        let prose = prose_number_len(&text[offset..]);
        if prose != 0 { offset += prose; continue; }
        let percentage_start = text[offset..].starts_with("百分之").then_some(offset);
        if percentage_start.is_some() { offset += "百分之".len(); if offset == text.len() { break; } }
        let c = text[offset..].chars().next().unwrap();
        if !c.is_ascii_digit() && !chinese_digit(c) { offset += c.len_utf8(); continue; }
        let start = percentage_start.unwrap_or(offset); let numeric_start = offset;
        let arabic = c.is_ascii_digit();
        offset += c.len_utf8();
        while offset < text.len() {
            let next = text[offset..].chars().next().unwrap();
            let dot = arabic && next == '.' && text[offset + 1..].chars().next().is_some_and(|c| c.is_ascii_digit());
            let group = arabic && next == ',' && grouped_suffix(&text[offset + 1..]);
            let kilogram = !arabic && next == '千' && text[offset..].starts_with("千克");
            let fractional_scale = !arabic && text[numeric_start..offset].contains('点') && matches!(next, '千' | '万' | '亿');
            if (arabic && next.is_ascii_digit()) || (!arabic && (chinese_digit(next) || next == '点') && !kilogram && !fractional_scale) || dot || group { offset += next.len_utf8(); }
            else { break; }
        }
        let raw = &text[numeric_start..offset];
        let before = text[..start].trim_end();
        let previous = before.chars().next_back();
        // Source list ordinals are structure, not asserted quantities. Keep
        // real following amounts (第1条预算3500元) available for extraction.
        if previous == Some('第') { continue; }
        let version = raw.matches('.').count() >= 2 || before.ends_with("版本") || before.to_ascii_lowercase().ends_with("version")
            || (previous.is_some_and(|c| c == 'v' || c == 'V') && before[..before.len() - 1].chars().next_back().is_none_or(|c| !c.is_ascii_alphanumeric()));
        // Digits inside product identifiers such as Qwen3.8 are not quantities.
        if previous.is_some_and(|c| c.is_ascii_alphabetic()) && !version { continue; }
        if version {
            result.push(Fact { key: format!("version:{raw}"), start, end: offset });
            continue;
        }
        let mut end = offset; let mut scale = 1u128;
        let suffix = text[end..].trim_start();
        for (word, multiplier) in [("万亿", 1_000_000_000_000u128), ("亿", 100_000_000), ("万", 10_000), ("千", 1000)] {
            if (arabic || raw.contains('点')) && suffix.starts_with(word) && !suffix.starts_with("千克") { scale = multiplier; end = text.len() - suffix.len() + word.len(); break; }
        }
        let suffix = text[end..].trim_start();
        let mut unit = if percentage_start.is_some() { "percent" } else { "scalar" };
        if let Some((word, label, multiplier)) = UNITS.iter().filter(|(word, _, _)| begins(suffix, word)).max_by_key(|(word, _, _)| word.len()) {
            unit = label;
            let Some(multiplier) = scale.checked_mul(*multiplier) else { continue; };
            scale = multiplier; end = text.len() - suffix.len() + word.len();
        } else if previous.is_some_and(|c| matches!(c, '¥' | '￥' | '$')) {
            unit = if previous == Some('$') { "usd" } else { "cny" };
        }
        // A bare 一 in 一下/一般/唯一 is prose, not an asserted numeric fact.
        if !arabic && unit == "scalar" && !raw.chars().any(|c| matches!(c, '十' | '百' | '千' | '万' | '亿')) { continue; }
        if let Some(value) = decimal(raw, scale) {
            let negative = previous == Some('负') || (previous == Some('-') && before[..before.len() - 1].chars().next_back().is_none_or(|c| !c.is_ascii_alphanumeric()));
            result.push(Fact { key: format!("{unit}:{}{value}", if negative { "-" } else { "" }), start, end });
            offset = end;
        }
    }
    result
}

/// Match the label immediately before a quantity. Two amounts in one sentence
/// cannot satisfy each other merely because the unordered number set matches.
pub(super) fn local_label<'a>(text: &'a str, facts: &[Fact], index: usize) -> &'a str {
    let fact = &facts[index];
    let start = if index == 0 { 0 } else { facts[index - 1].end };
    let prefix = &text[start..fact.start];
    let mut label = prefix;
    for separator in ["以及", "和", "及", "、", " and "] {
        if let Some((_, trailing)) = label.rsplit_once(separator) { label = trailing; }
    }
    label
}

pub(super) fn local_scope(text: &str, facts: &[Fact], index: usize) -> String {
    let fact = &facts[index];
    // A leading qualifier such as 不是5000元 has no independent subject label;
    // the caller's qualifier guard binds its negation to the actual value.
    let mut label = super::without_anchors(local_label(text, facts, index));
    if fact.key.starts_with("version:") {
        label = label.replace("版本", " ");
        if let Some(prefix) = label.trim_end().strip_suffix("version").or_else(|| label.trim_end().strip_suffix("Version")) { label = prefix.to_owned(); }
        else if let Some(prefix) = label.trim_end().strip_suffix('v').or_else(|| label.trim_end().strip_suffix('V')) { label = prefix.to_owned(); }
    }
    label
}

pub(super) fn canonical_text(text: &str) -> String {
    let mut result = String::new(); let mut start = 0;
    for fact in extract(text) {
        result.push_str(&text[start..fact.start]); result.push(' '); result.push_str(&fact.key); result.push(' '); start = fact.end;
    }
    result.push_str(&text[start..]); result
}
