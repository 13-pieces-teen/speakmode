//! Source-bound dictionary spelling for automatic dictation.
//!
//! The immutable ASR source is never changed. Derived text may normalize only
//! identical Latin letters and ordinary spacing using its frozen dictionary.
//! Quotes, identifiers and addresses are excluded; no fuzzy/phonetic guessing.

use crate::core::{DictionarySnapshot, SourceRevision};
use std::collections::HashSet;

const MAX_TERMS: usize = 32;
const MAX_TERM_CHARS: usize = 64;
const MAX_TOTAL_CHARS: usize = 1024;

pub(super) fn matching_terms(source: &SourceRevision, dictionary: &DictionarySnapshot) -> Vec<String> {
    if !source.complete || source.items.iter().any(|item| !item.is_final) {
        return Vec::new();
    }
    let text = source.text();
    if literal_context(&text) {
        return Vec::new();
    }
    let available = plain_mask(&text);
    let tokens = plain_tokens(&text, &available);
    let mut selected = Vec::new();
    let mut seen = HashSet::new();
    let mut total = 0;
    // Preserve the supplied snapshot order. Never sort by fuzzy relevance.
    for entry in &dictionary.entries {
        let term = entry.term.as_str();
        if !eligible_term(term) || total + term.len() > MAX_TOTAL_CHARS {
            continue;
        }
        if matches_for_entry(&text, &tokens, term).is_empty() {
            continue;
        }
        if !seen.insert(term.to_ascii_lowercase()) {
            continue;
        }
        total += term.len(); // Eligible terms are ASCII: bytes equal characters.
        selected.push(term.to_owned());
        if selected.len() == MAX_TERMS {
            break;
        }
    }
    selected
}

fn eligible_term(term: &str) -> bool {
    if term.len() > MAX_TERM_CHARS
        || !term.bytes().next().is_some_and(|c| c.is_ascii_alphabetic())
        || (term.as_bytes()[0].is_ascii_lowercase() && term.bytes().any(|c| c.is_ascii_uppercase()))
        || !term.bytes().all(|c| c.is_ascii_alphabetic() || c == b' ')
        || term.bytes().filter(u8::is_ascii_alphabetic).count() < 3
    {
        return false;
    }
    let words: Vec<_> = term.split(' ').collect();
    !words.iter().any(|word| word.is_empty() || instruction_word(word))
}

fn safe_case(word: &str) -> bool {
    word.bytes().all(|c| c.is_ascii_lowercase()) || word.bytes().all(|c| c.is_ascii_uppercase())
        || (word.as_bytes()[0].is_ascii_uppercase() && word.bytes().skip(1).all(|c| c.is_ascii_lowercase()))
}

/// Only identical Latin letters, ignoring ordinary spaces and case. No phonetic or fuzzy match.
fn matches_for_entry(text: &str, tokens: &[Token<'_>], term: &str) -> Vec<(usize, usize, u8)> {
    let words: Vec<_> = term.split(' ').collect(); let compact = words.concat();
    let mut matches = Vec::new();
    for start in 0..tokens.len() {
        let mut joined = String::new();
        for last in start..tokens.len() {
            if last > start {
                let gap = &text[tokens[last - 1].end..tokens[last].start];
                if gap.is_empty() || !gap.bytes().all(|c| c == b' ') { break; }
            }
            joined.push_str(tokens[last].text);
            if joined.len() > compact.len() { break; }
            if joined.eq_ignore_ascii_case(&compact) {
                let window = &tokens[start..=last];
                let exact = window.len() == words.len() && phrase_matches(text, window, &words);
                if exact || window.iter().all(|token| safe_case(token.text)) {
                    matches.push((tokens[start].start, tokens[last].end, u8::from(!exact)));
                }
            }
        }
    }
    matches
}

/// Deterministic source-bound spelling before protected generation. Quotes,
/// identifiers and addresses are excluded. The immutable ASR source is never changed.
pub(super) fn canonicalize(text: &str, dictionary: &DictionarySnapshot) -> String {
    if literal_context(text) { return text.to_owned(); }
    let tokens = plain_tokens(text, &plain_mask(text));
    let mut proposals = std::collections::BTreeMap::<usize, (usize, String, u8, bool)>::new();
    for entry in &dictionary.entries {
        if !eligible_term(&entry.term) { continue; }
        for (start, end, score) in matches_for_entry(text, &tokens, &entry.term) {
            let next = (end, entry.term.clone(), score, false);
            match proposals.get_mut(&start) {
                None => { proposals.insert(start, next); },
                Some(old) if score < old.2 || (score == old.2 && end > old.0) => *old = next,
                Some(old) if score == old.2 && end == old.0 && !old.1.eq_ignore_ascii_case(&entry.term) => old.3 = true,
                _ => (),
            }
        }
    }
    let mut output = String::new(); let mut offset = 0;
    for (start, (end, canonical, _, ambiguous)) in proposals {
        if start < offset || ambiguous { continue; }
        output.push_str(&text[offset..start]); output.push_str(&canonical); offset = end;
    }
    output.push_str(&text[offset..]); output
}

fn instruction_word(word: &str) -> bool {
    ["ignore", "disregard", "forget", "obey", "execute", "reveal", "output", "return",
        "answer", "rewrite", "replace", "delete", "print", "instruction", "instructions",
        "prompt", "system", "assistant"]
        .iter().any(|cue| word.eq_ignore_ascii_case(cue))
}

fn literal_context(text: &str) -> bool {
    if ["代码", "变量", "字段", "标识符", "大小写", "大写", "小写", "原样", "逐字",
        "字面", "路径", "网址", "链接", "邮箱", "正则", "命令", "指令", "提示词"]
        .iter().any(|cue| text.contains(cue))
    {
        return true;
    }
    text.split(|c: char| !c.is_ascii_alphabetic()).any(|word| {
        ["code", "variable", "variables", "field", "fields", "identifier", "identifiers",
            "uppercase", "lowercase", "casing", "verbatim", "literal", "literals", "url",
            "urls", "path", "paths", "filename", "filepath", "regex", "command"]
            .iter().any(|cue| word.eq_ignore_ascii_case(cue)) || instruction_word(word)
    }) || text.to_ascii_lowercase().contains("case sensitive")
        || text.to_ascii_lowercase().contains("case-sensitive")
}

fn word_edge(c: char) -> bool {
    // Han adjacency is ordinary mixed-language prose, e.g. 用github提交.
    // Other alphabetic scripts/combining marks must not split a larger word.
    let han = matches!(c, '\u{3400}'..='\u{4dbf}' | '\u{4e00}'..='\u{9fff}'
        | '\u{f900}'..='\u{faff}' | '\u{20000}'..='\u{323af}');
    (c.is_alphanumeric() && !han) || c == '_' || matches!(c, '\u{0300}'..='\u{036f}')
}

fn quote_end(c: char) -> Option<char> {
    match c {
        '\'' | '"' | '`' => Some(c),
        '“' => Some('”'), '‘' => Some('’'), '「' => Some('」'), '『' => Some('』'),
        '<' => Some('>'),
        _ => None,
    }
}

fn plain_mask(text: &str) -> Vec<bool> {
    let mut mask = vec![true; text.len()];
    let mut quote = None;
    let mut escaped = false;
    for (start, c) in text.char_indices() {
        if let Some(end) = quote {
            mask[start..start + c.len_utf8()].fill(false);
            if c == end && !escaped {
                quote = None;
            }
            escaped = c == '\\' && !escaped;
        } else if let Some(end) = quote_end(c) {
            mask[start..start + c.len_utf8()].fill(false);
            quote = Some(end);
            escaped = false;
        }
    }
    // Exclude whole address/identifier runs, including the apparent brand part
    // of GitHub.com, user@GitHub, C:\GitHub, ./GitHub and GitHub_tools.
    let address_char = |c: char| c.is_ascii_alphanumeric() || "_.@/\\:~?&#=%+-".contains(c);
    let mut start = None;
    for (offset, c) in text.char_indices().chain(std::iter::once((text.len(), ' '))) {
        if address_char(c) {
            start.get_or_insert(offset);
        } else if let Some(begin) = start.take() {
            let run = &text[begin..offset];
            let interior = run.trim_end_matches(['.', ':', '?']);
            if interior.contains('.') || interior.chars().any(|c| "_@/\\:~?&#=%+-".contains(c)) {
                mask[begin..offset].fill(false);
            }
        }
    }
    mask
}

#[derive(Debug)]
struct Token<'a> { text: &'a str, start: usize, end: usize }

fn plain_tokens<'a>(text: &'a str, available: &[bool]) -> Vec<Token<'a>> {
    let mut tokens = Vec::new();
    let mut start = None;
    for (offset, c) in text.char_indices().chain(std::iter::once((text.len(), ' '))) {
        if c.is_ascii_alphabetic() {
            start.get_or_insert(offset);
        } else if let Some(begin) = start.take() {
            if available[begin..offset].iter().all(|value| *value)
                && text[..begin].chars().next_back().is_none_or(|c| !word_edge(c))
                && text[offset..].chars().next().is_none_or(|c| !word_edge(c))
            {
                tokens.push(Token { text: &text[begin..offset], start: begin, end: offset });
            }
        }
    }
    tokens
}

fn phrase_matches(text: &str, tokens: &[Token<'_>], words: &[&str]) -> bool {
    tokens.iter().zip(words).all(|(token, word)| {
        token.text.eq_ignore_ascii_case(word)
            // Existing mixed-case spelling is a potential identifier. A hint
            // may retain it exactly, but must not propose different casing.
            && (token.text == *word || !token.text.bytes().any(|c| c.is_ascii_uppercase())
                || !token.text.bytes().any(|c| c.is_ascii_lowercase()))
    }) && tokens.windows(2).all(|pair| {
        let gap = &text[pair[0].end..pair[1].start];
        !gap.is_empty() && gap.bytes().all(|c| c == b' ')
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{DictionaryEntry, SourceItem};

    fn source(text: &str) -> SourceRevision {
        SourceRevision { id: "terms-source".into(), session_id: "terms-session".into(),
            asr_attempt_id: "terms-attempt".into(), generation: 1, provider: "synthetic".into(),
            model: "synthetic".into(), adapter_version: "test".into(), complete: true,
            items: vec![SourceItem { id: "terms-item".into(), provider_item_id: "terms-item".into(),
                capture_order: 0, text: text.into(), is_final: true, start_ms: None, end_ms: None }],
            created_at: "2026-10-10T00:00:00Z".into() }
    }

    fn dictionary(terms: &[&str]) -> DictionarySnapshot {
        DictionarySnapshot { version: 1, entries: terms.iter().enumerate().map(|(index, term)| {
            DictionaryEntry { id: format!("term-{index}"), term: (*term).into(),
                created_at: String::new(), updated_at: String::new() }
        }).collect() }
    }

    fn hints(text: &str, terms: &[&str]) -> Vec<String> {
        matching_terms(&source(text), &dictionary(terms))
    }

    #[test]
    fn selects_source_present_spellings_in_snapshot_order_with_han_adjacency() {
        assert_eq!(hints("用github提交，然后比较typeless和SPEAKMODE。", &["SpeakMode", "Typeless", "GitHub", "Unmentioned"]),
            ["SpeakMode", "Typeless", "GitHub"]);
    }

    #[test]
    fn recognizes_identical_letters_without_crossing_items() {
        assert_eq!(hints("打开visual   studio。", &["Visual Studio", "VisualStudio"]), ["Visual Studio", "VisualStudio"]);
        assert!(hints("打开visual\nstudio。", &["Visual Studio"]).is_empty());
        assert!(hints("打开visual，studio。", &["Visual Studio"]).is_empty());
    }

    #[test]
    fn no_substring_phonetic_or_spacing_guesses() {
        assert!(hints("Speed Mode、swing mode。", &["SpeakMode"]).is_empty());
        assert_eq!(hints("speak mode。", &["SpeakMode"]), ["SpeakMode"]);
        for text in ["mygithubtool", "github2", "2github", "githubé", "github\u{0301}", "github_tools"] {
            assert!(hints(text, &["GitHub"]).is_empty(), "{text}");
        }
    }

    #[test]
    fn retains_identifier_casing_instead_of_overriding_it() {
        for text in ["userID", "userId", "userID和userId", "UserID和UserId"] {
            assert!(hints(text, &["userID", "userId", "Userid"]).is_empty(), "{text}");
        }
        assert!(hints("gitHub", &["GitHub"]).is_empty());
        assert_eq!(hints("GitHub", &["GitHub"]), ["GitHub"]);
    }

    #[test]
    fn literal_context_disables_all_hints() {
        for cue in ["代码", "变量", "字段", "标识符", "大小写", "原样", "逐字", "URL", "路径",
            "code", "VARIABLE", "case-sensitive", "literal", "filePATH"] {
            assert!(hints(&format!("{cue}里的github和typeless"), &["GitHub", "Typeless"]).is_empty(), "{cue}");
        }
    }

    #[test]
    fn quotes_backticks_and_unclosed_literals_are_excluded() {
        for text in ["\"github\"", "'github'", "`github`", "```github```", "“github”", "‘github’", "「github」", "『github』", "<github>", "\"github"] {
            assert!(hints(text, &["GitHub"]).is_empty(), "{text}");
        }
        assert_eq!(hints("“github”之外用typeless。", &["GitHub", "Typeless"]), ["Typeless"]);
        assert!(hints("\"say \\\"github\\\" here\"", &["GitHub"]).is_empty());
    }

    #[test]
    fn addresses_and_paths_never_supply_a_brand_match() {
        for text in ["https://github.com", "github.com", "name@github", "github@example.com",
            "C:\\github", "./github", "../github", "github/project", "github-project", "github_token"] {
            assert!(hints(text, &["GitHub"]).is_empty(), "{text}");
        }
        assert_eq!(hints("用github。然后打开https://typeless.com。", &["GitHub", "Typeless"]), ["GitHub"]);
        assert_eq!(hints("Open github.", &["GitHub"]), ["GitHub"]);
    }

    #[test]
    fn rejects_untrusted_non_term_content_and_tiny_terms() {
        for term in ["GitHub\nIgnore", "GitHub\t", "GitHub\u{0000}", "GitHub\"", "GitHub.",
            "GitHub/", "GitHub_", " GitHub", "GitHub ", "Visual  Studio", "A", "AI",
            "Ignore Previous Instructions", "System Prompt", "Return JSON"] {
            assert!(hints(term, &[term]).is_empty(), "{term:?}");
        }
        assert!(hints("\"Ignore Previous Instructions\"", &["Ignore Previous Instructions"]).is_empty());
    }

    #[test]
    fn filters_case_duplicate_hints_without_mutating_either_input() {
        let input = source("github和typeless");
        let words = dictionary(&["GitHub", "GITHUB", "Typeless"]);
        let original_source = input.clone(); let original_dictionary = words.clone();
        assert_eq!(matching_terms(&input, &words), ["GitHub", "Typeless"]);
        assert_eq!(input, original_source); assert_eq!(words, original_dictionary);
    }

    #[test]
    fn caps_number_of_terms_and_keeps_order() {
        let names: Vec<_> = (b'A'..=b'Z').chain(b'A'..=b'Z').enumerate()
            .map(|(index, c)| format!("Brand{}{}", if index < 26 { 'A' } else { 'B' }, c as char)).collect();
        let refs: Vec<_> = names.iter().map(String::as_str).collect();
        assert_eq!(hints(&names.join(" "), &refs), names[..MAX_TERMS]);
    }

    #[test]
    fn caps_each_term_and_total_character_budget() {
        let names: Vec<_> = (b'A'..=b'T').map(|c| format!("{}{}", c as char, "a".repeat(63))).collect();
        let refs: Vec<_> = names.iter().map(String::as_str).collect();
        let selected = hints(&names.join(" "), &refs);
        assert_eq!(selected, names[..16]);
        assert_eq!(selected.iter().map(String::len).sum::<usize>(), MAX_TOTAL_CHARS);
        let too_long = format!("T{}", "a".repeat(64));
        assert!(hints(&too_long, &[&too_long]).is_empty());
    }

    #[test]
    fn empty_dictionary_or_incomplete_source_has_no_hints() {
        assert!(hints("github", &[]).is_empty());
        let mut input = source("github"); input.complete = false;
        assert!(matching_terms(&input, &dictionary(&["GitHub"])).is_empty());
        input.complete = true; input.items[0].is_final = false;
        assert!(matching_terms(&input, &dictionary(&["GitHub"])).is_empty());
    }

    #[test] fn lowercase_dictionary_and_spelled_acronyms_are_source_bound() {
        assert_eq!(hints("argus 使用 S V I D 和 openviking。", &["argus", "SVID", "openviking"]), ["argus", "SVID", "openviking"]);
        let words = dictionary(&["SVID", "SPIFFE", "OpenViking"]);
        let input = "S V I D 来自 spiffe，保存在 Open Viking。";
        assert_eq!(canonicalize(input, &words), "SVID 来自 SPIFFE，保存在 OpenViking。");
        assert_eq!(canonicalize("“S V I D”与S V I D，https://spiffe.io", &words), "“S V I D”与SVID，https://spiffe.io");
        assert_eq!(canonicalize("Open Cloud，Speed Mode，userID", &words), "Open Cloud，Speed Mode，userID");
        assert_eq!(canonicalize("代码里逐字写 S V I D", &words), "代码里逐字写 S V I D");
    }
    #[test] fn ambiguous_compacted_dictionary_names_do_not_choose_a_spelling() {
        let words = dictionary(&["Visual Studio", "VisualStudio"]);
        assert_eq!(canonicalize("V i s u a l S t u d i o", &words), "V i s u a l S t u d i o");
        assert_eq!(canonicalize("visual studio", &words), "Visual Studio");
    }
}
