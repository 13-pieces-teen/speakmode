use super::*;
use crate::platform::macos::{self, ffi::*};
use objc2::{class, msg_send, runtime::AnyObject};
use objc2_foundation::NSString;
use std::sync::{Mutex, OnceLock, mpsc::{SyncSender, sync_channel}};

const MAC_TARGET: i32 = 0x534d_4d41;
type Work = Box<dyn FnOnce() + Send>;
fn worker(slot: &'static OnceLock<SyncSender<Work>>, name: &str) -> &'static SyncSender<Work> {
    slot.get_or_init(|| {
        let (tx, rx) = sync_channel::<Work>(1);
        std::thread::Builder::new().name(name.into()).spawn(move || {
            for job in rx { objc2::rc::autoreleasepool(|_| job()); }
        }).expect("AX worker could not start");
        tx
    })
}
fn bounded<T: Send + 'static>(context: bool, work: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T> {
    static TARGET: OnceLock<SyncSender<Work>> = OnceLock::new();
    static CONTEXT: OnceLock<SyncSender<Work>> = OnceLock::new();
    let (tx, rx) = sync_channel(1);
    worker(if context { &CONTEXT } else { &TARGET }, if context { "speakmode-ax-context" } else { "speakmode-ax-target" })
        .try_send(Box::new(move || { let _ = tx.send(work()); }))
        .map_err(|_| target_failure(TargetCaptureFailure::TargetInspectionTimeout))?;
    rx.recv_timeout(Duration::from_millis(250)).map_err(|_| target_failure(TargetCaptureFailure::TargetInspectionTimeout))?
}

// Retained CFEqual identities, never raw pointers or hashes in the target DTO.
// Bounded eviction fails old targets closed; captures of the same control reuse
// the token so a changed control cannot be accepted just because its PID matches.
#[derive(Default)]
struct Identities { next: i32, values: Vec<(i32, Owned)> }
fn token(value: Owned) -> Result<i32> {
    static IDS: OnceLock<Mutex<Identities>> = OnceLock::new();
    let mut ids = IDS.get_or_init(Default::default).lock().map_err(|_| target_failure(TargetCaptureFailure::TargetCapabilityUnsupported))?;
    if let Some((token, _)) = ids.values.iter().find(|(_, old)| unsafe { CFEqual(old.0, value.0) }) { return Ok(*token); }
    ids.next = ids.next.checked_add(1).ok_or_else(|| target_failure(TargetCaptureFailure::TargetCapabilityUnsupported))?;
    let token = ids.next;
    if ids.values.len() >= 64 { ids.values.remove(0); }
    ids.values.push((token, value));
    Ok(token)
}
fn unsupported() -> PlatformError { target_failure(TargetCaptureFailure::TargetCapabilityUnsupported) }
fn focused(pid: u32) -> Result<Owned> {
    let app = Owned::new(unsafe { AXUIElementCreateApplication(pid as i32) }).ok_or_else(unsupported)?;
    unsafe { AXUIElementSetMessagingTimeout(app.0, 0.08); }
    let element = attribute(app.0, "AXFocusedUIElement").ok_or_else(unsupported)?;
    unsafe { AXUIElementSetMessagingTimeout(element.0, 0.08); }
    let mut actual = 0;
    if unsafe { AXUIElementGetPid(element.0, &mut actual) } != 0 || actual != pid as i32 { return Err(unsupported()); }
    Ok(element)
}
fn selection(element: Ref) -> Result<(usize, usize)> {
    let value = attribute(element, "AXSelectedTextRange").ok_or_else(unsupported)?;
    let mut range = Range { location: 0, length: 0 };
    if unsafe { CFGetTypeID(value.0) != AXValueGetTypeID() || AXValueGetType(value.0) != 4 } || !unsafe { AXValueGetValue(value.0, 4, (&mut range as *mut Range).cast()) } { return Err(unsupported()); }
    range_bounds(range)
}
fn range_bounds(range: Range) -> Result<(usize, usize)> {
    let start = usize::try_from(range.location).map_err(|_| unsupported())?;
    let length = usize::try_from(range.length).map_err(|_| unsupported())?;
    let end = start.checked_add(length).filter(|end| *end <= isize::MAX as usize).ok_or_else(unsupported)?;
    Ok((start, end))
}
fn editable(element: Ref) -> Result<()> {
    let role = attribute(element, "AXRole").and_then(|v| string(v.0, 64)).ok_or_else(unsupported)?;
    let subrole = attribute(element, "AXSubrole").and_then(|v| string(v.0, 64));
    let enabled = match attribute_result(element, "AXEnabled") {
        Ok(value) => enabled_admission(0, boolean(value.0))?,
        Err(error) => enabled_admission(error, None)?,
    };
    if !matches!(role.as_str(), "AXTextField" | "AXTextArea" | "AXComboBox") ||
        subrole.as_deref() == Some("AXSecureTextField") || !enabled ||
        !(settable(element, "AXValue") || settable(element, "AXSelectedText")) {
        return Err(target_failure(TargetCaptureFailure::TargetNotEditable));
    }
    Ok(())
}
fn enabled_admission(error: i32, value: Option<bool>) -> Result<bool> {
    match (error, value) {
        (0, Some(enabled)) => Ok(enabled),
        // AppKit NSTextView omits AXEnabled. Only this explicit unsupported
        // attribute response is optional; editable() still requires a known
        // text role and positive writable-capability checks. Transport errors
        // and malformed values never count as an enabled control.
        (-25205, None) => Ok(true),
        _ => Err(unsupported()),
    }
}
fn application_anchor() -> Result<ForegroundAnchor> {
    let process_id = macos::frontmost_pid().ok_or_else(|| target_failure(TargetCaptureFailure::StopAnchorUnavailable))?;
    let process_created_at = macos::process_lifetime(process_id).ok_or_else(|| target_failure(TargetCaptureFailure::StopAnchorUnavailable))?;
    Ok(ForegroundAnchor { hwnd: process_id as usize, process_id, process_created_at, focused_hwnd: 0, selection: None })
}
pub fn capture_anchor() -> Result<ForegroundAnchor> {
    let anchor = application_anchor()?;
    // The recorder's own Mode surface is resolved explicitly on the UI thread.
    if anchor.process_id == std::process::id() { return Ok(anchor); }
    // Freeze control and caret at the stop boundary, before queued inspection
    // could accidentally adopt another field in the same application.
    bounded(false, move || {
        if !unsafe { AXIsProcessTrusted() } { return Err(unsupported()); }
        let element = focused(anchor.process_id)?;
        let selection = Some(selection(element.0)?);
        let focused_hwnd = token(element)? as usize;
        compare_capture_identity(&anchor, &application_anchor()?)?;
        Ok(ForegroundAnchor { focused_hwnd, selection, ..anchor })
    })
}
fn compare_application(anchor: &ForegroundAnchor) -> Result<()> {
    let current = application_anchor()?;
    if anchor.process_id != current.process_id || anchor.process_created_at != current.process_created_at || anchor.hwnd != current.hwnd {
        return Err(target_failure(TargetCaptureFailure::StopFocusChanged));
    }
    Ok(())
}
pub fn capture_bounded() -> Result<TargetSnapshot> { capture_bounded_for_anchor(capture_anchor()?) }
pub fn capture_bounded_for_anchor(anchor: ForegroundAnchor) -> Result<TargetSnapshot> {
    bounded(false, move || inspect(anchor))
}
// macOS delivery requires the frozen AX control and selection. The shared
// engine can supply optional Windows activity metadata through this interface.
pub fn capture_bounded_with_compatibility(anchor: ForegroundAnchor, _: Option<InputActivity>) -> Result<TargetSnapshot> {
    capture_bounded_for_anchor(anchor)
}
pub fn capture_activity() -> Result<InputActivity> { Err(unsupported()) }
pub fn inspect_wechat_window(_: ForegroundAnchor, _: InputActivity) -> Result<TargetSnapshot> { Err(unsupported()) }
pub fn recapture_keyboard_target(_: ForegroundAnchor, _: InputActivity) -> Result<TargetSnapshot> { Err(unsupported()) }
fn inspect(anchor: ForegroundAnchor) -> Result<TargetSnapshot> {
    require_external_target_process(anchor.process_id, std::process::id())?;
    if !unsafe { AXIsProcessTrusted() } || unsafe { IsSecureEventInputEnabled() } { return Err(target_failure(TargetCaptureFailure::TargetNotEditable)); }
    compare_application(&anchor)?;
    let element = focused(anchor.process_id)?;
    editable(element.0)?;
    let selection = Some(selection(element.0)?);
    let window = attribute(element.0, "AXWindow").or_else(|| attribute(element.0, "AXTopLevelUIElement")).ok_or_else(unsupported)?;
    let element_token = token(element)?;
    let runtime_id = vec![MAC_TARGET, token(window)?, element_token];
    compare_application(&anchor)?;
    let target = TargetSnapshot { hwnd: anchor.hwnd, process_id: anchor.process_id, process_created_at: anchor.process_created_at, focused_hwnd: element_token as usize, runtime_id, selection, compatibility: None };
    compare_anchor(&anchor, &target)?;
    Ok(target)
}
pub fn capture_input_context(target: TargetSnapshot) -> Result<crate::core::writing_context::InputContext> {
    bounded(true, move || {
        let anchor = validation_anchor(&target);
        let current = inspect(anchor)?;
        compare_targets(&target, &current)?;
        let element = focused(target.process_id)?;
        let (start, end) = selection(element.0)?;
        if Some((start, end)) != target.selection { return Err(target_failure(TargetCaptureFailure::StopFocusChanged)); }
        // Parameterized AXStringForRange bounds data at the provider, rather than
        // obtaining the whole document. Unsupported controls yield no host hint.
        let begin = start.saturating_sub(1000);
        let mut range = Range { location: begin as isize, length: end.saturating_sub(begin).min(1500) as isize };
        if end - start > 500 { return Err(unsupported()); }
        let before_selected = read_range(element.0, &mut range)?;
        let length = attribute(element.0, "AXNumberOfCharacters").ok_or_else(unsupported)?;
        let mut total = 0i64;
        if !unsafe { CFGetTypeID(length.0) == CFNumberGetTypeID() && CFNumberGetValue(length.0, 4, (&mut total as *mut i64).cast()) } { return Err(unsupported()); }
        let total = usize::try_from(total).map_err(|_| unsupported())?;
        if end > total { return Err(unsupported()); }
        let mut after_range = Range { location: end as isize, length: (total-end).min(500) as isize };
        let after = if after_range.length == 0 { String::new() } else { read_range(element.0, &mut after_range)? };
        let wide: Vec<u16> = before_selected.encode_utf16().collect();
        let local_start = start - begin;
        let local_end = end - begin;
        let mut context = split_native_context(&wide, (local_start, local_end))?;
        context.after_text = first_chars(&after, 500);
        compare_targets(&target, &inspect(anchor)?)?;
        Ok(context)
    })
}
fn read_range(element: Ref, range: &mut Range) -> Result<String> {
    let parameter = Owned::new(unsafe { AXValueCreate(4, (range as *const Range).cast()) }).ok_or_else(unsupported)?;
    let name = NSString::from_str("AXStringForRange");
    let mut value = std::ptr::null();
    if unsafe { AXUIElementCopyParameterizedAttributeValue(element, (&*name as *const NSString).cast(), parameter.0, &mut value) } != 0 { return Err(unsupported()); }
    let value = Owned::new(value).ok_or_else(unsupported)?;
    string(value.0, 2000).ok_or_else(unsupported)
}
pub fn modifiers_released() -> bool {
    // Caps Lock is a toggle, not a held shortcut modifier.
    let flags = unsafe { CGEventSourceFlagsState(1) };
    flags & ((1 << 17) | (1 << 18) | (1 << 19) | (1 << 20) | (1 << 23)) == 0 && !unsafe { IsSecureEventInputEnabled() }
}
pub fn paste_chord() -> u32 {
    if !unsafe { AXIsProcessTrusted() && CGPreflightPostEventAccess() } || !modifiers_released() { return 0; }
    let Some(source) = Owned::new(unsafe { CGEventSourceCreate(-1) }) else { return 0; };
    let mut events = Vec::with_capacity(4);
    for (key, down, flags) in [(0x37, true, 1u64 << 20), (0x09, true, 1 << 20), (0x09, false, 1 << 20), (0x37, false, 0)] {
        let Some(event) = Owned::new(unsafe { CGEventCreateKeyboardEvent(source.0, key, down) }) else { return 0; };
        unsafe { CGEventSetFlags(event.0, flags); CGEventSetIntegerValueField(event.0, 42, 0x534d_5041); }
        events.push(event);
    }
    for event in events { unsafe { CGEventPost(0, event.0); } }
    4 // Dispatch only; the shared receipt explicitly does not claim insertion.
}
pub fn copy_text(text: &str) -> std::result::Result<bool, ClipboardError> {
    if text.contains('\0') || text.len() > 4 * 1024 * 1024 { return Err(ClipboardError { cleared: false, reason: "invalid or oversized clipboard text".into() }); }
    objc2::rc::autoreleasepool(|_| unsafe {
        let pasteboard: *mut AnyObject = msg_send![class!(NSPasteboard), generalPasteboard];
        if pasteboard.is_null() { return Err(ClipboardError { cleared: false, reason: "系统剪贴板不可用".into() }); }
        let _: isize = msg_send![pasteboard, clearContents];
        let value = NSString::from_str(text);
        let kind = NSString::from_str("public.utf8-plain-text");
        let saved: bool = msg_send![pasteboard, setString: &*value, forType: &*kind];
        if saved { Ok(true) } else { Err(ClipboardError { cleared: true, reason: "未能写入系统剪贴板".into() }) }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn appkit_optional_enabled_is_distinct_from_disabled_or_failed_inspection() {
        assert!(enabled_admission(-25205, None).unwrap());
        assert!(enabled_admission(0, Some(true)).unwrap());
        assert!(!enabled_admission(0, Some(false)).unwrap());
        for code in [0, -25200, -25204, -25208] { assert!(enabled_admission(code, None).is_err()); }
        assert!(enabled_admission(-25205, Some(false)).is_err());
    }
    #[test] fn ax_ranges_reject_negative_and_overflow() {
        assert!(range_bounds(Range { location: -1, length: 2 }).is_err());
        assert!(range_bounds(Range { location: 0, length: -1 }).is_err());
        assert_eq!(range_bounds(Range { location: 3, length: 5 }).unwrap(), (3, 8));
    }
    #[test] fn pid_reuse_cannot_revalidate_a_mac_target() {
        let before = ForegroundAnchor { hwnd: 10, process_id: 10, process_created_at: 100, focused_hwnd: 0, selection: None };
        assert!(compare_capture_identity(&before, &ForegroundAnchor { process_created_at: 101, ..before }).is_err());
    }
}
