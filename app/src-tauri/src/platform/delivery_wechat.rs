//! Automatic keyboard routing for writable controls with no caret provider,
//! plus the observed opaque WeChat window. Neither route invents a caret.
use serde::{Deserialize, Serialize};

/// Metadata frozen synchronously at stop, before scheduling native inspection.
/// Equality is a conservative activity veto, not proof of an editor selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InputActivity {
    pub(super) generation: u64,
    pub(super) interactions: u64,
}

pub(super) fn supported_window(executable: &str, class_name: &str) -> bool {
    if !["Weixin.exe", "WeChat.exe"].iter().any(|name| executable.eq_ignore_ascii_case(name)) {
        return false;
    }
    let Some(version) = class_name.strip_prefix("Qt").and_then(|name| name.strip_suffix("QWindowIcon")) else { return false; };
    !version.is_empty() && version.len() <= 12 && version.bytes().all(|c| c.is_ascii_digit())
}

pub(super) fn valid_shape(target: &super::TargetSnapshot) -> bool {
    target.compatibility.is_some_and(|activity| activity.generation != 0) && target.hwnd != 0 && target.process_id != 0
        && target.process_created_at != 0 && target.focused_hwnd != 0 && target.selection.is_none()
        && target.runtime_id.len() <= 256
        && (!target.runtime_id.is_empty() || target.focused_hwnd == target.hwnd)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn compatibility_is_limited_to_the_observed_app_and_native_window_family() {
        for name in ["Weixin.exe", "WECHAT.EXE"] {
            assert!(supported_window(name, "Qt51514QWindowIcon"));
        }
        for (name, class) in [("editor.exe", "Qt51514QWindowIcon"), ("Weixin.exe.bak", "Qt51514QWindowIcon"),
            ("Weixin.exe", "QtQWindowIcon"), ("Weixin.exe", "Qt51514Popup"), ("WeChat.exe", "Edit"),
            ("Weixin.exe", "Qt51514xQWindowIcon"), ("Weixin.exe", "Qt1234567890123QWindowIcon")] {
            assert!(!supported_window(name, class), "{name}/{class}");
        }
    }
}
