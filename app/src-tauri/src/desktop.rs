//! Native desktop composition. Engine owns session/data semantics; this layer owns
//! windows, OS integration and authenticated-by-window IPC routing.
//! Native audio, focus and keyboard QA must run on each supported OS; geometry
//! tests do not stand in for accessibility, delivery or microphone acceptance.
use crate::engine::{ApiError, AppEvent, AppHub, CommandRequest, Result};
use crate::platform::hotkeys::{HotkeyEvent, HotkeyService};
use serde_json::{Value, json};
use std::{path::PathBuf, sync::{Arc, Mutex, Weak, atomic::{AtomicBool, AtomicU8, Ordering}}, time::Duration};
use tauri::{AppHandle, Manager, State, WebviewWindow, WebviewWindowBuilder, WindowEvent, RunEvent,
    ipc::Channel, menu::{Menu, MenuItem}, tray::TrayIconBuilder};
#[cfg(windows)]
use tauri::tray::{TrayIconEvent, MouseButton, MouseButtonState};
use tauri_plugin_autostart::ManagerExt;

struct DesktopState {
    hub: Arc<AppHub>,
    // 0 running, 1 cleanup requested, 2 confirmed, 3 cleanup failed/retry only. ExitRequested fires
    // again after AppHandle::exit: allow it only after state 2, without recursion.
    exit: AtomicU8,
    hook_starting: AtomicBool,
    maintenance_starting: AtomicBool,
    maintenance: Mutex<Option<crate::maintenance::Server>>,
    ui_thread: std::thread::ThreadId,
    recorder: Mutex<RecorderState>,
    mode_item: Mutex<Option<MenuItem<tauri::Wry>>>,
}
#[derive(Default)]
struct RecorderState {
    menu_open: bool,
    visible: bool,
    return_focus: Option<ForegroundIdentity>,
    tray_request: Option<RecorderModeRequest>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ForegroundIdentity { hwnd: usize, pid: u32, created: u64 }
#[derive(Clone, Copy)]
enum ModeEntry { Mouse, Tray }
fn mode_return_focus(entry: ModeEntry, observed: Option<ForegroundIdentity>, recorder_hwnd: usize) -> Option<ForegroundIdentity> {
    // Notification-area navigation has already changed foreground ownership.
    // Without a verified pre-tray window, do not guess or reuse the session host.
    match entry { ModeEntry::Tray => None, ModeEntry::Mouse => observed.filter(|saved| saved.hwnd != recorder_hwnd) }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct RecorderModeRequest { session_id: String, generation: u64 }

fn mode_menu_request(snapshot: &Value, exit: u8) -> Result<RecorderModeRequest> {
    if exit != 0 { return Err(native_error("app_exiting", "正在退出，无法调整本次 Mode")); }
    let session = &snapshot["session"];
    if !matches!(session["state"].as_str(), Some("starting" | "recording")) {
        return Err(native_error("mode_frozen", "仅在录音期间调整本次 Mode"));
    }
    let id = session["sessionId"].as_str().or(session["id"].as_str())
        .filter(|id| !id.is_empty()).ok_or_else(|| native_error("no_session", "没有正在进行的口述"))?;
    let generation = session["generation"].as_u64().ok_or_else(|| native_error("stale_session", "这次口述已变化"))?;
    Ok(RecorderModeRequest { session_id: id.to_owned(), generation })
}
fn mode_request_is_current(snapshot: &Value, exit: u8, request: &RecorderModeRequest) -> bool {
    mode_menu_request(snapshot, exit).is_ok_and(|current| &current == request)
}
fn mode_entry_enabled(snapshot: &Value, exit: u8, visible: bool) -> bool {
    // Starting exists before target capture. The engine shows the recorder only
    // after that attempt; do not let tray activation race its original focus.
    visible && mode_menu_request(snapshot, exit).is_ok()
}
fn hotkey_error_scope(error: ApiError, before: Option<&Value>) -> ApiError {
    // Only a stop/busy error belongs to the session observed at this operation's
    // entry. New-start configuration errors must not inherit an old active ID.
    if error.scope.as_deref() == Some("session") { return error; }
    if let Some(before) = before {
        let session = &before["session"];
        if matches!(session["state"].as_str(), Some("starting" | "recording")) || error.code == "session_busy" {
            if let (Some(id), Some(generation)) = (session["sessionId"].as_str().or(session["id"].as_str()), session["generation"].as_u64()) {
                return error.for_session(id, generation);
            }
        }
    }
    error
}
fn reveal_error_without_recorder(error: &ApiError, visible: bool, exit: u8, starting_or_recording: bool) -> bool {
    exit == 0 && !visible && !starting_or_recording && !matches!(error.code.as_str(), "cancelled" | "stale_session")
}
fn error_matches_current_session(error: &ApiError, snapshot: &Value) -> bool {
    if error.scope.as_deref() != Some("session") { return true; }
    match (error.session_id.as_deref(), error.generation) {
        (Some(id), Some(generation)) => Some(id) == snapshot["session"]["sessionId"].as_str().or(snapshot["session"]["id"].as_str()) &&
            Some(generation) == snapshot["session"]["generation"].as_u64(),
        _ => false,
    }
}

fn native_error(code: &str, message: &str) -> ApiError { ApiError::new(code, "native", message) }
fn window_error(_: impl std::fmt::Display) -> ApiError { native_error("window_unavailable", "窗口操作未完成，请从托盘打开主面板") }
fn current_state(app: &AppHandle) -> Option<Arc<DesktopState>> { app.try_state::<Arc<DesktopState>>().map(|s| Arc::clone(s.inner())) }

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
enum MainWindowAction { State, Drag, Minimize, ToggleMaximize, Close }

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct MainWindowStatus { maximized: bool, minimized: bool }

/// Titlebar actions use the actual caller window, never a frontend-supplied
/// label. The recording surface receives no window-management capability.
#[tauri::command]
fn control_main_window(window: WebviewWindow, action: MainWindowAction) -> Result<MainWindowStatus> {
    if window.label() != "main" { return Err(native_error("forbidden_window", "此界面不能管理主窗口")); }
    match action {
        MainWindowAction::State => {},
        MainWindowAction::Drag => window.start_dragging().map_err(window_error)?,
        MainWindowAction::Minimize => {
            if let Some(state) = current_state(window.app_handle()) { state.hub.onboarding_main_hidden(); }
            window.minimize().map_err(window_error)?;
        },
        MainWindowAction::ToggleMaximize => {
            if window.is_maximized().map_err(window_error)? { window.unmaximize().map_err(window_error)?; }
            else { window.maximize().map_err(window_error)?; }
        },
        // Keep the existing CloseRequested -> hide-to-tray behavior, including
        // native shortcut-capture lease release. This is not application exit.
        MainWindowAction::Close => window.close().map_err(window_error)?,
    }
    Ok(MainWindowStatus { maximized: window.is_maximized().map_err(window_error)?, minimized: window.is_minimized().map_err(window_error)? })
}

#[tauri::command]
async fn native_permissions(window: WebviewWindow, state: State<'_, Arc<DesktopState>>, action: String) -> Result<Value> {
    if window.label() != "main" { return Err(native_error("forbidden_window", "请在主面板管理权限")); }
    if action != "status" && state.exit.load(Ordering::Acquire) != 0 { return Err(native_error("app_exiting", "正在退出，请等待本机任务释放")); }
    #[cfg(target_os = "macos")]
    {
        use crate::platform::macos::permissions;
        match action.as_str() {
            "status" => {},
            "microphone" => { on_ui(window.app_handle(), &state, || { permissions::request_microphone(); Ok(()) })?; },
            "accessibility" => { on_ui(window.app_handle(), &state, || { permissions::request_accessibility(); Ok(()) })?; },
            "inputMonitoring" => { on_ui(window.app_handle(), &state, || { permissions::request_input_monitoring(); Ok(()) })?; },
            "restartShortcuts" => {
                if state.hook_starting.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_ok() {
                    let released = state.hub.hooks.lock().unwrap().as_ref().is_none_or(|owner| owner.owner_released());
                    state.hook_starting.store(false, Ordering::Release);
                    if released {
                        let shortcut = state.hub.snapshot("main")?["preferences"]["shortcut"].as_str().unwrap_or("alt_right").to_owned();
                        start_hotkeys(window.app_handle().clone(), Arc::clone(&state), shortcut);
                    }
                }
            },
            _ => return Err(native_error("invalid_permission", "未知权限操作")),
        }
        let mut status = json!(permissions::status());
        status["shortcutsReady"] = json!(state.hub.hooks.lock().unwrap().as_ref().is_some_and(|owner| owner.available()));
        status["shortcutsStarting"] = json!(state.hook_starting.load(Ordering::Acquire));
        Ok(status)
    }
    #[cfg(not(target_os = "macos"))]
    { let _ = (action, state); Ok(json!({"platform":"windows"})) }
}

#[tauri::command]
async fn app_command(window: WebviewWindow, state: State<'_, Arc<DesktopState>>, request: CommandRequest) -> Result<Value> {
    // Never accept a client-supplied window label. Engine applies the recording
    // surface whitelist to this label before payloads are processed.
    let exit = state.exit.load(Ordering::Acquire);
    let allowed_during_exit = matches!(request.name.as_str(), "state.snapshot" | "app.showMain" | "app.quit") ||
        exit == 3 && matches!(request.name.as_str(), "preferences.get" | "mode.list" | "provider.status" |
            "credential.status" | "history.get" | "history.list" | "usage.today" | "usage.total" | "result.copy" | "recoverable.copy");
    if exit != 0 && !allowed_during_exit {
        return Err(native_error("app_exiting", if exit == 3 { "退出尚未完成，可核对复制已有内容，并从托盘再次选择退出" } else { "正在退出，请等待本机任务释放" }));
    }
    let cancel = request.name == "session.cancel";
    let result = state.hub.command(window.label(), request).await;
    if cancel {
        if result.is_ok() { recorder_visibility(&window.app_handle().clone(), state.inner(), false)?; }
        else { let _ = show_main(window.app_handle()); }
    }
    result
}

#[tauri::command]
fn subscribe_state(window: WebviewWindow, state: State<'_, Arc<DesktopState>>, channel: Channel<AppEvent>) -> Result<Value> {
    let handle = window.app_handle().clone();
    let weak = Arc::downgrade(state.inner());
    let result = state.hub.subscribe(window.label(), Arc::new(move |event| {
        // Engine holds its publication ordering lock here. Do not call back into
        // subscribe/unsubscribe on send failure; reload replaces this sink.
        // Queue the tray refresh; never query the hub or wait for UI under that lock.
        if matches!(event.kind.as_str(), "session" | "changed" | "interrupted") {
            if let Some(state) = weak.upgrade() { refresh_mode_entry(&handle, &state); }
        }
        let _ = channel.send(event);
    }));
    if result.is_ok() { refresh_mode_entry(window.app_handle(), state.inner()); }
    result
}

fn show_main(app: &AppHandle) -> Result<()> {
    let window = app.get_webview_window("main").ok_or_else(|| window_error("missing main"))?;
    window.unminimize().map_err(window_error)?;
    window.show().map_err(window_error)?;
    window.set_focus().map_err(window_error)
}
fn publish_error(state: &DesktopState, error: ApiError) { state.hub.publish("error", json!(error)); }
fn reveal_desktop_error(app: &AppHandle, state: &Arc<DesktopState>, error: ApiError) {
    let presented = error.clone();
    // First broadcast the actionable error, then show its main-panel entry.
    // Never reveal the main window before the normal start captures its target.
    publish_error(state, error);
    let handle = app.clone();
    let state2 = Arc::clone(state);
    let _ = on_ui(app, state, move || {
        let snapshot = state2.hub.snapshot("recording")?;
        let session = &snapshot["session"];
        if !error_matches_current_session(&presented, &snapshot) { return Ok(()); }
        let visible = state2.recorder.lock().map(|recorder| recorder.visible).unwrap_or(true);
        let capturing = matches!(session["state"].as_str(), Some("starting" | "recording"));
        if reveal_error_without_recorder(&presented, visible, state2.exit.load(Ordering::Acquire), capturing) { show_main(&handle)?; }
        Ok(())
    });
}
fn refresh_mode_entry(app: &AppHandle, state: &Arc<DesktopState>) {
    let weak = Arc::downgrade(state);
    let handle = app.clone();
    // run_on_main_thread may execute inline when invoked on the owning thread.
    // Leave the publication callback first to avoid reentrant hub/UI lock work.
    tauri::async_runtime::spawn(async move {
        let ui_handle = handle.clone();
        let _ = handle.run_on_main_thread(move || {
            let Some(state) = weak.upgrade() else { return; };
            let snapshot = state.hub.snapshot("recording").ok();
            let visible = state.recorder.lock().map(|recorder| recorder.visible).unwrap_or(false);
            let enabled = snapshot.as_ref()
                .is_some_and(|snapshot| mode_entry_enabled(snapshot, state.exit.load(Ordering::Acquire), visible));
            if let Ok(slot) = state.mode_item.lock() { if let Some(item) = slot.as_ref() { let _ = item.set_enabled(enabled); } }
            #[cfg(any(windows, target_os = "macos"))]
            if let Some(window) = ui_handle.get_webview_window("recording") {
                let _ = win::close_stale_mode(&window, &state, snapshot.as_ref());
            }
        });
    });
}
fn begin_exit(app: &AppHandle, state: &Arc<DesktopState>) -> bool {
    let previous = state.exit.load(Ordering::Acquire);
    if !matches!(previous, 0 | 3) || state.exit.compare_exchange(previous, 1, Ordering::AcqRel, Ordering::Acquire).is_err() { return false; }
    refresh_mode_entry(app, state);true
}
async fn cleanup_resources(state: &DesktopState) -> Result<()> {
    // Native hook startup must acknowledge its late owner before shutdown can
    // finish. Database shutdown likewise drains accepted jobs and drops Store.
    let deadline=std::time::Instant::now()+Duration::from_secs(8);
    while state.hook_starting.load(Ordering::Acquire) {
        if std::time::Instant::now()>=deadline {return Err(native_error("shortcut_startup_busy","快捷键仍在启动或释放，请稍候再退出"));}
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    state.hub.shutdown_ready().await
}
fn exit_failed(app:&AppHandle,state:&DesktopState,mut error:ApiError){
    state.exit.store(3,Ordering::Release);
    error.scope=Some("application".into());error.session_id=None;error.generation=None;
    error.message.push_str("。退出尚未完成，请从托盘再次选择退出。");
    publish_error(state,error);let _=show_main(app);
}
async fn stop_maintenance(state:&DesktopState)->Result<()> {
    let deadline=std::time::Instant::now()+Duration::from_secs(5);
    loop {
        if !state.maintenance_starting.load(Ordering::Acquire) {
            let released={let server=state.maintenance.lock().unwrap();server.as_ref().is_none_or(|server|{server.stop();server.released()})};
            if released{return Ok(());}
        }
        if std::time::Instant::now()>=deadline{return Err(native_error("maintenance_busy","安装维护连接尚未释放，请稍候再退出"));}
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
#[cfg(any(windows, target_os="macos"))]
fn start_maintenance(app:AppHandle,state:Arc<DesktopState>){
    state.maintenance_starting.store(true,Ordering::Release);
    tauri::async_runtime::spawn(async move{
        struct Startup(Arc<DesktopState>);impl Drop for Startup{fn drop(&mut self){self.0.maintenance_starting.store(false,Ordering::Release);}}
        let _startup=Startup(Arc::clone(&state));
        let weak=Arc::downgrade(&state);let prepare_app=app.clone();
        let prepare:crate::maintenance::Prepare=Arc::new(move||{
            let weak=weak.clone();let app=prepare_app.clone();
            Box::pin(async move{
                let state=weak.upgrade().ok_or_else(||std::io::Error::other("application unavailable"))?;
                if !begin_exit(&app,&state){return Err(std::io::Error::other("exit already in progress"));}
                match cleanup_resources(&state).await {
                    Ok(())=>{state.exit.store(2,Ordering::Release);Ok(())},
                    Err(error)=>{exit_failed(&app,&state,error);Err(std::io::Error::other("application resource cleanup incomplete"))}
                }
            })
        });
        let ready_app=app.clone();
        match crate::maintenance::start_server(prepare,Arc::new(move|_delivery|ready_app.exit(0))){
            Ok(server)=>{if state.exit.load(Ordering::Acquire)!=0{server.stop();}*state.maintenance.lock().unwrap()=Some(server);},
            Err(_)=>publish_error(&state,native_error("maintenance_unavailable","安装维护检查未启动。卸载前请先从托盘退出应用"))
        }
    });
}
fn request_exit(app: AppHandle, state: Arc<DesktopState>) {
    if !begin_exit(&app,&state){return;}
    tauri::async_runtime::spawn(async move {
        let cleanup = async {
            cleanup_resources(&state).await?;
            stop_maintenance(&state).await
        }.await;
        match cleanup {
            Ok(()) => { state.exit.store(2, Ordering::Release); app.exit(0); }
            Err(error) => exit_failed(&app,&state,error)
        }
    });
}

// Dispatch Win32 geometry/focus work to the owning UI thread. A queued operation
// that times out is cancelled before it can execute later and steal focus.
fn on_ui<T:Send+'static>(app: &AppHandle, state: &DesktopState, work: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T> {
    if std::thread::current().id() == state.ui_thread { return work(); }
    let ticket = Arc::new(AtomicU8::new(0)); // queued / started / cancelled
    let run_ticket = Arc::clone(&ticket);
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    app.run_on_main_thread(move || {
        if run_ticket.compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire).is_ok() {
            let _ = tx.send(work());
        }
    }).map_err(window_error)?;
    match rx.recv_timeout(Duration::from_secs(3)) {
        Ok(result) => result,
        Err(_) => {
            let _ = ticket.compare_exchange(0, 2, Ordering::AcqRel, Ordering::Acquire);
            Err(native_error("window_timeout", "窗口暂未响应，请重试"))
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Rect { x: i32, y: i32, width: u32, height: u32 }
fn recorder_rect(width: f64, height: f64, scale: f64, area: Rect) -> Result<Rect> {
    if !width.is_finite() || !height.is_finite() || !scale.is_finite() || scale <= 0.0 ||
        !(240.0..=640.0).contains(&width) || !(64.0..=420.0).contains(&height) || area.width == 0 || area.height == 0 {
        return Err(native_error("invalid_geometry", "浮层尺寸或显示器信息无效"));
    }
    let w = ((width * scale).round() as u32).min(area.width).max(1);
    let h = ((height * scale).round() as u32).min(area.height).max(1);
    let gap = ((12.0 * scale).round() as u32).min(area.height.saturating_sub(h));
    let x = i64::from(area.x) + i64::from((area.width - w) / 2);
    let y = i64::from(area.y) + i64::from(area.height - h - gap);
    Ok(Rect { x: x.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32,
        y: y.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32, width: w, height: h })
}

#[cfg(target_os = "macos")]
#[path = "platform/macos/window.rs"]
mod mac_window;
#[cfg(target_os = "macos")]
use mac_window as win;

#[cfg(windows)]
mod win {
    use super::*;
    use windows::Win32::{Foundation::{HWND, FILETIME, RECT, CloseHandle},
        System::Threading::{OpenProcess, GetProcessTimes, PROCESS_QUERY_LIMITED_INFORMATION},
        UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowRect, GetWindowThreadProcessId, IsWindow,
            SetForegroundWindow, GetWindowLongPtrW, SetWindowLongPtrW, SetWindowPos, ShowWindow,
            GWL_EXSTYLE, WS_EX_NOACTIVATE, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
            SWP_FRAMECHANGED, SW_SHOWNOACTIVATE, SW_HIDE, HWND_TOPMOST}};
    fn hwnd(window: &WebviewWindow) -> Result<HWND> { window.hwnd().map(|h| HWND(h.0 as _)).map_err(window_error) }
    fn identity(handle: HWND) -> Option<ForegroundIdentity> {
        if handle.0.is_null() || !unsafe { IsWindow(handle) }.as_bool() { return None; }
        let mut pid = 0;
        unsafe { GetWindowThreadProcessId(handle, Some(&mut pid)); }
        if pid == 0 { return None; }
        let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
        let (mut created, mut exited, mut kernel, mut user) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
        let result = unsafe { GetProcessTimes(process, &mut created, &mut exited, &mut kernel, &mut user) };
        let _ = unsafe { CloseHandle(process) };
        result.ok()?;
        Some(ForegroundIdentity { hwnd: handle.0 as usize, pid,
            created: (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime) })
    }
    fn no_activate(handle: HWND, enabled: bool) -> Result<()> {
        let old = unsafe { GetWindowLongPtrW(handle, GWL_EXSTYLE) };
        let next = if enabled { old | WS_EX_NOACTIVATE.0 as isize } else { old & !(WS_EX_NOACTIVATE.0 as isize) };
        unsafe { SetWindowLongPtrW(handle, GWL_EXSTYLE, next); }
        unsafe { SetWindowPos(handle, None, 0, 0, 0, 0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED) }.map_err(window_error)
    }
    fn close_menu(handle: HWND, recorder: &mut RecorderState, restore_focus: bool) -> Result<()> {
        if recorder.menu_open {
            // Restore only the identity explicitly captured BEFORE menu activation.
            // If the user switched away, the window/PID was recycled, or process
            // identity is unreadable: do nothing. Never AttachThreadInput and never
            // restore in delivery. Delivery independently validates its UIA target;
            // an unavailable target must fall back to manual copy, not refocus/paste.
            if restore_focus && unsafe { GetForegroundWindow() } == handle {
                if let Some(saved) = recorder.return_focus.take() {
                    let target = HWND(saved.hwnd as _);
                    if identity(target) == Some(saved) { let _ = unsafe { SetForegroundWindow(target) }; }
                }
            }
        }
        recorder.menu_open = false;
        recorder.return_focus = None;
        recorder.tray_request = None;
        no_activate(handle, true)
    }
    fn monitor(window: &WebviewWindow, prefer_foreground: bool) -> Result<tauri::Monitor> {
        if prefer_foreground {
            let mut rect = RECT::default();
            if unsafe { GetWindowRect(GetForegroundWindow(), &mut rect) }.is_ok() {
                let x = (f64::from(rect.left) + f64::from(rect.right)) / 2.0;
                let y = (f64::from(rect.top) + f64::from(rect.bottom)) / 2.0;
                if let Some(value) = window.monitor_from_point(x, y).map_err(window_error)? { return Ok(value); }
            }
        }
        window.current_monitor().map_err(window_error)?.or(window.primary_monitor().map_err(window_error)?)
            .ok_or_else(|| native_error("monitor_unavailable", "无法读取显示器工作区"))
    }
    fn place(window: &WebviewWindow, width: f64, height: f64, prefer_foreground: bool) -> Result<()> {
        let m = monitor(window, prefer_foreground)?;
        let area = m.work_area();
        let rect = recorder_rect(width, height, m.scale_factor(), Rect {
            x: area.position.x, y: area.position.y, width: area.size.width, height: area.size.height })?;
        unsafe { SetWindowPos(hwnd(window)?, HWND_TOPMOST, rect.x, rect.y, rect.width as i32, rect.height as i32,
            SWP_NOACTIVATE) }.map_err(window_error)
    }
    pub(super) fn visible(window: &WebviewWindow, state: &DesktopState, show: bool) -> Result<()> {
        let handle = hwnd(window)?;
        let mut recorder = state.recorder.lock().map_err(|_| window_error("lock"))?;
        // Hiding may be part of dispatch, so it MUST NOT restore a former target.
        close_menu(handle, &mut recorder, false)?;
        if show { place(window, 336.0, 66.0, true)?; }
        // All recorder visibility/style/size changes are Win32-only after creation.
        // Mixing Tauri/Tao .show()/set_focusable() here would reapply Tao's cached
        // VISIBLE flags and may reactivate or hide a native-shown overlay.
        // ShowWindow returns previous visibility, NOT operation success.
        let _ = unsafe { ShowWindow(handle, if show { SW_SHOWNOACTIVATE } else { SW_HIDE }) };
        recorder.visible = show;
        Ok(())
    }
    pub(super) fn blurred(window: &WebviewWindow, state: &DesktopState) -> Result<()> {
        let handle = hwnd(window)?;
        // Ignore an older blur event if the user already reopened/focused Mode.
        if unsafe { GetForegroundWindow() } == handle { return Ok(()); }
        let mut recorder = state.recorder.lock().map_err(|_| window_error("lock"))?;
        close_menu(handle, &mut recorder, false)
    }
    pub(super) fn close_stale_mode(window: &WebviewWindow, state: &DesktopState, snapshot: Option<&Value>) -> Result<()> {
        let handle = hwnd(window)?;
        let mut recorder = state.recorder.lock().map_err(|_| window_error("lock"))?;
        let current = snapshot.is_some_and(|snapshot| {
            mode_menu_request(snapshot, state.exit.load(Ordering::Acquire)).is_ok() &&
            recorder.tray_request.as_ref().is_none_or(|request| mode_request_is_current(snapshot, state.exit.load(Ordering::Acquire), request))
        });
        if recorder.menu_open && !current {
            // A stale intent may never leave the native surface activated just
            // because the frontend rejected it before opening its DOM menu.
            close_menu(handle, &mut recorder, true)?;
            if recorder.visible { place(window, 336.0, 66.0, false)?; }
        }
        Ok(())
    }
    pub(super) fn stop_target(window:&WebviewWindow,state:&DesktopState,expected:crate::platform::delivery::ForegroundAnchor,menu_return:Option<ForegroundIdentity>)->Result<crate::platform::delivery::ForegroundAnchor> {
        use crate::platform::delivery::capture_foreground_anchor;
        let handle=hwnd(window)?;
        let current=capture_foreground_anchor().map_err(ApiError::from)?;
        let mut recorder=state.recorder.lock().map_err(|_|window_error("lock"))?;
        if current!=expected{
            // A phase/blur resize may already have closed the menu while this
            // UI callback was queued. Accept only the return identity captured
            // synchronously by this stop, never whichever window is now active.
            if expected.hwnd==handle.0 as usize&&menu_return.is_some_and(|saved|(current.hwnd,current.process_id,current.process_created_at)==(saved.hwnd,saved.pid,saved.created)&&identity(HWND(saved.hwnd as _))==Some(saved)){
                close_menu(handle,&mut recorder,false)?;return Ok(current);
            }
            return Err(native_error("stop_target_changed","停止后输入位置已改变，成稿将保留供复制"));
        }
        if expected.hwnd!=handle.0 as usize{
            // The stop happened in an external editor. Closing a stale menu
            // must not restore an older window over this current target.
            close_menu(handle,&mut recorder,false)?;
            return Ok(expected);
        }
        // Only an open Mode menu has an explicit, verified return identity.
        // If the user switched away before this callback, the anchor check
        // above rejects it instead of adopting the newly focused window.
        let saved=menu_return.filter(|saved|recorder.menu_open&&recorder.return_focus==Some(*saved)).ok_or_else(||native_error("stop_target_unavailable","请在输入位置结束口述，成稿将保留供复制"))?;
        if identity(HWND(saved.hwnd as _))!=Some(saved){return Err(native_error("stop_target_unavailable","原输入窗口已变化，成稿将保留供复制"));}
        close_menu(handle,&mut recorder,true)?;
        drop(recorder);
        let restored=capture_foreground_anchor().map_err(ApiError::from)?;
        if (restored.hwnd,restored.process_id,restored.process_created_at)!=(saved.hwnd,saved.pid,saved.created){return Err(native_error("stop_target_changed","未能返回输入位置，成稿将保留供复制"));}
        Ok(restored)
    }
    pub(super) fn open_mode_from_tray(window: &WebviewWindow, state: &DesktopState, request: RecorderModeRequest) -> Result<()> {
        let handle = hwnd(window)?;
        let mut recorder = state.recorder.lock().map_err(|_| window_error("lock"))?;
        // Tray keyboard navigation no longer exposes the prior external window.
        // Clear any older menu receipt too; never substitute the initial host.
        recorder.return_focus = mode_return_focus(ModeEntry::Tray, None, handle.0 as usize);
        if !recorder.visible {
            return Err(native_error("mode_not_ready", "录音浮层尚未就绪，请稍候再进入本次 Mode"));
        }
        no_activate(handle, false)?;
        let _ = unsafe { SetForegroundWindow(handle) };
        if unsafe { GetForegroundWindow() } != handle {
            close_menu(handle, &mut recorder, false)?;
            return Err(native_error("mode_focus_unavailable", "未能进入本次 Mode 菜单，请在录音浮层点击 Mode"));
        }
        recorder.menu_open = true;
        recorder.tray_request = Some(request);
        Ok(())
    }
    pub(super) fn resize(window: &WebviewWindow, state: &DesktopState, width: f64, height: f64, menu_open: bool) -> Result<()> {
        let handle = hwnd(window)?;
        let mut recorder = state.recorder.lock().map_err(|_| window_error("lock"))?;
        place(window, width, height, false)?;
        if menu_open && !recorder.menu_open && recorder.visible {
            let foreground = unsafe { GetForegroundWindow() };
            recorder.return_focus = mode_return_focus(ModeEntry::Mouse, identity(foreground), handle.0 as usize);
            recorder.tray_request = None;
            no_activate(handle, false)?;
            // Mouse entry activates only after the user explicitly opened Mode.
            // Its existing DOM focus/Arrow/Enter/Escape logic then gets keys.
            let _ = unsafe { SetForegroundWindow(handle) };
            recorder.menu_open = true;
        } else if !menu_open && recorder.menu_open {
            close_menu(handle, &mut recorder, true)?;
        }
        Ok(())
    }
}

fn recorder_visibility(app: &AppHandle, state: &Arc<DesktopState>, visible: bool) -> Result<()> {
    let window = app.get_webview_window("recording").ok_or_else(|| window_error("missing recorder"))?;
    let state2 = Arc::clone(state);
    on_ui(app, state, move || {
        #[cfg(any(windows, target_os = "macos"))] { win::visible(&window, &state2, visible) }
        #[cfg(not(any(windows, target_os = "macos")))] { let _ = (window, state2, visible); Err(native_error("unsupported_platform", "MVP 需要 Windows")) }
    })
}
fn resize_recorder(app: &AppHandle, state: &Arc<DesktopState>, width: f64, height: f64, menu_open: bool) -> Result<()> {
    let window = app.get_webview_window("recording").ok_or_else(|| window_error("missing recorder"))?;
    let state2 = Arc::clone(state);
    on_ui(app, state, move || {
        #[cfg(any(windows, target_os = "macos"))] {
            if menu_open {
                let snapshot = state2.hub.snapshot("recording")?;
                if mode_menu_request(&snapshot, state2.exit.load(Ordering::Acquire)).is_err() {
                    return win::close_stale_mode(&window, &state2, Some(&snapshot));
                }
            }
            win::resize(&window, &state2, width, height, menu_open)
        }
        #[cfg(not(any(windows, target_os = "macos")))] { let _ = (window, state2, width, height, menu_open); Err(native_error("unsupported_platform", "MVP 需要 Windows")) }
    })
}
fn request_recorder_mode(app: &AppHandle, state: &Arc<DesktopState>) -> Result<()> {
    let snapshot = state.hub.snapshot("recording")?;
    let request = mode_menu_request(&snapshot, state.exit.load(Ordering::Acquire))?;
    let error_identity = request.clone();
    let window = app.get_webview_window("recording").ok_or_else(|| window_error("missing recorder"))?;
    let state2 = Arc::clone(state);
    on_ui(app, state, move || {
        #[cfg(any(windows, target_os = "macos"))] {
            let before = state2.hub.snapshot("recording")?;
            if !mode_request_is_current(&before, state2.exit.load(Ordering::Acquire), &request) {
                win::close_stale_mode(&window, &state2, Some(&before))?;
                return Ok(());
            }
            // Activate/show before publishing. The DOM receives this intent only
            // after native menu state is set, so its resize cannot recapture tray.
            win::open_mode_from_tray(&window, &state2, request.clone())?;
            let activated = state2.hub.snapshot("recording");
            if !activated.as_ref().is_ok_and(|snapshot| mode_request_is_current(snapshot, state2.exit.load(Ordering::Acquire), &request)) {
                win::close_stale_mode(&window, &state2, activated.as_ref().ok())?;
                return activated.map(|_| ());
            }
            state2.hub.publish("recorder.openMode", json!(request));
            // Both payload and envelope are checked by the frontend. Also close
            // natively if a stop/new session won the publication race; a rejected
            // frontend event may not produce any resize(false) acknowledgement.
            let after = state2.hub.snapshot("recording").ok();
            win::close_stale_mode(&window, &state2, after.as_ref())
        }
        #[cfg(not(any(windows, target_os = "macos")))] { let _ = (window, state2, request); Err(native_error("unsupported_platform", "MVP 需要 Windows")) }
    }).map_err(|error| error.for_session(&error_identity.session_id, error_identity.generation))
}
fn cancel_shortcut_capture(app: AppHandle, state: Arc<DesktopState>) {
    let epoch = state.hub.shortcut_capture_focus_changed(false);
    // Native OFF ACK may wait for the hook owner. Never block the window thread
    // and do not rely exclusively on a WebView blur/hidden IPC acknowledgement.
    // Atomic-only focus epochs make a late worker harmless after focus returns;
    // the engine checks the epoch and releases the lease under its identity lock.
    tauri::async_runtime::spawn_blocking(move || {
        if let Err(error) = state.hub.cancel_shortcut_capture_at_focus_epoch(epoch) { reveal_desktop_error(&app, &state, error); }
    });
}
fn set_autostart(app: &AppHandle, enabled: bool) -> Result<()> {
    let manager = app.autolaunch();
    if enabled { manager.enable() } else { manager.disable() }.map_err(|_| native_error("autostart_failed", "未能更改开机启动设置"))?;
    if manager.is_enabled().map_err(|_| native_error("autostart_unverified", "无法确认开机启动状态"))? != enabled {
        return Err(native_error("autostart_unverified", "系统未确认开机启动设置"));
    }
    Ok(())
}
fn install_native_actions(app: &AppHandle, state: &Arc<DesktopState>) {
    let handle = app.clone(); let weak: Weak<DesktopState> = Arc::downgrade(state);
    state.hub.set_native_actions(Arc::new(move |name, enabled| {
        let state = weak.upgrade().ok_or_else(|| native_error("app_exiting", "应用已退出"))?;
        match name {
            "showMain" => show_main(&handle),
            "recording" => {
                let result = recorder_visibility(&handle, &state, enabled);
                refresh_mode_entry(&handle, &state);
                result
            },
            "quit" => { request_exit(handle.clone(), state); Ok(()) },
            "autostart" => set_autostart(&handle, enabled),
            _ => Err(native_error("unknown_native_action", "未提供此本机操作")),
        }
    }));
    let handle = app.clone(); let weak = Arc::downgrade(state);
    state.hub.set_recorder_control(Arc::new(move |w, h, menu| {
        let state = weak.upgrade().ok_or_else(|| native_error("app_exiting", "应用已退出"))?;
        resize_recorder(&handle, &state, w, h, menu)
    }));
    let handle=app.clone();let weak:Weak<DesktopState>=Arc::downgrade(state);
    state.hub.set_stop_target_control(Arc::new(move|anchor|{
        let state=weak.upgrade().ok_or_else(||native_error("app_exiting","应用已退出"))?;
        let window=handle.get_webview_window("recording").ok_or_else(||window_error("missing recorder"))?;
        let menu_return=state.recorder.lock().map_err(|_|window_error("lock"))?.return_focus;
        let state2=Arc::clone(&state);
        on_ui(&handle,&state,move||{
            #[cfg(any(windows, target_os = "macos"))]{win::stop_target(&window,&state2,anchor,menu_return)}
            #[cfg(not(any(windows, target_os = "macos")))]{let _=(window,state2,anchor);Err(native_error("unsupported_platform","MVP 需要 Windows"))}
        })
    }));
}
fn start_hotkeys(app: AppHandle, state: Arc<DesktopState>, shortcut: String) {
    if state.hook_starting.swap(true, Ordering::AcqRel) { return; }
    tauri::async_runtime::spawn_blocking(move || {
        let weak = Arc::downgrade(&state);
        let callback_app = app.clone();
        // A callback runs on the hook thread. Only enqueue async work here: never
        // wait on HotkeyService ACK/shutdown from the very callback it must serve.
        #[cfg(target_os="macos")]
        let restarting = state.hub.hooks.lock().unwrap().is_some();
        let started = HotkeyService::start(&shortcut, Duration::from_secs(1), move |event| {
            let Some(state) = weak.upgrade() else { return; };
            if state.exit.load(Ordering::Acquire) != 0 { return; }
            let hook_intent=std::time::Instant::now();
            // Reserve in the physical hook callback, before async scheduling can
            // reorder Locked/Resumed events. This call is atomic-only, no ACK.
            let lifecycle_epoch = if matches!(&event, HotkeyEvent::LifecycleReset(_) | HotkeyEvent::Unavailable(_)) {
                Some(state.hub.reserve_lifecycle_epoch())
            } else { None };
            let handle = callback_app.clone();
            tauri::async_runtime::spawn(async move {
                let is_toggle = matches!(&event, HotkeyEvent::Toggle);
                let before = if is_toggle { state.hub.snapshot("recording").ok() } else { None };
                let outcome = match event {
                    HotkeyEvent::Toggle => state.hub.toggle_from_hotkey_at(hook_intent).await,
                    HotkeyEvent::LifecycleReset(reason) => state.hub.interrupt_session_at_epoch(&reason, lifecycle_epoch.unwrap()).await,
                    HotkeyEvent::Unavailable(_) => {
                        let result = state.hub.interrupt_session_at_epoch("HooksUnavailable", lifecycle_epoch.unwrap()).await;
                        reveal_desktop_error(&handle, &state, native_error("shortcut_unavailable", "全局快捷键不可用，请检查权限或重新启动"));
                        result
                    },
                    HotkeyEvent::Ready => { state.hub.changed(); Ok(()) },
                    HotkeyEvent::Captured(_) => Ok(()),
                };
                if let Err(error) = outcome {
                    let error = if is_toggle { hotkey_error_scope(error, before.as_ref()) } else { error };
                    reveal_desktop_error(&handle, &state, error);
                }
            });
        });
        match started {
            Ok(service) => {
                let mut slot = state.hub.hooks.lock().unwrap();
                // Always retain the actual owner, including startup which
                // completed after Quit. shutdown_ready will join this slot.
                *slot = Some(service);drop(slot);
                if state.exit.load(Ordering::Acquire)==0{state.hub.changed();}
                #[cfg(target_os="macos")]
                state.hub.publish("native.shortcutsReady",json!({}));
                #[cfg(target_os="macos")]
                if restarting && state.exit.load(Ordering::Acquire)==0 {
                    let state2=Arc::clone(&state); let handle=app.clone();
                    tauri::async_runtime::spawn(async move {
                        if let Err(error)=state2.hub.interrupt_session("HooksReinstalled").await { reveal_desktop_error(&handle,&state2,error); }
                    });
                }
            },
            Err(error) => {
                let (_error,owner)=error.into_parts();
                if let Some(owner)=owner{*state.hub.hooks.lock().unwrap()=Some(owner);}
                let error=native_error("shortcut_unavailable","全局快捷键尚未就绪，可在主面板检查设置");
                if state.exit.load(Ordering::Acquire)==0{reveal_desktop_error(&app,&state,error);}else{publish_error(&state,error);}
            },
        }
        state.hook_starting.store(false, Ordering::Release);
    });
}

fn local_navigation(url: &tauri::Url) -> bool {
    let packaged = matches!((url.scheme(), url.host_str()), ("tauri", Some("localhost")) | ("http", Some("tauri.localhost")) | ("https", Some("tauri.localhost")));
    let dev = cfg!(debug_assertions) && url.scheme() == "http" && url.host_str() == Some("127.0.0.1") && url.port() == Some(1420);
    packaged || dev
}
fn development_runtime_root() -> std::io::Result<PathBuf> {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let project = manifest.parent().and_then(|p| p.parent())
        .ok_or_else(|| std::io::Error::other("project root unavailable"))?.canonicalize()?;
    Ok(project.join("runtime"))
}
fn profile_path_key(path: &std::path::Path) -> String {
    let value = path.to_string_lossy();
    #[cfg(windows)]
    { value.trim_start_matches(r"\\?\").trim_end_matches('\\').to_ascii_lowercase() }
    #[cfg(not(windows))]
    { value.trim_end_matches('/').to_owned() }
}
fn profile_path_is_within(path: &std::path::Path, root: &std::path::Path) -> bool {
    let path = profile_path_key(path);
    let root = profile_path_key(root);
    path == root || path.starts_with(&(root + std::path::MAIN_SEPARATOR_STR))
}
fn reject_unsafe_profile_directory(path: &std::path::Path) -> std::io::Result<()> {
    if !path.is_absolute() || path.file_name().is_none()
        || path.components().any(|part| matches!(part, std::path::Component::ParentDir)) {
        return Err(std::io::Error::other("profile must use an absolute dedicated directory"));
    }
    #[cfg(windows)]
    {
        use std::path::{Component, Prefix};
        if !matches!(path.components().next(), Some(Component::Prefix(prefix))
            if matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_))) {
            return Err(std::io::Error::other("profile must use a local drive directory"));
        }
        // A dedicated child of the user's projects or AppData is permitted.
        // System trees, shared profile roots and their ancestors are not.
        for (name, reject_descendants) in [
            ("SystemRoot", true), ("WINDIR", true), ("ProgramFiles", true),
            ("ProgramFiles(x86)", true), ("ProgramW6432", true), ("ProgramData", true),
            ("USERPROFILE", false), ("LOCALAPPDATA", false), ("APPDATA", false),
        ] {
            let Some(protected) = std::env::var_os(name).map(PathBuf::from) else { continue; };
            let protected = protected.canonicalize().unwrap_or(protected);
            if profile_path_is_within(&protected, path)
                || (reject_descendants && profile_path_is_within(path, &protected)) {
                return Err(std::io::Error::other("system or shared profile directory refused"));
            }
        }
    }
    #[cfg(target_os="macos")]
    {
        let mut protected = vec![
            (PathBuf::from("/System"),true),(PathBuf::from("/Library"),true),
            (PathBuf::from("/Applications"),true),(PathBuf::from("/usr"),true),
            (PathBuf::from("/bin"),true),(PathBuf::from("/sbin"),true),
            (PathBuf::from("/Users"),false),(PathBuf::from("/private"),false),
            (PathBuf::from("/private/tmp"),false),(PathBuf::from("/private/etc"),true),
        ];
        if let Some(home)=std::env::var_os("HOME").map(PathBuf::from) {
            protected.extend([(home.clone(),false),(home.join("Library/Application Support"),false),(home.join("Library/Caches"),false)]);
        }
        for (root,descendants) in protected {
            let root=root.canonicalize().unwrap_or(root);
            if profile_path_key(path)==profile_path_key(&root) || descendants && profile_path_is_within(path,&root) {
                return Err(std::io::Error::other("profile must use a dedicated user data directory"));
            }
        }
    }
    Ok(())
}
// Resolve an existing ancestor before any directory creation. This also rejects
// a junction into a system tree. Maintenance uses the same resolver without
// creating or migrating a profile.
fn resolve_profile_directory(path: &std::path::Path) -> std::io::Result<PathBuf> {
    reject_unsafe_profile_directory(path)?;
    let mut ancestor = path.to_path_buf();
    let mut suffix = Vec::new();
    while !ancestor.exists() {
        suffix.push(ancestor.file_name().ok_or_else(|| std::io::Error::other("profile ancestor unavailable"))?.to_os_string());
        if !ancestor.pop() { return Err(std::io::Error::other("profile ancestor unavailable")); }
    }
    for entry in ancestor.ancestors() {
        let metadata = std::fs::symlink_metadata(entry)?;
        #[cfg(windows)]
        let redirected = {
            use std::os::windows::fs::MetadataExt;
            metadata.file_attributes() & 0x400 != 0
        };
        #[cfg(not(windows))]
        let redirected = metadata.file_type().is_symlink();
        if redirected { return Err(std::io::Error::other("reparse profile path refused")); }
    }
    let mut resolved = ancestor.canonicalize()?;
    for part in suffix.into_iter().rev() { resolved.push(part); }
    reject_unsafe_profile_directory(&resolved)?;
    Ok(resolved)
}
fn data_override() -> std::io::Result<Option<PathBuf>> {
    let requested = std::env::var_os("SPEAKMODE_DATA_DIR").map(PathBuf::from);
    let explicit = requested.is_some();
    let default_scope = if cfg!(debug_assertions) { Some(development_runtime_root()?) } else { None };
    let path = requested.or_else(|| default_scope.as_ref().map(|root| root.join("desktop")));
    let Some(path) = path else { return Ok(None); };
    let path = resolve_profile_directory(&path)?;
    if !explicit && default_scope.as_ref().is_some_and(|root| !profile_path_is_within(&path, root)) {
        return Err(std::io::Error::other("development runtime must stay in project runtime"));
    }
    std::fs::create_dir_all(&path)?;
    let path = resolve_profile_directory(&path)?;
    if !explicit && default_scope.as_ref().is_some_and(|root| !profile_path_is_within(&path, root)) {
        return Err(std::io::Error::other("development runtime must stay in project runtime"));
    }
    Ok(Some(path))
}
fn tray_icon() -> tauri::image::Image<'static> {
    // Embedded RGBA avoids a runtime image decoder or file-system lookup.
    let mut rgba = vec![0u8; 32 * 32 * 4];
    for y in 0..32 { for x in 0..32 {
        let offset = (y * 32 + x) * 4;
        if (3..29).contains(&x) && (3..29).contains(&y) {
            rgba[offset..offset + 4].copy_from_slice(&[66, 112, 194, 255]);
            if ((9..12).contains(&x) && (12..20).contains(&y)) || ((14..17).contains(&x) && (8..24).contains(&y)) || ((19..22).contains(&x) && (11..21).contains(&y)) {
                rgba[offset..offset + 4].copy_from_slice(&[255, 255, 255, 255]);
            }
        }
    }}
    tauri::image::Image::new_owned(rgba, 32, 32)
}

// This branch runs before Tauri plugins, profile creation, replay and WebViews.
// It cannot silently fall through to the GUI on a malformed maintenance call.
fn maintenance_entry()->Option<i32>{
    let options=match crate::maintenance::parse_arguments(std::env::args().skip(1)){
        Ok(None)=>return None,Ok(Some(options))=>options,Err(_)=>return Some(2)
    };
    let result=(||->std::io::Result<()>{
        let runtime=tokio::runtime::Builder::new_current_thread().enable_all().build()?;
        runtime.block_on(crate::maintenance::prepare_uninstall())?;
        if !options.delete_data&&!options.delete_key{return Ok(());}
        let directory=maintenance_directory()?;
        // Verify ownership even for credential metadata. Never create or migrate
        // a database during uninstall, and never retrieve the Key itself.
        let owned=crate::maintenance::validate_data_root(&directory,&directory)?;
        if let Some(root)=owned {
            if options.delete_key {
                let path=root.join("speakmode.sqlite");
                if path.exists(){
                    let connection=rusqlite::Connection::open_with_flags(path,rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(std::io::Error::other)?;
                    let encoded:String=connection.query_row("SELECT data FROM provider_config WHERE singleton=1",[],|r|r.get(0)).map_err(std::io::Error::other)?;
                    let config:crate::core::ProviderConfig=serde_json::from_str(&encoded).map_err(std::io::Error::other)?;
                    use crate::cloud::credentials::{self,CredentialStage};
                    for stage in [CredentialStage::Asr,CredentialStage::Rewrite] {
                        // A never-configured Aliyun stage has no region account.
                        // Other invalid metadata must fail cleanup visibly.
                        let unconfigured=stage.provider(&config)=="aliyun"&&config.region.is_none();
                        if !unconfigured {credentials::delete_for(&config,stage).map_err(|_|std::io::Error::other("current cloud credential cleanup incomplete"))?;}
                    }
                }
            }
            if options.delete_data{crate::maintenance::delete_data(&directory,&directory)?;}
        }
        Ok(())
    })();
    Some(if result.is_ok(){0}else{1})
}
fn maintenance_directory()->std::io::Result<PathBuf>{
    let default_scope=if cfg!(debug_assertions){Some(development_runtime_root()?)}else{None};
    let default=if let Some(scope)=&default_scope{scope.join("desktop")}else{
        #[cfg(target_os="macos")]
        let base = PathBuf::from(std::env::var_os("HOME").ok_or_else(||std::io::Error::other("home directory unavailable"))?).join("Library/Application Support");
        #[cfg(not(target_os="macos"))]
        let base = PathBuf::from(std::env::var_os("LOCALAPPDATA").ok_or_else(||std::io::Error::other("local data directory unavailable"))?);
        base.join(crate::maintenance::APP_IDENTIFIER)
    };
    let requested=std::env::var_os("SPEAKMODE_DATA_DIR").map(PathBuf::from);
    let explicit=requested.is_some();
    let root=resolve_profile_directory(&requested.unwrap_or(default))?;
    if !explicit&&default_scope.as_ref().is_some_and(|scope|!profile_path_is_within(&root,scope)){return Err(std::io::Error::other("development cleanup must stay in project runtime"));}
    Ok(root)
}

pub fn run() {
    #[cfg(all(target_os="macos",debug_assertions))]
    if std::env::args().nth(1).as_deref()==Some("--native-probe") {
        match crate::platform::macos::lab::run(std::env::args().skip(2)) {
            Ok(())=>return,
            Err(error)=>{eprintln!("native probe failed: {error}");std::process::exit(1);}
        }
    }
    if let Some(code)=maintenance_entry(){std::process::exit(code);}
    let override_dir = data_override().expect("SpeakMode data directory could not be prepared");
    // Before Tauri starts threads, pin WebView2's process override and temporary
    // files too. Debug/QA uses a dedicated profile rather than the release profile.
    #[cfg(windows)]
    if let Some(root) = &override_dir {
        let webview = root.join("webview"); let temp = root.join("tmp");
        std::fs::create_dir_all(&webview).expect("WebView directory unavailable");
        std::fs::create_dir_all(&temp).expect("temporary directory unavailable");
        unsafe { std::env::set_var("WEBVIEW2_USER_DATA_FOLDER", &webview); std::env::set_var("TEMP", &temp); std::env::set_var("TMP", &temp); }
    }
    let mut context = tauri::generate_context!();
    if let Some(path) = &override_dir {
        context.config_mut().app.app_directories_override = Some(tauri::utils::config::AppDirectoriesOverride::Root(path.clone()));
    }
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| { let _ = show_main(app); }))
        .plugin(tauri_plugin_autostart::Builder::new().app_name("SpeakMode").arg("--autostart").build())
        .invoke_handler(tauri::generate_handler![app_command, subscribe_state, control_main_window, native_permissions])
        .setup(move |app| {
            #[cfg(target_os="macos")]
            crate::platform::macos::lifecycle::prepare_notification_centers();
            let directory = match override_dir.clone() { Some(path) => path, None => app.path().app_local_data_dir()? };
            std::fs::create_dir_all(&directory)?;
            crate::maintenance::mark_profile(&directory)?;
            let hub = AppHub::open(&directory).map_err(|e| std::io::Error::other(e.message))?;
            let state = Arc::new(DesktopState { hub, exit: AtomicU8::new(0), hook_starting: AtomicBool::new(false), maintenance_starting: AtomicBool::new(false),maintenance:Mutex::new(None),ui_thread: std::thread::current().id(), recorder: Mutex::new(RecorderState::default()), mode_item: Mutex::new(None) });
            app.manage(Arc::clone(&state));
            let autostart_launch = std::env::args().any(|arg| arg == "--autostart");
            state.hub.shortcut_capture_focus_changed(!autostart_launch);
            for config in &app.config().app.windows {
                if !matches!(config.label.as_str(), "main" | "recording") || config.create { return Err("Only explicitly created main/recording windows are permitted".into()); }
                let data = directory.join("webview").join(&config.label);
                std::fs::create_dir_all(&data)?;
                let mut window = WebviewWindowBuilder::from_config(app, config)?.data_directory(data).on_navigation(local_navigation);
                if config.label == "recording" { window = window.focusable(false).focused(false).visible(false).shadow(false); }
                else {
                    window = window.visible(!autostart_launch).focused(!autostart_launch);
                    #[cfg(target_os = "macos")]
                    { window = window.decorations(true).title_bar_style(tauri::TitleBarStyle::Overlay).hidden_title(true); }
                }
                window.build()?;
            }
            install_native_actions(app.handle(), &state);
            let open = MenuItem::with_id(app, "show-main", "打开 SpeakMode", true, None::<&str>)?;
            let mode = MenuItem::with_id(app, "recording-mode", "本次 Mode…", false, None::<&str>)?;
            *state.mode_item.lock().unwrap() = Some(mode.clone());
            let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open, &mode, &quit])?;
            TrayIconBuilder::with_id("speakmode").icon(tray_icon()).tooltip("SpeakMode")
                .menu(&menu).show_menu_on_left_click(cfg!(target_os="macos"))
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show-main" => { let _ = show_main(app); },
                    "recording-mode" => { if let Some(state) = current_state(app) {
                        if let Err(error) = request_recorder_mode(app, &state) { reveal_desktop_error(app, &state, error); }
                    } },
                    "quit" => { if let Some(state) = current_state(app) { request_exit(app.clone(), state); } },
                    _ => {},
                })
                .on_tray_icon_event(|_tray, _event| {
                    #[cfg(not(target_os="macos"))]
                    if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = _event { let _ = show_main(_tray.app_handle()); }
                }).build(app)?;
            let snapshot = state.hub.snapshot("main").map_err(|e| std::io::Error::other(e.message))?;
            let shortcut = snapshot["preferences"]["shortcut"].as_str().unwrap_or("alt_right").to_owned();
            // Merely launching does not change autostart registration. A mismatch
            // is reported; an explicit preferences change is the only writer.
            let requested = snapshot["preferences"]["launchAtLogin"].as_bool().unwrap_or(false);
            if app.autolaunch().is_enabled().ok() != Some(requested) {
                publish_error(&state, native_error("autostart_mismatch", "开机启动的系统状态与偏好不一致，请在设置中重新确认"));
            }
            #[cfg(any(windows, target_os="macos"))]
            start_maintenance(app.handle().clone(),Arc::clone(&state));
            start_hotkeys(app.handle().clone(), state, shortcut);
            Ok(())
        })
        .on_window_event(|window, event| {
            let Some(state) = current_state(window.app_handle()) else { return; };
            match event {
                WindowEvent::CloseRequested { api, .. } if state.exit.load(Ordering::Acquire) != 2 => {
                    api.prevent_close();
                    if window.label() == "main" {
                        cancel_shortcut_capture(window.app_handle().clone(), Arc::clone(&state));
                        state.hub.onboarding_main_hidden();
                        let _ = window.hide();
                    }
                    else if window.label() == "recording" {
                        let state = Arc::clone(&state);
                        let app = window.app_handle().clone();
                        tauri::async_runtime::spawn(async move {
                            let result = async {
                                let snapshot = state.hub.snapshot("recording")?;
                                let session = &snapshot["session"];
                                if let (Some(id), Some(generation)) = (session["sessionId"].as_str().or(session["id"].as_str()), session["generation"].as_u64()) {
                                    state.hub.command("recording", CommandRequest { name: "session.cancel".into(), payload: json!({
                                        "sessionId": id, "expectedGeneration": generation, "requestId": uuid::Uuid::new_v4().to_string()
                                    }) }).await.map_err(|error| error.for_session(id, generation))?;
                                    return recorder_visibility(&app, &state, false).map_err(|error| error.for_session(id, generation));
                                }
                                recorder_visibility(&app, &state, false)
                            }.await;
                            if let Err(error) = result { publish_error(&state, error); }
                        });
                    }
                },
                WindowEvent::Focused(false) if window.label() == "main" => {
                    state.hub.onboarding_focus_changed(false);
                    cancel_shortcut_capture(window.app_handle().clone(), Arc::clone(&state));
                },
                WindowEvent::Resized(_) if window.label() == "main" && window.is_minimized().unwrap_or(false) => {
                    state.hub.onboarding_main_hidden();
                },
                WindowEvent::Focused(true) if window.label() == "main" => {
                    state.hub.shortcut_capture_focus_changed(true);
                    state.hub.onboarding_focus_changed(true);
                },
                WindowEvent::Focused(false) if window.label() == "recording" => {
                    let app = window.app_handle().clone();
                    let state = Arc::clone(&state);
                    // Schedule after the current Win32 focus call unwinds. Never
                    // synchronously re-lock recorder state inside SetForegroundWindow.
                    tauri::async_runtime::spawn(async move {
                        let Some(window) = app.get_webview_window("recording") else { return; };
                        let state2 = Arc::clone(&state);
                        let result = on_ui(&app, &state, move || {
                            #[cfg(any(windows, target_os = "macos"))] { win::blurred(&window, &state2) }
                            #[cfg(not(any(windows, target_os = "macos")))] { let _ = (window, state2); Ok(()) }
                        });
                        if let Err(error) = result { publish_error(&state, error); }
                    });
                },
                WindowEvent::Destroyed => state.hub.unsubscribe(window.label()),
                _ => {},
            }
        });
    let app = builder.build(context).expect("SpeakMode desktop failed to initialize");
    app.run(|app, event| {
        #[cfg(target_os = "macos")]
        if let RunEvent::Reopen { .. } = event { let _ = show_main(app); }
        if let RunEvent::ExitRequested { api, .. } = event {
            if let Some(state) = current_state(app) {
                if state.exit.load(Ordering::Acquire) != 2 { api.prevent_exit(); request_exit(app.clone(), state); }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn profile_override_rejects_roots_relative_paths_and_traversal_without_creating_them() {
        assert!(resolve_profile_directory(std::path::Path::new("relative/runtime")).is_err());
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR")).canonicalize().unwrap();
        let root = manifest.ancestors().last().unwrap();
        assert!(resolve_profile_directory(root).is_err());
        let lexical_manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        assert!(resolve_profile_directory(&lexical_manifest.join("runtime/../escape")).is_err());
        let missing = manifest.join(".test-data/profile-resolver").join(uuid::Uuid::new_v4().to_string());
        let resolved = resolve_profile_directory(&missing).unwrap();
        assert_eq!(resolved, missing);
        assert!(!missing.exists(), "resolution and maintenance must not create a profile");
    }
    #[cfg(windows)]
    #[test] fn profile_override_rejects_system_trees_and_shared_user_roots() {
        for name in ["SystemRoot", "WINDIR", "ProgramFiles", "ProgramFiles(x86)", "ProgramW6432", "ProgramData"] {
            if let Some(root) = std::env::var_os(name).map(PathBuf::from) {
                assert!(resolve_profile_directory(&root).is_err(), "{name}");
                assert!(resolve_profile_directory(&root.join("SpeakMode-test")).is_err(), "{name} child");
            }
        }
        for name in ["USERPROFILE", "LOCALAPPDATA", "APPDATA"] {
            if let Some(root) = std::env::var_os(name).map(PathBuf::from) {
                assert!(resolve_profile_directory(&root).is_err(), "{name}");
            }
        }
    }
    #[cfg(target_os="macos")]
    #[test] fn mac_profile_rejects_system_trees_and_shared_user_roots() {
        for path in ["/System/Library/speakmode", "/Applications/SpeakMode.app/data", "/private/tmp", "/Users"] {
            assert!(reject_unsafe_profile_directory(std::path::Path::new(path)).is_err());
        }
        if let Some(home)=std::env::var_os("HOME").map(PathBuf::from) {
            assert!(reject_unsafe_profile_directory(&home).is_err());
            assert!(reject_unsafe_profile_directory(&home.join("Library/Application Support")).is_err());
            assert!(reject_unsafe_profile_directory(&home.join("Library/Application Support/dev.speakmode.desktop")).is_ok());
        }
    }
    #[test] fn work_area_excludes_taskbar() {
        assert_eq!(recorder_rect(440.0, 86.0, 1.0, Rect{x:0,y:0,width:1920,height:1040}).unwrap(), Rect{x:740,y:942,width:440,height:86});
    }
    #[test] fn compact_recorder_and_details_share_a_dpi_safe_bottom_anchor() {
        for scale in [1.0,1.25,1.5,2.0] {
            let area=Rect{x:-1920,y:-100,width:1920,height:1080};
            let small=recorder_rect(336.0,66.0,scale,area).unwrap();
            let details=recorder_rect(344.0,360.0,scale,area).unwrap();
            assert_eq!(small.y+small.height as i32,details.y+details.height as i32);
            assert!(small.x>=area.x&&details.x>=area.x);
        }
        assert!(recorder_rect(239.0,64.0,1.0,Rect{x:0,y:0,width:1920,height:1080}).is_err());
    }
    #[test] fn mixed_dpi_negative_origin_is_physical() {
        assert_eq!(recorder_rect(440.0, 420.0, 1.5, Rect{x:-1920,y:-100,width:1920,height:1080}).unwrap(), Rect{x:-1290,y:332,width:660,height:630});
    }
    #[test] fn tiny_work_area_clamps_both_dimensions() {
        assert_eq!(recorder_rect(640.0, 420.0, 2.0, Rect{x:10,y:20,width:300,height:100}).unwrap(), Rect{x:10,y:20,width:300,height:100});
    }
    #[test] fn malformed_geometry_cannot_reach_native_api() {
        let area = Rect{x:0,y:0,width:1920,height:1080};
        assert!(recorder_rect(f64::NAN, 80.0, 1.0, area).is_err());
        assert!(recorder_rect(440.0, 421.0, 1.0, area).is_err());
        assert!(recorder_rect(440.0, 100.0, 0.0, area).is_err());
    }
    #[test] fn resized_menu_keeps_bottom_anchor() {
        let area = Rect{x:0,y:0,width:1920,height:1040};
        let small = recorder_rect(440.0, 86.0, 1.0, area).unwrap();
        let menu = recorder_rect(440.0, 418.0, 1.0, area).unwrap();
        assert_eq!(small.y + small.height as i32, menu.y + menu.height as i32);
    }
    fn live(state: &str, id: &str, generation: u64) -> Value {
        json!({"session": {"state":state,"sessionId":id,"generation":generation}})
    }
    #[test] fn tray_mode_only_accepts_current_recording_phases() {
        for phase in ["starting", "recording"] {
            let request = mode_menu_request(&live(phase, "first", 7), 0).unwrap();
            assert_eq!(json!(request), json!({"sessionId":"first","generation":7}));
            assert!(mode_entry_enabled(&live(phase, "first", 7), 0, true));
            assert!(!mode_entry_enabled(&live(phase, "first", 7), 0, false));
        }
        for phase in ["finalizingASR", "rewriting", "ready", "failed", "cancelled", "interrupted", "idle"] {
            assert!(mode_menu_request(&live(phase, "first", 7), 0).is_err(), "{phase}");
        }
        for exit in [1, 2, 3] { assert!(mode_menu_request(&live("recording", "first", 7), exit).is_err()); }
        assert!(mode_menu_request(&Value::Null, 0).is_err());
        assert!(mode_menu_request(&json!({"session":{"state":"recording","sessionId":"first"}}), 0).is_err());
        assert!(mode_menu_request(&live("recording", "", 7), 0).is_err());
    }
    #[test] fn tray_mode_intent_rejects_phase_and_identity_races() {
        let request = mode_menu_request(&live("recording", "first", 7), 0).unwrap();
        assert!(mode_request_is_current(&live("recording", "first", 7), 0, &request));
        assert!(!mode_request_is_current(&live("starting", "second", 7), 0, &request));
        assert!(!mode_request_is_current(&live("recording", "first", 8), 0, &request));
        assert!(!mode_request_is_current(&live("rewriting", "first", 7), 0, &request));
        assert!(!mode_request_is_current(&live("recording", "first", 7), 1, &request));
    }
    #[test] fn tray_does_not_guess_the_initial_or_current_host() {
        let work = ForegroundIdentity { hwnd: 42, pid: 9, created: 12 };
        assert_eq!(mode_return_focus(ModeEntry::Tray, Some(work), 7), None);
        assert_eq!(mode_return_focus(ModeEntry::Tray, None, 7), None);
        assert_eq!(mode_return_focus(ModeEntry::Mouse, Some(work), 7), Some(work));
        assert_eq!(mode_return_focus(ModeEntry::Mouse, Some(work), 42), None);
    }
    #[test] fn hidden_preflight_failure_reveals_only_without_new_capture() {
        let error = native_error("provider_unconfigured", "请检查模型设置");
        assert!(reveal_error_without_recorder(&error, false, 0, false));
        assert!(!reveal_error_without_recorder(&error, true, 0, false));
        assert!(!reveal_error_without_recorder(&error, false, 1, false));
        assert!(!reveal_error_without_recorder(&error, false, 0, true));
        for code in ["cancelled", "stale_session"] {
            assert!(!reveal_error_without_recorder(&native_error(code, ""), false, 0, false));
        }
    }
    #[test] fn hotkey_error_identity_is_bound_at_operation_entry() {
        let recording = live("recording", "first", 7);
        let stop = hotkey_error_scope(native_error("native_unavailable", "停止未完成"), Some(&recording));
        assert_eq!(stop.scope.as_deref(), Some("session"));
        assert!(error_matches_current_session(&stop, &recording));
        assert!(!error_matches_current_session(&stop, &live("recording", "second", 8)));
        let ready = live("ready", "old", 3);
        let preflight = hotkey_error_scope(native_error("provider_unconfigured", "请检查配置"), Some(&ready));
        assert_eq!(preflight.scope.as_deref(), Some("application"));
        assert!(preflight.session_id.is_none());
        assert!(error_matches_current_session(&preflight, &recording));
        let scoped = native_error("native_unavailable", "原始错误").for_session("original", 1);
        let kept = hotkey_error_scope(scoped, Some(&recording));
        assert_eq!(kept.session_id.as_deref(), Some("original"));
        assert_eq!(kept.generation, Some(1));
    }
    #[test] fn busy_and_system_errors_keep_distinct_scopes() {
        let busy = hotkey_error_scope(native_error("session_busy", "仍在处理"), Some(&live("rewriting", "first", 7)));
        assert_eq!(busy.scope.as_deref(), Some("session"));
        for code in ["shortcut_unavailable", "autostart_mismatch", "shutdown_busy"] {
            let global = json!(native_error(code, "系统错误"));
            assert_eq!(global["scope"], "application");
            assert!(global.get("sessionId").is_none());
            assert!(global.get("generation").is_none());
        }
    }
}

