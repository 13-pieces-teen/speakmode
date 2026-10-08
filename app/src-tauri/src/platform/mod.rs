//! Native operations are explicit; importing this module never starts a hook,
//! opens a microphone, mutates a clipboard, or sends input.
pub mod audio;
pub mod delivery;
pub mod hotkeys;
mod resampler;

#[derive(Debug, thiserror::Error)]
pub enum PlatformError {
    #[error("target has higher integrity; use explicit copy/manual paste")]
    TargetHigherIntegrity,
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
