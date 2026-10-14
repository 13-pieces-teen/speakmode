//! Bounded spoken whole-number measurements. No dates, entities or free prose
//! inference. Keep byte offsets in the original text; never rewrite the source.
use super::{begins, Fact, UNITS};

fn small(word: &str) -> Option<u128> {
    ["zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten", "eleven", "twelve", "thirteen", "fourteen", "fifteen", "sixteen", "seventeen", "eighteen", "nineteen"]
        .iter().position(|v| *v == word).map(|v| v as u128)
}
fn tens(word: &str) -> Option<u128> {
    ["twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety"]
        .iter().position(|v| *v == word).map(|v| (v as u128 + 2) * 10)
}
fn under_hundred(words: &[&str]) -> Option<u128> {
    match words {
        [a] => small(a).or_else(|| tens(a)),
        [a, b] => Some(tens(a)? + small(b).filter(|v| (1..=9).contains(v))?),
        _ => None,
    }
}
fn hundreds(words: &[&str]) -> Option<u128> {
    let Some(index) = words.iter().position(|v| *v == "hundred") else { return under_hundred(words); };
    let base = under_hundred(&words[..index]).filter(|v| *v > 0)? * 100;
    let rest = &words[index + 1..];
    if rest.is_empty() { return Some(base); }
    let rest = rest.strip_prefix(&["and"]).unwrap_or(rest);
    Some(base + under_hundred(rest)?)
}
fn number(words: &[&str]) -> Option<u128> {
    let Some(index) = words.iter().position(|v| *v == "thousand") else { return hundreds(words); };
    let base = hundreds(&words[..index]).filter(|v| (1..1000).contains(v))? * 1000;
    let rest = &words[index + 1..];
    if rest.is_empty() { return Some(base); }
    let rest = rest.strip_prefix(&["and"]).unwrap_or(rest);
    Some(base + hundreds(rest).filter(|v| *v < 1000)?)
}
fn number_word(word: &str) -> bool {
    small(word).is_some() || tens(word).is_some() || ["hundred", "thousand", "and"].contains(&word)
}
fn numeric_context(word: &str) -> bool {
    // Do not extract a supported suffix of an unsupported decimal/scale/sign.
    number_word(word) || ["million", "billion", "point", "minus", "negative"].contains(&word)
}

pub(super) fn extract_at(text: &str, start: usize) -> Option<Fact> {
    if text[..start].chars().next_back().is_some_and(|c| c.is_ascii_alphanumeric() || c == '_') { return None; }
    let before = text[..start].trim_end_matches(|c: char| c.is_whitespace() || c == '-');
    let previous = before.rsplit(|c: char| !c.is_ascii_alphabetic()).next().unwrap_or("").to_ascii_lowercase();
    if numeric_context(&previous) && previous != "and" { return None; }
    if previous == "and" {
        let prefix=before[..before.len()-3].trim_end_matches(|c:char|c.is_whitespace()||c=='-');
        let preceding=prefix.rsplit(|c:char|!c.is_ascii_alphabetic()).next().unwrap_or("").to_ascii_lowercase();
        if numeric_context(&preceding) { return None; }
    }
    let mut offset = start;
    let mut words = Vec::new();
    for _ in 0..12 {
        let size = text[offset..].bytes().take_while(u8::is_ascii_alphabetic).count();
        if size == 0 { return None; }
        let word = text[offset..offset + size].to_ascii_lowercase();
        if !number_word(&word) { return None; }
        words.push(word); offset += size;
        let rest = text[offset..].trim_start();
        if let Some((unit, label, multiplier)) = UNITS.iter().filter(|(unit, _, _)| begins(rest, unit)).max_by_key(|(unit, _, _)| unit.len()) {
            let value = number(&words.iter().map(String::as_str).collect::<Vec<_>>())?.checked_mul(*multiplier)?;
            return Some(Fact { key: format!("{label}:{value}"), start, end: text.len() - rest.len() + unit.len() });
        }
        let tail = text[offset..].trim_start_matches(|c: char| c.is_whitespace() || c == '-');
        if tail.len() == text.len() - offset { return None; }
        offset = text.len() - tail.len();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn spoken_measurements_have_exact_values_and_original_byte_offsets() {
        for (spoken, key) in [("twenty seven hundred dollars", "usd:2700"), ("two thousand seven hundred dollars", "usd:2700"),
            ("sixty seconds", "millisecond:60000"), ("twenty-seven dollars", "usd:27"), ("one hundred and five dollars", "usd:105"), ("two years", "year:2")] {
            let text = format!("嗯 I have {spoken} remaining."); let start = text.find(spoken).unwrap();
            let fact = extract_at(&text, start).unwrap(); assert_eq!(fact.key, key); assert_eq!(&text[fact.start..fact.end], spoken);
        }
    }
    #[test]
    fn unsupported_forms_do_not_turn_their_tail_into_a_measurement() {
        for text in ["one two dollars", "twenty twenty seven dollars", "one hundred and dollars", "one thousand thousand dollars",
            "someone dollars", "one_point_two dollars", "twenty seven", "one million two dollars", "two point five dollars", "negative five dollars",
            "one thousand thousand and five dollars", "one million and five dollars"] {
            for (offset, _) in text.char_indices() { assert!(extract_at(text, offset).is_none(), "{text} at {offset}"); }
        }
    }
}
