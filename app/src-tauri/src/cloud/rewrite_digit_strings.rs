//! Checks clearly labeled digit strings and digits used in a direct comparison.
//! Unlike a scalar value, an identifier retains its leading zeroes.
use super::super::super::literals;
use std::collections::BTreeMap;

type Sequences = BTreeMap<&'static str, Vec<String>>;
pub(super) const COMPARISON: &str = "comparison";

const LABELS: &[(&str, &str)] = &[
    ("验证码", "verification"), ("校验码", "verification"), ("识别码", "identifier"),
    ("工单编号", "ticket"), ("工单号", "ticket"), ("订单编号", "order"),
    ("订单号", "order"), ("快递单号", "tracking"), ("快递编号", "tracking"),
    ("手机号码", "phone"), ("手机号", "phone"), ("电话号码", "phone"), ("号码", "number"),
    ("尾号", "lastDigits"), ("编号", "identifier"), ("序列号", "serial"),
    ("邮政编码", "postcode"), ("邮编", "postcode"), ("学号", "student"),
];

const LINKERS: &[&str] = &["分别为", "设置为", "也是", "均为", "设为", "是", "为"];
const HORIZONTAL: [char; 3] = [' ', '\t', '\u{3000}'];

pub(in super::super::super) fn digit(c: char) -> Option<char> {
    match c {
        '0'..='9' => Some(c),
        '０'..='９' => Some(char::from(b'0' + (c as u32 - '０' as u32) as u8)),
        '〇' => Some('0'),
        '幺' => Some('1'),
        _ => "零一二三四五六七八九".chars().position(|candidate| candidate == c)
            .map(|value| char::from(b'0' + value as u8)),
    }
}

pub(in super::super::super) fn labeled_prefix(text: &str) -> Option<(&'static str, &str)> {
    let (name, tail) = LABELS.iter().find_map(|&(name, _)| text.strip_prefix(name).map(|tail| (name, tail)))?;
    let tail = tail.trim_start_matches(HORIZONTAL);
    let tail = LINKERS.iter().find_map(|linker| tail.strip_prefix(linker)).unwrap_or(tail);
    let tail = tail.trim_start_matches(HORIZONTAL);
    let tail = tail.strip_prefix(['：', ':']).unwrap_or(tail);
    Some((name, tail.trim_start_matches(HORIZONTAL)))
}

fn labeled_suffix(before: &str) -> Option<(&'static str, &str)> {
    let before = before.trim_end().trim_end_matches(['：', ':']).trim_end();
    let before = LINKERS.iter()
        .find_map(|ending| before.strip_suffix(ending))
        .unwrap_or(before).trim_end();
    LABELS.iter().find_map(|&(name, role)| before.strip_suffix(name).map(|owner| (role, owner.trim())))
}

fn value_boundary(after: &str) -> bool {
    let begins_new_line = after.trim_start_matches(HORIZONTAL).starts_with(['\n', '\r']);
    let after = after.trim_start();
    after.chars().next().is_none_or(|c| "、，,。.!！?？；;）)".contains(c))
        || begins_new_line && after.chars().next().and_then(digit).is_none()
        || ["和", "及", "与", "另一个", "呢", "是否", "吗", "有效", "可用", "已发"]
            .iter().any(|suffix| after.starts_with(suffix))
}

fn list_continues(after: &str) -> bool {
    ["分别", "各自", "逐一"].iter().any(|cue| after.starts_with(cue))
}

// An adjacent ASR line may contain the next independently recorded value.
// A directly following list cue supplies the evidence to keep both values.
fn list_seam(after: &str) -> bool {
    let after = after.trim_start_matches(HORIZONTAL);
    let Some(next) = after.strip_prefix("\r\n").or_else(|| after.strip_prefix('\n')) else {
        return false;
    };
    let next = next.trim_start_matches(HORIZONTAL);
    let len: usize = next.chars().take(24).take_while(|&c| digit(c).is_some()).map(char::len_utf8).sum();
    let tail = next[len..].trim_start_matches(HORIZONTAL);
    let tail = tail.strip_prefix(['，', ',']).unwrap_or(tail).trim_start_matches(HORIZONTAL);
    len > 0 && list_continues(tail)
}

fn comparison(before: &str, after: &str) -> bool {
    let before = before.trim_end().trim_end_matches(['：', ':']).trim_end();
    let following = after.trim_start();
    let cue_after = following.strip_prefix(['，', ',']).unwrap_or(following).trim_start();
    ["应该是", "应当是", "应为", "等于", "得出"].iter().any(|cue| before.ends_with(cue))
        && value_boundary(after)
        || ["应该是", "应当是", "应为", "等于", "减去", "加上", "除以", "乘以"]
            .iter().any(|cue| cue_after.starts_with(cue))
}

fn grouped_ascii(next: &str) -> bool {
    let count = next.bytes().take_while(u8::is_ascii_digit).count();
    count == 3 && next[count..].chars().next().is_none_or(|c| !c.is_ascii_alphanumeric())
}

pub(in super::super) fn extract(text: &str) -> Sequences {
    let mut sequences = BTreeMap::<_, Vec<_>>::new();
    let mut chars = text.char_indices().peekable();
    let mut literals = literals::spans(text).into_iter().peekable();
    let mut previous_identifier = None;
    while let Some((start, first)) = chars.next() {
        let Some(first_digit) = digit(first) else { continue; };
        let mut value = String::from(first_digit);
        let mut end = start + first.len_utf8();
        let mut ascii = first.is_ascii_digit();
        while let Some(&(at, c)) = chars.peek() {
            if let Some(next) = digit(c) {
                value.push(next);
                ascii &= c.is_ascii_digit();
            } else if !(ascii && c == ',' && grouped_ascii(&text[at+1..])) {
                break;
            }
            chars.next();
            end = at + c.len_utf8();
        }
        while literals.peek().is_some_and(|span| span.end <= start) {
            literals.next();
        }
        let before = &text[..start];
        let after = &text[end..];
        let joined_token = |c: char| c.is_ascii_alphanumeric() || digit(c).is_some() || "_/@\\-年月日时点十百千万亿".contains(c);
        let decimal_or_colon = |c: char| matches!(c, '.' | ':' | '．' | '：');
        let continued_number = |tail: &str| {
            let mut chars = tail.chars();
            chars.next().is_some_and(decimal_or_colon) && chars.next().and_then(digit).is_some()
        };
        let mut before_chars = before.chars().rev();
        if literals.peek().is_some_and(|span| span.start < end)
            || before.chars().next_back().is_some_and(joined_token)
            || after.chars().next().is_some_and(joined_token)
            || continued_number(after)
            || before_chars.next().is_some_and(decimal_or_colon) && before_chars.next().and_then(digit).is_some()
        {
            continue;
        }
        let role = labeled_suffix(before).map(|(role, _)| role).or_else(|| {
            let (last_end, last_role) = previous_identifier?;
            let gap = &text[last_end..start];
            (matches!(gap.trim(), "," | "，" | "、" | "和" | "及" | "与")
                || gap.trim().is_empty() && list_seam(&text[last_end..])).then_some(last_role)
        });
        if let Some(role) = role.filter(|_| value_boundary(after) || list_continues(after.trim_start()) || list_seam(after)) {
            sequences.entry(role).or_default().push(value);
            previous_identifier = Some((end, role));
        } else if value.len() >= 3 && comparison(before, after) {
            sequences.entry(COMPARISON).or_default().push(value);
        }
    }
    sequences
}

// Only a single standalone labeled value can be fixed by an adjacent spoken
// correction. Keep the old label when the speaker supplies just the new digits.
pub(in super::super) fn identifier(text: &str) -> Option<(&'static str, usize, &str)> {
    let start = text.char_indices().find_map(|(index, c)| digit(c).map(|_| index))?;
    if !text[start..].chars().all(|c| digit(c).is_some())
        || !literals::spans(text).is_empty()
    {
        return None;
    }
    let (role, owner) = labeled_suffix(&text[..start])?;
    Some((role, start, owner))
}

pub(in super::super) fn standalone(text: &str) -> Option<&str> {
    let rest = text.trim().strip_prefix('是').unwrap_or(text).trim();
    (!rest.is_empty() && rest.chars().all(|c| digit(c).is_some())).then_some(rest)
}
