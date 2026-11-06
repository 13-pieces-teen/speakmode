//! Bounded local editing for a complete source when cloud editing cannot finish.
use super::{RewriteResult, literals, spoken_format, terms};
use crate::core::{BaseMode, DictionarySnapshot, OutputBlock, SourceRevision};

fn clean_sentence(sentence: &str) -> String {
    // Explicit discussion of wording owns the whole sentence, including text
    // that ASR transcribed without quotation marks.
    if ["逐字", "原样", "原话", "字面", "这个字", "这个词", "词典", "名字", "叫做", "变量", "标识符"]
        .iter().any(|cue| sentence.contains(cue)) {
        return sentence.to_owned();
    }
    let mut output = String::new();
    for part in sentence.split_inclusive([',', '，']) {
        let body = part.trim().trim_end_matches([',', '，']).trim();
        let hesitation = !body.is_empty() && body.chars().count() <= 4
            && body.chars().all(|ch| matches!(ch, '嗯' | '呃'));
        if hesitation && part.ends_with([',', '，']) {
            continue;
        }
        let leading = part.len() - part.trim_start().len();
        let body = &part[leading..];
        let repaired = [
            ("我我觉得", "我觉得"), ("我我认为", "我认为"),
            ("我我想", "我想"), ("我我在", "我在"),
            ("就是就是", "就是"),
        ].iter().find_map(|(from, to)| body.strip_prefix(from).map(|rest| format!("{to}{rest}")));
        output.push_str(&part[..leading]);
        output.push_str(repaired.as_deref().unwrap_or(body));
    }
    output
}

fn clean_plain(text: &str) -> String {
    text.split_inclusive(['。', '！', '？', '\n', ';', '；'])
        .map(clean_sentence).collect()
}

pub(crate) fn text(source: &SourceRevision, dictionary: &DictionarySnapshot) -> String {
    let original = source.text();
    let mut cleaned = String::new();
    let mut offset = 0;
    for literal in literals::spans(&original) {
        cleaned.push_str(&clean_plain(&original[offset..literal.start]));
        cleaned.push_str(&original[literal.start..literal.end]);
        offset = literal.end;
    }
    cleaned.push_str(&clean_plain(&original[offset..]));
    if cleaned.trim().is_empty() {
        return original;
    }
    spoken_format::normalize(&terms::canonicalize(cleaned.trim(), dictionary),
        super::spoken_english(source, None))
}

pub(crate) fn generate(source: &SourceRevision, dictionary: &DictionarySnapshot) -> RewriteResult {
    RewriteResult { resolved_mode: BaseMode::Light, blocks: vec![OutputBlock {
        id: uuid::Uuid::new_v4().to_string(), text: text(source, dictionary),
        source_refs: source.items.iter().map(|item| item.id.clone()).collect(),
    }] }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::SourceItem;

    fn source(text: &str) -> SourceRevision {
        SourceRevision { id: "source".into(), session_id: "session".into(), asr_attempt_id: "attempt".into(),
            generation: 7, provider: "test".into(), model: "test".into(), adapter_version: "test".into(),
            complete: true, created_at: "now".into(), items: vec![SourceItem {
                id: "item".into(), provider_item_id: "item".into(), capture_order: 0,
                text: text.into(), is_final: true, start_ms: None, end_ms: None,
            }] }
    }

    #[test]
    fn cleans_hesitation_and_stutters_while_retaining_every_fact() {
        let input = source("嗯，呃呃，我我觉得预算3500元，先不要发送。\n呃，用户可以在 A P P 里启动录音。");
        let before = input.clone();
        let result = generate(&input, &DictionarySnapshot::default());
        assert_eq!(result.blocks[0].text, "我觉得预算3500元，先不要发送。\n用户可以在 APP 里启动录音。");
        assert_eq!(result.blocks[0].source_refs, vec!["item"]);
        assert_eq!(input, before);
    }

    #[test]
    fn preserves_quotes_code_emphasis_uncertain_letters_and_spoken_corrections() {
        for input in [
            "引用‘嗯，呃，我我觉得’和 `userID`。",
            "```text\n嗯，呃，\n我我觉得\n```",
            "请逐字保留嗯，呃，我我觉得。",
            "这个词是嗯，呃是另一个字。",
            "不不不，快快快，先不要发送。",
            "三个独立字母 A P P，分别读。",
            "预算三千，不对，三千五，场地两千，不是五千。",
            "嗯，呃，",
            "“未闭合，嗯，呃，我我觉得",
        ] {
            assert_eq!(text(&source(input), &DictionarySnapshot::default()), input, "{input}");
        }
    }
}
