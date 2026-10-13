//! Local protection of quoted speech and code. No model verdict can waive it.
use super::CloudError;
use std::collections::HashMap;

#[derive(Clone, Copy)]
pub(super) struct Literal { pub start: usize, pub end: usize, content_start: usize, content_end: usize, code: bool }

fn escaped(text: &str, offset: usize) -> bool {
    text[..offset].bytes().rev().take_while(|c| *c == b'\\').count() % 2 == 1
}

pub(super) fn spans(text: &str) -> Vec<Literal> {
    let mut found = Vec::new(); let mut offset = 0;
    while offset < text.len() {
        let c = text[offset..].chars().next().unwrap();
        let apostrophe = c == '\'' && text[..offset].chars().next_back().is_some_and(|c| c.is_ascii_alphanumeric())
            && text[offset + 1..].chars().next().is_some_and(|c| c.is_ascii_alphanumeric());
        let close = match c { '"' | '\'' => Some(c), '“' => Some('”'), '‘' => Some('’'),
            '「' => Some('」'), '『' => Some('』'), '《' => Some('》'), _ => None };
        if (close.is_none() && c != '`') || apostrophe || escaped(text, offset) { offset += c.len_utf8(); continue; }
        let width = if c == '`' { text[offset..].bytes().take_while(|c| *c == b'`').count() } else { c.len_utf8() };
        let begin = offset + width;
        let delimiter = if c == '`' { "`".repeat(width) } else { close.unwrap().to_string() };
        let mut end = None;
        for (relative, _) in text[begin..].char_indices() {
            let at = begin + relative;
            if text[at..].starts_with(&delimiter) && !escaped(text, at)
                && (c != '`' || (text[..at].chars().next_back() != Some('`')
                    && text[at + width..].chars().next() != Some('`'))) { end = Some(at); break; }
        }
        let content_end = end.unwrap_or(text.len());
        let last = end.map_or(text.len(), |at| at + delimiter.len());
        found.push(Literal { start: offset, end: last, content_start: begin, content_end, code: c == '`' });
        offset = last;
    }
    found
}

pub(super) fn validate(source: &str, output: &str) -> Result<(), CloudError> {
    let collect = |text: &str| {
        let mut counts = HashMap::new();
        for span in spans(text) {
            let key = (span.code, text[span.content_start..span.content_end].to_owned());
            *counts.entry(key).or_insert(0usize) += 1;
        }
        counts
    };
    let required = collect(source); let actual = collect(output);
    if required.iter().any(|(literal, count)| actual.get(literal).copied().unwrap_or(0) < *count) {
        return Err(CloudError::new("rewrite_literal_changed", "rewrite", "整理结果改变了引语或代码，识别稿仍保留"));
    }
    Ok(())
}

#[cfg(test)] mod tests {
    use super::*;
    #[test] fn quoted_fillers_are_data_even_when_other_fillers_can_be_cleaned() {
        let source = "嗯，请逐字保留‘嗯，这个，等我想一下’，呃，说明这是原话。";
        assert!(validate(source, "请逐字保留\"嗯，这个，等我想一下\"，说明这是原话。").is_ok());
        assert!(validate(source, "请保留‘等我想一下’，说明这是原话。").is_err());
        assert!(validate(source, "嗯，这个，等我想一下。说明这是原话。").is_err());
    }
    #[test] fn code_escaping_and_literal_multiplicity_cannot_be_rewritten_or_merged() {
        for (source, output) in [
            ("保留 `userID`。", "保留 `userId`。"),
            ("两条都写‘未确认’，第二条也写‘未确认’。", "两条都写‘未确认’。"),
            ("代码 ```rust\nlet a = 1;\n```。", "代码 ```rust\nlet a = 2;\n```。"),
            ("引用 \"他说\\\"嗯\\\"\"。", "引用 \"他说\\\"好\\\"\"。"),
        ] { assert!(validate(source, output).is_err(), "{source}"); assert!(validate(source, source).is_ok()); }
    }
    #[test] fn apostrophes_titles_nested_and_unclosed_literals_have_bounded_ranges() {
        assert!(spans("don't change Peter's name").is_empty());
        assert!(validate("嗯，don't change Peter's name", "don't change Peter's name").is_ok());
        for text in ["“他说‘嗯’”", "《谍战风云》", "未闭合‘嗯，先不要删", "```text\n原样的嗯", "‘abc’和`x`"] {
            assert!(validate(text, text).is_ok());
            for span in spans(text) { assert!(text.is_char_boundary(span.start)); assert!(text.is_char_boundary(span.end)); }
        }
        assert!(validate("未闭合‘嗯，先不要删", "未闭合‘先不要删").is_err());
    }
}
