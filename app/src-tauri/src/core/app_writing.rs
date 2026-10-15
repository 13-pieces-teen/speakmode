//! Local application preferences. No process paths, window titles or host text.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AppWritingScene { #[default] General, Chat, Document, Email, Technical }

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AppWritingOverride { pub application: String, pub scene: AppWritingScene }

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AppWritingPreferences {
    pub enabled: bool, pub overrides: Vec<AppWritingOverride>,
    #[serde(default)] pub input_context_enabled: bool,
    #[serde(default)] pub excluded_applications: Vec<String>,
    #[serde(default)] pub output_language: super::DraftLanguage,
}
impl Default for AppWritingPreferences {
    fn default() -> Self { Self { enabled: true, overrides: Vec::new(), input_context_enabled:false, excluded_applications:Vec::new(), output_language:Default::default() } }
}

pub fn application_key(value: &str) -> Option<String> {
    let value = value.trim().to_ascii_lowercase();
    (value.len() > 4 && value.len() <= 80 && value.ends_with(".exe")
        && !value.starts_with('.') && !value.contains("..")
        && value.bytes().all(|c| c.is_ascii_alphanumeric() || b"._- ".contains(&c)))
        .then_some(value)
}

impl AppWritingPreferences {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.excluded_applications.len()>32 {return Err("上下文排除应用最多保存 32 项");}
        let mut excluded=std::collections::HashSet::new();
        for app in &self.excluded_applications {
            let key=application_key(app).ok_or("排除应用需填写程序名，例如 Code.exe，不包含路径")?;
            if !excluded.insert(key) {return Err("排除应用不能重复");}
        }
        if self.overrides.len() > 32 { return Err("应用偏好最多保存 32 项"); }
        let mut seen = std::collections::HashSet::new();
        for item in &self.overrides {
            let key = application_key(&item.application).ok_or("请填写应用程序名，例如 Code.exe，不包含路径")?;
            if !seen.insert(key) { return Err("同一应用只能保存一项表达偏好"); }
        }
        Ok(())
    }
    pub fn permits_input_context(&self, application: &str) -> bool {
        self.input_context_enabled && application_key(application).is_some_and(|key|
            !self.excluded_applications.iter().any(|app|application_key(app).as_ref()==Some(&key)))
    }
    pub fn scene_for(&self, application: &str) -> AppWritingScene {
        if !self.enabled { return AppWritingScene::General; }
        let Some(key) = application_key(application) else { return AppWritingScene::General; };
        if let Some(item) = self.overrides.iter().find(|item| application_key(&item.application).as_deref() == Some(&key)) {
            return item.scene;
        }
        match key.as_str() {
            "wechat.exe" | "weixin.exe" | "qq.exe" | "telegram.exe" | "slack.exe" |
            "feishu.exe" | "lark.exe" | "ms-teams.exe" | "discord.exe" => AppWritingScene::Chat,
            "winword.exe" | "wps.exe" | "onenote.exe" => AppWritingScene::Document,
            "outlook.exe" | "thunderbird.exe" => AppWritingScene::Email,
            "code.exe" | "cursor.exe" | "windsurf.exe" => AppWritingScene::Technical,
            // Browsers and ambiguous/unknown executables need a user preference.
            _ => AppWritingScene::General,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_application_identity_and_explicit_general_override() {
        let mut p = AppWritingPreferences::default();
        assert_eq!(p.scene_for("WeChat.exe"), AppWritingScene::Chat);
        assert_eq!(p.scene_for("WINWORD.EXE"), AppWritingScene::Document);
        assert_eq!(p.scene_for("Code.exe"), AppWritingScene::Technical);
        for app in ["msedge.exe", "chrome.exe", "my-slack.exe", "unknown.exe"] {
            assert_eq!(p.scene_for(app), AppWritingScene::General);
        }
        p.overrides.push(AppWritingOverride { application: "wechat.exe".into(), scene: AppWritingScene::General });
        p.overrides.push(AppWritingOverride { application: "msedge.exe".into(), scene: AppWritingScene::Chat });
        assert_eq!(p.scene_for("WeChat.exe"), AppWritingScene::General);
        assert_eq!(p.scene_for("msedge.exe"), AppWritingScene::Chat);
        p.enabled = false;
        assert_eq!(p.scene_for("msedge.exe"), AppWritingScene::General);
    }
    #[test]
    fn preferences_reject_paths_duplicates_and_unbounded_entries() {
        for value in ["", "*.exe", "C:\\apps\\code.exe", "../code.exe", "code.exe\nother", "code", "../.exe"] {
            assert!(application_key(value).is_none(), "{value}");
        }
        let item = AppWritingOverride { application: "Code.exe".into(), scene: AppWritingScene::Technical };
        let mut p = AppWritingPreferences { enabled: true, overrides: vec![item.clone()], ..Default::default() };
        assert!(p.validate().is_ok());
        p.overrides.push(AppWritingOverride { application: "CODE.EXE".into(), ..item.clone() });
        assert!(p.validate().is_err());
        p.overrides = vec![item; 33];
        assert!(p.validate().is_err());
    }
}
