//! Native operations are explicit; importing this module never starts a hook,
//! opens a microphone, mutates a clipboard, or sends input.
pub mod audio;
pub mod audio_conditioner;
pub mod delivery;
pub mod hotkeys;
pub mod typeless_dictionary;
mod resampler;

/// Stable, content-free failures at the input-target boundary. Native provider
/// text, window titles and COM error strings never become delivery diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TargetCaptureFailure {
    #[error("stop anchor unavailable")]
    StopAnchorUnavailable,
    #[error("stop focus changed")]
    StopFocusChanged,
    #[error("target is not editable")]
    TargetNotEditable,
    #[error("target capability unsupported")]
    TargetCapabilityUnsupported,
    #[error("target inspection timed out")]
    TargetInspectionTimeout,
}
impl TargetCaptureFailure {
    pub fn delivery_reason(self) -> &'static str {
        match self {
            Self::StopAnchorUnavailable => "stopAnchorUnavailable",
            Self::StopFocusChanged => "stopFocusChanged",
            Self::TargetNotEditable => "targetNotEditable",
            Self::TargetCapabilityUnsupported => "targetCapabilityUnsupported",
            Self::TargetInspectionTimeout => "targetInspectionTimeout",
        }
    }
    pub fn api_code(self) -> &'static str {
        match self {
            Self::StopAnchorUnavailable => "stop_anchor_unavailable",
            Self::StopFocusChanged => "stop_focus_changed",
            Self::TargetNotEditable => "target_not_editable",
            Self::TargetCapabilityUnsupported => "target_capability_unsupported",
            Self::TargetInspectionTimeout => "target_inspection_timeout",
        }
    }
    pub fn message(self) -> &'static str {
        match self {
            Self::StopAnchorUnavailable => "录音已结束，未能确定停止时的输入窗口；成稿已保留",
            Self::StopFocusChanged => "停止后输入位置已改变，本次未自动输入；成稿已保留",
            Self::TargetNotEditable => "当前目标不是可写输入位置，本次未自动输入；成稿已保留",
            Self::TargetCapabilityUnsupported => "当前应用未提供可验证的输入位置，本次未自动输入；成稿已保留",
            Self::TargetInspectionTimeout => "输入位置检查超时，本次未自动输入；成稿已保留",
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PlatformError {
    #[error("target has higher integrity; use explicit copy/manual paste")]
    TargetHigherIntegrity,
    #[error("{0}")]
    TargetCapture(TargetCaptureFailure),
    #[error("{0}")]
    Unavailable(String),
    #[error("{0}")]
    Invalid(String),
    #[error("native operation timed out: {0}")]
    Timeout(&'static str),
    #[error("native worker is no longer running")]
    Disconnected,
}
pub type Result<T> = std::result::Result<T, PlatformError>;
