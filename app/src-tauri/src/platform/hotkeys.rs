//! Tap policy around handy-keys. The hook is nonblocking so AltGr/native
//! combinations remain available; standalone Alt menu effects need native QA.
use super::{PlatformError, Result};
use handy_keys::{Hotkey, Key, KeyEvent};
use std::collections::HashSet;
use std::sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, bounded};

#[derive(Debug, Clone)]
pub enum HotkeyEvent {
    Ready,
    Toggle,
    Captured(String),
    LifecycleReset(String),
    Unavailable(String),
}

/// A full gesture, not key-down. Reconciled releases must call reset instead.
pub struct ShortcutTap {
    shortcut: Hotkey,
    started: Option<Instant>,
    trigger_key: Option<Key>,
    cancelled: bool,
    held: HashSet<Key>,
    maximum: Duration,
}

impl ShortcutTap {
    pub fn new(shortcut: Hotkey, maximum: Duration) -> Self {
        Self { shortcut, started: None, trigger_key: None, cancelled: false, held: HashSet::new(), maximum }
    }
    pub fn reset(&mut self) {
        self.started = None;
        self.cancelled = false;
        self.trigger_key = None;
        self.held.clear();
    }
    pub fn feed(&mut self, event: KeyEvent, now: Instant) -> bool {
        let was_held = event.key.is_some_and(|k| self.held.contains(&k));
        if let Some(key) = event.key {
            if event.is_key_down { self.held.insert(key); } else { self.held.remove(&key); }
        }
        let match_mods = self.shortcut.modifiers.matches(event.modifiers);
        if self.shortcut.key.is_some() {
            if event.key == self.shortcut.key {
                if event.is_key_down && !was_held && self.started.is_none() && match_mods {
                    self.started = Some(now);
                    self.trigger_key = event.key;
                    self.cancelled = self.held.len() != 1;
                } else if !event.is_key_down && self.trigger_key == event.key {
                    let valid = self.started.take().is_some_and(|start| now.duration_since(start) <= self.maximum)
                        && !self.cancelled && match_mods;
                    self.trigger_key = None;
                    self.cancelled = false;
                    return valid;
                }
            } else if self.started.is_some() && (event.key.is_some() || !match_mods) {
                self.cancelled = true;
            }
        } else if event.changed_modifier.is_some() {
            if event.is_key_down && match_mods && self.started.is_none() {
                self.started = Some(now);
                self.cancelled = !self.held.is_empty();
            } else if self.started.is_some() && !event.is_key_down && event.modifiers.is_empty() {
                let valid = self.started.take().is_some_and(|start| now.duration_since(start) <= self.maximum)
                    && !self.cancelled;
                self.cancelled = false;
                return valid;
            } else if self.started.is_some() && !match_mods && event.is_key_down {
                self.cancelled = true;
            }
        } else if self.started.is_some() && event.key.is_some() {
            self.cancelled = true;
        }
        false
    }
}

enum Command {
    Replace(Hotkey, UpdateAck),
    Capture(bool, UpdateAck),
    Enable(bool, UpdateAck),
    Reset,
}
enum UpdateState { Pending, Completed(std::result::Result<(), String>), Cancelled }
struct UpdateAck { signal: Sender<()>, state: Arc<Mutex<UpdateState>> }
impl UpdateAck {
    fn apply(&self, update: impl FnOnce() -> std::result::Result<(), String>) {
        if let Ok(mut state) = self.state.lock() {
            if matches!(*state, UpdateState::Pending) {
                *state = UpdateState::Completed(update());
                let _ = self.signal.try_send(());
            }
        }
    }
}

/// Even failed startup can own a live native resource thread. The desktop must
/// retain `owner` and retry shutdown until its join ACK; an error is not release.
#[derive(Debug)]
pub struct HotkeyStartError {
    pub error: PlatformError,
    pub owner: Option<HotkeyService>,
}
impl HotkeyStartError {
    pub fn into_parts(self) -> (PlatformError, Option<HotkeyService>) {
        (self.error, self.owner)
    }
}
impl std::fmt::Display for HotkeyStartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.error.fmt(f)
    }
}
impl std::error::Error for HotkeyStartError {}

pub struct HotkeyService {
    control: Sender<Command>,
    stopping: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    ready: bool,
}
impl std::fmt::Debug for HotkeyService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HotkeyService")
            .field("ready", &self.ready)
            .field("stopping", &self.stopping.load(Ordering::SeqCst))
            .field("owner_released", &self.owner_released())
            .finish_non_exhaustive()
    }
}

impl HotkeyService {
    /// `maximum_tap` is an engineering parameter to validate on actual keyboards.
    /// No global hooks are started until the caller explicitly invokes start.
    /// Run startup off the UI/Inner lock. Its readiness wait is bounded, but a
    /// timeout returns the owner of cleanup that may still be blocked in Win32.
    pub fn start(shortcut: &str, maximum_tap: Duration,
        callback: impl Fn(HotkeyEvent) + Send + 'static)
        -> std::result::Result<Self, HotkeyStartError> {
        let shortcut = parse_shortcut(shortcut)
            .map_err(|error| HotkeyStartError { error, owner: None })?;
        Self::start_worker(Duration::from_secs(6), move |rx, stop, ready| {
            run(shortcut, maximum_tap, rx, stop, ready, callback)
        })
    }
    fn start_worker(readiness_deadline: Duration,
        runner: impl FnOnce(Receiver<Command>, Arc<AtomicBool>,
            Sender<std::result::Result<(), String>>) + Send + 'static)
        -> std::result::Result<Self, HotkeyStartError> {
        let (tx, rx) = bounded(8);
        let (ready_tx, ready_rx) = bounded(1);
        let stopping = Arc::new(AtomicBool::new(false));
        let stop = Arc::clone(&stopping);
        let worker = thread::Builder::new().name("speakmode-hotkeys".into())
            .spawn(move || runner(rx, stop, ready_tx))
            .map_err(|e| HotkeyStartError {
                error: PlatformError::Unavailable(e.to_string()), owner: None,
            })?;
        // Construct before waiting: every path after spawn returns its actual
        // JoinHandle, including readiness disconnect and an already-finished owner.
        let mut owner = Self { control: tx, stopping, worker: Some(worker), ready: false };
        let error = match ready_rx.recv_timeout(readiness_deadline) {
            Ok(Ok(())) => { owner.ready = true; return Ok(owner); },
            Ok(Err(error)) => PlatformError::Unavailable(error),
            Err(RecvTimeoutError::Timeout) => PlatformError::Timeout("hook readiness"),
            Err(RecvTimeoutError::Disconnected) => PlatformError::Disconnected,
        };
        owner.stopping.store(true, Ordering::SeqCst);
        Err(HotkeyStartError { error, owner: Some(owner) })
    }
    /// Read-only observation of the real thread. A stop request is not release;
    /// shutdown still joins it and reports a panic instead of claiming normal ACK.
    pub fn owner_released(&self) -> bool {
        self.worker.as_ref().is_none_or(JoinHandle::is_finished)
    }
    fn require_running(&self) -> Result<()> {
        if !self.ready || self.stopping.load(Ordering::SeqCst) || self.owner_released() {
            Err(PlatformError::Disconnected)
        } else { Ok(()) }
    }
    pub fn replace_shortcut(&self, text: &str) -> Result<String> {
        let shortcut = parse_shortcut(text)?;
        self.ack(|ack| Command::Replace(shortcut, ack))?;
        Ok(shortcut.to_handy_string())
    }
    pub fn set_capture(&self, capture: bool) -> Result<()> { self.ack(|ack| Command::Capture(capture, ack)) }
    pub fn suspend(&self) -> Result<()> { self.ack(|ack| Command::Enable(false, ack)) }
    /// Core may resume only after its own resource/delivery cleanup gate passes.
    pub fn resume(&self) -> Result<()> { self.ack(|ack| Command::Enable(true, ack)) }
    pub fn reset(&self) -> Result<()> {
        self.require_running()?;
        self.control.send_timeout(Command::Reset, Duration::from_secs(1)).map_err(|_| PlatformError::Disconnected)
    }
    fn ack(&self, make: impl FnOnce(UpdateAck) -> Command) -> Result<()> {
        self.require_running()?;
        let (tx, rx) = bounded(1);
        let state = Arc::new(Mutex::new(UpdateState::Pending));
        self.control.send_timeout(make(UpdateAck { signal: tx, state: Arc::clone(&state) }), Duration::from_secs(1)).map_err(|_| PlatformError::Disconnected)?;
        let _ = rx.recv_timeout(Duration::from_secs(1));
        let mut state = state.lock().map_err(|_| PlatformError::Unavailable("shortcut update aborted".into()))?;
        match &*state {
            UpdateState::Completed(result) => result.clone().map_err(PlatformError::Unavailable),
            UpdateState::Pending => { *state = UpdateState::Cancelled; Err(PlatformError::Timeout("shortcut update")) },
            UpdateState::Cancelled => Err(PlatformError::Timeout("shortcut update")),
        }
    }
    pub fn shutdown(&mut self, deadline: Duration) -> Result<()> {
        self.ready = false;
        self.stopping.store(true, Ordering::SeqCst);
        let start = Instant::now();
        if let Some(worker) = &self.worker {
            while !worker.is_finished() {
                if start.elapsed() >= deadline { return Err(PlatformError::Timeout("hook shutdown")); }
                thread::sleep(Duration::from_millis(5));
            }
        }
        if let Some(worker) = self.worker.take() {
            worker.join().map_err(|_| PlatformError::Unavailable("hook owner panicked during shutdown".into()))?;
        }
        Ok(())
    }
}
impl Drop for HotkeyService {
    // Nonblocking safety intent only. Callers must retain failed-start and
    // timed-out owners until shutdown ACK; Drop cannot promise resource release.
    fn drop(&mut self) { self.stopping.store(true, Ordering::SeqCst); }
}

pub fn parse_shortcut(text: &str) -> Result<Hotkey> {
    let hotkey = text.parse::<Hotkey>().map_err(|e| PlatformError::Invalid(e.to_string()))?;
    if matches!(hotkey.key, Some(Key::MouseLeft | Key::MouseRight | Key::MouseMiddle | Key::MouseX1 | Key::MouseX2)) {
        return Err(PlatformError::Invalid("MVP shortcut must be a keyboard gesture".into()));
    }
    Ok(hotkey)
}

#[cfg(windows)]
fn run(shortcut: Hotkey, maximum: Duration, control: Receiver<Command>, stopping: Arc<AtomicBool>,
    ready: Sender<std::result::Result<(), String>>, callback: impl Fn(HotkeyEvent)) {
    use handy_keys::{KeyOrigin, Lifecycle, ObservedEvent, KeyboardListener};
    let listener = match KeyboardListener::new_observed() {
        Ok(listener) => listener,
        Err(error) => { let _ = ready.send(Err(error.to_string())); return; },
    };
    if stopping.load(Ordering::SeqCst) { return; }
    let mut tap = ShortcutTap::new(shortcut, maximum);
    let mut enabled = true;
    let mut hook_available = true;
    let mut capture = false;
    let mut recorded: Option<Hotkey> = None;
    let _ = ready.send(Ok(()));
    callback(HotkeyEvent::Ready);
    while !stopping.load(Ordering::SeqCst) {
        while let Ok(command) = control.try_recv() {
            match command {
                Command::Replace(shortcut, ack) => {
                    ack.apply(|| {
                        if hook_available { tap = ShortcutTap::new(shortcut, maximum); Ok(()) }
                        else { Err("keyboard hook is unavailable".into()) }
                    });
                },
                Command::Capture(value, ack) => ack.apply(|| { capture = value; recorded = None; tap.reset(); Ok(()) }),
                Command::Enable(value, ack) => {
                    ack.apply(|| {
                        if value && !hook_available { Err("keyboard hook is unavailable".into()) }
                        else { enabled = value; tap.reset(); Ok(()) }
                    });
                },
                Command::Reset => tap.reset(),
            }
        }
        match listener.recv_observed_timeout(Duration::from_millis(10)) {
            Ok(ObservedEvent::Lifecycle(lifecycle)) => {
                tap.reset(); recorded = None;
                if matches!(lifecycle, Lifecycle::Locked | Lifecycle::Suspended | Lifecycle::HooksUnavailable) { enabled = false; }
                if lifecycle == Lifecycle::HooksUnavailable { hook_available = false; }
                if lifecycle == Lifecycle::HooksReinstalled { hook_available = true; }
                callback(HotkeyEvent::LifecycleReset(format!("{lifecycle:?}")));
                if lifecycle == Lifecycle::HooksUnavailable { callback(HotkeyEvent::Unavailable("hook reinstall failed".into())); }
            },
            Ok(ObservedEvent::Key { origin: KeyOrigin::Reconciled, .. }) => { tap.reset(); recorded = None; },
            Ok(ObservedEvent::Key { event, origin: KeyOrigin::Physical }) if capture => {
                // Capture a complete gesture and keep recording Toggle disabled.
                if event.is_key_down {
                    if let Ok(value) = event.as_hotkey() { recorded = Some(value); }
                } else if event.modifiers.is_empty() && event.key.is_none() ||
                    !event.is_key_down && recorded.is_some_and(|r| r.key == event.key && r.key.is_some()) {
                    if let Some(value) = recorded.take() {
                        if parse_shortcut(&value.to_handy_string()).is_ok() { callback(HotkeyEvent::Captured(value.to_handy_string())); }
                    }
                }
            },
            Ok(ObservedEvent::Key { event, origin: KeyOrigin::Physical }) if enabled => {
                if tap.feed(event, Instant::now()) && (tap.shortcut.key.is_some() || !unrelated_key_is_held()) { callback(HotkeyEvent::Toggle); }
            },
            Ok(_) | Err(handy_keys::Error::Timeout) => {},
            Err(error) => { callback(HotkeyEvent::Unavailable(error.to_string())); break; },
        }
    }
}

#[cfg(windows)]
fn unrelated_key_is_held() -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
    // Include keys held before hook startup; modifier-only tapping must not
    // appear independent simply because their down event predates the listener.
    (0x08..=0xFE).filter(|vk| !matches!(*vk, 0x10..=0x12 | 0x5B..=0x5C | 0xA0..=0xA5))
        .any(|vk| unsafe { GetAsyncKeyState(vk) } as u16 & 0x8000 != 0)
}

#[cfg(not(windows))]
fn run(_: Hotkey, _: Duration, _: Receiver<Command>, _: Arc<AtomicBool>,
    ready: Sender<std::result::Result<(), String>>, _: impl Fn(HotkeyEvent)) {
    let _ = ready.send(Err("Windows is the MVP target".into()));
}

#[cfg(test)]
mod tests {
    use super::*;
    use handy_keys::Modifiers;
    fn modifier(modifiers: Modifiers, changed: Modifiers, down: bool) -> KeyEvent {
        KeyEvent { modifiers, key: None, is_key_down: down, changed_modifier: Some(changed) }
    }
    fn key(value: Key, modifiers: Modifiers, down: bool) -> KeyEvent {
        KeyEvent { modifiers, key: Some(value), is_key_down: down, changed_modifier: None }
    }
    fn tap() -> ShortcutTap { ShortcutTap::new("alt_right".parse().unwrap(), Duration::from_secs(1)) }
    #[test] fn tap_requires_full_independent_release() {
        let mut t = tap(); let now = Instant::now();
        assert!(!t.feed(modifier(Modifiers::OPT_RIGHT, Modifiers::OPT_RIGHT, true), now));
        assert!(t.feed(modifier(Modifiers::empty(), Modifiers::OPT_RIGHT, false), now));
        assert!(!t.feed(modifier(Modifiers::empty(), Modifiers::OPT_RIGHT, false), now));
    }
    #[test] fn altgr_chord_never_toggles() {
        let mut t = tap(); let n = Instant::now();
        t.feed(modifier(Modifiers::OPT_RIGHT, Modifiers::OPT_RIGHT, true), n);
        t.feed(key(Key::Q, Modifiers::OPT_RIGHT, true), n);
        t.feed(key(Key::Q, Modifiers::OPT_RIGHT, false), n);
        assert!(!t.feed(modifier(Modifiers::empty(), Modifiers::OPT_RIGHT, false), n));
    }
    #[test] fn reset_cancels_synthetic_or_late_release() {
        let mut t = tap(); let n = Instant::now();
        t.feed(modifier(Modifiers::OPT_RIGHT, Modifiers::OPT_RIGHT, true), n); t.reset();
        assert!(!t.feed(modifier(Modifiers::empty(), Modifiers::OPT_RIGHT, false), n));
    }
    #[test] fn side_and_extra_modifiers_are_preserved() {
        let mut t = tap(); let n = Instant::now();
        assert!(!t.feed(modifier(Modifiers::OPT_LEFT, Modifiers::OPT_LEFT, true), n));
        assert!(!t.feed(modifier(Modifiers::empty(), Modifiers::OPT_LEFT, false), n));
        t.feed(modifier(Modifiers::OPT_RIGHT, Modifiers::OPT_RIGHT, true), n);
        t.feed(modifier(Modifiers::OPT_RIGHT | Modifiers::CTRL_LEFT, Modifiers::CTRL_LEFT, true), n);
        t.feed(modifier(Modifiers::OPT_RIGHT, Modifiers::CTRL_LEFT, false), n);
        assert!(!t.feed(modifier(Modifiers::empty(), Modifiers::OPT_RIGHT, false), n));
    }
    #[test] fn repeat_cannot_extend_hold() {
        let mut t = tap(); let n = Instant::now();
        t.feed(modifier(Modifiers::OPT_RIGHT, Modifiers::OPT_RIGHT, true), n);
        t.feed(modifier(Modifiers::OPT_RIGHT, Modifiers::OPT_RIGHT, true), n + Duration::from_millis(900));
        assert!(!t.feed(modifier(Modifiers::empty(), Modifiers::OPT_RIGHT, false), n + Duration::from_millis(1200)));
    }
    #[test] fn configurable_chord_toggles_once_on_release() {
        let mut t = ShortcutTap::new("ctrl+k".parse().unwrap(), Duration::from_secs(1)); let n = Instant::now();
        t.feed(modifier(Modifiers::CTRL_LEFT, Modifiers::CTRL_LEFT, true), n);
        assert!(!t.feed(key(Key::K, Modifiers::CTRL_LEFT, true), n));
        assert!(!t.feed(key(Key::K, Modifiers::CTRL_LEFT, true), n));
        assert!(t.feed(key(Key::K, Modifiers::CTRL_LEFT, false), n));
        assert!(!t.feed(key(Key::K, Modifiers::CTRL_LEFT, false), n));
    }
    #[test] fn timed_out_preference_update_cannot_apply_later() {
        let (tx, _rx) = bounded(1);
        let state = Arc::new(Mutex::new(UpdateState::Cancelled));
        let ack = UpdateAck { signal: tx, state };
        let mut applied = false; ack.apply(|| { applied = true; Ok(()) });
        assert!(!applied);
    }
    // These runners own real threads/resources but never install native hooks.
    struct ReleaseProbe(Arc<AtomicBool>);
    impl Drop for ReleaseProbe {
        fn drop(&mut self) { self.0.store(true, Ordering::SeqCst); }
    }
    fn assert_retryable_owner(mut owner: HotkeyService, release: Sender<()>, released: Arc<AtomicBool>) {
        assert!(!owner.owner_released());
        assert!(!released.load(Ordering::SeqCst));
        assert!(matches!(owner.shutdown(Duration::ZERO), Err(PlatformError::Timeout("hook shutdown"))));
        assert!(owner.worker.is_some(), "timeout must not detach the actual owner");
        assert!(matches!(owner.set_capture(true), Err(PlatformError::Disconnected)));
        assert!(matches!(owner.resume(), Err(PlatformError::Disconnected)));
        assert!(matches!(owner.reset(), Err(PlatformError::Disconnected)));
        release.send(()).unwrap();
        owner.shutdown(Duration::from_secs(2)).unwrap();
        assert!(released.load(Ordering::SeqCst), "ACK follows resource destruction");
        assert!(owner.owner_released());
        assert!(owner.worker.is_none(), "successful shutdown actually joined");
        owner.shutdown(Duration::ZERO).unwrap();
    }
    #[test]
    fn readiness_timeout_preserves_owner_until_actual_cleanup() {
        let (release, wait) = bounded(1);
        let released = Arc::new(AtomicBool::new(false));
        let probe = ReleaseProbe(Arc::clone(&released));
        let failure = HotkeyService::start_worker(Duration::ZERO, move |_, stopping, ready| {
            let _probe = probe;
            wait.recv().unwrap();
            assert!(stopping.load(Ordering::SeqCst));
            // A late constructor response cannot turn a returned failure into ready.
            let _ = ready.send(Ok(()));
        }).unwrap_err();
        let (error, owner) = failure.into_parts();
        assert!(matches!(error, PlatformError::Timeout("hook readiness")));
        assert_retryable_owner(owner.unwrap(), release, released);
    }
    #[test]
    fn nested_startup_cleanup_join_keeps_outer_owner_until_inner_exit() {
        let (release, wait) = bounded(1);
        let released = Arc::new(AtomicBool::new(false));
        let probe = ReleaseProbe(Arc::clone(&released));
        let failure = HotkeyService::start_worker(Duration::ZERO, move |_, _, ready| {
            // Mirrors the vendored timeout cleanup: outer constructor cannot
            // return until its actual inner resource-owner thread has joined.
            let inner = thread::spawn(move || {
                let _probe = probe;
                wait.recv().unwrap();
            });
            inner.join().unwrap();
            let _ = ready.send(Err("nested constructor readiness timeout".into()));
        }).unwrap_err();
        assert!(matches!(failure.error, PlatformError::Timeout("hook readiness")));
        assert_retryable_owner(failure.owner.unwrap(), release, released);
    }
    #[test]
    fn readiness_error_preserves_owner_while_cleanup_is_pending() {
        let (release, wait) = bounded(1);
        let released = Arc::new(AtomicBool::new(false));
        let probe = ReleaseProbe(Arc::clone(&released));
        let failure = HotkeyService::start_worker(Duration::from_secs(2), move |_, _, ready| {
            let _probe = probe;
            ready.send(Err("watcher initialization failed".into())).unwrap();
            wait.recv().unwrap();
        }).unwrap_err();
        assert!(matches!(failure.error, PlatformError::Unavailable(_)));
        assert_retryable_owner(failure.owner.unwrap(), release, released);
    }
    #[test]
    fn readiness_disconnect_preserves_owner_while_cleanup_is_pending() {
        let (release, wait) = bounded(1);
        let released = Arc::new(AtomicBool::new(false));
        let probe = ReleaseProbe(Arc::clone(&released));
        let failure = HotkeyService::start_worker(Duration::from_secs(2), move |_, _, ready| {
            let _probe = probe;
            drop(ready);
            wait.recv().unwrap();
        }).unwrap_err();
        assert!(matches!(failure.error, PlatformError::Disconnected));
        assert_retryable_owner(failure.owner.unwrap(), release, released);
    }
    #[test]
    fn ready_owner_shutdown_timeout_is_retryable_and_ack_is_after_drop() {
        let (release, wait) = bounded(1);
        let released = Arc::new(AtomicBool::new(false));
        let probe = ReleaseProbe(Arc::clone(&released));
        let owner = HotkeyService::start_worker(Duration::from_secs(2), move |_, _, ready| {
            let _probe = probe;
            ready.send(Ok(())).unwrap();
            wait.recv().unwrap();
        }).unwrap();
        assert!(owner.ready);
        assert_retryable_owner(owner, release, released);
    }
    #[test]
    fn owner_panic_is_reported_once_after_join_then_release_is_idempotent() {
        let (release, wait) = bounded(1);
        let released = Arc::new(AtomicBool::new(false));
        let probe = ReleaseProbe(Arc::clone(&released));
        let mut owner = HotkeyService::start_worker(Duration::from_secs(2), move |_, _, ready| {
            let _probe = probe;
            ready.send(Ok(())).unwrap();
            wait.recv().unwrap();
            panic!("isolated owner failure");
        }).unwrap();
        release.send(()).unwrap();
        assert!(matches!(owner.shutdown(Duration::from_secs(2)), Err(PlatformError::Unavailable(_))));
        assert!(released.load(Ordering::SeqCst));
        assert!(owner.worker.is_none());
        assert!(owner.owner_released());
        owner.shutdown(Duration::ZERO).unwrap();
    }
    #[test]
    fn invalid_shortcut_has_no_spawned_owner() {
        let failure = HotkeyService::start("not-a-valid-shortcut", Duration::from_secs(1), |_| {}).unwrap_err();
        assert!(matches!(failure.error, PlatformError::Invalid(_)));
        assert!(failure.owner.is_none());
    }

}
