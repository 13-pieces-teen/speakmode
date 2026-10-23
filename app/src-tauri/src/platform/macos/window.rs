//! All AppKit window operations run through desktop::on_ui.
use super::*;
use crate::platform::{delivery::{capture_foreground_anchor, ForegroundAnchor}, macos};
use objc2::{msg_send, runtime::AnyObject};
fn object(window: &WebviewWindow) -> Result<*mut AnyObject> { window.ns_window().map(|value| value.cast()).map_err(window_error) }
fn identity() -> Option<ForegroundIdentity> {
    let pid = macos::frontmost_pid()?;
    Some(ForegroundIdentity { hwnd: pid as usize, pid, created: macos::process_lifetime(pid)? })
}
fn place(window: &WebviewWindow, width: f64, height: f64, prefer_pointer: bool) -> Result<()> {
    let pointer = window.cursor_position().ok();
    let monitor = if prefer_pointer { pointer.and_then(|point| window.monitor_from_point(point.x, point.y).ok().flatten()) } else { None }
        .or(window.current_monitor().map_err(window_error)?).or(window.primary_monitor().map_err(window_error)?)
        .ok_or_else(|| native_error("monitor_unavailable", "无法读取显示器工作区"))?;
    let area = monitor.work_area();
    let rect = recorder_rect(width, height, monitor.scale_factor(), Rect { x: area.position.x, y: area.position.y, width: area.size.width, height: area.size.height })?;
    window.set_size(tauri::PhysicalSize::new(rect.width, rect.height)).map_err(window_error)?;
    window.set_position(tauri::PhysicalPosition::new(rect.x, rect.y)).map_err(window_error)
}
fn no_activate(window: &WebviewWindow, enabled: bool) -> Result<()> {
    window.set_focusable(!enabled).map_err(window_error)?;
    unsafe {
        let object = object(window)?;
        let _: () = msg_send![object, setLevel: 3isize];
        let _: () = msg_send![object, setCollectionBehavior: (1usize | (1 << 6) | (1 << 8))];
        let _: () = msg_send![object, setHidesOnDeactivate: false];
    }
    Ok(())
}
fn close_menu(window: &WebviewWindow, recorder: &mut RecorderState, restore: bool) -> Result<()> {
    if recorder.menu_open && restore && window.is_focused().unwrap_or(false) {
        if let Some(saved) = recorder.return_focus.take() { let _ = macos::activate(saved.pid, saved.created); }
    }
    recorder.menu_open = false; recorder.return_focus = None; recorder.tray_request = None;
    no_activate(window, true)
}
pub(super) fn visible(window: &WebviewWindow, state: &DesktopState, show: bool) -> Result<()> {
    let mut recorder = state.recorder.lock().map_err(|_| window_error("lock"))?;
    close_menu(window, &mut recorder, false)?;
    if show {
        place(window, 336.0, 66.0, true)?;
        unsafe { let _: () = msg_send![object(window)?, orderFrontRegardless]; }
    } else { window.hide().map_err(window_error)?; }
    recorder.visible = show;
    Ok(())
}
pub(super) fn blurred(window: &WebviewWindow, state: &DesktopState) -> Result<()> {
    if window.is_focused().unwrap_or(false) { return Ok(()); }
    let mut recorder = state.recorder.lock().map_err(|_| window_error("lock"))?;
    close_menu(window, &mut recorder, false)
}
pub(super) fn close_stale_mode(window: &WebviewWindow, state: &DesktopState, snapshot: Option<&Value>) -> Result<()> {
    let mut recorder = state.recorder.lock().map_err(|_| window_error("lock"))?;
    let current = snapshot.is_some_and(|snapshot| mode_menu_request(snapshot, state.exit.load(Ordering::Acquire)).is_ok() &&
        recorder.tray_request.as_ref().is_none_or(|request| mode_request_is_current(snapshot, state.exit.load(Ordering::Acquire), request)));
    if recorder.menu_open && !current {
        close_menu(window, &mut recorder, true)?;
        if recorder.visible { place(window, 336.0, 66.0, false)?; }
    }
    Ok(())
}
pub(super) fn resize(window: &WebviewWindow, state: &DesktopState, width: f64, height: f64, menu_open: bool) -> Result<()> {
    let mut recorder = state.recorder.lock().map_err(|_| window_error("lock"))?;
    place(window, width, height, false)?;
    if menu_open && !recorder.menu_open && recorder.visible {
        recorder.return_focus = mode_return_focus(ModeEntry::Mouse, identity(), std::process::id() as usize);
        recorder.tray_request = None;
        no_activate(window, false)?;
        window.set_focus().map_err(window_error)?;
        if !window.is_focused().map_err(window_error)? {
            close_menu(window, &mut recorder, false)?;
            return Err(native_error("mode_focus_unavailable", "未能进入场合菜单，请在浮层中重试"));
        }
        recorder.menu_open = true;
    } else if !menu_open && recorder.menu_open { close_menu(window, &mut recorder, true)?; }
    Ok(())
}
pub(super) fn open_mode_from_tray(window: &WebviewWindow, state: &DesktopState, request: RecorderModeRequest) -> Result<()> {
    let mut recorder = state.recorder.lock().map_err(|_| window_error("lock"))?;
    if !recorder.visible { return Err(native_error("mode_not_ready", "录音浮层尚未就绪")); }
    recorder.return_focus = mode_return_focus(ModeEntry::Tray, None, std::process::id() as usize);
    no_activate(window, false)?;
    window.set_focus().map_err(window_error)?;
    if !window.is_focused().map_err(window_error)? {
        close_menu(window, &mut recorder, false)?;
        return Err(native_error("mode_focus_unavailable", "未能进入场合菜单，请在浮层中重试"));
    }
    recorder.menu_open = true; recorder.tray_request = Some(request);
    Ok(())
}
pub(super) fn stop_target(window: &WebviewWindow, state: &DesktopState, expected: ForegroundAnchor, menu_return: Option<ForegroundIdentity>) -> Result<ForegroundAnchor> {
    let current = capture_foreground_anchor().map_err(ApiError::from)?;
    let mut recorder = state.recorder.lock().map_err(|_| window_error("lock"))?;
    let matches_saved = |saved: ForegroundIdentity| (current.process_id, current.process_created_at) == (saved.pid, saved.created) && macos::process_lifetime(saved.pid) == Some(saved.created);
    if current != expected {
        if expected.process_id == std::process::id() && menu_return.is_some_and(matches_saved) {
            close_menu(window, &mut recorder, false)?; return Ok(current);
        }
        return Err(native_error("stop_target_changed", "停止后输入位置已改变，成稿将保留供复制"));
    }
    if expected.process_id != std::process::id() { close_menu(window, &mut recorder, false)?; return Ok(expected); }
    let saved = menu_return.filter(|saved| recorder.menu_open && recorder.return_focus == Some(*saved))
        .ok_or_else(|| native_error("stop_target_unavailable", "请在输入位置结束口述，成稿将保留供复制"))?;
    close_menu(window, &mut recorder, true)?;
    let restored = capture_foreground_anchor().map_err(ApiError::from)?;
    if (restored.process_id, restored.process_created_at) != (saved.pid, saved.created) { return Err(native_error("stop_target_changed", "未能返回输入位置，成稿将保留供复制")); }
    Ok(restored)
}
