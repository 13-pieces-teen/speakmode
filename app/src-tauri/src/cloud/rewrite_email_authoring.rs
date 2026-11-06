//! Finite Email-only authoring permission. This is a native analysis receipt,
//! never a modified source, a general intent parser, or semantic approval.
use crate::core::{AppWritingScene, ModeSnapshot, SourceRevision, writing_context::WritingContext};

/// Exact original UTF-8 range in the first source item. A quoted, negated,
/// reported, nonleading or fragment-split authoring request is not recognized.
pub(super) struct Omission<'a> {
    pub source_id: &'a str,
    pub start: usize,
    pub end: usize,
    pub formatting: bool,
    body_start: usize,
}

impl Omission<'_> {
    pub(super) fn body<'a>(&self, source: &'a str) -> &'a str { &source[self.body_start..] }

    pub(super) fn contains(&self, source_id: &str, start: usize, end: usize) -> bool {
        source_id == self.source_id && start >= self.start && end <= self.end
    }
}

pub(super) fn omission<'a>(mode: &ModeSnapshot, source: &'a SourceRevision,
    context: Option<&WritingContext>) -> Option<Omission<'a>> {
    let scene = crate::core::writing_context::selected_scene(mode).or_else(||context.map(|value|value.scene));
    if scene != Some(AppWritingScene::Email) { return None; }
    let item = source.items.first()?;
    let leading = item.text.trim_start();
    let start = item.text.len() - leading.len();
    // Finite complete clauses only. A named recipient, document, condition,
    // negation, spelling or other material inside a wrapper is never erased.
    let (quote, formatting) = [
        ("能不能帮我分段回邮件", true), ("帮我分段回邮件", true),
        ("能不能帮我回邮件", false), ("帮我回邮件", false),
        ("帮我写封邮件", false), ("帮我回一下", false),
    ].into_iter().find(|(quote, _)|leading.starts_with(quote))?;
    let end = start + quote.len();
    let separator = item.text[end..].chars().next()?;
    if !matches!(separator, ','|'，'|':'|'：'|';'|'；'|'?'|'？') { return None; }
    if super::literals::spans(&item.text).iter().any(|span|span.start < end && span.end > start) { return None; }
    let full = source.text();
    let rest = full[end + separator.len_utf8()..].trim_start();
    // A trailing attribution can identify the leading words as reported speech
    // rather than a live authoring request. Recognize only this bounded cue;
    // other ambiguous forms remain subject to the independent full review.
    let clarification=rest.split([',','，',':','：',';','；','。','!','！','?','？','\n']).next().unwrap_or("").chars().take(96).collect::<String>();
    let reported=["这是","这句","这段","以上是","前面是","刚才是"].iter().any(|prefix|clarification.starts_with(prefix))
        && ["原话","转述","问我","让我","要求我","告诉我"].iter().any(|cue|clarification.contains(cue));
    if !rest.chars().any(char::is_alphanumeric)
        || reported
        || ["不是让你", "不是要你", "不要帮我", "不用帮我", "不需要帮我", "别帮我", "不能帮我",
            "不要回邮件", "不用回邮件"].iter().any(|prefix|rest.starts_with(prefix)) { return None; }
    let body_start = full.len() - rest.len();
    Some(Omission { source_id: &item.id, start, end, formatting, body_start })
}
