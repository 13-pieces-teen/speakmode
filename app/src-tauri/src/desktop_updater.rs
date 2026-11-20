//! The main panel receives update metadata. The native coordinator owns the
//! accepted release, verified installer bytes and final shutdown.
use super::{DesktopState, cleanup_resources, exit_failed, refresh_mode_entry, stop_maintenance};
use crate::engine::{ApiError, Result};
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    sync::{Arc, Mutex, atomic::Ordering},
    time::Duration,
};
use tauri::AppHandle;
use tauri_plugin_updater::{Update, UpdaterExt};

const RELEASE_DOWNLOAD: &str = "https://github.com/13-pieces-teen/speakmode/releases/download/";
const CHECK_TIMEOUT: Duration = Duration::from_secs(12);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(180);
const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
enum Phase {
    Disabled,
    Idle,
    Checking,
    UpToDate,
    Available,
    Downloading,
    Downloaded,
    Installing,
    Error,
}

impl Phase {
    fn busy(self) -> bool {
        matches!(self, Self::Checking | Self::Downloading | Self::Installing)
    }
}

struct Pending {
    release: Update,
    verified: Option<Vec<u8>>,
}

struct State {
    phase: Phase,
    pending: Option<Pending>,
    received: u64,
    total: Option<u64>,
    message: Option<String>,
    ready_to_install: bool,
}

pub(super) struct Updates {
    version: String,
    state: Mutex<State>,
}

fn update_error(code: &str, message: &str) -> ApiError {
    ApiError::new(code, "update", message)
}

fn release_url(version: &str) -> Option<String> {
    if version.len() > 32
        || version.split('.').count() != 3
        || version.split('.').any(|part| part.is_empty() || !part.bytes().all(|c| c.is_ascii_digit()))
    {
        return None;
    }
    Some(format!("{RELEASE_DOWNLOAD}v{version}/SpeakMode_{version}_x64-setup.exe"))
}

impl Updates {
    pub(super) fn new(app: &AppHandle, installed_profile: bool) -> tauri::Result<Self> {
        use tauri::utils::{config::BundleType, platform::bundle_type};

        let enabled = installed_profile
            && cfg!(target_arch = "x86_64")
            && bundle_type() == Some(BundleType::Nsis)
            && app.config().plugins.0.get("updater")
                .and_then(|config| config["pubkey"].as_str())
                .is_some_and(|key| !key.trim().is_empty());
        if enabled {
            // The updater's reqwest backend enables rustls-no-provider and
            // can make the first HTTPS request before voice service setup.
            let _ = rustls::crypto::ring::default_provider().install_default();
            app.plugin(tauri_plugin_updater::Builder::new().build())?;
        }
        Ok(Self {
            version: app.package_info().version.to_string(),
            state: Mutex::new(State {
                phase: if enabled { Phase::Idle } else { Phase::Disabled },
                pending: None,
                received: 0,
                total: None,
                message: None,
                ready_to_install: false,
            }),
        })
    }

    pub(super) fn status(&self) -> Value {
        let state = self.state.lock().unwrap();
        let mut view = json!({"currentVersion": self.version, "state": state.phase});
        if let Some(pending) = &state.pending {
            view["version"] = json!(pending.release.version);
            if let Some(notes) = pending.release.body.as_deref() {
                view["notes"] = json!(notes.chars().take(6000).collect::<String>());
            }
        }
        if let Some(message) = &state.message {
            view["message"] = json!(message);
        }
        if state.phase == Phase::Downloading {
            view["downloadedBytes"] = json!(state.received);
            if let Some(total) = state.total {
                view["totalBytes"] = json!(total);
            }
        }
        view
    }

    pub(super) async fn check(&self, app: &AppHandle) {
        {
            let mut state = self.state.lock().unwrap();
            if state.phase == Phase::Disabled || state.phase.busy() || state.ready_to_install {
                return;
            }
            state.phase = Phase::Checking;
            state.message = None;
        }
        let result = match app.updater_builder().timeout(CHECK_TIMEOUT).build() {
            Ok(updater) => updater.check().await,
            Err(error) => Err(error),
        };
        let mut state = self.state.lock().unwrap();
        match result {
            Ok(Some(release)) if release_url(&release.version).as_deref() == Some(release.download_url.as_str()) => {
                let verified = state.pending.take().filter(|old| {
                    old.release.version == release.version && old.release.signature == release.signature
                }).and_then(|old| old.verified);
                state.phase = if verified.is_some() { Phase::Downloaded } else { Phase::Available };
                state.pending = Some(Pending { release, verified });
            }
            Ok(None) => {
                state.pending = None;
                state.phase = Phase::UpToDate;
            }
            Ok(Some(_)) => {
                state.pending = None;
                state.phase = Phase::Error;
                state.message = Some("更新清单中的 Windows 安装包地址无效。".into());
            }
            Err(_) => {
                state.phase = if state.pending.as_ref().is_some_and(|p| p.verified.is_some()) {
                    Phase::Downloaded
                } else {
                    Phase::Error
                };
                state.message = Some("无法检查更新，请检查网络后重试。".into());
            }
        }
    }

    fn selected_release(state: &State, version: &str) -> Result<Update> {
        state.pending.as_ref().filter(|pending| pending.release.version == version)
            .map(|pending| pending.release.clone())
            .ok_or_else(|| update_error("stale_update", "待更新版本已变化，请重新核对版本。"))
    }

    pub(super) async fn download(&self, version: &str) -> Result<()> {
        let mut release = {
            let mut state = self.state.lock().unwrap();
            let release = Self::selected_release(&state, version)?;
            if state.phase == Phase::Downloaded {
                return Ok(());
            }
            if !matches!(state.phase, Phase::Available | Phase::Error) {
                return Err(update_error("update_busy", "更新正在处理，请稍候。"));
            }
            state.phase = Phase::Downloading;
            state.received = 0;
            state.total = None;
            state.message = None;
            release
        };
        release.timeout = Some(DOWNLOAD_TIMEOUT);
        let result = release.download(|chunk, total| {
            let mut state = self.state.lock().unwrap();
            state.received = state.received.saturating_add(chunk as u64);
            state.total = total;
        }, || {}).await;
        let mut state = self.state.lock().unwrap();
        match result {
            Ok(verified) if !verified.is_empty() => {
                if let Some(pending) = state.pending.as_mut() {
                    pending.verified = Some(verified);
                }
                state.phase = Phase::Downloaded;
            }
            other => {
                use tauri_plugin_updater::Error;
                state.phase = Phase::Error;
                state.message = Some(match other {
                    Err(Error::Minisign(_) | Error::Base64(_) | Error::SignatureUtf8(_) |
                        Error::SignedVersionMismatch { .. } | Error::MissingSignedVersion) =>
                        "更新包签名或版本校验失败，请重新检查更新。",
                    _ => "下载更新失败，请检查网络后重试。",
                }.into());
            }
        }
        Ok(())
    }

    pub(super) async fn install(&self, app: &AppHandle, desktop: &Arc<DesktopState>, version: &str) -> Result<()> {
        let (release, verified, already_prepared) = {
            let mut state = self.state.lock().unwrap();
            let release = Self::selected_release(&state, version)?;
            if state.phase != Phase::Downloaded {
                return Err(update_error("update_not_downloaded", "请先下载并验证此版本。"));
            }
            let verified = state.pending.as_mut().and_then(|pending| pending.verified.take())
                .ok_or_else(|| update_error("update_not_downloaded", "请重新下载此版本。"))?;
            state.phase = Phase::Installing;
            state.message = None;
            (release, verified, state.ready_to_install)
        };
        if !already_prepared {
            if let Err(error) = prepare_install(app, desktop).await {
                self.restore_package(verified, error.message);
                return Ok(());
            }
            self.state.lock().unwrap().ready_to_install = true;
        } else {
            desktop.exit.store(1, Ordering::Release);
        }

        // Windows exits the current process on successful installer launch.
        // A launch error returns with the verified bytes intact for a retry.
        let result = tauri::async_runtime::spawn_blocking(move || {
            let launched = release.install(&verified);
            (verified, launched)
        }).await;
        match result {
            Ok((_, Ok(()))) => {},
            Ok((verified, Err(_))) => {
                desktop.exit.store(2, Ordering::Release);
                self.restore_package(verified, "无法启动更新安装程序，可重试安装或重新打开 SpeakMode。".into());
            }
            Err(_) => {
                desktop.exit.store(2, Ordering::Release);
                let mut state = self.state.lock().unwrap();
                state.phase = Phase::Error;
                state.message = Some("更新启动意外中断，请重新打开 SpeakMode 后再试。".into());
            }
        }
        Ok(())
    }

    fn restore_package(&self, verified: Vec<u8>, message: String) {
        let mut state = self.state.lock().unwrap();
        if let Some(pending) = state.pending.as_mut() {
            pending.verified = Some(verified);
        }
        state.phase = Phase::Downloaded;
        state.message = Some(message);
    }
}

async fn prepare_install(app: &AppHandle, desktop: &Arc<DesktopState>) -> Result<()> {
    if desktop.exit.compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire).is_err() {
        return Err(update_error("update_exit_busy", "退出流程已开始，请重新打开 SpeakMode 后再安装。"));
    }
    if let Err(error) = desktop.hub.begin_update_shutdown() {
        desktop.exit.store(0, Ordering::Release);
        return Err(error);
    }
    refresh_mode_entry(app, desktop);
    let prepared = async {
        cleanup_resources(desktop).await?;
        stop_maintenance(desktop).await
    }.await;
    if let Err(error) = prepared {
        exit_failed(app, desktop, error.clone());
        return Err(error);
    }
    Ok(())
}

pub(super) fn schedule(app: AppHandle, desktop: &Arc<DesktopState>) {
    if desktop.updates.state.lock().unwrap().phase == Phase::Disabled {
        return;
    }
    let desktop = Arc::downgrade(desktop);
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(8)).await;
        loop {
            let Some(state) = desktop.upgrade() else { return; };
            if state.exit.load(Ordering::Acquire) == 0 {
                state.updates.check(&app).await;
            }
            drop(state);
            tokio::time::sleep(CHECK_INTERVAL).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signed_windows_assets_are_bound_to_a_stable_versioned_location() {
        assert_eq!(
            release_url("0.2.48").as_deref(),
            Some("https://github.com/13-pieces-teen/speakmode/releases/download/v0.2.48/SpeakMode_0.2.48_x64-setup.exe")
        );
        for version in ["0.2.48-beta.1", "0.2.48+other", "0.2/47", "v0.2.47", "0..47", "0.2.47?alt"] {
            assert!(release_url(version).is_none(), "{version}");
        }
    }
}
