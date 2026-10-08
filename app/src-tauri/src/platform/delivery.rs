//! Fail-closed external delivery. Every native mutation is an explicit call.
//! The core must durably record an attempt before constructing its permit.
use super::{PlatformError, Result};
use serde::{Serialize, Deserialize};
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TargetSnapshot {
    pub hwnd: usize,
    pub process_id: u32,
    pub focused_hwnd: usize,
    pub runtime_id: Vec<i32>,
    /// UTF-16 offsets, obtained from native Edit selection or UIA selection. No
    /// target text is stored here, logged, or forwarded to cloud requests.
    pub selection: Option<(usize, usize)>,
    pub process_created_at: u64,
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
pub fn validate_target(snapshot: &TargetSnapshot) -> Result<()> {
    let current = capture_target()?;
    compare_targets(snapshot, &current)
}
fn compare_targets(before: &TargetSnapshot, now: &TargetSnapshot) -> Result<()> {
    if before.selection.is_none() || before.runtime_id.is_empty() { return Err(PlatformError::Unavailable("input target/caret cannot be verified".into())); }
    if before != now { return Err(PlatformError::Unavailable("original target, caret, or process identity changed".into())); }
    Ok(())
}
fn compare_anchor(anchor: &ForegroundAnchor, target: &TargetSnapshot) -> Result<()> {
    if anchor.hwnd != target.hwnd || anchor.process_id != target.process_id ||
        anchor.process_created_at != target.process_created_at || anchor.focused_hwnd != target.focused_hwnd ||
        anchor.selection.is_some_and(|selection| target.selection != Some(selection)) {
        return Err(PlatformError::Unavailable("stop target identity or caret changed before inspection".into()));
    }
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
    if let Err(error) = ops.validate(target) { return DispatchReceipt::skipped("targetChangedBeforeDelivery", error.to_string(), false); }
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
    if let Err(error) = ops.validate(target) { return DispatchReceipt::skipped("targetChangedBeforeDelivery", error.to_string(), changed); }
    if !gate() || !ops.modifiers_released() || !permit.begin_injection() {
        let code = if !gate() || permit.stage.load(Ordering::SeqCst) != 1 { "cancelledBeforeInjection" } else { "modifiersNotReleased" };
        return DispatchReceipt::skipped(code, "cancelled/target gate invalid before native injection", changed);
    }
    let count = ops.chord();
    DispatchReceipt { outcome: if count == 0 { DispatchOutcome::Failed } else { DispatchOutcome::Unconfirmed },
        clipboard_changed: changed, input_events: count,
        reason_code: Some(if count == 4 { "inputDispatchedUnconfirmed" } else if count == 0 { "nativeDispatchFailed" } else { "partialInputUnconfirmed" }.into()),
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
    if style & 0x0020 != 0 { return Err(PlatformError::Unavailable("password input control".into())); }
    if style & 0x0800 != 0 { return Err(PlatformError::Unavailable("read-only input control".into())); }
    if style & 0x0800_0000 != 0 { return Err(PlatformError::Unavailable("disabled input control".into())); }
    Ok(())
}
#[cfg(windows)]
fn explicit_writable_text_attribute(value: &windows::core::VARIANT) -> bool {
    // VARIANT equality first requires the exact type. Do not use
    // VariantToBoolean: VT_EMPTY, numbers, BSTRs or reserved IUnknown values
    // could be coerced, and unknown/mixed attributes must never mean writable.
    value == &windows::core::VARIANT::from(false)
}

struct ClipboardError { cleared: bool, reason: String }

#[cfg(windows)]
mod native {
    use super::*;
    use std::{mem::size_of, thread};
    use windows::core::{PCWSTR, Interface};
    use windows::Win32::Foundation::{HANDLE, HWND, FILETIME, CloseHandle, GlobalFree, WPARAM, LPARAM};
    use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, CoCreateInstance, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, SAFEARRAY};
    use windows::Win32::System::DataExchange::{OpenClipboard, CloseClipboard, EmptyClipboard, SetClipboardData};
    use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
    use windows::Win32::System::Ole::{SafeArrayAccessData, SafeArrayUnaccessData, SafeArrayDestroy, SafeArrayGetLBound, SafeArrayGetUBound, SafeArrayGetDim};
    use windows::Win32::System::Threading::{OpenProcess, GetCurrentProcess, GetCurrentProcessId, GetProcessTimes, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION};
    use windows::Win32::Security::{GetTokenInformation, GetSidSubAuthority, GetSidSubAuthorityCount, TokenIntegrityLevel, TOKEN_MANDATORY_LABEL, TOKEN_QUERY};
    use windows::Win32::UI::Accessibility::{CUIAutomation8, IUIAutomation, IUIAutomation2, IUIAutomationTextPattern, IUIAutomationValuePattern,
        UIA_EditControlTypeId, UIA_DocumentControlTypeId, UIA_GroupControlTypeId, UIA_IsReadOnlyAttributeId,
        UIA_TextPatternId, UIA_ValuePatternId, TextPatternRangeEndpoint_Start, TextPatternRangeEndpoint_End};
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP,
        VIRTUAL_KEY, VK_CONTROL, VK_SHIFT, VK_MENU, VK_LWIN, VK_RWIN, IsWindowEnabled};
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId, GetGUIThreadInfo, GUITHREADINFO,
        CreateWindowExW, DestroyWindow, WINDOW_EX_STYLE, WINDOW_STYLE,
        GetClassNameW, GetWindowLongPtrW, GWL_STYLE, GetAncestor, GA_ROOT,
        SendMessageTimeoutW, SMTO_ABORTIFHUNG, SMTO_BLOCK, SMTO_ERRORONEXIT};

    struct ComGuard;
    impl Drop for ComGuard { fn drop(&mut self) { unsafe { CoUninitialize(); } } }
    fn fail(e: impl std::fmt::Display) -> PlatformError { PlatformError::Unavailable(e.to_string()) }
    struct ArrayGuard(*mut SAFEARRAY);
    impl Drop for ArrayGuard { fn drop(&mut self) { unsafe { let _ = SafeArrayDestroy(self.0); } } }
    struct HandleGuard(HANDLE);
    impl Drop for HandleGuard { fn drop(&mut self) { unsafe { let _ = CloseHandle(self.0); } } }
    unsafe fn integrity_level(process: HANDLE) -> Result<u32> {
        let mut token = HANDLE::default();
        unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token).map_err(fail)?; }
        let _token = HandleGuard(token);
        let mut size = 0;
        let _ = unsafe { GetTokenInformation(token, TokenIntegrityLevel, None, 0, &mut size) };
        if size == 0 || size > 16_384 { return Err(fail("process integrity is unavailable")); }
        let mut storage = vec![0usize; (size as usize).div_ceil(size_of::<usize>())];
        unsafe { GetTokenInformation(token, TokenIntegrityLevel, Some(storage.as_mut_ptr().cast()), size, &mut size).map_err(fail)?; }
        let label = unsafe { &*storage.as_ptr().cast::<TOKEN_MANDATORY_LABEL>() };
        let count = unsafe { *GetSidSubAuthorityCount(label.Label.Sid) };
        if count == 0 { return Err(fail("invalid process integrity SID")); }
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
            return Err(fail("native input owner changed"));
        }
        if !unsafe { IsWindowEnabled(focus) }.as_bool() { return Err(fail("disabled input control")); }
        writable_native_style(unsafe { GetWindowLongPtrW(focus, GWL_STYLE) } as u32)?;
        // EM_GETSEL is a system message: Windows marshals the two DWORD
        // outputs across processes. It obtains offsets without reading text.
        // Unlike EM_EXGETSEL (WM_USER + 52), it needs no remote allocation.
        let (mut start, mut end) = (0u32, 0u32);
        let mut result = 0usize;
        let accepted = unsafe { SendMessageTimeoutW(focus, 0x00B0,
            WPARAM((&mut start as *mut u32) as usize), LPARAM((&mut end as *mut u32) as isize),
            SMTO_ABORTIFHUNG | SMTO_BLOCK | SMTO_ERRORONEXIT, 200, Some(&mut result)) };
        if accepted.0 == 0 { return Err(fail("native input selection unavailable or timed out")); }
        if end < start { return Err(fail("native input selection is invalid")); }
        let raw = focus.0 as usize;
        // Native edit controls already have an OS focus identity. This stable
        // namespace is distinct from UIA provider runtime IDs; validation also
        // checks root HWND, PID, process lifetime, class/style and caret again.
        let class_hash = class_name.to_ascii_lowercase().bytes().fold(0x811c9dc5u32, |h, b| (h ^ u32::from(b)).wrapping_mul(0x01000193));
        Ok(Some((vec![0x534d_4e41, raw as i32, ((raw as u64) >> 32) as i32, class_hash as i32], (start as usize, end as usize))))
    }

    pub fn capture_bounded() -> Result<TargetSnapshot> {
        capture_bounded_inner(None)
    }
    pub fn capture_bounded_for_anchor(anchor: ForegroundAnchor) -> Result<TargetSnapshot> {
        capture_bounded_inner(Some(anchor))
    }
    fn capture_bounded_inner(anchor: Option<ForegroundAnchor>) -> Result<TargetSnapshot> {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        thread::spawn(move || { let _ = tx.send(capture_with_anchor(anchor)); });
        rx.recv_timeout(Duration::from_secs(1)).map_err(|_| PlatformError::Timeout("input target validation"))?
    }
    pub fn capture_anchor() -> Result<ForegroundAnchor> {
        unsafe {
            let hwnd = GetForegroundWindow();
            if hwnd.0.is_null() { return Err(fail("no foreground window at stop intent")); }
            let mut process_id = 0;
            let thread_id = GetWindowThreadProcessId(hwnd, Some(&mut process_id));
            if process_id == 0 { return Err(fail("foreground process identity unavailable at stop intent")); }
            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id).map_err(fail)?;
            let _process = HandleGuard(process);
            // Preserve the typed permission reason before EM_GETSEL or UIA can
            // fail with a generic error on a higher-integrity host.
            ensure_delivery_integrity(process)?;
            let process_created_at = process_created_at(process)?;
            let mut gui = GUITHREADINFO { cbSize: size_of::<GUITHREADINFO>() as u32, ..Default::default() };
            GetGUIThreadInfo(thread_id, &mut gui).map_err(fail)?;
            let selection = native_edit_selection(hwnd, gui.hwndFocus, process_id)?.map(|(_, selection)| selection);
            let mut final_gui = GUITHREADINFO { cbSize: size_of::<GUITHREADINFO>() as u32, ..Default::default() };
            GetGUIThreadInfo(thread_id, &mut final_gui).map_err(fail)?;
            if GetForegroundWindow() != hwnd || final_gui.hwndFocus != gui.hwndFocus {
                return Err(fail("foreground target changed while freezing stop intent"));
            }
            Ok(ForegroundAnchor { hwnd: hwnd.0 as usize, process_id, process_created_at,
                focused_hwnd: gui.hwndFocus.0 as usize, selection })
        }
    }
    fn capture_with_anchor(anchor: Option<ForegroundAnchor>) -> Result<TargetSnapshot> {
        if let Some(expected) = anchor {
            if capture_anchor()? != expected { return Err(fail("stop target changed before input inspection")); }
        }
        let target = capture()?;
        if let Some(expected) = anchor {
            compare_anchor(&expected, &target)?;
            if capture_anchor()? != expected { return Err(fail("stop target changed during input inspection")); }
        }
        Ok(target)
    }
    fn capture() -> Result<TargetSnapshot> {
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED).ok().map_err(fail)?;
            let _com = ComGuard;
            let hwnd = GetForegroundWindow();
            if hwnd.0.is_null() { return Err(fail("no foreground window")); }
            let mut process_id = 0;
            let thread_id = GetWindowThreadProcessId(hwnd, Some(&mut process_id));
            if process_id == 0 || process_id == GetCurrentProcessId() { return Err(fail("own/unknown window is not an external input target")); }
            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id).map_err(fail)?;
            let _process = HandleGuard(process);
            ensure_delivery_integrity(process)?;
            let mut gui = GUITHREADINFO { cbSize: size_of::<GUITHREADINFO>() as u32, ..Default::default() };
            GetGUIThreadInfo(thread_id, &mut gui).map_err(fail)?;
            if let Some((runtime_id, selection)) = native_edit_selection(hwnd, gui.hwndFocus, process_id)? {
                let created = process_created_at(process)?;
                let mut final_gui = GUITHREADINFO { cbSize: size_of::<GUITHREADINFO>() as u32, ..Default::default() };
                GetGUIThreadInfo(thread_id, &mut final_gui).map_err(fail)?;
                if GetForegroundWindow() != hwnd || final_gui.hwndFocus != gui.hwndFocus {
                    return Err(fail("native input field changed during target inspection"));
                }
                return Ok(TargetSnapshot { hwnd: hwnd.0 as usize, process_id, focused_hwnd: gui.hwndFocus.0 as usize,
                    runtime_id, selection: Some(selection), process_created_at: created });
            }
            let automation: IUIAutomation = CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER).map_err(fail)?;
            if let Ok(options) = automation.cast::<IUIAutomation2>() {
                options.SetConnectionTimeout(200).map_err(fail)?;
                options.SetTransactionTimeout(200).map_err(fail)?;
            }
            let focused = automation.GetFocusedElement().map_err(fail)?;
            if focused.CurrentProcessId().map_err(fail)? as u32 != process_id || focused.CurrentIsPassword().map_err(fail)?.as_bool() || !focused.CurrentIsEnabled().map_err(fail)?.as_bool() {
                return Err(fail("password/disabled/foreign input control"));
            }
            let control_type = focused.CurrentControlType().map_err(fail)?;
            if control_type != UIA_EditControlTypeId && control_type != UIA_DocumentControlTypeId && control_type != UIA_GroupControlTypeId { return Err(fail("unknown non-editable control")); }
            if control_type == UIA_GroupControlTypeId && !focused.CurrentIsKeyboardFocusable().map_err(fail)?.as_bool() {
                return Err(fail("non-focusable input group"));
            }
            let text = focused.GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId).map_err(fail)?;
            if let Ok(value) = focused.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId) {
                if value.CurrentIsReadOnly().map_err(fail)?.as_bool() { return Err(fail("read-only input control")); }
            } else {
                // Chromium contenteditable is a UIA Group with TextPattern,
                // without ValuePattern. Read the provider's explicit readonly
                // text attribute; a readable Group/Document alone proves nothing.
                let readonly = text.DocumentRange().map_err(fail)?.GetAttributeValue(UIA_IsReadOnlyAttributeId).map_err(fail)?;
                if !explicit_writable_text_attribute(&readonly) {
                    return Err(fail("input is read-only or writable text capability unavailable/mixed"));
                }
            }
            let array = focused.GetRuntimeId().map_err(fail)?;
            let _array = ArrayGuard(array);
            if array.is_null() || SafeArrayGetDim(array) != 1 { return Err(fail("invalid input runtime identity")); }
            let lower = SafeArrayGetLBound(array, 1).map_err(fail)?;
            let upper = SafeArrayGetUBound(array, 1).map_err(fail)?;
            let count = (upper - lower + 1).max(0) as usize;
            if count == 0 || count > 256 { return Err(fail("input runtime identity unavailable/oversized")); }
            let mut data = std::ptr::null_mut();
            SafeArrayAccessData(array, &mut data).map_err(fail)?;
            let runtime_id = std::slice::from_raw_parts(data.cast::<i32>(), count).to_vec();
            let _ = SafeArrayUnaccessData(array);
            let selections = text.GetSelection().map_err(fail)?;
            if selections.Length().map_err(fail)? != 1 { return Err(fail("input selection cannot be verified")); }
            let selection = selections.GetElement(0).map_err(fail)?;
            let prefix = text.DocumentRange().map_err(fail)?;
            prefix.MoveEndpointByRange(TextPatternRangeEndpoint_End, &selection, TextPatternRangeEndpoint_Start).map_err(fail)?;
            // This transient prefix is only needed to count UTF-16 caret
            // offset. Never return/store/log it. Bound unusually large fields.
            let value = prefix.GetText(65_537).map_err(fail)?;
            let start = value.len();
            if start > 65_536 { return Err(fail("target text exceeds bounded caret inspection")); }
            let selected = selection.GetText(65_537).map_err(fail)?;
            if selected.len() > 65_536 { return Err(fail("target selection too large")); }
            let created = process_created_at(process)?;
            if GetForegroundWindow() != hwnd { return Err(fail("foreground changed during target inspection")); }
            let final_focused = automation.GetFocusedElement().map_err(fail)?;
            if !automation.CompareElements(&focused, &final_focused).map_err(fail)?.as_bool() {
                return Err(fail("input field changed during target inspection"));
            }
            Ok(TargetSnapshot { hwnd: hwnd.0 as usize, process_id, focused_hwnd: gui.hwndFocus.0 as usize,
                runtime_id, selection: Some((start, start + selected.len())),
                process_created_at: created })
        }
    }

    pub fn modifiers_released() -> bool {
        [VK_CONTROL, VK_SHIFT, VK_MENU, VK_LWIN, VK_RWIN].iter().all(|vk| unsafe { GetAsyncKeyState(i32::from(vk.0)) } as u16 & 0x8000 == 0)
    }
    pub fn paste_chord() -> u32 {
        unsafe {
            let input = |vk: u16, up: bool| INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(vk), dwFlags: if up { KEYEVENTF_KEYUP } else { Default::default() }, dwExtraInfo: 0x534D_5041, ..Default::default() } } };
            let events = [input(VK_CONTROL.0, false), input(0x56, false), input(0x56, true), input(VK_CONTROL.0, true)];
            let count = SendInput(&events, size_of::<INPUT>() as i32);
            if count != 0 && count != 4 {
                // Release only keys introduced by this chord. Never repeat V-down.
                let cleanup = [input(0x56, true), input(VK_CONTROL.0, true)];
                let _ = SendInput(&cleanup, size_of::<INPUT>() as i32);
            }
            count
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

#[cfg(not(windows))]
mod native {
    use super::*;
    pub fn capture_bounded() -> Result<TargetSnapshot> { Err(PlatformError::Unavailable("Windows target required".into())) }
    pub fn capture_anchor() -> Result<ForegroundAnchor> { Err(PlatformError::Unavailable("Windows target required".into())) }
    pub fn capture_bounded_for_anchor(_: ForegroundAnchor) -> Result<TargetSnapshot> { Err(PlatformError::Unavailable("Windows target required".into())) }
    pub fn modifiers_released() -> bool { false }
    pub fn paste_chord() -> u32 { 0 }
    pub fn copy_text(_: &str) -> std::result::Result<bool, ClipboardError> { Err(ClipboardError { cleared: false, reason: "Windows clipboard required".into() }) }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn snapshot() -> TargetSnapshot { TargetSnapshot { hwnd: 10, process_id: 20, focused_hwnd: 30, runtime_id: vec![1,2,3], selection: Some((4,4)), process_created_at: 50 } }
    #[test] fn rejects_different_field_in_same_window_and_changed_caret() {
        let before = snapshot(); let mut now = before.clone(); now.runtime_id.push(4);
        assert!(compare_targets(&before, &now).is_err()); now = before.clone(); now.selection = Some((5,5));
        assert!(compare_targets(&before, &now).is_err()); assert!(compare_targets(&before, &before).is_ok());
    }
    #[test] fn process_reuse_does_not_reauthorize_old_target() {
        let before = snapshot(); let mut now = before.clone(); now.process_created_at += 1;
        assert!(compare_targets(&before, &now).is_err());
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
    #[test] fn text_only_writable_requires_exact_false_bool_not_coercible_or_empty_variant() {
        use windows::core::VARIANT;
        assert!(explicit_writable_text_attribute(&VARIANT::from(false)));
        assert!(!explicit_writable_text_attribute(&VARIANT::from(true)));
        assert!(!explicit_writable_text_attribute(&VARIANT::new()));
        assert!(!explicit_writable_text_attribute(&VARIANT::from(0i32)));
        assert!(!explicit_writable_text_attribute(&VARIANT::from("false")));
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
