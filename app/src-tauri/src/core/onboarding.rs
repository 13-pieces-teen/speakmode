use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OnboardingStep { Welcome, Setup, Shortcut, Chat, Document, Ready }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OnboardingLesson { Chat, Document }

impl OnboardingStep {
    pub fn lesson(self) -> Option<OnboardingLesson> {
        match self { Self::Chat => Some(OnboardingLesson::Chat), Self::Document => Some(OnboardingLesson::Document), _ => None }
    }
}
impl OnboardingLesson {
    pub fn mode_id(self) -> &'static str { match self { Self::Chat => "chat", Self::Document => "document" } }
}

/// Progress only: no transcripts, credentials, target text or fake service receipts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OnboardingProgress {
    pub version: u64,
    pub auto_show: bool,
    pub step: OnboardingStep,
    pub completed_lessons: Vec<OnboardingLesson>,
    pub completed_at: Option<String>,
}
impl OnboardingProgress {
    pub fn new_install(fresh: bool) -> Self {
        Self { version: 1, auto_show: fresh, step: OnboardingStep::Welcome, completed_lessons: Vec::new(), completed_at: None }
    }
}
