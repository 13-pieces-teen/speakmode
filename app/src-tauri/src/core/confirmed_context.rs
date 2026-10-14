//! Explicit, per-revision recognition corrections. Never a global alias table.
use super::SourceRevision;
use serde::{Deserialize, Serialize};
use unicode_categories::UnicodeCategories;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DraftLanguage { #[default] AsSpoken, English }

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceCorrection {
    /// Unicode scalar offsets into SourceRevision::text(), including its LF joins.
    pub start: usize,
    pub end: usize,
    pub expected_text: String,
    pub replacement: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfirmedContext {
    pub source_revision_id: String,
    pub language: DraftLanguage,
    pub corrections: Vec<SourceCorrection>,
}

impl ConfirmedContext {
    /// Revalidate even persisted context. The stored ASR revision is never changed.
    pub fn reference_text(&self, source: &SourceRevision) -> Result<String, &'static str> {
        if self.source_revision_id != source.id || !source.complete || source.items.is_empty()
            || source.items.iter().any(|item| !item.is_final) || self.corrections.len() > 8 {
            return Err("确认内容与识别来源不符，请重新选择");
        }
        let original = source.text();
        let scalars: Vec<char> = original.chars().collect();
        if scalars.len() > 80_000 { return Err("识别内容超过容量限制"); }
        let mut sorted: Vec<_> = self.corrections.iter().collect();
        sorted.sort_by_key(|c| c.start);
        let mut cursor = 0;
        let mut reference = String::new();
        for correction in sorted {
            if correction.start < cursor || correction.start >= correction.end || correction.end > scalars.len()
                || correction.end - correction.start > 240 || correction.replacement.trim().is_empty()
                || correction.replacement.trim() != correction.replacement || correction.replacement.chars().count() > 240
                || correction.replacement.chars().any(|c| c.is_control() || c.is_other_format() || matches!(c, '\u{2028}' | '\u{2029}')) {
                return Err("每处确认需选择不重叠的原文，正确内容限 240 字且不能含换行或隐藏字符");
            }
            let selected: String = scalars[correction.start..correction.end].iter().collect();
            if selected != correction.expected_text || selected == correction.replacement {
                return Err("选中的原文已变化或没有实际修改");
            }
            reference.extend(&scalars[cursor..correction.start]);
            reference.push_str(&correction.replacement);
            cursor = correction.end;
        }
        reference.extend(&scalars[cursor..]);
        if reference.chars().count() > 80_000 { return Err("确认后文本超过容量限制"); }
        Ok(reference)
    }
}
