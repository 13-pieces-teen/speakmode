//! Bounded, ephemeral host hints. Never part of the immutable speech source.
use super::{AppWritingScene, DraftLanguage, ModeSnapshot};
use serde::{Deserialize, Serialize};
use unicode_categories::UnicodeCategories;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InputContext {
    pub control_label: String,
    pub before_text: String,
    pub selected_text: String,
    pub after_text: String,
}
fn visible(c: char) -> bool { (!c.is_control() || matches!(c, '\n' | '\r' | '\t')) && !c.is_other_format() }
impl InputContext {
    pub fn bounded(self) -> Self {
        let clean = |s: String, limit| s.chars().filter(|c| visible(*c)).take(limit).collect();
        // Keep the nearest suffix before the insertion point, not the document start.
        let before: String = self.before_text.chars().filter(|c| visible(*c)).rev().take(1000).collect::<Vec<_>>().into_iter().rev().collect();
        Self { control_label: clean(self.control_label,160), before_text: before,
            selected_text: clean(self.selected_text,500), after_text: clean(self.after_text,500) }
    }
    pub fn validate(&self) -> Result<(), &'static str> {
        for (value, limit) in [(&self.control_label,160),(&self.before_text,1000),(&self.selected_text,500),(&self.after_text,500)] {
            if value.chars().count()>limit || value.chars().any(|c|!visible(c)) { return Err("输入位置上下文超过范围或含隐藏字符"); }
        }
        Ok(())
    }
    pub fn is_empty(&self) -> bool { [&self.control_label,&self.before_text,&self.selected_text,&self.after_text].iter().all(|v|v.trim().is_empty()) }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WritingContext {
    pub scene: AppWritingScene,
    pub language: DraftLanguage,
    pub input: Option<InputContext>,
}
impl WritingContext {
    pub fn validate(&self) -> Result<(), &'static str> { self.input.as_ref().map_or(Ok(()),InputContext::validate) }
}

/// Content-free record of the frozen writing policy. No host text is persisted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all="camelCase", deny_unknown_fields)]
pub struct WritingReceipt {
    pub scene: AppWritingScene, pub language: DraftLanguage, pub context_used: bool, pub contract_version: String,
}

pub fn selected_scene(mode: &ModeSnapshot) -> Option<AppWritingScene> {
    match mode.mode_id.as_str() {
        "chat"=>Some(AppWritingScene::Chat), "document"=>Some(AppWritingScene::Document),
        "email"=>Some(AppWritingScene::Email), "technical"=>Some(AppWritingScene::Technical), _=>None,
    }
}

/// Labels are hints for presentation only. Ambiguous fields remain General;
/// the language model still sees the bounded field plus the whole utterance.
pub fn input_scene(input: &InputContext) -> AppWritingScene {
    let label=input.control_label.trim().to_lowercase();
    let contains=|values:&[&str]| values.iter().any(|value|label.contains(value));
    if contains(&["邮件正文","email body","message body","compose email"]) { AppWritingScene::Email }
    else if contains(&["文档正文","文档编辑","document body","document editor","正文编辑"]) { AppWritingScene::Document }
    else if contains(&["代码编辑","code editor"]) { AppWritingScene::Technical }
    else if contains(&["发送消息","输入消息","聊天输入","chat message","message input","send a message"]) { AppWritingScene::Chat }
    else { AppWritingScene::General }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn limits_preserve_nearest_unicode_and_strip_hidden_characters() {
        let input=InputContext {control_label:"x".repeat(180),before_text:format!("{}近😀", "远".repeat(1200)),selected_text:"🙂".repeat(600),after_text:"\u{202e}hello\0\nworld".into()}.bounded();
        assert!(input.validate().is_ok());assert_eq!(input.before_text.chars().count(),1000);
        assert!(input.before_text.ends_with("近😀"));assert_eq!(input.selected_text.chars().count(),500);
        assert_eq!(input.after_text,"hello\nworld");
    }
    #[test] fn unbounded_or_hidden_context_is_not_accepted_from_callers() {
        for text in ["a".repeat(501),"secret\u{2066}".into()] {
            assert!(InputContext {selected_text:text,..Default::default()}.validate().is_err());
        }
    }
    #[test] fn same_browser_can_offer_different_field_hints_without_reading_page_titles() {
        for (label,scene) in [("Send a message",AppWritingScene::Chat),("Document body",AppWritingScene::Document),("Email body",AppWritingScene::Email),("Search",AppWritingScene::General),("Message",AppWritingScene::General)] {
            assert_eq!(input_scene(&InputContext{control_label:label.into(),..Default::default()}),scene);
        }
    }
}
