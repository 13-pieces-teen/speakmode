//! Windows low-level keyboard hook implementation

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::collections::HashMap;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::RemoteDesktop::{
    WTSRegisterSessionNotification, WTSUnRegisterSessionNotification,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS,
    KEYEVENTF_KEYUP, KEYEVENTF_EXTENDEDKEY, VIRTUAL_KEY, VK_LCONTROL, VK_LMENU, VK_LSHIFT, VK_LWIN, VK_RCONTROL, VK_RMENU,
    VK_RSHIFT, VK_RWIN,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
    MsgWaitForMultipleObjects, PeekMessageW, RegisterClassW, SetWindowsHookExW, TranslateMessage,
    UnhookWindowsHookEx, HHOOK, KBDLLHOOKSTRUCT, LLKHF_EXTENDED, MSG, MSLLHOOKSTRUCT, PM_REMOVE,
    QS_ALLINPUT, WH_KEYBOARD_LL, WH_MOUSE_LL, WINDOW_EX_STYLE, WINDOW_STYLE, WM_KEYDOWN,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_POWERBROADCAST, WM_QUIT,
    WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SYSKEYDOWN, WM_XBUTTONDOWN, WM_XBUTTONUP, WNDCLASSW,
};

use crate::error::{Error, Result};
use crate::listener::{ObservedEvent, KeyOrigin, Lifecycle, StandaloneTapMaskControl, StandaloneTapMenuMask,
    standalone_menu_release_batch, standalone_release_disposition, StandaloneReleaseDisposition,
    observed_keyboard_event_is_physical, standalone_tap_has_other_modifier};
use crate::platform::state::{release_events, stale_modifiers, BlockingHotkeys, RECONCILABLE};
use crate::types::{Key, KeyEvent, Modifiers};

use super::keycode::{is_altgr_phantom_ctrl, map_key, vk_to_modifier};

const HOOK_LOOP_TIMEOUT_MS: u32 = 10;

/// Modifiers whose lone press-and-release makes the shell activate a menu:
/// Win opens the Start menu, Alt focuses the active window's menu bar.
const MENU_MODIFIERS: Modifiers = Modifiers::CMD.union(Modifiers::OPT);

/// Unassigned virtual key injected as the "menu mask" (AutoHotkey's default
/// mask key). The shell arms its menu when a Win/Alt down→up pair passes
/// with no other key in between; when we swallow the real key of a Win/Alt
/// hotkey, this inert keystroke is substituted so the modifier release no
/// longer reads as a lone tap (cjpais/Handy#917). The VK maps to nothing in
/// this crate (`vk_to_modifier` and `map_key` both return None), so even
/// without the dwExtraInfo marker it could never be blocked on its way to
/// the shell.
const MENU_MASK_VK: u16 = 0xE8;

// WTS session notification plumbing not exposed by the `windows` crate bindings.
const NOTIFY_FOR_THIS_SESSION: u32 = 0;
const WM_WTSSESSION_CHANGE: u32 = 0x02B1;
// WM_WTSSESSION_CHANGE wParam values (wtsapi32.h).
const WTS_CONSOLE_CONNECT: usize = 0x1;
const WTS_REMOTE_CONNECT: usize = 0x3;
const WTS_SESSION_LOCK: usize = 0x7;
const WTS_SESSION_UNLOCK: usize = 0x8;
// WM_POWERBROADCAST wParam values (winuser.h) -- the PBT_* constants live
// behind the Win32_System_Power feature, not worth enabling for two values.
// RESUMEAUTOMATIC fires on every wake; RESUMESUSPEND follows it only when the
// wake was user-initiated.
const PBT_APMSUSPEND: usize = 0x4;
const PBT_APMRESUMESUSPEND: usize = 0x7;
const PBT_APMRESUMEAUTOMATIC: usize = 0x12;

/// The side-specific modifier keys we track, paired with their virtual-key
/// codes. Ordered to match `state::MODIFIER_RELEASE_ORDER`.
const MODIFIER_KEYS: [(VIRTUAL_KEY, Modifiers); 8] = [
    (VK_LWIN, Modifiers::CMD_LEFT),
    (VK_RWIN, Modifiers::CMD_RIGHT),
    (VK_LSHIFT, Modifiers::SHIFT_LEFT),
    (VK_RSHIFT, Modifiers::SHIFT_RIGHT),
    (VK_LCONTROL, Modifiers::CTRL_LEFT),
    (VK_RCONTROL, Modifiers::CTRL_RIGHT),
    (VK_LMENU, Modifiers::OPT_LEFT),
    (VK_RMENU, Modifiers::OPT_RIGHT),
];

/// Thread-local state for the keyboard hook callback.
///
/// Windows low-level hooks require a callback function with a specific signature,
/// so we use thread-local storage to access our state from within the callback.
struct HookContext {
    event_sender: Sender<KeyEvent>,
    observed_sender: Option<Sender<ObservedEvent>>,
    current_modifiers: Modifiers,
    pressed_keys: HashMap<u16, Key>,
    blocking_hotkeys: Option<BlockingHotkeys>,
    /// AltGr's phantom Left Ctrl is currently held. The hook drops the
    /// phantom's own events, but GetAsyncKeyState still reports LCtrl as
    /// down while AltGr is held, so reconciliation must not adopt it.
    altgr_phantom_ctrl: bool,
    /// A menu mask has been injected for the current Win/Alt hold. One per
    /// hold defuses the shell's menu heuristic for the whole hold; cleared
    /// once no Win/Alt modifier remains held.
    menu_mask_sent: bool,
    standalone_tap: Option<StandaloneTapMenuMask>,
}

thread_local! {
    static HOOK_CONTEXT: std::cell::RefCell<Option<HookContext>> = const { std::cell::RefCell::new(None) };

    /// Set by `watcher_wndproc` when a message delivered via SendMessage
    /// (bypassing the queue) asks for a hook re-install. The message loop
    /// owns the hook handles, so the wndproc can only request the re-install,
    /// not perform it. Thread-local: the wndproc runs on the hook thread, and
    /// a second listener in the same process must not see this one's requests.
    static REINSTALL_REQUESTED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Take (and clear) a pending hook re-install request from the wndproc.
fn take_reinstall_request() -> bool {
    REINSTALL_REQUESTED.with(|flag| flag.replace(false))
}

/// What draining the thread message queue observed.
#[derive(Default)]
struct DrainOutcome {
    /// WM_QUIT received -- exit the message loop.
    quit: bool,
    /// A session change (lock/unlock/connect) or a resume from sleep
    /// occurred -- reconcile modifier state, since key-ups on the secure
    /// desktop (or from before a suspend) never reach the hook.
    reconcile: bool,
    /// The interactive desktop came back (unlock, console/remote connect, or
    /// resume from sleep) -- re-install hooks in case Windows silently
    /// removed them.
    reinstall_hooks: bool,
}

/// Drain all pending thread messages.
fn drain_thread_messages(msg: &mut MSG) -> DrainOutcome {
    let mut outcome = DrainOutcome::default();
    unsafe {
        while PeekMessageW(msg, None, 0, 0, PM_REMOVE).as_bool() {
            if msg.message == WM_QUIT {
                outcome.quit = true;
                return outcome;
            }
            if msg.message == WM_WTSSESSION_CHANGE {
                match msg.wParam.0 {
                    WTS_SESSION_LOCK => outcome.reconcile = true,
                    WTS_SESSION_UNLOCK | WTS_CONSOLE_CONNECT | WTS_REMOTE_CONNECT => {
                        outcome.reconcile = true;
                        outcome.reinstall_hooks = true;
                    }
                    _ => {}
                }
            }
            // WM_POWERBROADCAST is documented as sent, not posted, so the
            // wndproc is the expected delivery path; this arm is a cheap
            // safety net in case some Windows build posts it instead.
            if msg.message == WM_POWERBROADCAST
                && matches!(msg.wParam.0, PBT_APMRESUMESUSPEND | PBT_APMRESUMEAUTOMATIC)
            {
                outcome.reconcile = true;
                outcome.reinstall_hooks = true;
            }
            let _ = TranslateMessage(msg);
            DispatchMessageW(msg);
        }
    }
    outcome
}

/// Wait for new input/messages or until timeout expires.
fn wait_for_message_or_timeout(timeout_ms: u32) {
    unsafe {
        let _ = MsgWaitForMultipleObjects(None, false, timeout_ms, QS_ALLINPUT);
    }
}

/// Snapshot which tracked modifier keys are physically held right now.
///
/// Note: if the calling thread's desktop is not active (e.g. the lock screen's
/// secure desktop is up), GetAsyncKeyState reports every key as up -- which is
/// the correct answer for our purposes: treat everything as released.
fn physical_modifiers() -> Modifiers {
    let mut held = Modifiers::empty();
    for (vk, modifier) in MODIFIER_KEYS {
        // High bit set = key currently down.
        if unsafe { GetAsyncKeyState(vk.0 as i32) } as u16 & 0x8000 != 0 {
            held |= modifier;
        }
    }
    held
}

/// Physically-held modifiers we missed the press of and should adopt.
/// (`stale_modifiers`/`release_events`, the other half of reconciliation,
/// live in crate::platform::state, shared with the Linux listener.)
///
/// While AltGr's phantom Left Ctrl is held, CTRL_LEFT is excluded: it is
/// physically down per GetAsyncKeyState, but the user only pressed Right
/// Alt. A real Left Ctrl press during that window still sets CTRL_LEFT
/// through its own hook event (which carries a non-phantom scancode).
fn adoptable_modifiers(
    tracked: Modifiers,
    physical: Modifiers,
    altgr_phantom_ctrl: bool,
) -> Modifiers {
    let mut adopt = physical & RECONCILABLE & !tracked;
    if altgr_phantom_ctrl {
        adopt &= !Modifiers::CTRL_LEFT;
    }
    adopt
}

/// Reconcile tracked modifiers against the physical keyboard state.
///
/// Corrects drift from missed events: secure desktop transitions swallow
/// key-ups (Win+L delivers the Win key DOWN but its UP happens on the secure
/// desktop), leaving modifiers stuck on until the user happens to press them
/// again. Mirrors `reconcile_modifiers` in the macOS listener.
///
/// Stale modifiers are cleared with synthetic release events so consumers
/// tracking press/release state recover; missed presses are adopted silently
/// and ride along on the next real event.
///
/// Known micro-race: GetAsyncKeyState reads the state *now*, not at the time
/// the event being processed was generated, so a modifier pressed in the
/// sub-millisecond window between a keystroke entering the queue and its hook
/// callback running gets adopted onto that earlier keystroke's event. It is
/// self-correcting (the modifier's own event arrives right after) and
/// unavoidable with this API — macOS avoids it because CGEvent flags are
/// stamped per event.
fn reconcile_modifiers(ctx: &mut HookContext) {
    let physical = physical_modifiers();
    // Phantom Ctrl only exists while AltGr is held; if LCtrl reads as up,
    // the phantom is gone (covers a phantom key-up swallowed by a secure
    // desktop transition, which would otherwise suppress CTRL_LEFT adoption
    // forever).
    if ctx.altgr_phantom_ctrl && !physical.contains(Modifiers::CTRL_LEFT) {
        ctx.altgr_phantom_ctrl = false;
    }
    for event in release_events(
        ctx.current_modifiers,
        stale_modifiers(ctx.current_modifiers, physical),
    ) {
        if let Some(mask)=ctx.standalone_tap.as_mut(){mask.reset();}
        ctx.current_modifiers = event.modifiers;
        emit_key(ctx, event, KeyOrigin::Reconciled);
    }
    ctx.current_modifiers |=
        adoptable_modifiers(ctx.current_modifiers, physical, ctx.altgr_phantom_ctrl);
    // A Win/Alt-up swallowed by a secure desktop transition ends the hold
    // without the modifier branch observing it; clear the mask flag here so
    // the next hold gets its own mask.
    if !ctx.current_modifiers.intersects(MENU_MODIFIERS) {
        ctx.menu_mask_sent = false;
    }
}

/// Run only after draining native messages, never inside a key hook: async
/// state does not include the in-flight hook event. A lost key-up must cancel
/// the gesture, not leave ShortcutTap's held set blocking every later F8 tap.
/// Only release observed keys; synthetic paste chords must not add presses.
fn reconcile_released_keys(ctx: &mut HookContext, is_down: impl Fn(u16) -> bool) {
    let released = ctx.pressed_keys.iter().filter(|(vk, _)| !observed_key_is_down(**vk, &is_down))
        .map(|(vk, key)| (*vk, *key)).collect::<Vec<_>>();
    for (vk, key) in released {
        ctx.pressed_keys.remove(&vk);
        if let Some(mask) = ctx.standalone_tap.as_mut() { mask.reset(); }
        emit_key(ctx, KeyEvent { modifiers: ctx.current_modifiers, key: Some(key),
            is_key_down: false, changed_modifier: None }, KeyOrigin::Reconciled);
    }
}

fn track_key(ctx: &mut HookContext, vk: u16, key: Key, down: bool) -> Key {
    if down { *ctx.pressed_keys.entry(vk).or_insert(key) }
    else { ctx.pressed_keys.remove(&vk).unwrap_or(key) }
}

fn observed_key_is_down(vk: u16, is_down: &impl Fn(u16) -> bool) -> bool {
    // Async mouse state is physical. Keep primary-button holds until BOTH
    // buttons are up, independent of swapping or remapping during a hold.
    // A real mouse-up still removes its entry immediately in track_key.
    if matches!(vk, 0x01 | 0x02) { is_down(0x01) || is_down(0x02) }
    else { is_down(vk) }
}

fn emit_key(ctx: &HookContext, event: KeyEvent, origin: KeyOrigin) {
    emit_key_at(ctx,event,origin,std::time::Instant::now());
}
fn emit_key_at(ctx: &HookContext, event: KeyEvent, origin: KeyOrigin, at: std::time::Instant) {
    if let Some(sender) = &ctx.observed_sender {
        let _ = sender.send(ObservedEvent::Key { event, origin, at });
    } else {
        let _ = ctx.event_sender.send(event);
    }
}

fn lifecycle_event(event: Lifecycle) {
    super::activity::invalidate();
    HOOK_CONTEXT.with(|cell| {
        if let Some(ctx) = cell.borrow_mut().as_mut() {
            // Invalidate at the native event boundary, before a queued worker
            // can observe a later physical release after lock/suspend/resume.
            if let Some(mask)=ctx.standalone_tap.as_mut(){mask.disable();}
            if let Some(sender) = &ctx.observed_sender {
                let _ = sender.send(ObservedEvent::Lifecycle(event));
            }
        }
    });
}

/// Reconcile modifier state from the hook thread's message loop (used on
/// session change and resume from sleep, where no input event accompanies
/// the state change).
fn reconcile_modifiers_in_context() {
    HOOK_CONTEXT.with(|ctx_cell| {
        if let Some(ctx) = ctx_cell.borrow_mut().as_mut() {
            reconcile_modifiers(ctx);
        }
    });
}

/// Wndproc for the watcher window. (The `windows` crate's DefWindowProcW is
/// a generic Rust wrapper, so it cannot be used as lpfnWndProc directly.)
///
/// Handles both delivery modes together with the drain loop: the drain loop
/// covers posted delivery of WM_WTSSESSION_CHANGE (what Windows 11 26200
/// does, verified live), while this path covers builds that deliver it via
/// SendMessage, which bypasses the message queue — and WM_POWERBROADCAST,
/// which is documented as always sent. Double handling on the posted path is
/// harmless: the second reconcile finds nothing stale, and re-install
/// requests coalesce through REINSTALL_REQUESTED. The re-install itself
/// stays on the message loop, which owns the hook handles; this wndproc only
/// requests it.
unsafe extern "system" fn watcher_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_WTSSESSION_CHANGE => match wparam.0 {
            WTS_SESSION_LOCK => {
                lifecycle_event(Lifecycle::Locked);
                reconcile_modifiers_in_context();
            },
            WTS_SESSION_UNLOCK | WTS_CONSOLE_CONNECT | WTS_REMOTE_CONNECT => {
                lifecycle_event(if wparam.0 == WTS_SESSION_UNLOCK { Lifecycle::Resumed } else { Lifecycle::Connected });
                reconcile_modifiers_in_context();
                REINSTALL_REQUESTED.with(|flag| flag.set(true));
            }
            _ => {}
        },
        // Resume from sleep/hibernate: Windows can silently drop LL hooks
        // across a suspend (Handy #1620), and key-ups from before the
        // suspend are long gone -- re-arm the hooks and reconcile.
        WM_POWERBROADCAST => {
            if wparam.0 == PBT_APMSUSPEND {
                lifecycle_event(Lifecycle::Suspended);
                reconcile_modifiers_in_context();
            }
            if matches!(wparam.0, PBT_APMRESUMESUSPEND | PBT_APMRESUMEAUTOMATIC) {
                lifecycle_event(Lifecycle::Resumed);
                reconcile_modifiers_in_context();
                REINSTALL_REQUESTED.with(|flag| flag.set(true));
            }
        }
        _ => {}
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

/// Create a hidden top-level window that watches WTS session changes and
/// power (suspend/resume) broadcasts. Top-level, not message-only, is load-
/// bearing: broadcast messages such as WM_POWERBROADCAST are never delivered
/// to message-only windows. Without WS_VISIBLE it has no taskbar or Alt-Tab
/// presence. WM_WTSSESSION_CHANGE is posted to it and picked out of the
/// queue by `drain_thread_messages` (verified live: lock 0x7 and unlock 0x8
/// both arrive); WM_POWERBROADCAST is sent and lands in `watcher_wndproc`.
///
/// Returns None on failure. Non-fatal: hooks still work, and stale modifiers
/// are still corrected lazily by `reconcile_modifiers` on the next key event.
unsafe fn create_watcher_window() -> Option<HWND> {
    let class_name: Vec<u16> = "HandyKeysSystemWatcher\0".encode_utf16().collect();
    let wnd_class = WNDCLASSW {
        lpfnWndProc: Some(watcher_wndproc),
        lpszClassName: PCWSTR(class_name.as_ptr()),
        ..Default::default()
    };
    // May fail with ERROR_CLASS_ALREADY_EXISTS (e.g. a second listener in the
    // same process); CreateWindowExW below still succeeds against the
    // existing class, so the result is deliberately ignored.
    RegisterClassW(&wnd_class);

    let hwnd = CreateWindowExW(
        WINDOW_EX_STYLE::default(),
        PCWSTR(class_name.as_ptr()),
        PCWSTR::null(),
        WINDOW_STYLE::default(),
        0,
        0,
        0,
        0,
        None,
        None,
        None,
        None,
    )
    .ok()?;

    if WTSRegisterSessionNotification(hwnd, NOTIFY_FOR_THIS_SESSION).is_err() {
        let _ = DestroyWindow(hwnd);
        return None;
    }

    Some(hwnd)
}

/// Clean up the watcher window. Must run on the creating thread.
unsafe fn destroy_watcher_window(hwnd: HWND) {
    let _ = WTSUnRegisterSessionNotification(hwnd);
    let _ = DestroyWindow(hwnd);
}

/// Re-install the low-level hooks, defensively: Windows silently removes an LL
/// hook whose callback exceeds its timeout budget, and a session away from the
/// interactive desktop or a suspend/resume cycle is a common moment for that
/// to surface (Handy #1620). (Note lock/unlock does NOT inherently invalidate
/// hooks -- they survived it in live testing.)
///
/// The replacements are installed before the old hooks are removed, so a
/// failure never leaves us hook-less. No messages are pumped between install
/// and unhook, so no event is delivered twice.
unsafe fn reinstall_hooks(kb_hook: &mut HHOOK, mouse_hook: &mut HHOOK) -> bool {
    let new_kb = match SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook_proc), None, 0) {
        Ok(h) => h,
        Err(_) => return false,
    };
    let new_mouse = match SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook_proc), None, 0) {
        Ok(h) => h,
        Err(_) => {
            let _ = UnhookWindowsHookEx(new_kb);
            return false;
        }
    };
    let _ = UnhookWindowsHookEx(*kb_hook);
    let _ = UnhookWindowsHookEx(*mouse_hook);
    *kb_hook = new_kb;
    *mouse_hook = new_mouse;
    true
}

/// Internal listener state returned to KeyboardListener
pub(crate) struct WindowsListenerState {
    pub event_receiver: mpsc::Receiver<KeyEvent>,
    pub observed_receiver: Option<mpsc::Receiver<ObservedEvent>>,
    pub thread_handle: Option<JoinHandle<()>>,
    pub running: Arc<AtomicBool>,
    pub blocking_hotkeys: Option<BlockingHotkeys>,
}

/// Spawn a Windows low-level keyboard hook listener
pub(crate) fn spawn(blocking_hotkeys: Option<BlockingHotkeys>) -> Result<WindowsListenerState> {
    spawn_internal(blocking_hotkeys, false, None)
}

pub(crate) fn spawn_observed(control: Option<StandaloneTapMaskControl>) -> Result<WindowsListenerState> {
    spawn_internal(None, true, control)
}

fn spawn_internal(blocking_hotkeys: Option<BlockingHotkeys>, observed: bool,
                  standalone_tap: Option<StandaloneTapMaskControl>) -> Result<WindowsListenerState> {
    let (tx, rx) = mpsc::channel();
    let (observe_tx, observe_rx) = mpsc::channel();
    let (ready_tx, ready_rx) = mpsc::sync_channel::<std::result::Result<(), String>>(1);
    let running = Arc::new(AtomicBool::new(true));
    let thread_running = Arc::clone(&running);
    let thread_blocking = blocking_hotkeys.clone();

    let handle = thread::spawn(move || {
        // Initialize thread-local hook context
        HOOK_CONTEXT.with(|ctx| {
            *ctx.borrow_mut() = Some(HookContext {
                event_sender: tx,
                observed_sender: observed.then_some(observe_tx),
                current_modifiers: Modifiers::empty(),
                pressed_keys: HashMap::new(),
                blocking_hotkeys: thread_blocking,
                altgr_phantom_ctrl: false,
                menu_mask_sent: false,
                standalone_tap: standalone_tap.map(StandaloneTapMenuMask::new),
            });
        });

        // Install the low-level keyboard hook
        let kb_hook =
            unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook_proc), None, 0) };

        let mut kb_hook = match kb_hook {
            Ok(h) => h,
            Err(e) => {
                let _ = ready_tx.send(Err(format!("keyboard hook: {e}")));
                return;
            }
        };

        // Install the low-level mouse hook
        let mouse_hook = unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook_proc), None, 0) };

        let mut mouse_hook = match mouse_hook {
            Ok(h) => h,
            Err(e) => {
                let _ = ready_tx.send(Err(format!("mouse hook: {e}")));
                // Clean up keyboard hook before returning
                unsafe {
                    let _ = UnhookWindowsHookEx(kb_hook);
                }
                return;
            }
        };

        // Watch for session changes (Win+L lock/unlock, RDP connect) and
        // suspend/resume: the secure desktop swallows key-up events, so
        // modifier state must be reconciled when the session comes back, and
        // Windows can silently drop LL hooks across a sleep, so they are
        // re-armed on resume.
        let watcher_hwnd = unsafe { create_watcher_window() };
        if watcher_hwnd.is_none() {
            let _ = ready_tx.send(Err("session/power watcher unavailable".into()));
            unsafe {
                let _ = UnhookWindowsHookEx(kb_hook);
                let _ = UnhookWindowsHookEx(mouse_hook);
            }
            return;
        }
        let _activity_owner = super::activity::Owner::enter();
        let _ = ready_tx.send(Ok(()));

        // Message loop - required for low-level hooks to function.
        // Keep the short timeout so shutdown polling behavior remains unchanged.
        let mut msg = MSG::default();
        loop {
            // Check if we should stop
            if !thread_running.load(Ordering::SeqCst) {
                break;
            }

            // Process all pending messages
            let outcome = drain_thread_messages(&mut msg);
            if outcome.quit {
                break;
            }
            HOOK_CONTEXT.with(|cell| {
                if let Some(ctx) = cell.borrow_mut().as_mut() {
                    reconcile_released_keys(ctx, |vk| unsafe { GetAsyncKeyState(i32::from(vk)) } as u16 & 0x8000 != 0);
                }
            });
            if outcome.reconcile {
                reconcile_modifiers_in_context();
            }
            // The wndproc requests re-installs for messages that arrive via
            // SendMessage (dispatched inside PeekMessageW, never seen by the
            // drain loop) -- WM_POWERBROADCAST always, WM_WTSSESSION_CHANGE
            // on builds that send rather than post it.
            let system_reinstall = outcome.reinstall_hooks || take_reinstall_request();
            let requested = HOOK_CONTEXT.with(|cell| cell.borrow().as_ref()
                .and_then(|ctx| ctx.standalone_tap.as_ref()).and_then(StandaloneTapMenuMask::pending_hook_refresh));
            if system_reinstall || requested.is_some() {
                super::activity::invalidate();
                let succeeded = unsafe { reinstall_hooks(&mut kb_hook, &mut mouse_hook) };
                if let Some(request) = requested {
                    HOOK_CONTEXT.with(|cell| { if let Some(mask) = cell.borrow().as_ref()
                        .and_then(|ctx| ctx.standalone_tap.as_ref()) { mask.complete_hook_refresh(request, succeeded); } });
                }
                if succeeded {
                    // Routine recovery must not impersonate an unlock/resume
                    // and interrupt a newer application task.
                    if system_reinstall { lifecycle_event(Lifecycle::HooksReinstalled); }
                } else {
                    lifecycle_event(Lifecycle::HooksUnavailable);
                }
            }

            // Wait for messages or timeout — unlike thread::sleep, this returns
            // immediately when a message arrives, so hook callbacks are never delayed.
            super::activity::heartbeat();
            wait_for_message_or_timeout(HOOK_LOOP_TIMEOUT_MS);
        }

        // Clean up the watcher window, then the hooks
        if let Some(hwnd) = watcher_hwnd {
            unsafe {
                destroy_watcher_window(hwnd);
            }
        }
        unsafe {
            let _ = UnhookWindowsHookEx(kb_hook);
            let _ = UnhookWindowsHookEx(mouse_hook);
        }

        // Clear thread-local state
        HOOK_CONTEXT.with(|ctx| {
            *ctx.borrow_mut() = None;
        });
    });

    match ready_rx.recv_timeout(std::time::Duration::from_secs(5)) {
        Ok(Ok(())) => {},
        Ok(Err(message)) => {
            running.store(false, Ordering::SeqCst);
            let _ = handle.join();
            return Err(Error::Platform(message));
        },
        Err(_) => {
            running.store(false, Ordering::SeqCst);
            // SpeakMode invokes this constructor only on its dedicated hotkey
            // owner thread. Its outer readiness timeout returns that owner to the
            // coordinator for retryable shutdown. Join here so outer completion
            // proves the native hook thread actually exited; never detach it on
            // a timeout and mistake the outer error for native resource release.
            let _ = handle.join();
            return Err(Error::Timeout);
        },
    }
    Ok(WindowsListenerState {
        event_receiver: rx,
        observed_receiver: observed.then_some(observe_rx),
        thread_handle: Some(handle),
        running,
        blocking_hotkeys,
    })
}

/// Low-level keyboard hook callback
///
/// This function is called by Windows for every keyboard event system-wide.
/// It must return quickly to avoid input lag.
unsafe extern "system" fn keyboard_hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // If code < 0, we must pass to next hook without processing
    if code < 0 {
        return CallNextHookEx(None, code, wparam, lparam);
    }

    let observed_at=std::time::Instant::now();
    let mut should_block = false;
    let mut needs_mask = false;
    let mut standalone_release:Option<(u16,KeyEvent)>=None;

    // Process the keyboard event
    HOOK_CONTEXT.with(|ctx_cell| {
        let mut ctx_ref = ctx_cell.borrow_mut();
        if let Some(ctx) = ctx_ref.as_mut() {
            // Extract key information from KBDLLHOOKSTRUCT
            let kb_struct = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
            let vk_code = kb_struct.vkCode as u16;
            let scan_code = kb_struct.scanCode;
            let is_extended = (kb_struct.flags.0 & LLKHF_EXTENDED.0) != 0;

            let is_key_down = matches!(wparam.0 as u32, WM_KEYDOWN | WM_SYSKEYDOWN);

            super::activity::observe(wparam.0 as u32,
                matches!(kb_struct.dwExtraInfo, crate::STANDALONE_MENU_MASK_EXTRA_INFO | 0x534D_5041));

            // Our own injected menu mask: pass it through untouched. It
            // exists solely for the shell's benefit and must not update
            // modifier state, trigger reconciliation, or be emitted.
            // Inert masks, their replacement modifier release and synthetic
            // clipboard chords bypass both the recognizer and observer stream.
            if !observed_keyboard_event_is_physical(vk_code,kb_struct.flags.0,kb_struct.dwExtraInfo){return;}

            // On AltGr layouts Windows synthesizes a Left Ctrl press/release
            // around every Right Alt event (captured live on UK layout:
            // identical timestamps, scancode 0x21D). Drop it entirely — no
            // modifier update, no emit — so AltGr+key reads as OptRight
            // only. The event is still passed down the hook chain.
            if is_altgr_phantom_ctrl(vk_code, scan_code) {
                ctx.altgr_phantom_ctrl = is_key_down;
                return;
            }

            // Check if this is a modifier key
            if let Some(modifier) = vk_to_modifier(vk_code) {
                let prev_modifiers = ctx.current_modifiers;

                // Update modifier state
                if is_key_down {
                    ctx.current_modifiers |= modifier;
                } else {
                    ctx.current_modifiers &= !modifier;
                }

                // Only emit event if modifiers actually changed
                if ctx.current_modifiers != prev_modifiers {
                    let event=KeyEvent {modifiers:ctx.current_modifiers,key:None,is_key_down,
                        changed_modifier:Some(modifier)};
                    let unrelated=ctx.standalone_tap.is_some()&&modifier.intersects(MENU_MODIFIERS)&&
                        (standalone_tap_has_other_modifier(modifier,physical_modifiers(),ctx.altgr_phantom_ctrl)||
                         standalone_unrelated_key_is_held());
                    let standalone=ctx.standalone_tap.as_mut().is_some_and(|mask|
                        mask.feed(event,KeyOrigin::Physical,std::time::Instant::now(),unrelated));
                    // Check if modifier-only combo should be blocked
                    should_block =
                        should_block_hotkey(&ctx.blocking_hotkeys, ctx.current_modifiers, None);

                    // A blocked modifier press (the Shift of a Win+Shift
                    // modifier-only hotkey) is as invisible to the shell as
                    // a blocked regular key, so it needs the same mask.
                    if needs_menu_mask(should_block, ctx.current_modifiers, ctx.menu_mask_sent) {
                        ctx.menu_mask_sent = true;
                        needs_mask = true;
                    }

                    if standalone {standalone_release=Some((vk_code,event));}
                    else {emit_key_at(ctx,event,KeyOrigin::Physical,observed_at);}
                }

                // The hold ends once no Win/Alt modifier remains held; the
                // next hold gets a fresh mask.
                if !ctx.current_modifiers.intersects(MENU_MODIFIERS) {
                    ctx.menu_mask_sent = false;
                }
            } else {
                // Any real non-modifier cancels the standalone gesture, even
                // if the key has no portable handy-keys mapping.
                if let Some(mask)=ctx.standalone_tap.as_mut(){mask.reset();}
                // Non-modifier key: reconcile tracked modifiers against the
                // physical keyboard first, so a key-up missed during a secure
                // desktop transition can't stick a modifier onto this event.
                // (Not done for modifier events: inside a low-level hook the
                // async key state does not yet include the in-flight change,
                // so reconciling there would fight the toggle logic above.)
                reconcile_modifiers(ctx);

                if let Some(key) = map_key(vk_code, scan_code, is_extended) {
                    let key = track_key(ctx, vk_code, key, is_key_down);
                    // Regular key event
                    should_block = should_block_hotkey(
                        &ctx.blocking_hotkeys,
                        ctx.current_modifiers,
                        Some(key),
                    );

                    // The shell can no longer see the key we are about to
                    // swallow; without a substitute, releasing Win/Alt would
                    // read as a lone tap and open the Start menu (#917).
                    if needs_menu_mask(should_block, ctx.current_modifiers, ctx.menu_mask_sent) {
                        ctx.menu_mask_sent = true;
                        needs_mask = true;
                    }

                    emit_key_at(ctx, KeyEvent {
                        modifiers: ctx.current_modifiers,
                        key: Some(key),
                        is_key_down,
                        changed_modifier: None,
                    }, KeyOrigin::Physical, observed_at);
                }else if ctx.observed_sender.is_some(){
                    // The app's tap recognizer must also cancel on real unmapped
                    // keys; injected/own inert keys were filtered above.
                    emit_key_at(ctx,KeyEvent{modifiers:ctx.current_modifiers,key:None,is_key_down,
                        changed_modifier:None},KeyOrigin::Physical,observed_at);
                }
            }
        }
    });

    // Outside the HOOK_CONTEXT borrow: the injected events re-enter this
    // hook (where the dwExtraInfo marker passes them straight through).
    if needs_mask {
        send_menu_mask();
    }
    if let Some((virtual_key,event))=standalone_release {
        let still_current=HOOK_CONTEXT.with(|cell|cell.borrow().as_ref().is_some_and(|ctx|
            ctx.standalone_tap.as_ref().is_some_and(StandaloneTapMenuMask::policy_is_current)));
        // Do not rely on SendInput(mask) versus CallNextHookEx(real AltUp)
        // ordering. Replace only this accepted release, with one serial batch.
        // https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-sendinput
        if still_current&&replace_standalone_release(virtual_key){should_block=true;}
        // Even an injection failure must release the policy's physical hold and
        // reach the existing gesture observer. Failure retains original key-up.
        HOOK_CONTEXT.with(|cell|{if let Some(ctx)=cell.borrow().as_ref(){emit_key_at(ctx,event,KeyOrigin::Physical,observed_at);}});
    }

    if should_block {
        // Return non-zero to block the event from propagating
        LRESULT(1)
    } else {
        // Pass to next hook in chain
        CallNextHookEx(None, code, wparam, lparam)
    }
}

/// Low-level mouse hook callback
///
/// This function is called by Windows for every mouse event system-wide.
unsafe extern "system" fn mouse_hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // If code < 0, we must pass to next hook without processing
    if code < 0 {
        return CallNextHookEx(None, code, wparam, lparam);
    }

    let observed_at=std::time::Instant::now();
    super::activity::observe(wparam.0 as u32, false);

    // Only button transitions matter; bail out early for moves and wheel
    // events so the hot path stays free of state access.
    if !matches!(
        wparam.0 as u32,
        WM_LBUTTONDOWN
            | WM_LBUTTONUP
            | WM_RBUTTONDOWN
            | WM_RBUTTONUP
            | WM_MBUTTONDOWN
            | WM_MBUTTONUP
            | WM_XBUTTONDOWN
            | WM_XBUTTONUP
    ) {
        return CallNextHookEx(None, code, wparam, lparam);
    }

    let mut should_block = false;
    let mut needs_mask = false;

    // Process the mouse event
    HOOK_CONTEXT.with(|ctx_cell| {
        let mut ctx_ref = ctx_cell.borrow_mut();
        if let Some(ctx) = ctx_ref.as_mut() {
            let mouse_struct = &*(lparam.0 as *const MSLLHOOKSTRUCT);

            if mouse_struct.flags & 1 != 0 { return; } // LLMHF_INJECTED
            if let Some(mask)=ctx.standalone_tap.as_mut(){mask.reset();}

            // Stale modifiers must not gate button reporting (or decorate the
            // event), so reconcile before reading them.
            reconcile_modifiers(ctx);

            // The observer tracks complete holds. If Alt is released before
            // the mouse button, its up must still balance the earlier down.
            let has_modifiers = ctx.observed_sender.is_some() || !ctx.current_modifiers.is_empty();

            let (key, is_down) = match wparam.0 as u32 {
                WM_LBUTTONDOWN if has_modifiers => (Some(Key::MouseLeft), true),
                WM_LBUTTONUP if has_modifiers => (Some(Key::MouseLeft), false),
                WM_RBUTTONDOWN if has_modifiers => (Some(Key::MouseRight), true),
                WM_RBUTTONUP if has_modifiers => (Some(Key::MouseRight), false),
                // Middle and X buttons always reported
                WM_MBUTTONDOWN => (Some(Key::MouseMiddle), true),
                WM_MBUTTONUP => (Some(Key::MouseMiddle), false),
                WM_XBUTTONDOWN => {
                    // High word of mouseData contains which X button (1 or 2)
                    let xbutton = (mouse_struct.mouseData >> 16) & 0xFFFF;
                    let key = if xbutton == 1 {
                        Some(Key::MouseX1)
                    } else if xbutton == 2 {
                        Some(Key::MouseX2)
                    } else {
                        None
                    };
                    (key, true)
                }
                WM_XBUTTONUP => {
                    let xbutton = (mouse_struct.mouseData >> 16) & 0xFFFF;
                    let key = if xbutton == 1 {
                        Some(Key::MouseX1)
                    } else if xbutton == 2 {
                        Some(Key::MouseX2)
                    } else {
                        None
                    };
                    (key, false)
                }
                _ => (None, false),
            };

            if let Some(key) = key {
                let vk = match key {
                    Key::MouseLeft => 0x01, Key::MouseRight => 0x02,
                    Key::MouseMiddle => 0x04, Key::MouseX1 => 0x05, Key::MouseX2 => 0x06,
                    _ => unreachable!(),
                };
                let key = track_key(ctx, vk, key, is_down);
                should_block =
                    should_block_hotkey(&ctx.blocking_hotkeys, ctx.current_modifiers, Some(key));

                // A blocked Win/Alt+click hides the click from the shell
                // just like a blocked key, so the hold needs the same mask.
                if needs_menu_mask(should_block, ctx.current_modifiers, ctx.menu_mask_sent) {
                    ctx.menu_mask_sent = true;
                    needs_mask = true;
                }

                emit_key_at(ctx, KeyEvent {
                    modifiers: ctx.current_modifiers,
                    key: Some(key),
                    is_key_down: is_down,
                    changed_modifier: None,
                }, KeyOrigin::Physical, observed_at);
            }
        }
    });

    if needs_mask {
        send_menu_mask();
    }

    if should_block {
        // Return non-zero to block the event from propagating
        LRESULT(1)
    } else {
        // Pass to next hook in chain
        CallNextHookEx(None, code, wparam, lparam)
    }
}

/// Whether blocking this event must be accompanied by a menu-mask injection:
/// a real event is being swallowed while a Win/Alt modifier is held, and the
/// current hold has not been masked yet. Key-ups need masking too: a key
/// pressed *before* the modifier and released during the hold has its up
/// blocked, but its down passed to the shell before the hold began, so it
/// never disarmed the menu heuristic (verified live: without the up-mask,
/// F20↓ Win↓ F20↑ Win↑ with Win+F20 blocked opens Start). `already_sent`
/// dedupes auto-repeat, release edges, and later events within one hold.
fn needs_menu_mask(should_block: bool, modifiers: Modifiers, already_sent: bool) -> bool {
    should_block && !already_sent && modifiers.intersects(MENU_MODIFIERS)
}

/// Inject a press+release of the menu mask key via SendInput. Must be called
/// outside the HOOK_CONTEXT borrow, since the injected events re-enter the
/// keyboard hook on this thread's next message pump.
fn send_menu_mask() {
    let key = |flags: KEYBD_EVENT_FLAGS| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(MENU_MASK_VK),
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: crate::STANDALONE_MENU_MASK_EXTRA_INFO,
            },
        },
    };
    let inputs = [key(KEYBD_EVENT_FLAGS(0)), key(KEYEVENTF_KEYUP)];
    let sent = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
    if sent != inputs.len() as u32 {
        // Rare (e.g. UIPI filtering); the cost is the pre-mask behavior:
        // the shell may open its menu when the modifier is released.
        eprintln!("handy-keys: menu mask injection failed");
    }
}

fn standalone_unrelated_key_is_held()->bool {
    // Include mouse buttons and keys held before the hook was installed.
    // Generic + side-specific modifiers are already present in the KeyEvent.
    (0x01..=0xfe).filter(|vk|!matches!(*vk,0x10..=0x12|0x5b..=0x5c|0xa0..=0xa5|0xe8))
        .any(|vk|unsafe{GetAsyncKeyState(vk)} as u16 & 0x8000!=0)
}
fn replace_standalone_release(virtual_key:u16)->bool {
    let Some(batch)=standalone_menu_release_batch(virtual_key) else{return false;};
    let inputs=batch.map(|event|INPUT{r#type:INPUT_KEYBOARD,Anonymous:INPUT_0{ki:KEYBDINPUT{
        wVk:VIRTUAL_KEY(event.virtual_key),
        dwFlags:(if event.key_up{KEYEVENTF_KEYUP}else{KEYBD_EVENT_FLAGS(0)})|
            (if event.extended{KEYEVENTF_EXTENDEDKEY}else{KEYBD_EVENT_FLAGS(0)}),
        dwExtraInfo:event.extra_info,..Default::default()}}});
    let inserted=unsafe{SendInput(&inputs,std::mem::size_of::<INPUT>() as i32)};
    match standalone_release_disposition(inserted) {
        StandaloneReleaseDisposition::ReplacePhysicalRelease=>true,
        StandaloneReleaseDisposition::PreservePhysicalRelease=>{
            if inserted==1 {
                // Only the inert down entered the stream; balance our own key.
                let _=unsafe{SendInput(&inputs[1..2],std::mem::size_of::<INPUT>() as i32)};
            }
            eprintln!("handy-keys: standalone menu mask/release unavailable ({inserted}/3); original release retained");
            false
        }
    }
}

/// Check if a hotkey combination should be blocked
fn should_block_hotkey(
    blocking_hotkeys: &Option<BlockingHotkeys>,
    modifiers: Modifiers,
    key: Option<Key>,
) -> bool {
    if let Some(ref hotkeys) = blocking_hotkeys {
        if let Ok(set) = hotkeys.lock() {
            return set
                .iter()
                .any(|h| h.modifiers.matches(modifiers) && h.key == key);
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    use windows::Win32::UI::WindowsAndMessaging::PostQuitMessage;

    #[test]
    fn queued_physical_event_keeps_its_hook_time_instead_of_its_delivery_time() {
        let (tx, _) = mpsc::channel();let (observed, rx) = mpsc::channel();
        let ctx=HookContext {event_sender:tx,observed_sender:Some(observed),
            current_modifiers:Modifiers::empty(),pressed_keys:HashMap::new(),blocking_hotkeys:None,
            altgr_phantom_ctrl:false,menu_mask_sent:false,standalone_tap:None};
        let original=Instant::now()-Duration::from_secs(5);
        emit_key_at(&ctx,KeyEvent{modifiers:Modifiers::empty(),key:Some(Key::F8),
            is_key_down:false,changed_modifier:None},KeyOrigin::Physical,original);
        let ObservedEvent::Key{at,origin,..}=rx.try_recv().unwrap()else{panic!("expected key")};
        assert_eq!(at,original);assert_eq!(origin,KeyOrigin::Physical);
    }

    #[test]
    fn mouse_state_polling_preserves_either_held_primary_button_even_when_swapped() {
        for observed in [0x01, 0x02] {
            assert!(observed_key_is_down(observed, &|vk| vk == 0x01));
            assert!(observed_key_is_down(observed, &|vk| vk == 0x02));
            assert!(!observed_key_is_down(observed, &|_| false));
        }
        assert!(!observed_key_is_down(0x77, &|vk| vk == 0x01));
        assert!(observed_key_is_down(0x77, &|vk| vk == 0x77));
    }

    #[test]
    fn missed_regular_and_mouse_releases_emit_only_reconciled_events_once() {
        let (tx, _) = mpsc::channel();
        let (observed, rx) = mpsc::channel();
        let mut ctx = HookContext { event_sender: tx, observed_sender: Some(observed),
            current_modifiers: Modifiers::empty(), pressed_keys: HashMap::new(),
            blocking_hotkeys: None, altgr_phantom_ctrl: false, menu_mask_sent: false,
            standalone_tap: None };
        track_key(&mut ctx, 0x41, Key::A, true);
        track_key(&mut ctx, 0x01, Key::MouseLeft, true);
        track_key(&mut ctx, 0x77, Key::F8, true);
        reconcile_released_keys(&mut ctx, |vk| vk == 0x77);
        let released = rx.try_iter().map(|event| match event {
            ObservedEvent::Key { event, origin, .. } => {
                assert_eq!(origin, KeyOrigin::Reconciled);
                assert!(!event.is_key_down);
                event.key.unwrap()
            },
            _ => panic!("key cleanup must not interrupt the application lifecycle"),
        }).collect::<std::collections::HashSet<_>>();
        assert_eq!(released, [Key::A, Key::MouseLeft].into());
        assert_eq!(ctx.pressed_keys.len(), 1, "actually-held F8 must remain held");
        reconcile_released_keys(&mut ctx, |_| true);
        assert!(rx.try_recv().is_err());
        reconcile_released_keys(&mut ctx, |_| false);
        assert!(matches!(rx.try_recv().unwrap(), ObservedEvent::Key {
            event: KeyEvent { key: Some(Key::F8), is_key_down: false, .. }, origin: KeyOrigin::Reconciled, .. }));
        reconcile_released_keys(&mut ctx, |_| false);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn physical_release_balances_the_original_key_even_if_layout_mapping_changed() {
        let (tx, _) = mpsc::channel();
        let mut ctx = HookContext { event_sender: tx, observed_sender: None,
            current_modifiers: Modifiers::empty(), pressed_keys: HashMap::new(),
            blocking_hotkeys: None, altgr_phantom_ctrl: false, menu_mask_sent: false,
            standalone_tap: None };
        assert_eq!(track_key(&mut ctx, 0xba, Key::Semicolon, true), Key::Semicolon);
        assert_eq!(track_key(&mut ctx, 0xba, Key::Quote, false), Key::Semicolon);
        assert!(ctx.pressed_keys.is_empty());
    }

    fn clear_message_queue() {
        let mut msg = MSG::default();
        unsafe { while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {} }
    }

    #[test]
    fn wait_times_out_when_no_messages() {
        clear_message_queue();
        let start = Instant::now();
        wait_for_message_or_timeout(20);
        let elapsed = start.elapsed();
        assert!(
            elapsed >= Duration::from_millis(8),
            "expected wait to block close to timeout, elapsed={elapsed:?}"
        );
        clear_message_queue();
    }

    #[test]
    fn wait_returns_immediately_when_message_is_pending() {
        clear_message_queue();
        unsafe {
            PostQuitMessage(0);
        }
        let start = Instant::now();
        wait_for_message_or_timeout(200);
        let elapsed = start.elapsed();
        assert!(
            elapsed < Duration::from_millis(100),
            "expected pending message to wake wait early, elapsed={elapsed:?}"
        );
        clear_message_queue();
    }

    #[test]
    fn drain_messages_stops_on_wm_quit() {
        clear_message_queue();
        unsafe {
            PostQuitMessage(0);
        }
        let mut msg = MSG::default();
        assert!(drain_thread_messages(&mut msg).quit);
        clear_message_queue();
    }

    #[test]
    fn should_block_registered_mouse_hotkey() {
        use crate::types::Hotkey;
        let mut hotkeys = std::collections::HashSet::new();
        hotkeys.insert(Hotkey::new(Modifiers::empty(), Key::MouseX1).unwrap());
        let blocking_hotkeys = Some(std::sync::Arc::new(std::sync::Mutex::new(hotkeys)));

        assert!(should_block_hotkey(
            &blocking_hotkeys,
            Modifiers::empty(),
            Some(Key::MouseX1)
        ));
        assert!(!should_block_hotkey(
            &blocking_hotkeys,
            Modifiers::empty(),
            Some(Key::MouseX2)
        ));
    }

    fn post_thread_message(message: u32, wparam: usize) {
        use windows::Win32::System::Threading::GetCurrentThreadId;
        use windows::Win32::UI::WindowsAndMessaging::PostThreadMessageW;
        unsafe {
            PostThreadMessageW(GetCurrentThreadId(), message, WPARAM(wparam), LPARAM(0)).unwrap();
        }
    }

    #[test]
    fn drain_reports_session_unlock_with_reinstall() {
        clear_message_queue();
        post_thread_message(WM_WTSSESSION_CHANGE, WTS_SESSION_UNLOCK);
        let mut msg = MSG::default();
        let outcome = drain_thread_messages(&mut msg);
        assert!(outcome.reconcile);
        assert!(outcome.reinstall_hooks);
        assert!(!outcome.quit);
        clear_message_queue();
    }

    #[test]
    fn drain_reports_session_lock_without_reinstall() {
        clear_message_queue();
        post_thread_message(WM_WTSSESSION_CHANGE, WTS_SESSION_LOCK);
        let mut msg = MSG::default();
        let outcome = drain_thread_messages(&mut msg);
        assert!(outcome.reconcile);
        assert!(!outcome.reinstall_hooks);
        clear_message_queue();
    }

    #[test]
    fn drain_ignores_unrelated_session_events() {
        clear_message_queue();
        // WTS_SESSION_LOGOFF (0x6) is not a state we react to.
        post_thread_message(WM_WTSSESSION_CHANGE, 0x6);
        let mut msg = MSG::default();
        let outcome = drain_thread_messages(&mut msg);
        assert!(!outcome.reconcile);
        assert!(!outcome.reinstall_hooks);
        clear_message_queue();
    }

    #[test]
    fn drain_reports_resume_with_reinstall() {
        clear_message_queue();
        post_thread_message(WM_POWERBROADCAST, PBT_APMRESUMEAUTOMATIC);
        let mut msg = MSG::default();
        let outcome = drain_thread_messages(&mut msg);
        assert!(outcome.reconcile);
        assert!(outcome.reinstall_hooks);
        assert!(!outcome.quit);
        clear_message_queue();
    }

    #[test]
    fn drain_ignores_non_resume_power_events() {
        clear_message_queue();
        // PBT_APMSUSPEND (0x4): nothing to re-arm while going down.
        post_thread_message(WM_POWERBROADCAST, 0x4);
        let mut msg = MSG::default();
        let outcome = drain_thread_messages(&mut msg);
        assert!(!outcome.reconcile);
        assert!(!outcome.reinstall_hooks);
        clear_message_queue();
    }

    /// Drive the wndproc directly, as SendMessage delivery would. A null HWND
    /// is fine: DefWindowProcW ignores messages for windows it can't resolve,
    /// and our handling never touches the handle.
    fn send_to_wndproc(msg: u32, wparam: usize) {
        unsafe {
            watcher_wndproc(HWND::default(), msg, WPARAM(wparam), LPARAM(0));
        }
    }

    #[test]
    fn wndproc_requests_reinstall_on_resume() {
        let _ = take_reinstall_request();
        send_to_wndproc(WM_POWERBROADCAST, PBT_APMRESUMESUSPEND);
        assert!(take_reinstall_request());
        // The request is consumed by the take.
        assert!(!take_reinstall_request());
    }

    #[test]
    fn wndproc_requests_reinstall_on_session_unlock() {
        let _ = take_reinstall_request();
        send_to_wndproc(WM_WTSSESSION_CHANGE, WTS_SESSION_UNLOCK);
        assert!(take_reinstall_request());
    }

    #[test]
    fn wndproc_does_not_request_reinstall_on_lock_or_suspend() {
        let _ = take_reinstall_request();
        send_to_wndproc(WM_WTSSESSION_CHANGE, WTS_SESSION_LOCK);
        send_to_wndproc(WM_POWERBROADCAST, 0x4); // PBT_APMSUSPEND
        assert!(!take_reinstall_request());
    }

    // stale_modifiers / release_events themselves are covered in
    // crate::platform::state, where they now live.

    #[test]
    fn adoption_picks_up_missed_presses() {
        let adopt = adoptable_modifiers(
            Modifiers::CTRL_LEFT,
            Modifiers::CTRL_LEFT | Modifiers::SHIFT_LEFT,
            false,
        );
        assert_eq!(adopt, Modifiers::SHIFT_LEFT);
    }

    #[test]
    fn adoption_skips_ctrl_left_while_altgr_phantom_held() {
        // AltGr held: GetAsyncKeyState reports LCtrl+RAlt down, but only
        // OptRight reflects a user keypress. CTRL_LEFT must not be adopted.
        let adopt = adoptable_modifiers(
            Modifiers::OPT_RIGHT,
            Modifiers::CTRL_LEFT | Modifiers::OPT_RIGHT,
            true,
        );
        assert_eq!(adopt, Modifiers::empty());
    }

    #[test]
    fn adoption_keeps_other_modifiers_while_altgr_phantom_held() {
        // A genuinely-held Shift missed during a secure desktop transition
        // is still adopted while the phantom flag is set.
        let adopt = adoptable_modifiers(
            Modifiers::OPT_RIGHT,
            Modifiers::CTRL_LEFT | Modifiers::OPT_RIGHT | Modifiers::SHIFT_LEFT,
            true,
        );
        assert_eq!(adopt, Modifiers::SHIFT_LEFT);
    }

    #[test]
    fn menu_mask_fires_for_blocked_key_under_win() {
        assert!(needs_menu_mask(true, Modifiers::CMD_LEFT, false));
    }

    #[test]
    fn menu_mask_fires_under_alt() {
        // Alt has the sibling heuristic: a lone Alt tap focuses the menu bar.
        assert!(needs_menu_mask(
            true,
            Modifiers::OPT_LEFT | Modifiers::SHIFT_LEFT,
            false
        ));
    }

    #[test]
    fn menu_mask_only_when_blocking() {
        assert!(!needs_menu_mask(false, Modifiers::CMD_LEFT, false));
    }

    #[test]
    fn menu_mask_once_per_hold() {
        // Covers auto-repeat and the release edge of an already-masked press.
        assert!(!needs_menu_mask(true, Modifiers::CMD_LEFT, true));
    }

    #[test]
    fn menu_mask_not_for_ctrl_shift_combos() {
        // Ctrl/Shift releases trigger no shell menu; no mask needed.
        assert!(!needs_menu_mask(
            true,
            Modifiers::CTRL_LEFT | Modifiers::SHIFT_LEFT,
            false
        ));
    }

    #[test]
    fn menu_mask_vk_is_inert_to_our_own_mapping() {
        // Load-bearing: the injected mask must fall through the hook to
        // reach the shell. The dwExtraInfo marker guarantees that for our
        // own injections; this pins the fallback property that the VK maps
        // to nothing, so even an unmarked vkE8 press stays a no-op.
        assert!(vk_to_modifier(MENU_MASK_VK).is_none());
        assert!(map_key(MENU_MASK_VK, 0, false).is_none());
    }

    #[test]
    fn reconcile_clears_mask_flag_when_hold_ends_off_desktop() {
        // Win+L: the Win-up happens on the secure desktop and never reaches
        // the hook. Reconciliation emits the stale release and must also
        // clear the mask flag so the next hold gets its own mask. (No keys
        // are physically held while tests run, so GetAsyncKeyState reports
        // everything up and CMD_LEFT reads as stale.)
        let (tx, rx) = mpsc::channel();
        let mut ctx = HookContext {
            event_sender: tx,
            observed_sender: None,
            current_modifiers: Modifiers::CMD_LEFT,
            pressed_keys: HashMap::new(),
            blocking_hotkeys: None,
            altgr_phantom_ctrl: false,
            menu_mask_sent: true,
            standalone_tap: None,
        };
        reconcile_modifiers(&mut ctx);
        assert_eq!(ctx.current_modifiers, Modifiers::empty());
        assert!(!ctx.menu_mask_sent);
        // The stale CMD_LEFT was cleared via a synthetic release event.
        let release = rx.try_recv().expect("expected a synthetic release");
        assert_eq!(release.changed_modifier, Some(Modifiers::CMD_LEFT));
        assert!(!release.is_key_down);
    }

    #[test]
    fn phantom_flag_never_releases_a_tracked_ctrl() {
        // User really holds LCtrl, then AltGr: CTRL_LEFT is tracked from its
        // own event and physically down, so it must be neither stale nor
        // re-adopted.
        let tracked = Modifiers::CTRL_LEFT | Modifiers::OPT_RIGHT;
        let physical = Modifiers::CTRL_LEFT | Modifiers::OPT_RIGHT;
        assert_eq!(stale_modifiers(tracked, physical), Modifiers::empty());
        assert_eq!(
            adoptable_modifiers(tracked, physical, true),
            Modifiers::empty()
        );
    }
}
