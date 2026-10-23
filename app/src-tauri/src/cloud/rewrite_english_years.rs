//! Exact, bounded calendar-year notation after "in" or "year". Read the whole
//! source so a recognizer boundary cannot hide a year suffix from admission.
//! This is not a date parser, source correction or a general semantic proof.
#[derive(Clone)]
struct Word<'a> { text: &'a str, start: usize, end: usize }
fn words(text: &str) -> Vec<Word<'_>> {
    let mut result = Vec::new(); let mut start = None;
    for (index, c) in text.char_indices().chain(std::iter::once((text.len(), ' '))) {
        if c.is_ascii_alphanumeric() { if start.is_none() { start=Some(index); } }
        else if let Some(begin)=start.take() { result.push(Word{text:&text[begin..index],start:begin,end:index}); }
    }
    result
}
fn digit(word: &str) -> Option<u16> {
    ["one","two","three","four","five","six","seven","eight","nine"].iter()
        .position(|v|word.eq_ignore_ascii_case(v)).map(|v|v as u16+1)
}
fn tens(word: &str) -> Option<u16> {
    ["twenty","thirty","forty","fifty","sixty","seventy","eighty","ninety"].iter()
        .position(|v|word.eq_ignore_ascii_case(v)).map(|v|(v as u16+2)*10)
}
fn teen(word: &str) -> Option<u16> {
    ["ten","eleven","twelve","thirteen","fourteen","fifteen","sixteen","seventeen","eighteen","nineteen"].iter()
        .position(|v|word.eq_ignore_ascii_case(v)).map(|v|v as u16+10)
}
fn whitespace(text: &str) -> bool { !text.is_empty() && text.chars().all(char::is_whitespace) }
fn recognition_break(text: &str) -> bool {
    text.contains('\n') && text.chars().all(|c|c.is_whitespace()||c=='.') && text.matches('.').count()==1
}
struct YearMention { key: String, start: usize, end: usize }
fn year_mention(year: u16, context: &Word<'_>, end: usize) -> YearMention {
    YearMention { key: format!("calendar-year:{year}"), start: context.start, end }
}
fn mentions(text: &str) -> Vec<YearMention> {
    let tokens=words(text); let mut result=Vec::new();
    for (index, context) in tokens.iter().enumerate() {
        if !context.text.eq_ignore_ascii_case("in") && !context.text.eq_ignore_ascii_case("year") { continue; }
        let Some(first)=tokens.get(index+1) else { continue; };
        if !whitespace(&text[context.end..first.start]) { continue; }
        if first.text.len()==4 && first.text.bytes().all(|c|c.is_ascii_digit()) {
            if let Ok(year @ 1900..=2099)=first.text.parse::<u16>() { result.push(year_mention(year, context, first.end)); }
            continue;
        }
        let (base, tail)=if first.text.eq_ignore_ascii_case("nineteen") { (1900,index+2) }
            else if first.text.eq_ignore_ascii_case("twenty") { (2000,index+2) }
            else if first.text.eq_ignore_ascii_case("two") && tokens.get(index+2).is_some_and(|t|t.text.eq_ignore_ascii_case("thousand")&&whitespace(&text[first.end..t.start])) {
                (2000,index+3)
            } else { continue; };
        let thousand=tail==index+3;
        let tail=if thousand && tokens.get(tail).is_some_and(|t|t.text.eq_ignore_ascii_case("and")) { tail+1 } else { tail };
        let Some(second)=tokens.get(tail) else {
            if thousand {result.push(year_mention(2000, context, tokens[tail-1].end));} continue;
        };
        // The two-part year must be established before any fragmented suffix.
        if !whitespace(&text[tokens[tail-1].end..second.start]) { continue; }
        let value=tens(second.text).or_else(||teen(second.text)).or_else(||if thousand {digit(second.text)} else {None});
        let Some(value)=value else {
            if thousand { result.push(year_mention(2000, context, tokens[tail-1].end)); } continue;
        };
        let mut year=base+value; let mut end=second.end;
        if let Some(third)=tokens.get(tail+1).filter(|_|value>=20 && value%10==0) {
            let gap=&text[second.end..third.start];
            let after=&text[third.end..];
            let isolated_suffix=after.trim_start().chars().next().is_none_or(|c|matches!(c,'.'|','|'!'|'?'|';'));
            if whitespace(gap) || (recognition_break(gap) && isolated_suffix) {
                if let Some(value)=digit(third.text) { year+=value; end=third.end; }
            }
        }
        result.push(year_mention(year, context, end));
    }
    result
}

pub(super) fn keys(text: &str) -> Vec<String> {
    mentions(text).into_iter().map(|year| year.key).collect()
}

/// Resolve only a direct year-to-year spoken repair. An apology, discourse
/// marker or different claim elsewhere in the source cannot waive any year.
pub(super) fn source_keys(text: &str) -> Vec<String> {
    let years=mentions(text);
    years.iter().enumerate().filter(|(index, year)| {
        let Some(next)=years.get(index+1) else { return true; };
        let gap=&text[year.end..next.start];
        // A separate sentence may start with another year's circumstance.
        // This restriction concerns repairs only, not recognition_break above.
        if gap.chars().any(|c|matches!(c,'.'|'!'|'?'|'。'|'！'|'？')) { return true; }
        let marker=gap.trim_matches(|c:char| c.is_whitespace() || matches!(c,','|';'|':'|'，'|'；'|'：'))
            .split_whitespace().collect::<Vec<_>>().join(" ").to_ascii_lowercase();
        !["sorry", "i mean", "actually", "correction", "不对", "改口", "说错", "更正"].contains(&marker.as_str())
    }).map(|(_,year)|year.key.clone()).collect()
}

/// Output lookup also accepts four-digit spelling without an added preposition.
/// Only source-established values are checked. New dates, owners and unsupported
/// spoken forms still require semantic review, not this finite retention check.
pub(super) fn retained_keys(text: &str) -> Vec<String> {
    let mut result=keys(text);
    for word in words(text) {
        if word.text.len()==4 {
            if let Ok(year @ 1900..=2099)=word.text.parse::<u16>() { result.push(format!("calendar-year:{year}")); }
        }
    }
    result
}
pub(super) fn ranges(text:&str)->Vec<(usize,usize,u16)> {
    mentions(text).into_iter().filter_map(|m|Some((m.start,m.end,m.key.strip_prefix("calendar-year:")?.parse().ok()?))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn joins_only_an_unambiguous_fragmented_year_suffix() {
        for (text, key) in [("I will graduate in twenty thirty.\nTwo. I am learning.","calendar-year:2032"),
            ("In nineteen ninety nine.","calendar-year:1999"),("I graduated in 2027.","calendar-year:2027"),
            ("In two thousand and twenty seven.","calendar-year:2027"),("In two thousand nineteen.","calendar-year:2019"),
            ("In twenty twenty.\nTwo teams joined.","calendar-year:2020"),
            ("In twenty twenty. Two teams joined.","calendar-year:2020")] {
            assert_eq!(keys(text),[key],"{text}");
        }
        for text in ["twenty thirty two", "in twenty five projects", "in var2032", "in Qwen3.8", "in 1000 iterations", "in 中文 2027"] { assert!(keys(text).is_empty(),"{text}"); }
    }
}
