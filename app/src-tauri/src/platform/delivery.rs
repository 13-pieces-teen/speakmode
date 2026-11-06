//! Fail-closed external delivery. Every native mutation is an explicit call.
//! The core must durably record an attempt before constructing its permit.
use super::{PlatformError, Result, TargetCaptureFailure};
use serde::{Serialize, Deserialize};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::time::{Duration, Instant};

const NATIVE_EDIT_RUNTIME_ID: i32 = 0x534d_4e41;
#[path="delivery_wechat.rs"]
mod wechat;
pub use wechat::InputActivity;
#[cfg(test)]
#[path="delivery_wechat_tests.rs"]
mod wechat_tests;
#[cfg(all(test, windows))]
#[allow(dead_code)]
#[path="vendor/handy-keys/src/platform/windows/activity.rs"]
mod activity_contract_tests;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TargetSnapshot {
    /// Windows HWND; on macOS an opaque application anchor (never an AX pointer).
    pub hwnd: usize,
    pub process_id: u32,
    pub focused_hwnd: usize,
    pub runtime_id: Vec<i32>,
    /// UTF-16 offsets, obtained from native Edit selection or UIA selection. No
    /// target text is stored here, logged, or forwarded to cloud requests.
    pub selection: Option<(usize, usize)>,
    pub process_created_at: u64,
    /// Automatic window-level route; no field/caret proof implied.
    #[serde(default, skip_serializing_if="Option::is_none")]
    pub compatibility: Option<InputActivity>,
}

/// A cheap synchronous stop-intent identity, before an asynchronous UIA worker
/// can observe a different foreground application. It contains no target text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ForegroundAnchor {
    pub hwnd: usize,
    pub process_id: u32,
    pub process_created_at: u64,
    pub focused_hwnd: usize,
    pub selection: Option<(usize, usize)>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DispatchOutcome { Skipped, Failed, Unconfirmed }
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DispatchReceipt {
    pub outcome: DispatchOutcome,
    pub clipboard_changed: bool,
    pub input_events: u32,
    pub reason: Option<String>,
    pub reason_code: Option<String>,
}
impl DispatchReceipt {
    fn skipped(code: &str, reason: impl Into<String>, clipboard_changed: bool) -> Self {
        Self { outcome: DispatchOutcome::Skipped, clipboard_changed, input_events: 0,
            reason: Some(reason.into()), reason_code: Some(code.into()) }
    }
}

// 0 prepared, 1 claimed but cancellable, 2 native injection boundary,
// 3 terminal/consumed, 4 cancelled. No state transitions back to 0.
pub struct DeliveryPermit {
    pub attempt_id: String,
    pub generation: u64,
    stage: AtomicU8,
}
impl DeliveryPermit {
    /// Caller attests that the unique attempt has already been write-ahead
    /// committed. This in-memory object does not replace the DB ledger.
    pub fn from_committed_attempt(attempt_id: String, generation: u64) -> Self {
        Self { attempt_id, generation, stage: AtomicU8::new(0) }
    }
    pub fn cancel_before_injection(&self) -> bool {
        loop {
            let stage = self.stage.load(Ordering::SeqCst);
            if stage == 4 { return true; }
            if stage >= 2 { return false; }
            if self.stage.compare_exchange(stage, 4, Ordering::SeqCst, Ordering::SeqCst).is_ok() { return true; }
        }
    }
    pub fn consumed(&self) -> bool { self.stage.load(Ordering::SeqCst) != 0 }
    fn claim(&self) -> bool { self.stage.compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst).is_ok() }
    fn begin_injection(&self) -> bool { self.stage.compare_exchange(1, 2, Ordering::SeqCst, Ordering::SeqCst).is_ok() }
    #[allow(deprecated)] // fetch_update also compiles on pre-1.99 stable Rust.
    fn finish(&self) { let _ = self.stage.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |s| if s == 4 { None } else { Some(3) }); }
}

pub fn capture_target() -> Result<TargetSnapshot> { native::capture_bounded() }
pub fn capture_foreground_anchor() -> Result<ForegroundAnchor> { native::capture_anchor() }
pub fn capture_target_for_anchor(anchor: &ForegroundAnchor) -> Result<TargetSnapshot> { native::capture_bounded_for_anchor(*anchor) }
pub fn capture_compatibility_activity() -> Option<InputActivity> { native::capture_activity().ok() }
pub fn capture_target_for_anchor_with_activity(anchor:&ForegroundAnchor, activity:Option<InputActivity>) -> Result<TargetSnapshot> {
    native::capture_bounded_with_compatibility(*anchor,activity)
}
/// Metadata-only inspection of the exact automatic fallback implementation.
/// Unlike precise editor capture, it never calls Value or TextPattern.GetText.
pub fn inspect_wechat_window(anchor:&ForegroundAnchor,activity:InputActivity)->Result<TargetSnapshot> {
    native::inspect_wechat_window(*anchor,activity)
}
/// Best-effort context from the already frozen editable control. This never
/// changes focus/selection, reads a clipboard, or obtains a page/window title.
pub fn capture_input_context(target: &TargetSnapshot) -> Result<crate::core::writing_context::InputContext> {
    if target.compatibility.is_some() { return Err(target_failure(TargetCaptureFailure::TargetCapabilityUnsupported)); }
    native::capture_input_context(target.clone()).map(crate::core::writing_context::InputContext::bounded)
}
pub fn validate_target(snapshot: &TargetSnapshot) -> Result<()> {
    // Validation may inspect only the frozen host. Never read a newly focused
    // application's UIA prefix and discover the identity mismatch afterwards.
    if snapshot.compatibility.is_some() && !wechat::valid_shape(snapshot) {
        return Err(target_failure(TargetCaptureFailure::TargetCapabilityUnsupported));
    }
    let current = match snapshot.compatibility {
        // Keep the route selected at stop. A provider may expose TextPattern
        // later; that must not turn the same writable field into a different
        // snapshot shape during validation.
        Some(activity)=>native::recapture_keyboard_target(validation_anchor(snapshot),activity)?,
        None=>capture_target_for_anchor(&validation_anchor(snapshot))?,
    };
    compare_targets(snapshot, &current)
}
fn validation_anchor(snapshot: &TargetSnapshot) -> ForegroundAnchor {
    ForegroundAnchor { hwnd: snapshot.hwnd, process_id: snapshot.process_id,
        process_created_at: snapshot.process_created_at, focused_hwnd: snapshot.focused_hwnd,
        // UIA selection is verified by compare_targets after inspecting this
        // same host; cheap native identity cannot observe its virtual caret.
        selection: (snapshot.runtime_id.len() == 4 && snapshot.runtime_id[0] == NATIVE_EDIT_RUNTIME_ID)
            .then_some(snapshot.selection).flatten() }
}
fn compare_capture_identity(expected: &ForegroundAnchor, current: &ForegroundAnchor) -> Result<()> {
    if expected.hwnd != current.hwnd || expected.process_id != current.process_id ||
        expected.process_created_at != current.process_created_at || expected.focused_hwnd != current.focused_hwnd {
        return Err(target_failure(TargetCaptureFailure::StopFocusChanged));
    }
    Ok(())
}
fn compare_targets(before: &TargetSnapshot, now: &TargetSnapshot) -> Result<()> {
    if before.compatibility.is_some() {
        if !wechat::valid_shape(before) || !wechat::valid_shape(now) { return Err(target_failure(TargetCaptureFailure::TargetCapabilityUnsupported)); }
    } else if before.selection.is_none() || before.runtime_id.is_empty() { return Err(target_failure(TargetCaptureFailure::TargetCapabilityUnsupported)); }
    if before.compatibility.is_some() && before.compatibility != now.compatibility {
        return Err(target_failure(TargetCaptureFailure::InputActivityChanged));
    }
    if before != now { return Err(target_failure(TargetCaptureFailure::StopFocusChanged)); }
    Ok(())
}
fn compare_anchor(anchor: &ForegroundAnchor, target: &TargetSnapshot) -> Result<()> {
    if anchor.hwnd != target.hwnd || anchor.process_id != target.process_id ||
        anchor.process_created_at != target.process_created_at || anchor.focused_hwnd != target.focused_hwnd ||
        anchor.selection.is_some_and(|selection| target.selection != Some(selection)) {
        return Err(target_failure(TargetCaptureFailure::StopFocusChanged));
    }
    Ok(())
}
fn target_failure(reason: TargetCaptureFailure) -> PlatformError { PlatformError::TargetCapture(reason) }
/// Persist only fixed reason labels. Losing a UIA capability, an interaction,
/// a timeout and a real focus change must not all be reported as focus changes.
pub(crate) fn validation_failure_reason(target:&TargetSnapshot,error:&PlatformError)->&'static str {
    match error {
        PlatformError::TargetHigherIntegrity=>"targetHigherIntegrity",
        PlatformError::TargetCapture(reason) if *reason!=TargetCaptureFailure::StopFocusChanged=>reason.delivery_reason(),
        PlatformError::Timeout(_)=>"targetInspectionTimeout",
        _=>if target.compatibility.is_some(){"compatibilityTargetChanged"}else{"targetChangedBeforeDelivery"},
    }
}
fn require_anchor_process(process_id: u32) -> Result<()> {
    if process_id == 0 { return Err(target_failure(TargetCaptureFailure::StopAnchorUnavailable)); }
    Ok(())
}
fn require_external_target_process(process_id: u32, own_process_id: u32) -> Result<()> {
    require_anchor_process(process_id)?;
    if process_id == own_process_id { return Err(target_failure(TargetCaptureFailure::TargetNotEditable)); }
    Ok(())
}

pub fn copy_text(text: &str) -> Result<()> { native::copy_text(text).map(|_| ()).map_err(|e| PlatformError::Unavailable(e.reason)) }

pub fn dispatch_once(permit: &DeliveryPermit, target: &TargetSnapshot, text: &str,
    final_gate: impl Fn() -> bool) -> DispatchReceipt {
    dispatch_with(&SystemOperations, permit, target, text, final_gate)
}
/// Test-only native mechanics; the real claim/check/CAS path stays unchanged.
/// This does not prove UI Automation, clipboard or SendInput acceptance.
#[cfg(test)]
pub(crate) fn dispatch_with_test_operations(permit:&DeliveryPermit,target:&TargetSnapshot,text:&str,
    final_gate:impl Fn()->bool,copy:impl Fn(&str)->bool,chord:impl Fn()->u32)->DispatchReceipt {
    struct Operations<C,I>{copy:C,chord:I}
    impl<C:Fn(&str)->bool,I:Fn()->u32> NativeOperations for Operations<C,I>{
        fn validate(&self,_:&TargetSnapshot)->Result<()>{Ok(())}
        fn modifiers_released(&self)->bool{true}
        fn copy(&self,text:&str)->std::result::Result<bool,ClipboardError>{Ok((self.copy)(text))}
        fn chord(&self)->u32{(self.chord)()}
    }
    dispatch_with(&Operations{copy,chord},permit,target,text,final_gate)
}
trait NativeOperations {
    fn validate(&self, snapshot: &TargetSnapshot) -> Result<()>;
    fn modifiers_released(&self) -> bool;
    fn copy(&self, text: &str) -> std::result::Result<bool, ClipboardError>;
    fn chord(&self) -> u32;
    fn pause(&self, duration: Duration) { std::thread::sleep(duration); }
}
struct SystemOperations;
impl NativeOperations for SystemOperations {
    fn validate(&self, snapshot: &TargetSnapshot) -> Result<()> { validate_target(snapshot) }
    fn modifiers_released(&self) -> bool { native::modifiers_released() }
    fn copy(&self, text: &str) -> std::result::Result<bool, ClipboardError> { native::copy_text(text) }
    fn chord(&self) -> u32 { native::paste_chord() }
}
fn dispatch_with(ops: &impl NativeOperations, permit: &DeliveryPermit, target: &TargetSnapshot,
    text: &str, final_gate: impl Fn() -> bool) -> DispatchReceipt {
    if !permit.claim() { return DispatchReceipt::skipped("attemptConsumed", "attempt already consumed/cancelled", false); }
    let result = dispatch_claimed(ops, permit, target, text, final_gate);
    permit.finish();
    result
}
fn dispatch_claimed(ops: &impl NativeOperations, permit: &DeliveryPermit, target: &TargetSnapshot, text: &str, gate: impl Fn() -> bool) -> DispatchReceipt {
    if !gate() || permit.stage.load(Ordering::SeqCst) != 1 { return DispatchReceipt::skipped("cancelledBeforeInjection", "cancelled/generation expired", false); }
    // A stop shortcut can finish while its physical Alt key is still down.
    // Wait briefly rather than immediately rejecting a otherwise valid target;
    // never release physical keys on the user's behalf or repeat a paste.
    if !wait_for_modifiers(ops, permit, &gate) {
        let code = if !gate() || permit.stage.load(Ordering::SeqCst) != 1 { "cancelledBeforeInjection" } else { "modifiersNotReleased" };
        return DispatchReceipt::skipped(code, "cancelled or held modifiers did not release within 350ms", false);
    }
    if let Err(error) = ops.validate(target) { return DispatchReceipt::skipped(validation_failure_reason(target,&error), error.to_string(), false); }
    if !gate() || permit.stage.load(Ordering::SeqCst) != 1 { return DispatchReceipt::skipped("cancelledBeforeInjection", "cancelled before clipboard", false); }
    let changed = match ops.copy(text) {
        Ok(changed) => changed,
        Err(error) => return DispatchReceipt { outcome: DispatchOutcome::Failed, clipboard_changed: error.cleared,
            input_events: 0, reason: Some(error.reason), reason_code: Some("clipboardUnavailable".into()) },
    };
    // Recheck after a contended clipboard operation; never refocus a window.
    if !wait_for_modifiers(ops, permit, &gate) {
        let code = if !gate() || permit.stage.load(Ordering::SeqCst) != 1 { "cancelledBeforeInjection" } else { "modifiersNotReleased" };
        return DispatchReceipt::skipped(code, "cancelled or held modifiers did not release after clipboard", changed);
    }
    if let Err(error) = ops.validate(target) { return DispatchReceipt::skipped(validation_failure_reason(target,&error), error.to_string(), changed); }
    if !gate() || !ops.modifiers_released() || !permit.begin_injection() {
        let code = if !gate() || permit.stage.load(Ordering::SeqCst) != 1 { "cancelledBeforeInjection" } else { "modifiersNotReleased" };
        return DispatchReceipt::skipped(code, "cancelled/target gate invalid before native injection", changed);
    }
    let count = ops.chord();
    DispatchReceipt { outcome: if count == 0 { DispatchOutcome::Failed } else { DispatchOutcome::Unconfirmed },
        clipboard_changed: changed, input_events: count,
        reason_code: Some(if count == 4 { if target.compatibility.is_some(){"keyboardFocusInputDispatchedUnconfirmed"}else{"inputDispatchedUnconfirmed"} } else if count == 0 { "nativeDispatchFailed" } else { "partialInputUnconfirmed" }.into()),
        reason: Some(if count == 4 { "input dispatched; target insertion has not been read back" } else { "input dispatch incomplete; no automatic retry" }.into()) }
}

fn wait_for_modifiers(ops: &impl NativeOperations, permit: &DeliveryPermit, gate: &impl Fn() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_millis(350);
    loop {
        if !gate() || permit.stage.load(Ordering::SeqCst) != 1 { return false; }
        if ops.modifiers_released() { return true; }
        if Instant::now() >= deadline { return false; }
        ops.pause(Duration::from_millis(10));
    }
}

fn native_edit_class(class_name: &str) -> bool {
    matches!(class_name.to_ascii_lowercase().as_str(),
        "edit" | "richedit" | "richedit20a" | "richedit20w" |
        "richedit50a" | "richedit50w" | "richedit60w" |
        "richeditd2d" | "richeditd2dpt")
}
fn writable_native_style(style: u32) -> Result<()> {
    if style & 0x0020 != 0 || style & 0x0800 != 0 || style & 0x0800_0000 != 0 {
        return Err(target_failure(TargetCaptureFailure::TargetNotEditable));
    }
    Ok(())
}

const INPUT_CONTEXT_TIMEOUT: Duration = Duration::from_millis(500);
const NATIVE_CONTEXT_UTF16_LIMIT: usize = 4_000;
fn context_timeout() -> PlatformError { target_failure(TargetCaptureFailure::TargetInspectionTimeout) }
fn check_context_deadline(deadline: Instant) -> Result<()> {
    if Instant::now() >= deadline { Err(context_timeout()) } else { Ok(()) }
}
fn first_chars(value: &str, count: usize) -> String { value.chars().take(count).collect() }
fn last_chars(value: &str, count: usize) -> String {
    value.chars().rev().take(count).collect::<Vec<_>>().into_iter().rev().collect()
}
fn range_inside_field(start_vs_field: i32, end_vs_field: i32) -> bool {
    start_vs_field >= 0 && end_vs_field <= 0
}
fn normalize_uia_context_objects(value: String) -> String {
    // UIA can represent an empty Chromium editor or an inline object with
    // OBJECT REPLACEMENT CHARACTER. It is structural metadata, not dictation
    // context. Preserve word boundaries around real inline objects and leave
    // every other character (including ordinary whitespace) unchanged.
    if !value.contains('\u{fffc}') { return value; }
    if value.chars().all(|c| c == '\u{fffc}' || c.is_whitespace()) {
        String::new()
    } else {
        value.replace('\u{fffc}', " ")
    }
}
fn split_native_context(wide: &[u16], selection: (usize, usize)) -> Result<crate::core::writing_context::InputContext> {
    let unsupported = || target_failure(TargetCaptureFailure::TargetCapabilityUnsupported);
    let (start, end) = selection;
    if wide.len() > NATIVE_CONTEXT_UTF16_LIMIT || start > end || end > wide.len() { return Err(unsupported()); }
    // Strict decoding rejects a caret in the middle of a surrogate pair rather
    // than silently inventing a replacement character or moving the selection.
    let before = String::from_utf16(&wide[..start]).map_err(|_| unsupported())?;
    let selected = String::from_utf16(&wide[start..end]).map_err(|_| unsupported())?;
    let after = String::from_utf16(&wide[end..]).map_err(|_| unsupported())?;
    Ok(crate::core::writing_context::InputContext { control_label: String::new(),
        before_text: last_chars(&before, 1_000), selected_text: first_chars(&selected, 500),
        after_text: first_chars(&after, 500) })
}

// Each worker's busy flag remains set after a caller times out until it has
// actually returned. A hung provider can disable best-effort inspection, but
// cannot create a new orphan thread on every recording/delivery attempt.
struct InspectionLease<'a>(&'a AtomicBool);
impl Drop for InspectionLease<'_> { fn drop(&mut self) { self.0.store(false, Ordering::Release); } }
fn claim_inspection(busy: &AtomicBool) -> Result<InspectionLease<'_>> {
    busy.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .map(|_| InspectionLease(busy)).map_err(|_| context_timeout())
}
#[cfg(windows)]
fn require_text_input_route(gui_flags: u32) -> Result<()> {
    use windows::Win32::UI::WindowsAndMessaging::{GUI_INMENUMODE, GUI_POPUPMENUMODE, GUI_SYSTEMMENUMODE};
    // hwndFocus and Edit selection may remain unchanged while Win32 sends the
    // keyboard to a menu. The identity anchor is still readable, but it cannot
    // authorize external input until ordinary text routing is restored.
    if gui_flags & (GUI_INMENUMODE | GUI_POPUPMENUMODE | GUI_SYSTEMMENUMODE).0 != 0 {
        return Err(target_failure(TargetCaptureFailure::TargetNotEditable));
    }
    Ok(())
}
#[cfg(windows)]
fn explicit_writable_text_attribute(value: &windows::core::VARIANT) -> bool {
    // VARIANT equality first requires the exact type. Do not use
    // VariantToBoolean: VT_EMPTY, numbers, BSTRs or reserved IUnknown values
    // could be coerced, and unknown/mixed attributes must never mean writable.
    value == &windows::core::VARIANT::from(false)
}
#[cfg(windows)]
fn native_target_error(error: windows::core::Error, fallback: TargetCaptureFailure) -> PlatformError {
    use windows::Win32::{Foundation::ERROR_TIMEOUT, UI::Accessibility::UIA_E_TIMEOUT};
    // Classify numeric HRESULTs only. Never retain the COM provider's message.
    let code = error.code();
    target_failure(if code.0 as u32 == UIA_E_TIMEOUT || code == windows::core::HRESULT::from_win32(ERROR_TIMEOUT.0) {
        TargetCaptureFailure::TargetInspectionTimeout
    } else { fallback })
}

struct ClipboardError { cleared: bool, reason: String }

#[cfg(windows)]
mod native {
    use super::*;
    use std::{mem::size_of, thread, sync::{Arc, OnceLock, mpsc::{SyncSender, sync_channel}}};
    use windows::core::{PCWSTR, Interface};
    use windows::Win32::Foundation::{HANDLE, HWND, FILETIME, CloseHandle, GlobalFree, WPARAM, LPARAM, GetLastError, SetLastError, ERROR_SUCCESS, ERROR_TIMEOUT};
    use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, CoCreateInstance, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, SAFEARRAY};
    use windows::Win32::System::DataExchange::{OpenClipboard, CloseClipboard, EmptyClipboard, SetClipboardData};
    use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
    use windows::Win32::System::Ole::{SafeArrayAccessData, SafeArrayUnaccessData, SafeArrayDestroy, SafeArrayGetLBound, SafeArrayGetUBound, SafeArrayGetDim};
    use windows::Win32::System::Threading::{OpenProcess, GetCurrentProcess, GetCurrentProcessId, GetProcessTimes, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION};
    use windows::Win32::Security::{GetTokenInformation, GetSidSubAuthority, GetSidSubAuthorityCount, TokenIntegrityLevel, TOKEN_MANDATORY_LABEL, TOKEN_QUERY};
    use windows::Win32::UI::Accessibility::{CUIAutomation8, IUIAutomation, IUIAutomation2, IUIAutomationElement, IUIAutomationTextPattern, IUIAutomationTextRange, IUIAutomationValuePattern,
        UIA_EditControlTypeId, UIA_DocumentControlTypeId, UIA_GroupControlTypeId, UIA_CustomControlTypeId, UIA_WindowControlTypeId, UIA_IsReadOnlyAttributeId,
        UIA_IsTextPatternAvailablePropertyId, UIA_IsValuePatternAvailablePropertyId,
        UIA_TextPatternId, UIA_ValuePatternId, TextPatternRangeEndpoint_Start, TextPatternRangeEndpoint_End, TextUnit_Character};
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP,
        GetKeyboardLayout, MapVirtualKeyExW, MAPVK_VK_TO_VSC_EX, KEYEVENTF_SCANCODE, KEYEVENTF_EXTENDEDKEY,
        VIRTUAL_KEY, VK_LCONTROL, VK_CONTROL, VK_SHIFT, VK_MENU, VK_LWIN, VK_RWIN, IsWindowEnabled};
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId, GetGUIThreadInfo, GUITHREADINFO,
        CreateWindowExW, DestroyWindow, WINDOW_EX_STYLE, WINDOW_STYLE,
        GetClassNameW, GetWindowLongPtrW, GWL_STYLE, GetAncestor, GA_ROOT,
        SendMessageTimeoutW, SMTO_ABORTIFHUNG, SMTO_BLOCK, SMTO_ERRORONEXIT};

    struct ComGuard;
    impl Drop for ComGuard { fn drop(&mut self) { unsafe { CoUninitialize(); } } }
    fn fail(error: windows::core::Error) -> PlatformError { native_target_error(error, TargetCaptureFailure::TargetCapabilityUnsupported) }
    fn anchor_fail(error: windows::core::Error) -> PlatformError { native_target_error(error, TargetCaptureFailure::StopAnchorUnavailable) }
    struct ArrayGuard(*mut SAFEARRAY);
    impl Drop for ArrayGuard { fn drop(&mut self) { unsafe { let _ = SafeArrayDestroy(self.0); } } }
    struct HandleGuard(HANDLE);
    impl Drop for HandleGuard { fn drop(&mut self) { unsafe { let _ = CloseHandle(self.0); } } }
    include!("delivery_wechat_native.rs");
    unsafe fn integrity_level(process: HANDLE) -> Result<u32> {
        let mut token = HANDLE::default();
        unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token).map_err(fail)?; }
        let _token = HandleGuard(token);
        let mut size = 0;
        let _ = unsafe { GetTokenInformation(token, TokenIntegrityLevel, None, 0, &mut size) };
        if size == 0 || size > 16_384 { return Err(target_failure(TargetCaptureFailure::TargetCapabilityUnsupported)); }
        let mut storage = vec![0usize; (size as usize).div_ceil(size_of::<usize>())];
        unsafe { GetTokenInformation(token, TokenIntegrityLevel, Some(storage.as_mut_ptr().cast()), size, &mut size).map_err(fail)?; }
        let label = unsafe { &*storage.as_ptr().cast::<TOKEN_MANDATORY_LABEL>() };
        let count = unsafe { *GetSidSubAuthorityCount(label.Label.Sid) };
        if count == 0 { return Err(target_failure(TargetCaptureFailure::TargetCapabilityUnsupported)); }
        Ok(unsafe { *GetSidSubAuthority(label.Label.Sid, u32::from(count - 1)) })
    }
    unsafe fn ensure_delivery_integrity(process: HANDLE) -> Result<()> {
        if unsafe { integrity_level(process)? > integrity_level(GetCurrentProcess())? } {
            return Err(PlatformError::TargetHigherIntegrity);
        }
        Ok(())
    }

    unsafe fn process_created_at(process: HANDLE) -> Result<u64> {
        let mut creation = FILETIME::default(); let mut exit = FILETIME::default();
        let mut kernel = FILETIME::default(); let mut user = FILETIME::default();
        unsafe { GetProcessTimes(process, &mut creation, &mut exit, &mut kernel, &mut user).map_err(fail)?; }
        Ok(u64::from(creation.dwHighDateTime) << 32 | u64::from(creation.dwLowDateTime))
    }
    unsafe fn native_edit_selection(root: HWND, focus: HWND, pid: u32) -> Result<Option<(Vec<i32>, (usize, usize))>> {
        if focus.0.is_null() { return Ok(None); }
        let mut class_buffer = [0u16; 128];
        let length = unsafe { GetClassNameW(focus, &mut class_buffer) };
        if length <= 0 { return Ok(None); }
        let class_name = String::from_utf16_lossy(&class_buffer[..length as usize]);
        if !native_edit_class(&class_name) { return Ok(None); }
        let mut owner = 0;
        unsafe { GetWindowThreadProcessId(focus, Some(&mut owner)); }
        if owner != pid || unsafe { GetAncestor(focus, GA_ROOT) } != root {
            return Err(target_failure(TargetCaptureFailure::StopFocusChanged));
        }
        if !unsafe { IsWindowEnabled(focus) }.as_bool() { return Err(target_failure(TargetCaptureFailure::TargetNotEditable)); }
        writable_native_style(unsafe { GetWindowLongPtrW(focus, GWL_STYLE) } as u32)?;
        // EM_GETSEL is a system message: Windows marshals the two DWORD
        // outputs across processes. It obtains offsets without reading text.
        // Unlike EM_EXGETSEL (WM_USER + 52), it needs no remote allocation.
        let (mut start, mut end) = (0u32, 0u32);
        let mut result = 0usize;
        unsafe { SetLastError(ERROR_SUCCESS); }
        let accepted = unsafe { SendMessageTimeoutW(focus, 0x00B0,
            WPARAM((&mut start as *mut u32) as usize), LPARAM((&mut end as *mut u32) as isize),
            SMTO_ABORTIFHUNG | SMTO_BLOCK | SMTO_ERRORONEXIT, 200, Some(&mut result)) };
        if accepted.0 == 0 {
            return Err(target_failure(if unsafe { GetLastError() } == ERROR_TIMEOUT {
                TargetCaptureFailure::TargetInspectionTimeout
            } else { TargetCaptureFailure::TargetCapabilityUnsupported }));
        }
        if end < start { return Err(target_failure(TargetCaptureFailure::TargetCapabilityUnsupported)); }
        let raw = focus.0 as usize;
        // Native edit controls already have an OS focus identity. This stable
        // namespace is distinct from UIA provider runtime IDs; validation also
        // checks root HWND, PID, process lifetime, class/style and caret again.
        let class_hash = class_name.to_ascii_lowercase().bytes().fold(0x811c9dc5u32, |h, b| (h ^ u32::from(b)).wrapping_mul(0x01000193));
        Ok(Some((vec![NATIVE_EDIT_RUNTIME_ID, raw as i32, ((raw as u64) >> 32) as i32, class_hash as i32], (start as usize, end as usize))))
    }

    pub fn capture_bounded() -> Result<TargetSnapshot> {
        capture_bounded_inner(None,None)
    }
    pub fn capture_bounded_for_anchor(anchor: ForegroundAnchor) -> Result<TargetSnapshot> {
        capture_bounded_inner(Some(anchor),None)
    }
    pub fn capture_bounded_with_compatibility(anchor:ForegroundAnchor,activity:Option<InputActivity>)->Result<TargetSnapshot> {
        capture_bounded_inner(Some(anchor),activity)
    }
    pub fn inspect_wechat_window(anchor:ForegroundAnchor,activity:InputActivity)->Result<TargetSnapshot> {
        let(tx,rx)=sync_channel(1);
        submit_target_inspection(InspectionRequest::Wechat(anchor,activity,tx))?;
        receive_inspection(rx,Duration::from_secs(1))
    }
    pub fn recapture_keyboard_target(anchor:ForegroundAnchor,activity:InputActivity)->Result<TargetSnapshot> {
        let(tx,rx)=sync_channel(1);
        submit_target_inspection(InspectionRequest::Keyboard(anchor,activity,tx))?;
        receive_inspection(rx,Duration::from_secs(1))
    }
    fn capture_bounded_inner(anchor: Option<ForegroundAnchor>,compatibility:Option<InputActivity>) -> Result<TargetSnapshot> {
        let (tx, rx) = sync_channel(1);
        submit_target_inspection(InspectionRequest::Target(anchor, compatibility, tx))?;
        receive_inspection(rx, Duration::from_secs(1))
    }
    pub fn capture_input_context(target: TargetSnapshot) -> Result<crate::core::writing_context::InputContext> {
        let (tx, rx) = sync_channel(1);
        let deadline = Instant::now() + INPUT_CONTEXT_TIMEOUT;
        submit_context_inspection(InspectionRequest::Context(target, deadline, tx))?;
        receive_inspection(rx, INPUT_CONTEXT_TIMEOUT)
    }
    type Context = crate::core::writing_context::InputContext;
    enum InspectionRequest {
        Target(Option<ForegroundAnchor>, Option<InputActivity>, SyncSender<Result<TargetSnapshot>>),
        Wechat(ForegroundAnchor,InputActivity,SyncSender<Result<TargetSnapshot>>),
        Keyboard(ForegroundAnchor,InputActivity,SyncSender<Result<TargetSnapshot>>),
        Context(TargetSnapshot, Instant, SyncSender<Result<Context>>),
        #[cfg(test)]
        Probe { entered: SyncSender<()>, release: Option<std::sync::mpsc::Receiver<()>>, reply: SyncSender<Result<()>> },
    }
    struct InspectionWorker { sender: SyncSender<InspectionRequest>, busy: Arc<AtomicBool> }
    static TARGET_WORKER: OnceLock<InspectionWorker> = OnceLock::new();
    static CONTEXT_WORKER: OnceLock<InspectionWorker> = OnceLock::new();
    fn submit_target_inspection(request: InspectionRequest) -> Result<()> { submit_inspection(&TARGET_WORKER, request) }
    fn submit_context_inspection(request: InspectionRequest) -> Result<()> { submit_inspection(&CONTEXT_WORKER, request) }
    fn submit_inspection(slot: &'static OnceLock<InspectionWorker>, request: InspectionRequest) -> Result<()> {
        let worker = slot.get_or_init(|| {
            let (sender, receiver) = sync_channel(1);
            let busy = Arc::new(AtomicBool::new(false));
            let worker_busy = busy.clone();
            thread::spawn(move || {
                // Optional context and required delivery target inspection have
                // independent fixed workers. A hung context provider never owns
                // the target worker's gate. All COM objects remain local to the
                // thread that creates them; only owned Rust data crosses threads.
                let initialized = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).ok() }.is_ok();
                let _com = initialized.then(|| ComGuard);
                while let Ok(request) = receiver.recv() {
                    let lease = InspectionLease(&worker_busy);
                    match request {
                        InspectionRequest::Target(anchor, compatibility, reply) => {
                            let result = capture_with_anchor(anchor,compatibility);
                            drop(lease);
                            let _ = reply.send(result);
                        }
                        InspectionRequest::Wechat(anchor,activity,reply) => {
                            let result=capture_compatibility_window(anchor,activity);
                            drop(lease);
                            let _=reply.send(result);
                        }
                        InspectionRequest::Keyboard(anchor,activity,reply) => {
                            let result=capture_keyboard_target(anchor,activity,false);
                            drop(lease);
                            let _=reply.send(result);
                        }
                        InspectionRequest::Context(target, deadline, reply) => {
                            let result = context_from_target(&target, deadline);
                            drop(lease);
                            let _ = reply.send(result);
                        }
                        #[cfg(test)]
                        InspectionRequest::Probe { entered, release, reply } => {
                            // Exercise the real worker routing/lease lifecycle
                            // without a UIA call or inspection of any window.
                            let _ = entered.send(());
                            let result = release.map_or(Ok(()), |receiver|
                                receiver.recv_timeout(Duration::from_secs(2)).map_err(|_| context_timeout()));
                            drop(lease);
                            let _ = reply.send(result);
                        }
                    }
                }
            });
            InspectionWorker { sender, busy }
        });
        let lease = claim_inspection(&worker.busy)?;
        worker.sender.try_send(request).map_err(|_| context_timeout())?;
        // Ownership of clearing busy passes to the worker, including after the
        // receiver has timed out. Requests never queue behind a stuck provider.
        std::mem::forget(lease);
        Ok(())
    }
    #[cfg(test)]
    pub(super) fn assert_context_worker_isolation() {
        struct ReleaseOnDrop(Option<SyncSender<()>>);
        impl Drop for ReleaseOnDrop {
            fn drop(&mut self) { if let Some(sender) = self.0.take() { let _ = sender.try_send(()); } }
        }
        let (context_started, entered) = sync_channel(1);
        let (release, blocked) = sync_channel(1);
        let release_guard = ReleaseOnDrop(Some(release));
        let (context_reply, completed) = sync_channel(1);
        submit_context_inspection(InspectionRequest::Probe { entered: context_started,
            release: Some(blocked), reply: context_reply }).unwrap();
        entered.recv_timeout(Duration::from_secs(1)).expect("context worker did not start");

        let (second_started, _) = sync_channel(1);
        let (second_reply, _) = sync_channel(1);
        assert!(submit_context_inspection(InspectionRequest::Probe { entered: second_started,
            release: None, reply: second_reply }).is_err(), "busy context worker accepted another request");

        let (target_started, target_entered) = sync_channel(1);
        let (target_reply, target_completed) = sync_channel(1);
        submit_target_inspection(InspectionRequest::Probe { entered: target_started,
            release: None, reply: target_reply }).expect("context work occupied the delivery target route");
        target_entered.recv_timeout(Duration::from_secs(1)).expect("target worker blocked behind context");
        target_completed.recv_timeout(Duration::from_secs(1)).unwrap().unwrap();

        drop(release_guard);
        completed.recv_timeout(Duration::from_secs(1)).unwrap().unwrap();
        let (resumed_started, resumed_entered) = sync_channel(1);
        let (resumed_reply, resumed_completed) = sync_channel(1);
        submit_context_inspection(InspectionRequest::Probe { entered: resumed_started,
            release: None, reply: resumed_reply }).unwrap();
        resumed_entered.recv_timeout(Duration::from_secs(1)).unwrap();
        resumed_completed.recv_timeout(Duration::from_secs(1)).unwrap().unwrap();
    }
    fn receive_inspection<T>(receiver: std::sync::mpsc::Receiver<Result<T>>, timeout: Duration) -> Result<T> {
        receiver.recv_timeout(timeout).map_err(|error| target_failure(match error {
            std::sync::mpsc::RecvTimeoutError::Timeout => TargetCaptureFailure::TargetInspectionTimeout,
            std::sync::mpsc::RecvTimeoutError::Disconnected => TargetCaptureFailure::TargetCapabilityUnsupported,
        }))?
    }
    pub fn capture_anchor() -> Result<ForegroundAnchor> {
        unsafe {
            let hwnd = GetForegroundWindow();
            if hwnd.0.is_null() { return Err(target_failure(TargetCaptureFailure::StopAnchorUnavailable)); }
            let mut process_id = 0;
            let thread_id = GetWindowThreadProcessId(hwnd, Some(&mut process_id));
            require_anchor_process(process_id)?;
            // An anchor is identity metadata, so an owned Mode window remains
            // inspectable by its UI coordinator. capture() still rejects it as
            // an external paste target after any verified Mode return.
            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id).map_err(anchor_fail)?;
            let _process = HandleGuard(process);
            // Preserve the typed permission reason before EM_GETSEL or UIA can
            // fail with a generic error on a higher-integrity host.
            ensure_delivery_integrity(process)?;
            let process_created_at = process_created_at(process)?;
            let mut gui = GUITHREADINFO { cbSize: size_of::<GUITHREADINFO>() as u32, ..Default::default() };
            GetGUIThreadInfo(thread_id, &mut gui).map_err(anchor_fail)?;
            let selection = native_edit_selection(hwnd, gui.hwndFocus, process_id)?.map(|(_, selection)| selection);
            let mut final_gui = GUITHREADINFO { cbSize: size_of::<GUITHREADINFO>() as u32, ..Default::default() };
            GetGUIThreadInfo(thread_id, &mut final_gui).map_err(anchor_fail)?;
            if GetForegroundWindow() != hwnd || final_gui.hwndFocus != gui.hwndFocus {
                return Err(target_failure(TargetCaptureFailure::StopFocusChanged));
            }
            Ok(ForegroundAnchor { hwnd: hwnd.0 as usize, process_id, process_created_at,
                focused_hwnd: gui.hwndFocus.0 as usize, selection })
        }
    }
    fn capture_with_anchor(anchor: Option<ForegroundAnchor>,compatibility:Option<InputActivity>) -> Result<TargetSnapshot> {
        if let Some(expected) = anchor {
            if capture_anchor()? != expected { return Err(target_failure(TargetCaptureFailure::StopFocusChanged)); }
        }
        let target = match capture(anchor) {
            Err(PlatformError::TargetCapture(TargetCaptureFailure::TargetCapabilityUnsupported))
                if anchor.is_some() && compatibility.is_some() => capture_keyboard_target(anchor.unwrap(),compatibility.unwrap(),false)?,
            result => result?,
        };
        if let Some(expected) = anchor {
            compare_anchor(&expected, &target)?;
            if capture_anchor()? != expected { return Err(target_failure(TargetCaptureFailure::StopFocusChanged)); }
        }
        // The final anchor check is intentionally identity-only. Recheck text
        // routing after it so a newly opened native menu cannot retain the Edit
        // focus/caret and become an apparently unchanged paste target.
        verify_input_route(&target)?;
        Ok(target)
    }
    fn verify_input_route(target: &TargetSnapshot) -> Result<()> {
        unsafe {
            let hwnd = HWND(target.hwnd as _);
            if GetForegroundWindow() != hwnd { return Err(target_failure(TargetCaptureFailure::StopFocusChanged)); }
            let mut process_id = 0;
            let thread_id = GetWindowThreadProcessId(hwnd, Some(&mut process_id));
            if process_id != target.process_id { return Err(target_failure(TargetCaptureFailure::StopFocusChanged)); }
            let mut gui = GUITHREADINFO { cbSize: size_of::<GUITHREADINFO>() as u32, ..Default::default() };
            GetGUIThreadInfo(thread_id, &mut gui).map_err(fail)?;
            require_text_input_route(gui.flags.0)?;
            if gui.hwndFocus.0 as usize != target.focused_hwnd || GetForegroundWindow() != hwnd {
                return Err(target_failure(TargetCaptureFailure::StopFocusChanged));
            }
            Ok(())
        }
    }
    fn capture(expected: Option<ForegroundAnchor>) -> Result<TargetSnapshot> {
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED).ok().map_err(fail)?;
            let _com = ComGuard;
            let hwnd = GetForegroundWindow();
            if hwnd.0.is_null() { return Err(target_failure(TargetCaptureFailure::StopAnchorUnavailable)); }
            let mut process_id = 0;
            let thread_id = GetWindowThreadProcessId(hwnd, Some(&mut process_id));
            require_external_target_process(process_id, GetCurrentProcessId())?;
            if expected.is_some_and(|anchor| anchor.hwnd != hwnd.0 as usize || anchor.process_id != process_id) {
                return Err(target_failure(TargetCaptureFailure::StopFocusChanged));
            }
            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id).map_err(fail)?;
            let _process = HandleGuard(process);
            ensure_delivery_integrity(process)?;
            let created = process_created_at(process)?;
            let mut gui = GUITHREADINFO { cbSize: size_of::<GUITHREADINFO>() as u32, ..Default::default() };
            GetGUIThreadInfo(thread_id, &mut gui).map_err(fail)?;
            require_text_input_route(gui.flags.0)?;
            if let Some(expected) = expected {
                compare_capture_identity(&expected, &ForegroundAnchor { hwnd: hwnd.0 as usize, process_id,
                    process_created_at: created, focused_hwnd: gui.hwndFocus.0 as usize, selection: None })?;
            }
            if let Some((runtime_id, selection)) = native_edit_selection(hwnd, gui.hwndFocus, process_id)? {
                if expected.and_then(|anchor| anchor.selection).is_some_and(|caret| caret != selection) {
                    return Err(target_failure(TargetCaptureFailure::StopFocusChanged));
                }
                let mut final_gui = GUITHREADINFO { cbSize: size_of::<GUITHREADINFO>() as u32, ..Default::default() };
                GetGUIThreadInfo(thread_id, &mut final_gui).map_err(fail)?;
                require_text_input_route(final_gui.flags.0)?;
                if GetForegroundWindow() != hwnd || final_gui.hwndFocus != gui.hwndFocus {
                    return Err(target_failure(TargetCaptureFailure::StopFocusChanged));
                }
                return Ok(TargetSnapshot { hwnd: hwnd.0 as usize, process_id, focused_hwnd: gui.hwndFocus.0 as usize,
                    runtime_id, selection: Some(selection), process_created_at: created, compatibility:None });
            }
            let automation: IUIAutomation = CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER).map_err(fail)?;
            if let Ok(options) = automation.cast::<IUIAutomation2>() {
                options.SetConnectionTimeout(200).map_err(fail)?;
                options.SetTransactionTimeout(200).map_err(fail)?;
            }
            let focused = automation.GetFocusedElement().map_err(fail)?;
            if focused.CurrentProcessId().map_err(fail)? as u32 != process_id { return Err(target_failure(TargetCaptureFailure::StopFocusChanged)); }
            if focused.CurrentIsPassword().map_err(fail)?.as_bool() || !focused.CurrentIsEnabled().map_err(fail)?.as_bool() {
                return Err(target_failure(TargetCaptureFailure::TargetNotEditable));
            }
            let control_type = focused.CurrentControlType().map_err(fail)?;
            if control_type == UIA_CustomControlTypeId {
                // Custom is a presentation type, not a read-only verdict. The
                // keyboard route separately requires explicit writable state,
                // keyboard focus and stable runtime identity.
                return Err(target_failure(TargetCaptureFailure::TargetCapabilityUnsupported));
            }
            if control_type != UIA_EditControlTypeId && control_type != UIA_DocumentControlTypeId && control_type != UIA_GroupControlTypeId {
                return Err(target_failure(if control_type==UIA_WindowControlTypeId {TargetCaptureFailure::TargetCapabilityUnsupported} else {TargetCaptureFailure::TargetNotEditable}));
            }
            if control_type == UIA_GroupControlTypeId && !focused.CurrentIsKeyboardFocusable().map_err(fail)?.as_bool() {
                return Err(target_failure(TargetCaptureFailure::TargetNotEditable));
            }
            let text = focused.GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId).map_err(fail)?;
            if let Ok(value) = focused.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId) {
                if value.CurrentIsReadOnly().map_err(fail)?.as_bool() { return Err(target_failure(TargetCaptureFailure::TargetNotEditable)); }
            } else {
                // Chromium contenteditable is a UIA Group with TextPattern,
                // without ValuePattern. Read the provider's explicit readonly
                // text attribute; a readable Group/Document alone proves nothing.
                let readonly = text.DocumentRange().map_err(fail)?.GetAttributeValue(UIA_IsReadOnlyAttributeId).map_err(fail)?;
                if !explicit_writable_text_attribute(&readonly) {
                    return Err(target_failure(if readonly == windows::core::VARIANT::from(true) {
                        TargetCaptureFailure::TargetNotEditable
                    } else { TargetCaptureFailure::TargetCapabilityUnsupported }));
                }
            }
            let array = focused.GetRuntimeId().map_err(fail)?;
            let _array = ArrayGuard(array);
            if array.is_null() || SafeArrayGetDim(array) != 1 { return Err(target_failure(TargetCaptureFailure::TargetCapabilityUnsupported)); }
            let lower = SafeArrayGetLBound(array, 1).map_err(fail)?;
            let upper = SafeArrayGetUBound(array, 1).map_err(fail)?;
            let count = (upper - lower + 1).max(0) as usize;
            if count == 0 || count > 256 { return Err(target_failure(TargetCaptureFailure::TargetCapabilityUnsupported)); }
            let mut data = std::ptr::null_mut();
            SafeArrayAccessData(array, &mut data).map_err(fail)?;
            let runtime_id = std::slice::from_raw_parts(data.cast::<i32>(), count).to_vec();
            let _ = SafeArrayUnaccessData(array);
            let selections = text.GetSelection().map_err(fail)?;
            if selections.Length().map_err(fail)? != 1 { return Err(target_failure(TargetCaptureFailure::TargetCapabilityUnsupported)); }
            let selection = selections.GetElement(0).map_err(fail)?;
            let prefix = text.DocumentRange().map_err(fail)?;
            prefix.MoveEndpointByRange(TextPatternRangeEndpoint_End, &selection, TextPatternRangeEndpoint_Start).map_err(fail)?;
            // This transient prefix is only needed to count UTF-16 caret
            // offset. Never return/store/log it. Bound unusually large fields.
            let value = prefix.GetText(65_537).map_err(fail)?;
            let start = value.len();
            if start > 65_536 { return Err(target_failure(TargetCaptureFailure::TargetCapabilityUnsupported)); }
            let selected = selection.GetText(65_537).map_err(fail)?;
            if selected.len() > 65_536 { return Err(target_failure(TargetCaptureFailure::TargetCapabilityUnsupported)); }
            if GetForegroundWindow() != hwnd { return Err(target_failure(TargetCaptureFailure::StopFocusChanged)); }
            let final_focused = automation.GetFocusedElement().map_err(fail)?;
            if !automation.CompareElements(&focused, &final_focused).map_err(fail)?.as_bool() {
                return Err(target_failure(TargetCaptureFailure::StopFocusChanged));
            }
            let mut final_gui = GUITHREADINFO { cbSize: size_of::<GUITHREADINFO>() as u32, ..Default::default() };
            GetGUIThreadInfo(thread_id, &mut final_gui).map_err(fail)?;
            require_text_input_route(final_gui.flags.0)?;
            if final_gui.hwndFocus != gui.hwndFocus || GetForegroundWindow() != hwnd {
                return Err(target_failure(TargetCaptureFailure::StopFocusChanged));
            }
            let target = TargetSnapshot { hwnd: hwnd.0 as usize, process_id, focused_hwnd: gui.hwndFocus.0 as usize,
                runtime_id, selection: Some((start, start + selected.len())),
                process_created_at: created, compatibility:None };
            Ok(target)
        }
    }

    fn validate_context_anchor(target: &TargetSnapshot, deadline: Instant) -> Result<()> {
        check_context_deadline(deadline)?;
        if target.selection.is_none() || target.runtime_id.is_empty() {
            return Err(target_failure(TargetCaptureFailure::TargetCapabilityUnsupported));
        }
        require_external_target_process(target.process_id, unsafe { GetCurrentProcessId() })?;
        compare_anchor(&capture_anchor()?, target)?;
        verify_input_route(target)?;
        check_context_deadline(deadline)
    }
    fn context_from_target(target: &TargetSnapshot, deadline: Instant) -> Result<Context> {
        if target.compatibility.is_some() {return Err(target_failure(TargetCaptureFailure::TargetCapabilityUnsupported));}
        validate_context_anchor(target, deadline)?;
        if target.runtime_id.first() == Some(&NATIVE_EDIT_RUNTIME_ID) {
            let context = unsafe { native_edit_context(target, deadline)? };
            validate_context_anchor(target, deadline)?;
            return Ok(context);
        }
        unsafe {
            let automation: IUIAutomation = CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER).map_err(fail)?;
            let options = automation.cast::<IUIAutomation2>().map_err(fail)?;
            options.SetConnectionTimeout(100).map_err(fail)?;
            options.SetTransactionTimeout(100).map_err(fail)?;
            let (focused, selection, field_range) = verify_context_selection(&automation, target, None, deadline)?;
            check_context_deadline(deadline)?;
            let before = selection.Clone().map_err(fail)?;
            before.MoveEndpointByRange(TextPatternRangeEndpoint_End, &selection, TextPatternRangeEndpoint_Start).map_err(fail)?;
            before.MoveEndpointByUnit(TextPatternRangeEndpoint_Start, TextUnit_Character, -1_000).map_err(fail)?;
            let after = selection.Clone().map_err(fail)?;
            after.MoveEndpointByRange(TextPatternRangeEndpoint_Start, &selection, TextPatternRangeEndpoint_End).map_err(fail)?;
            after.MoveEndpointByUnit(TextPatternRangeEndpoint_End, TextUnit_Character, 500).map_err(fail)?;
            // Chromium range movement can leave its focused TextPattern and
            // enter sibling controls in the page. Clamp using this field's
            // DocumentRange metadata BEFORE reading any text from the ranges.
            clamp_to_field(&before, &field_range)?;
            clamp_to_field(&after, &field_range)?;
            // Only these selected/adjacent ranges become model context. The
            // separate bounded prefix verification above remains transient.
            let before_text = read_context_range(&before, 2_000, deadline)?;
            let selected_text = read_context_range(&selection, 1_000, deadline)?;
            let after_text = read_context_range(&after, 1_000, deadline)?;
            check_context_deadline(deadline)?;
            // A Document provider's Name can be its file/page title, rather
            // than a field label. Do not collect it as input-position context.
            let control_label = if focused.CurrentControlType().map_err(fail)? == UIA_DocumentControlTypeId {
                String::new()
            } else { focused.CurrentName().map_err(fail)?.to_string() };
            verify_context_selection(&automation, target, Some(&selection), deadline)?;
            validate_context_anchor(target, deadline)?;
            Ok(Context { control_label: first_chars(&control_label, 160),
                before_text: last_chars(&before_text, 1_000), selected_text: first_chars(&selected_text, 500),
                after_text: first_chars(&after_text, 500) })
        }
    }
    unsafe fn verify_context_selection(automation: &IUIAutomation, target: &TargetSnapshot, expected_selection: Option<&IUIAutomationTextRange>, deadline: Instant)
        -> Result<(IUIAutomationElement, IUIAutomationTextRange, IUIAutomationTextRange)> {
        unsafe {
            check_context_deadline(deadline)?;
            let focused = automation.GetFocusedElement().map_err(fail)?;
            if focused.CurrentProcessId().map_err(fail)? as u32 != target.process_id ||
                runtime_id_of(&focused)? != target.runtime_id {
                return Err(target_failure(TargetCaptureFailure::StopFocusChanged));
            }
            if focused.CurrentIsPassword().map_err(fail)?.as_bool() || !focused.CurrentIsEnabled().map_err(fail)?.as_bool() {
                return Err(target_failure(TargetCaptureFailure::TargetNotEditable));
            }
            let control_type = focused.CurrentControlType().map_err(fail)?;
            if control_type != UIA_EditControlTypeId && control_type != UIA_DocumentControlTypeId && control_type != UIA_GroupControlTypeId ||
                (control_type == UIA_GroupControlTypeId && !focused.CurrentIsKeyboardFocusable().map_err(fail)?.as_bool()) {
                return Err(target_failure(TargetCaptureFailure::TargetNotEditable));
            }
            let text = focused.GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId).map_err(fail)?;
            let field_range = text.DocumentRange().map_err(fail)?;
            let selections = text.GetSelection().map_err(fail)?;
            if selections.Length().map_err(fail)? != 1 { return Err(target_failure(TargetCaptureFailure::TargetCapabilityUnsupported)); }
            let current = selections.GetElement(0).map_err(fail)?;
            if let Ok(value) = focused.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId) {
                if value.CurrentIsReadOnly().map_err(fail)?.as_bool() { return Err(target_failure(TargetCaptureFailure::TargetNotEditable)); }
            } else if !explicit_writable_text_attribute(&field_range.GetAttributeValue(UIA_IsReadOnlyAttributeId).map_err(fail)?) {
                return Err(target_failure(TargetCaptureFailure::TargetNotEditable));
            }
            require_range_inside_field(&current, &field_range)?;
            // Only after exact process/runtime/field identity and writability
            // proof may this worker inspect the same field's bounded prefix.
            // It verifies the original UTF-16 offsets without sharing COM range
            // bookmarks with the independent target worker. Never return or log
            // the transient prefix/selected-length verification text.
            verify_selection_offsets(&current, &field_range, target.selection, deadline)?;
            if let Some(expected) = expected_selection {
                if current.CompareEndpoints(TextPatternRangeEndpoint_Start, expected, TextPatternRangeEndpoint_Start).map_err(fail)? != 0 ||
                    current.CompareEndpoints(TextPatternRangeEndpoint_End, expected, TextPatternRangeEndpoint_End).map_err(fail)? != 0 {
                    return Err(target_failure(TargetCaptureFailure::StopFocusChanged));
                }
            }
            check_context_deadline(deadline)?;
            Ok((focused, current, field_range))
        }
    }
    unsafe fn runtime_id_of(element: &IUIAutomationElement) -> Result<Vec<i32>> {
        unsafe {
            let array = element.GetRuntimeId().map_err(fail)?;
            let _array = ArrayGuard(array);
            if array.is_null() || SafeArrayGetDim(array) != 1 { return Err(target_failure(TargetCaptureFailure::TargetCapabilityUnsupported)); }
            let lower = SafeArrayGetLBound(array, 1).map_err(fail)?;
            let upper = SafeArrayGetUBound(array, 1).map_err(fail)?;
            let count = (upper - lower + 1).max(0) as usize;
            if count == 0 || count > 256 { return Err(target_failure(TargetCaptureFailure::TargetCapabilityUnsupported)); }
            let mut data = std::ptr::null_mut();
            SafeArrayAccessData(array, &mut data).map_err(fail)?;
            let id = std::slice::from_raw_parts(data.cast::<i32>(), count).to_vec();
            let _ = SafeArrayUnaccessData(array);
            Ok(id)
        }
    }
    unsafe fn verify_selection_offsets(selection: &IUIAutomationTextRange, field: &IUIAutomationTextRange,
        offsets: Option<(usize, usize)>, deadline: Instant) -> Result<()> {
        let (start, end) = offsets.ok_or_else(|| target_failure(TargetCaptureFailure::TargetCapabilityUnsupported))?;
        if start > 65_536 || end < start || end - start > 65_536 {
            return Err(target_failure(TargetCaptureFailure::TargetCapabilityUnsupported));
        }
        unsafe {
            check_context_deadline(deadline)?;
            let prefix = field.Clone().map_err(fail)?;
            prefix.MoveEndpointByRange(TextPatternRangeEndpoint_End, selection, TextPatternRangeEndpoint_Start).map_err(fail)?;
            require_range_inside_field(&prefix, field)?;
            let prefix_length = prefix.GetText((start + 1) as i32).map_err(fail)?.len();
            check_context_deadline(deadline)?;
            let selected_length = selection.GetText((end - start + 1) as i32).map_err(fail)?.len();
            if prefix_length != start || selected_length != end - start {
                return Err(target_failure(TargetCaptureFailure::StopFocusChanged));
            }
            check_context_deadline(deadline)
        }
    }
    unsafe fn require_range_inside_field(range: &IUIAutomationTextRange, field: &IUIAutomationTextRange) -> Result<()> {
        let start = unsafe { range.CompareEndpoints(TextPatternRangeEndpoint_Start, field, TextPatternRangeEndpoint_Start).map_err(fail)? };
        let end = unsafe { range.CompareEndpoints(TextPatternRangeEndpoint_End, field, TextPatternRangeEndpoint_End).map_err(fail)? };
        if !range_inside_field(start, end) { return Err(target_failure(TargetCaptureFailure::TargetCapabilityUnsupported)); }
        Ok(())
    }
    unsafe fn clamp_to_field(range: &IUIAutomationTextRange, field: &IUIAutomationTextRange) -> Result<()> {
        unsafe {
            if range.CompareEndpoints(TextPatternRangeEndpoint_Start, field, TextPatternRangeEndpoint_Start).map_err(fail)? < 0 {
                range.MoveEndpointByRange(TextPatternRangeEndpoint_Start, field, TextPatternRangeEndpoint_Start).map_err(fail)?;
            }
            if range.CompareEndpoints(TextPatternRangeEndpoint_End, field, TextPatternRangeEndpoint_End).map_err(fail)? > 0 {
                range.MoveEndpointByRange(TextPatternRangeEndpoint_End, field, TextPatternRangeEndpoint_End).map_err(fail)?;
            }
            require_range_inside_field(range, field)
        }
    }
    unsafe fn read_context_range(range: &IUIAutomationTextRange, limit: i32, deadline: Instant) -> Result<String> {
        check_context_deadline(deadline)?;
        let text = unsafe { range.GetText(limit).map_err(fail)? };
        // BSTR truncation can split a supplementary character. Omit context
        // rather than feed the model an invented replacement character.
        let value = String::from_utf16(text.as_wide()).map_err(|_| target_failure(TargetCaptureFailure::TargetCapabilityUnsupported))?;
        check_context_deadline(deadline)?;
        Ok(normalize_uia_context_objects(value))
    }
    unsafe fn native_edit_context(target: &TargetSnapshot, deadline: Instant) -> Result<Context> {
        unsafe {
            let focus = HWND(target.focused_hwnd as _);
            let (runtime_id, selection) = native_edit_selection(HWND(target.hwnd as _), focus, target.process_id)?
                .ok_or_else(|| target_failure(TargetCaptureFailure::TargetCapabilityUnsupported))?;
            if runtime_id != target.runtime_id || Some(selection) != target.selection {
                return Err(target_failure(TargetCaptureFailure::StopFocusChanged));
            }
            let length = context_message(focus, 0x000E, WPARAM(0), LPARAM(0), deadline)?; // WM_GETTEXTLENGTH
            if length > NATIVE_CONTEXT_UTF16_LIMIT { return Err(target_failure(TargetCaptureFailure::TargetCapabilityUnsupported)); }
            let mut buffer = vec![0u16; length + 1];
            let copied = context_message(focus, 0x000D, WPARAM(buffer.len()), LPARAM(buffer.as_mut_ptr() as isize), deadline)?; // WM_GETTEXT
            let final_length = context_message(focus, 0x000E, WPARAM(0), LPARAM(0), deadline)?;
            if copied > length || final_length != length { return Err(target_failure(TargetCaptureFailure::StopFocusChanged)); }
            buffer.truncate(copied);
            let final_selection = native_edit_selection(HWND(target.hwnd as _), focus, target.process_id)?;
            if final_selection != Some((runtime_id, selection)) { return Err(target_failure(TargetCaptureFailure::StopFocusChanged)); }
            split_native_context(&buffer, selection)
        }
    }
    unsafe fn context_message(focus: HWND, message: u32, wparam: WPARAM, lparam: LPARAM, deadline: Instant) -> Result<usize> {
        check_context_deadline(deadline)?;
        let remaining = deadline.saturating_duration_since(Instant::now()).as_millis().min(100).max(1) as u32;
        let mut result = 0;
        unsafe { SetLastError(ERROR_SUCCESS); }
        let accepted = unsafe { SendMessageTimeoutW(focus, message, wparam, lparam,
            SMTO_ABORTIFHUNG | SMTO_BLOCK | SMTO_ERRORONEXIT, remaining, Some(&mut result)) };
        if accepted.0 == 0 { return Err(target_failure(if unsafe { GetLastError() } == ERROR_TIMEOUT {
            TargetCaptureFailure::TargetInspectionTimeout
        } else { TargetCaptureFailure::TargetCapabilityUnsupported })); }
        check_context_deadline(deadline)?;
        Ok(result)
    }

    pub fn modifiers_released() -> bool {
        [VK_CONTROL, VK_SHIFT, VK_MENU, VK_LWIN, VK_RWIN].iter().all(|vk| unsafe { GetAsyncKeyState(i32::from(vk.0)) } as u16 & 0x8000 == 0)
    }
    pub fn paste_chord() -> u32 {
        unsafe {
            // Some editors inspect the native scan code (including WebView
            // KeyboardEvent.code). Resolve logical Ctrl+V in the target thread's
            // layout instead of emitting four events with scanCode == 0.
            let layout=GetKeyboardLayout(GetWindowThreadProcessId(GetForegroundWindow(),None));
            let control=MapVirtualKeyExW(u32::from(VK_LCONTROL.0),MAPVK_VK_TO_VSC_EX,layout);
            let paste=MapVirtualKeyExW(0x56,MAPVK_VK_TO_VSC_EX,layout);
            let events=paste_inputs(control,paste);
            let count = SendInput(&events, size_of::<INPUT>() as i32);
            if count != 0 && count != 4 {
                // Release only keys introduced by this chord. Never repeat V-down.
                let cleanup = [events[2],events[3]];
                let _ = SendInput(&cleanup, size_of::<INPUT>() as i32);
            }
            count
        }
    }
    fn paste_inputs(control:u32,paste:u32)->[INPUT;4] {
        let input=|vk:u16,scan:u32,up:bool| {
            // A missing layout mapping uses logical keys, never an arbitrary
            // physical V position or a second paste strategy.
            let mapped=scan&0xff!=0;
            let mut flags=if up{KEYEVENTF_KEYUP}else{Default::default()};
            if mapped {flags|=KEYEVENTF_SCANCODE;if scan&0xff00==0xe000{flags|=KEYEVENTF_EXTENDEDKEY;}}
            INPUT{r#type:INPUT_KEYBOARD,Anonymous:INPUT_0{ki:KEYBDINPUT{
                wVk:VIRTUAL_KEY(if mapped{0}else{vk}),wScan:if mapped{(scan&0xff) as u16}else{0},
                dwFlags:flags,dwExtraInfo:0x534D_5041,..Default::default()}}}
        };
        [input(VK_LCONTROL.0,control,false),input(0x56,paste,false),input(0x56,paste,true),input(VK_LCONTROL.0,control,true)]
    }
    #[test]
    fn paste_events_have_target_layout_scan_codes_and_exactly_one_v_down() {
        for (control,paste) in [(0x1d,0x2f),(0x1d,0x34),(0xe01d,0x2f),(0,0)] {
            let events=paste_inputs(control,paste);
            let keys=events.map(|event|unsafe{event.Anonymous.ki});
            assert_eq!(keys[0].wScan,control as u16&0xff);
            assert_eq!(keys[1].wScan,paste as u16&0xff);
            assert_eq!(keys[2].wScan,keys[1].wScan);
            assert_eq!(keys[3].wScan,keys[0].wScan);
            for (index,key) in keys.iter().enumerate() {
                assert_eq!(key.dwFlags.contains(KEYEVENTF_KEYUP),index>=2);
                assert_eq!(key.dwFlags.contains(KEYEVENTF_SCANCODE),control!=0);
                assert_eq!(key.dwExtraInfo,0x534D_5041);
            }
            assert_eq!(keys[0].dwFlags.contains(KEYEVENTF_EXTENDEDKEY),control==0xe01d);
            if control==0 {assert_eq!(keys[0].wVk,VK_LCONTROL);assert_eq!(keys[1].wVk,VIRTUAL_KEY(0x56));}
        }
    }
    pub fn copy_text(text: &str) -> std::result::Result<bool, ClipboardError> {
        let error = |reason: String, cleared| ClipboardError { reason, cleared };
        if text.contains('\0') || text.len() > 4 * 1024 * 1024 { return Err(error("invalid or oversized clipboard text".into(), false)); }
        let mut wide: Vec<u16> = text.encode_utf16().collect(); wide.push(0);
        unsafe {
            let memory = GlobalAlloc(GMEM_MOVEABLE, wide.len() * size_of::<u16>()).map_err(|e| error(e.to_string(), false))?;
            let pointer = GlobalLock(memory);
            if pointer.is_null() { let _ = GlobalFree(memory); return Err(error("clipboard allocation lock failed".into(), false)); }
            std::ptr::copy_nonoverlapping(wide.as_ptr(), pointer.cast::<u16>(), wide.len());
            let _ = GlobalUnlock(memory);
            let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
            let owner = match CreateWindowExW(WINDOW_EX_STYLE::default(), PCWSTR(class.as_ptr()), PCWSTR::null(), WINDOW_STYLE::default(), 0,0,0,0, None, None, None, None) {
                Ok(hwnd) => hwnd,
                Err(e) => { let _ = GlobalFree(memory); return Err(error(e.to_string(), false)); },
            };
            let deadline = Instant::now() + Duration::from_millis(200);
            loop {
                if OpenClipboard(owner).is_ok() { break; }
                if Instant::now() >= deadline { let _ = GlobalFree(memory); let _ = DestroyWindow(owner); return Err(error("clipboard busy".into(), false)); }
                thread::sleep(Duration::from_millis(5));
            }
            if let Err(e) = EmptyClipboard() { let _ = CloseClipboard(); let _ = GlobalFree(memory); let _ = DestroyWindow(owner); return Err(error(e.to_string(), false)); }
            let result = SetClipboardData(13, HANDLE(memory.0));
            let _ = CloseClipboard(); let _ = DestroyWindow(owner);
            match result {
                Ok(_) => Ok(true),
                Err(e) => { let _ = GlobalFree(memory); Err(error(e.to_string(), true)) },
            }
        }
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
mod native {
    use super::*;
    pub fn capture_bounded() -> Result<TargetSnapshot> { Err(target_failure(TargetCaptureFailure::TargetCapabilityUnsupported)) }
    pub fn capture_anchor() -> Result<ForegroundAnchor> { Err(target_failure(TargetCaptureFailure::StopAnchorUnavailable)) }
    pub fn capture_bounded_for_anchor(_: ForegroundAnchor) -> Result<TargetSnapshot> { Err(target_failure(TargetCaptureFailure::TargetCapabilityUnsupported)) }
    pub fn capture_activity() -> Result<InputActivity> { Err(target_failure(TargetCaptureFailure::TargetCapabilityUnsupported)) }
    pub fn capture_bounded_with_compatibility(_:ForegroundAnchor,_:Option<InputActivity>)->Result<TargetSnapshot> { Err(target_failure(TargetCaptureFailure::TargetCapabilityUnsupported)) }
    pub fn inspect_wechat_window(_:ForegroundAnchor,_:InputActivity)->Result<TargetSnapshot> { Err(target_failure(TargetCaptureFailure::TargetCapabilityUnsupported)) }
    pub fn recapture_keyboard_target(_:ForegroundAnchor,_:InputActivity)->Result<TargetSnapshot> { Err(target_failure(TargetCaptureFailure::TargetCapabilityUnsupported)) }
    pub fn capture_input_context(_: TargetSnapshot) -> Result<crate::core::writing_context::InputContext> { Err(target_failure(TargetCaptureFailure::TargetCapabilityUnsupported)) }
    pub fn modifiers_released() -> bool { false }
    pub fn paste_chord() -> u32 { 0 }
    pub fn copy_text(_: &str) -> std::result::Result<bool, ClipboardError> { Err(ClipboardError { cleared: false, reason: "Windows clipboard required".into() }) }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn uia_context_omits_empty_object_placeholders() {
        for value in ["\u{fffc}", "\u{fffc}\u{fffc}", " \r\n\u{fffc}\t\u{3000}"] {
            assert_eq!(normalize_uia_context_objects(value.into()), "");
        }
    }
    #[test] fn uia_context_keeps_inline_object_word_boundaries_and_unicode() {
        assert_eq!(normalize_uia_context_objects("hello\u{fffc}world".into()), "hello world");
        assert_eq!(normalize_uia_context_objects("前😀\u{fffc}後😀".into()), "前😀 後😀");
        for value in ["", " \r\n\t", "前😀 后\n", "\u{fffd}", "  word  "] {
            assert_eq!(normalize_uia_context_objects(value.into()), value);
        }
        let native: Vec<_> = "a\u{fffc}b".encode_utf16().collect();
        assert_eq!(split_native_context(&native, (0, 0)).unwrap().after_text, "a\u{fffc}b");
    }
    #[test] fn input_context_rejects_ranges_that_enter_sibling_fields() {
        for (start, end) in [(0,0), (1,-1), (1,0), (0,-1)] { assert!(range_inside_field(start,end)); }
        for (start, end) in [(-1,0), (0,1), (-1,1), (-1,-1), (1,1)] { assert!(!range_inside_field(start,end)); }
    }
    #[test] fn input_context_preserves_unicode_and_reads_the_nearest_bounded_spans() {
        let before = format!("omitted{}", "前😀".repeat(600));
        let selected = "選😀".repeat(260);
        let after = "後😀".repeat(260);
        let start = before.encode_utf16().count();
        let end = start + selected.encode_utf16().count();
        let wide: Vec<_> = format!("{before}{selected}{after}").encode_utf16().collect();
        let context = split_native_context(&wide, (start, end)).unwrap();
        assert_eq!(context.before_text, "前😀".repeat(500));
        assert_eq!(context.selected_text, "選😀".repeat(250));
        assert_eq!(context.after_text, "後😀".repeat(250));
        assert!(context.control_label.is_empty());
        assert!(!context.before_text.contains('\u{fffd}'));
    }
    #[test] fn input_context_rejects_invalid_or_split_surrogate_carets_and_oversized_fields() {
        let wide: Vec<_> = "a😀b".encode_utf16().collect();
        assert!(split_native_context(&wide, (1, 3)).is_ok());
        for selection in [(2, 2), (3, 2), (0, 5)] { assert!(split_native_context(&wide, selection).is_err()); }
        assert!(split_native_context(&vec![65; NATIVE_CONTEXT_UTF16_LIMIT + 1], (0, 0)).is_err());
        let empty = split_native_context(&[], (0, 0)).unwrap();
        assert!(empty.before_text.is_empty() && empty.selected_text.is_empty() && empty.after_text.is_empty());
    }
    #[test] fn expired_input_context_deadline_never_reauthorizes_collection() {
        assert!(check_context_deadline(Instant::now() - Duration::from_millis(1)).is_err());
        assert!(check_context_deadline(Instant::now() + INPUT_CONTEXT_TIMEOUT).is_ok());
    }
    #[test] fn timed_out_inspection_caller_cannot_spawn_additional_native_work() {
        use std::sync::{Arc, mpsc::sync_channel};
        let busy = Arc::new(AtomicBool::new(false));
        let worker_busy = busy.clone();
        let (started_tx, started_rx) = sync_channel(1);
        let (release_tx, release_rx) = sync_channel(1);
        let worker = std::thread::spawn(move || {
            let _lease = claim_inspection(&worker_busy).unwrap();
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        started_rx.recv().unwrap();
        // A caller giving up does not own or release the native-work lease.
        drop(started_rx);
        for _ in 0..20 { assert!(claim_inspection(&busy).is_err()); }
        release_tx.send(()).unwrap(); worker.join().unwrap();
        assert!(claim_inspection(&busy).is_ok());
    }
    #[cfg(windows)]
    #[test] fn blocked_optional_context_does_not_occupy_required_target_gate() {
        native::assert_context_worker_isolation();
    }
    fn snapshot() -> TargetSnapshot { TargetSnapshot { hwnd: 10, process_id: 20, focused_hwnd: 30, runtime_id: vec![1,2,3], selection: Some((4,4)), process_created_at: 50, compatibility:None } }
    #[test] fn rejects_different_field_in_same_window_and_changed_caret() {
        let before = snapshot(); let mut now = before.clone(); now.runtime_id.push(4);
        assert!(compare_targets(&before, &now).is_err()); now = before.clone(); now.selection = Some((5,5));
        assert!(compare_targets(&before, &now).is_err()); assert!(compare_targets(&before, &before).is_ok());
    }
    #[test] fn process_reuse_does_not_reauthorize_old_target() {
        let before = snapshot(); let mut now = before.clone(); now.process_created_at += 1;
        assert!(compare_targets(&before, &now).is_err());
    }
    #[test] fn bound_capture_rejects_replaced_root_process_lifetime_or_focus_before_uia() {
        let original = validation_anchor(&snapshot());
        assert!(compare_capture_identity(&original, &original).is_ok());
        for field in 0..4 {
            let mut changed = original;
            match field {
                0 => changed.hwnd += 1,
                1 => changed.process_id += 1,
                2 => changed.process_created_at += 1,
                _ => changed.focused_hwnd += 1,
            }
            assert!(matches!(compare_capture_identity(&original, &changed),
                Err(PlatformError::TargetCapture(TargetCaptureFailure::StopFocusChanged))));
        }
    }
    #[test] fn validation_anchor_keeps_native_caret_without_inventing_a_virtual_uia_caret() {
        let mut target = snapshot();
        assert_eq!(validation_anchor(&target).selection, None);
        target.runtime_id = vec![NATIVE_EDIT_RUNTIME_ID, 1, 0, 2];
        assert_eq!(validation_anchor(&target).selection, Some((4,4)));
        target.selection = None;
        assert_eq!(validation_anchor(&target).selection, None);
        // The full UIA snapshot still rejects a caret change inside the bound
        // host; leaving its cheap anchor caret unknown never authorizes a paste.
        let original = snapshot(); let mut changed = original.clone();
        changed.selection = Some((5,5));
        assert!(compare_capture_identity(&validation_anchor(&original), &validation_anchor(&changed)).is_ok());
        assert!(compare_targets(&original, &changed).is_err());
    }
    #[test] fn own_mode_surface_can_be_an_identity_anchor_but_never_an_external_target() {
        let own_process_id = 17;
        assert!(require_anchor_process(own_process_id).is_ok());
        assert!(matches!(require_external_target_process(own_process_id, own_process_id),
            Err(PlatformError::TargetCapture(TargetCaptureFailure::TargetNotEditable))));
        assert!(matches!(require_anchor_process(0),
            Err(PlatformError::TargetCapture(TargetCaptureFailure::StopAnchorUnavailable))));
        assert!(require_external_target_process(23, own_process_id).is_ok());
    }
    #[test] fn stop_anchor_rejects_async_window_process_field_and_native_caret_changes() {
        let target = snapshot();
        let anchor = ForegroundAnchor { hwnd: target.hwnd, process_id: target.process_id,
            process_created_at: target.process_created_at, focused_hwnd: target.focused_hwnd, selection: target.selection };
        assert!(compare_anchor(&anchor, &target).is_ok());
        let mut changed = target.clone(); changed.hwnd += 1; assert!(compare_anchor(&anchor, &changed).is_err());
        let mut changed = target.clone(); changed.process_id += 1; assert!(compare_anchor(&anchor, &changed).is_err());
        let mut changed = target.clone(); changed.process_created_at += 1; assert!(compare_anchor(&anchor, &changed).is_err());
        let mut changed = target.clone(); changed.focused_hwnd += 1; assert!(compare_anchor(&anchor, &changed).is_err());
        let mut changed = target.clone(); changed.selection = Some((5, 5)); assert!(compare_anchor(&anchor, &changed).is_err());
    }
    #[test] fn virtual_uia_anchor_does_not_invent_unavailable_native_caret() {
        let target = snapshot();
        let anchor = ForegroundAnchor { hwnd: target.hwnd, process_id: target.process_id,
            process_created_at: target.process_created_at, focused_hwnd: target.focused_hwnd, selection: None };
        assert!(compare_anchor(&anchor, &target).is_ok());
        // Async UIA supplies the actual runtime ID and selection; dispatch still
        // compares them exactly, even when the earlier native anchor could not.
        let mut changed = target.clone(); changed.selection = Some((5, 5));
        assert!(compare_targets(&target, &changed).is_err());
    }
    #[test] fn unknown_selection_is_never_auto_delivered() {
        let mut before = snapshot(); before.selection = None;
        assert!(compare_targets(&before, &before).is_err());
    }
    #[test] fn attempt_is_single_use_even_after_failure() {
        let p = DeliveryPermit::from_committed_attempt("a".into(), 1);
        assert!(p.claim()); p.finish(); assert!(!p.claim()); assert!(!p.begin_injection());
    }
    #[test] fn claimed_attempt_can_be_cancelled_before_injection() {
        let p = DeliveryPermit::from_committed_attempt("a".into(), 1);
        assert!(p.claim()); assert!(p.cancel_before_injection()); assert!(!p.begin_injection()); p.finish(); assert!(!p.claim());
    }
    #[test] fn injection_boundary_cannot_be_cancelled_or_retried() {
        let p = DeliveryPermit::from_committed_attempt("a".into(), 1);
        assert!(p.claim()); assert!(p.begin_injection()); assert!(!p.cancel_before_injection()); p.finish(); assert!(!p.claim());
    }
    struct Fake<'a> {
        checks: std::cell::Cell<usize>,
        copies: std::cell::Cell<usize>,
        chords: std::cell::Cell<usize>,
        fail_check: usize,
        cancel_on_copy: Option<&'a DeliveryPermit>,
    }
    impl NativeOperations for Fake<'_> {
        fn validate(&self, _: &TargetSnapshot) -> Result<()> {
            self.checks.set(self.checks.get() + 1);
            if self.checks.get() == self.fail_check { Err(PlatformError::Unavailable("target changed".into())) } else { Ok(()) }
        }
        fn modifiers_released(&self) -> bool { true }
        fn copy(&self, _: &str) -> std::result::Result<bool, ClipboardError> {
            self.copies.set(self.copies.get() + 1);
            if let Some(p) = self.cancel_on_copy { assert!(p.cancel_before_injection()); }
            Ok(true)
        }
        fn chord(&self) -> u32 { self.chords.set(self.chords.get() + 1); 4 }
    }
    fn fake(fail_check: usize) -> Fake<'static> {
        Fake { checks: Default::default(), copies: Default::default(), chords: Default::default(), fail_check, cancel_on_copy: None }
    }
    #[test] fn invalid_target_does_not_change_clipboard() {
        let p = DeliveryPermit::from_committed_attempt("a".into(), 1); let f = fake(1);
        let r = dispatch_with(&f, &p, &snapshot(), "result", || true);
        assert!(matches!(r.outcome, DispatchOutcome::Skipped)); assert!(!r.clipboard_changed);
        assert_eq!((f.copies.get(), f.chords.get()), (0, 0));
    }
    #[test] fn target_change_after_clipboard_never_injects() {
        let p = DeliveryPermit::from_committed_attempt("a".into(), 1); let f = fake(2);
        let r = dispatch_with(&f, &p, &snapshot(), "result", || true);
        assert!(matches!(r.outcome, DispatchOutcome::Skipped)); assert!(r.clipboard_changed);
        assert_eq!((f.copies.get(), f.chords.get()), (1, 0));
    }
    #[test] fn cancellation_after_clipboard_is_honest_and_single_use() {
        let p = DeliveryPermit::from_committed_attempt("a".into(), 1);
        let f = Fake { cancel_on_copy: Some(&p), ..fake(0) };
        let r = dispatch_with(&f, &p, &snapshot(), "result", || true);
        assert!(matches!(r.outcome, DispatchOutcome::Skipped)); assert!(r.clipboard_changed);
        assert_eq!(f.chords.get(), 0);
        let _ = dispatch_with(&f, &p, &snapshot(), "result", || true);
        assert_eq!(f.copies.get(), 1);
    }
    #[test] fn dispatch_count_does_not_claim_insertion_and_cannot_repeat() {
        let p = DeliveryPermit::from_committed_attempt("a".into(), 1); let f = fake(0);
        let r = dispatch_with(&f, &p, &snapshot(), "result", || true);
        assert!(matches!(r.outcome, DispatchOutcome::Unconfirmed)); assert_eq!(r.input_events, 4);
        let _ = dispatch_with(&f, &p, &snapshot(), "result", || true);
        assert_eq!((f.copies.get(), f.chords.get()), (1, 1));
    }
    #[test] fn only_known_native_edit_classes_bypass_uia_value_text_requirement() {
        for class in ["Edit", "RICHEDIT50W", "RichEdit20W", "RichEditD2DPT"] { assert!(native_edit_class(class)); }
        for class in ["Chrome_RenderWidgetHostHWND", "STATIC", "Button", "SomeRichEditLookalike"] { assert!(!native_edit_class(class)); }
    }
    #[test] fn native_writable_proof_rejects_password_readonly_and_disabled() {
        assert!(writable_native_style(0x5021_0004).is_ok());
        for rejected in [0x0020, 0x0800, 0x0800_0000, 0x0820] { assert!(writable_native_style(rejected).is_err()); }
    }
    #[cfg(windows)]
    #[test] fn native_menu_routing_rejects_a_still_focused_edit_before_capture_or_validation() {
        use windows::Win32::UI::WindowsAndMessaging::{GUI_CARETBLINKING, GUI_INMENUMODE, GUI_POPUPMENUMODE, GUI_SYSTEMMENUMODE};
        // Windows can keep hwndFocus/EM_GETSEL on the Edit while the menu owns
        // keyboard routing. A matching field/caret must not authorize Ctrl+V.
        for flags in [GUI_INMENUMODE.0, GUI_POPUPMENUMODE.0, GUI_SYSTEMMENUMODE.0,
            (GUI_INMENUMODE | GUI_POPUPMENUMODE | GUI_CARETBLINKING).0] {
            assert!(matches!(require_text_input_route(flags),
                Err(PlatformError::TargetCapture(TargetCaptureFailure::TargetNotEditable))));
        }
        assert!(require_text_input_route(0).is_ok());
        assert!(require_text_input_route(GUI_CARETBLINKING.0).is_ok());
    }
    #[cfg(windows)]
    #[test] fn text_only_writable_requires_exact_false_bool_not_coercible_or_empty_variant() {
        use windows::core::VARIANT;
        assert!(explicit_writable_text_attribute(&VARIANT::from(false)));
        assert!(!explicit_writable_text_attribute(&VARIANT::from(true)));
        assert!(!explicit_writable_text_attribute(&VARIANT::new()));
        assert!(!explicit_writable_text_attribute(&VARIANT::from(0i32)));
        assert!(!explicit_writable_text_attribute(&VARIANT::from("false")));
    }
    #[cfg(windows)]
    #[test] fn native_provider_error_classification_retains_no_provider_message() {
        use windows::core::{Error, HRESULT};
        use windows::Win32::{Foundation::ERROR_TIMEOUT, UI::Accessibility::UIA_E_TIMEOUT};
        for code in [HRESULT(UIA_E_TIMEOUT as i32), HRESULT::from_win32(ERROR_TIMEOUT.0)] {
            let error = native_target_error(Error::new(code, "synthetic private provider payload"),
                TargetCaptureFailure::TargetCapabilityUnsupported);
            assert!(matches!(error, PlatformError::TargetCapture(TargetCaptureFailure::TargetInspectionTimeout)));
            assert!(!error.to_string().contains("private provider"));
        }
        let error = native_target_error(Error::new(HRESULT(0x8000_4005u32 as i32), "synthetic private provider payload"),
            TargetCaptureFailure::TargetCapabilityUnsupported);
        assert!(matches!(error, PlatformError::TargetCapture(TargetCaptureFailure::TargetCapabilityUnsupported)));
        assert!(!error.to_string().contains("private provider"));
    }
    struct Waiting<'a> {
        fake: Fake<'a>,
        held_polls: std::cell::Cell<usize>,
        pauses: std::cell::Cell<usize>,
        cancel_on_pause: Option<&'a DeliveryPermit>,
    }
    impl NativeOperations for Waiting<'_> {
        fn validate(&self, target: &TargetSnapshot) -> Result<()> { self.fake.validate(target) }
        fn modifiers_released(&self) -> bool { self.held_polls.get() == 0 }
        fn copy(&self, text: &str) -> std::result::Result<bool, ClipboardError> { self.fake.copy(text) }
        fn chord(&self) -> u32 { self.fake.chord() }
        fn pause(&self, _: Duration) {
            self.pauses.set(self.pauses.get() + 1);
            self.held_polls.set(self.held_polls.get().saturating_sub(1));
            if let Some(permit) = self.cancel_on_pause { permit.cancel_before_injection(); }
        }
    }
    #[test] fn a_stop_key_released_during_brief_wait_delivers_once_after_revalidation() {
        let permit = DeliveryPermit::from_committed_attempt("release-test".into(), 1);
        let ops = Waiting { fake: fake(0), held_polls: 2.into(), pauses: 0.into(), cancel_on_pause: None };
        let receipt = dispatch_with(&ops, &permit, &snapshot(), "known fixture", || true);
        assert_eq!(ops.pauses.get(), 2);
        assert_eq!((ops.fake.checks.get(), ops.fake.copies.get(), ops.fake.chords.get()), (2, 1, 1));
        assert_eq!(receipt.reason_code.as_deref(), Some("inputDispatchedUnconfirmed"));
        let again = dispatch_with(&ops, &permit, &snapshot(), "known fixture", || true);
        assert_eq!(again.reason_code.as_deref(), Some("attemptConsumed"));
        assert_eq!(ops.fake.chords.get(), 1);
    }
    #[test] fn cancellation_while_waiting_never_changes_clipboard_or_injects() {
        let permit = DeliveryPermit::from_committed_attempt("wait-cancel".into(), 1);
        let ops = Waiting { fake: fake(0), held_polls: 2.into(), pauses: 0.into(), cancel_on_pause: Some(&permit) };
        let receipt = dispatch_with(&ops, &permit, &snapshot(), "known fixture", || true);
        assert_eq!(receipt.reason_code.as_deref(), Some("cancelledBeforeInjection"));
        assert!(!receipt.clipboard_changed);
        assert_eq!((ops.fake.copies.get(), ops.fake.chords.get()), (0, 0));
    }
    #[test] fn target_change_during_modifier_wait_is_rechecked_before_clipboard() {
        let permit = DeliveryPermit::from_committed_attempt("wait-target".into(), 1);
        let ops = Waiting { fake: fake(1), held_polls: 2.into(), pauses: 0.into(), cancel_on_pause: None };
        let receipt = dispatch_with(&ops, &permit, &snapshot(), "known fixture", || true);
        assert_eq!(receipt.reason_code.as_deref(), Some("targetChangedBeforeDelivery"));
        assert_eq!((ops.fake.copies.get(), ops.fake.chords.get()), (0, 0));
    }
    #[test] fn modifier_hold_is_bounded_and_never_forcibly_released() {
        struct Held(Fake<'static>);
        impl NativeOperations for Held {
            fn validate(&self, target: &TargetSnapshot) -> Result<()> { self.0.validate(target) }
            fn modifiers_released(&self) -> bool { false }
            fn copy(&self, text: &str) -> std::result::Result<bool, ClipboardError> { self.0.copy(text) }
            fn chord(&self) -> u32 { self.0.chord() }
        }
        let permit = DeliveryPermit::from_committed_attempt("held-test".into(), 1);
        let ops = Held(fake(0)); let started = Instant::now();
        let receipt = dispatch_with(&ops, &permit, &snapshot(), "known fixture", || true);
        assert_eq!(receipt.reason_code.as_deref(), Some("modifiersNotReleased"));
        assert!(started.elapsed() >= Duration::from_millis(350));
        assert_eq!((ops.0.copies.get(), ops.0.chords.get()), (0, 0));
        assert!(permit.consumed());
    }
    #[test] fn clipboard_failure_has_stable_reason_and_cannot_retry() {
        struct ClipboardFails(Fake<'static>);
        impl NativeOperations for ClipboardFails {
            fn validate(&self, target: &TargetSnapshot) -> Result<()> { self.0.validate(target) }
            fn modifiers_released(&self) -> bool { true }
            fn copy(&self, _: &str) -> std::result::Result<bool, ClipboardError> { Err(ClipboardError { cleared: false, reason: "fixture busy".into() }) }
            fn chord(&self) -> u32 { self.0.chord() }
        }
        let permit = DeliveryPermit::from_committed_attempt("copy-failure".into(), 1);
        let ops = ClipboardFails(fake(0));
        let receipt = dispatch_with(&ops, &permit, &snapshot(), "known fixture", || true);
        assert_eq!(receipt.reason_code.as_deref(), Some("clipboardUnavailable"));
        assert!(matches!(receipt.outcome, DispatchOutcome::Failed));
        assert_eq!(ops.0.chords.get(), 0);
        assert_eq!(dispatch_with(&ops, &permit, &snapshot(), "known fixture", || true).reason_code.as_deref(), Some("attemptConsumed"));
    }
}

#[cfg(target_os = "macos")]
#[path = "macos/delivery.rs"]
mod native;
