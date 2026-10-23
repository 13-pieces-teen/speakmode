//! Keyboard listener for streaming raw key events
//!
//! This module provides a `KeyboardListener` that streams all keyboard events,
//! useful for implementing "record hotkey" UI flows.
//!
//! # Platform Notes
//!
//! - **macOS**: Uses CGEventTap. Requires accessibility permissions.
//! - **Windows**: Uses low-level keyboard hooks. Clean thread shutdown.
//! - **Linux**: Reads evdev devices directly (Wayland, X11, and console
//!   alike). Requires read access to `/dev/input` (`input` group).
//!   Blocking grabs keyboards exclusively and re-injects non-blocked
//!   events through uinput, so it additionally requires write access to
//!   `/dev/uinput`. Clean thread shutdown.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::error::{Error, Result};
use crate::types::{Hotkey, KeyEvent, Modifiers};

pub use crate::platform::state::BlockingHotkeys;

/// SpeakMode narrow Windows extension: ordering is shared with key events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyOrigin { Physical, Reconciled }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lifecycle { Locked, Suspended, Resumed, Connected, HooksReinstalled, HooksUnavailable }
#[derive(Debug, Clone, Copy)]
pub enum ObservedEvent { Key { event: KeyEvent, origin: KeyOrigin }, Lifecycle(Lifecycle) }

/// Narrow observer policy: physical downs and native chords remain untouched.
/// Only a completed standalone Alt/Win tap may replace its native release.
/// Updating any policy advances an epoch so an older hold cannot cross an ACK.
#[derive(Clone, Default)]
pub struct StandaloneTapMaskControl { state: Arc<AtomicU64> }
impl StandaloneTapMaskControl {
    pub fn configure(&self, shortcut: Option<Hotkey>, maximum: Duration) {
        let modifiers = shortcut.filter(|shortcut| shortcut.key.is_none() &&
            !shortcut.modifiers.is_empty() &&
            (Modifiers::OPT.contains(shortcut.modifiers) || Modifiers::CMD.contains(shortcut.modifiers)))
            .map(|shortcut| shortcut.modifiers.bits()).unwrap_or(0);
        let milliseconds = maximum.as_millis().min(0x003f_ffff) as u64;
        let policy = u64::from(modifiers) | (milliseconds << 10);
        #[allow(deprecated)] // Also builds with Rust versions preceding fetch_update's rename.
        let _ = self.state.fetch_update(Ordering::AcqRel, Ordering::Acquire, |state|
            Some(((state.wrapping_add(1u64 << 32)) & !0xffff_ffff) | policy));
    }
    pub fn reset(&self) {
        #[allow(deprecated)]
        let _ = self.state.fetch_update(Ordering::AcqRel, Ordering::Acquire, |state|
            Some(((state.wrapping_add(1u64 << 32)) & !0xffff_ffff) | (state & 0xffff_ffff)));
    }
    pub fn disable(&self) { self.configure(None, Duration::ZERO); }
}

/// The production hook's pure recognizer is public for no-hook behavior tests.
pub struct StandaloneTapMenuMask {
    control: StandaloneTapMaskControl,
    epoch: u64,
    started: Option<(Instant, Modifiers)>,
}
impl StandaloneTapMenuMask {
    pub fn new(control: StandaloneTapMaskControl) -> Self { Self { control, epoch: 0, started: None } }
    pub fn reset(&mut self) { self.started=None; self.control.reset(); }
    pub fn disable(&mut self) { self.started=None; self.control.disable(); }
    /// Recheck immediately before native replacement if the service changed its policy.
    pub fn policy_is_current(&self) -> bool {
        self.epoch & 0x3ff != 0 && self.control.state.load(Ordering::Acquire)==self.epoch
    }
    pub fn feed(&mut self, event: KeyEvent, origin: KeyOrigin, now: Instant,
                unrelated_held: bool) -> bool {
        let epoch=self.control.state.load(Ordering::Acquire);
        if self.epoch!=epoch {self.started=None;self.epoch=epoch;}
        let configured=Modifiers::from_bits_truncate(epoch as u32 & 0x3ff);
        if origin!=KeyOrigin::Physical || configured.is_empty() || unrelated_held {
            self.started=None;return false;
        }
        let Some(changed)=event.changed_modifier else {
            // Includes an unmapped physical key and mouse buttons: no chord is a tap.
            self.started=None;return false;
        };
        if event.is_key_down {
            if event.key.is_none() && event.modifiers.bits().count_ones()==1 &&
                configured.matches(event.modifiers) && event.modifiers==changed {
                // A repeat cannot refresh the original hold duration.
                if self.started.is_none(){self.started=Some((now,changed));}
            }else {self.started=None;}
            return false;
        }
        let Some((started,side))=self.started.take() else {return false;};
        event.key.is_none() && event.modifiers.is_empty() && changed==side &&
            now.checked_duration_since(started).is_some_and(|duration|
                duration<=Duration::from_millis(u64::from((epoch as u32)>>10)))
    }
}

/// Pure native replacement description. Modifier-up is in the same SendInput
/// batch as the mask, so it cannot overtake the mask through CallNextHookEx.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StandaloneMenuInput {
    pub virtual_key: u16, pub key_up: bool, pub extended: bool, pub extra_info: usize,
}
pub const STANDALONE_MENU_MASK_VK:u16=0xe8;
pub const STANDALONE_MENU_MASK_EXTRA_INFO:usize=0x484b_4d4d;
/// Own inert mask and all synthetic keyboard events bypass gesture recognition.
pub fn observed_keyboard_event_is_physical(virtual_key:u16,flags:u32,extra_info:usize)->bool {
    flags & 0x10 == 0 && !(virtual_key==STANDALONE_MENU_MASK_VK && extra_info==STANDALONE_MENU_MASK_EXTRA_INFO)
}
pub fn standalone_tap_has_other_modifier(side:Modifiers,physical:Modifiers,altgr_phantom_ctrl:bool)->bool {
    let physical=if altgr_phantom_ctrl{physical & !Modifiers::CTRL_LEFT}else{physical};
    !(physical & !side).is_empty()
}
pub fn standalone_menu_release_batch(virtual_key:u16) -> Option<[StandaloneMenuInput;3]> {
    let extended=match virtual_key {0xa4=>false,0xa5|0x5b|0x5c=>true,_=>return None};
    let input=|virtual_key,key_up,extended|StandaloneMenuInput{virtual_key,key_up,extended,
        extra_info:STANDALONE_MENU_MASK_EXTRA_INFO};
    Some([input(STANDALONE_MENU_MASK_VK,false,false),input(STANDALONE_MENU_MASK_VK,true,false),
        input(virtual_key,true,extended)])
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StandaloneReleaseDisposition { ReplacePhysicalRelease, PreservePhysicalRelease }
pub fn standalone_release_disposition(inserted:u32) -> StandaloneReleaseDisposition {
    if inserted==3 {StandaloneReleaseDisposition::ReplacePhysicalRelease}
    else {StandaloneReleaseDisposition::PreservePhysicalRelease}
}

/// Platform-agnostic Keyboard Listener
///
/// Streams all keyboard events. Can optionally block events that match
/// registered hotkeys.
pub struct KeyboardListener {
    event_receiver: Receiver<KeyEvent>,
    #[cfg(target_os = "macos")]
    reset_epoch: Arc<AtomicU64>,
    #[cfg(target_os = "windows")]
    observed_receiver: Option<Receiver<ObservedEvent>>,
    _thread_handle: Option<JoinHandle<()>>,
    running: Arc<AtomicBool>,
    blocking_hotkeys: Option<BlockingHotkeys>,
    standalone_tap_mask: Option<StandaloneTapMaskControl>,
    /// Backend-specific wakeup invoked on Drop after `running` is cleared.
    /// macOS parks its listener thread in a CFRunLoop with no periodic
    /// wakeups (see run_event_tap), so Drop must stop that loop explicitly.
    stop_wakeup: Option<Box<dyn Fn() + Send>>,
}

impl KeyboardListener {
    /// Create a new KeyboardListener (non-blocking mode)
    ///
    /// Events are observed but not blocked. Use this for "record hotkey" UI flows.
    ///
    /// On macOS, this will check for accessibility permissions and fail if not granted.
    pub fn new() -> Result<Self> {
        Self::new_internal(None)
    }

    /// Create a new KeyboardListener with blocking support
    ///
    /// Events matching hotkeys in the provided set will be blocked from reaching
    /// other applications. The set can be modified after creation to add/remove
    /// hotkeys dynamically.
    ///
    /// Note: On Linux this grabs keyboards exclusively and re-injects
    /// non-blocked events through uinput, so it requires write access to
    /// `/dev/uinput` (and fails with an actionable error without it).
    /// Mouse-button hotkeys are detected but not blocked — pointer devices
    /// are never grabbed (same behavior as Windows).
    pub fn new_with_blocking(blocking_hotkeys: BlockingHotkeys) -> Result<Self> {
        Self::new_internal(Some(blocking_hotkeys))
    }

    fn new_internal(blocking_hotkeys: Option<BlockingHotkeys>) -> Result<Self> {
        #[cfg(target_os = "macos")]
        {
            use crate::platform::macos::listener;
            let state = listener::spawn(blocking_hotkeys)?;
            let run_loop = state.run_loop;
            Ok(KeyboardListener {
                event_receiver: state.event_receiver,
                reset_epoch: state.reset_epoch,
                _thread_handle: state.thread_handle,
                running: state.running,
                blocking_hotkeys: state.blocking_hotkeys,
                standalone_tap_mask: None,
                stop_wakeup: Some(Box::new(move || run_loop.stop())),
            })
        }

        #[cfg(target_os = "windows")]
        {
            use crate::platform::windows::listener;
            let state = listener::spawn(blocking_hotkeys)?;
            Ok(KeyboardListener {
                event_receiver: state.event_receiver,
                #[cfg(target_os = "windows")]
                observed_receiver: state.observed_receiver,
                _thread_handle: state.thread_handle,
                running: state.running,
                blocking_hotkeys: state.blocking_hotkeys,
                standalone_tap_mask: None,
                stop_wakeup: None,
            })
        }

        #[cfg(target_os = "linux")]
        {
            use crate::platform::linux::listener;
            let state = listener::spawn(blocking_hotkeys)?;
            Ok(KeyboardListener {
                event_receiver: state.event_receiver,
                #[cfg(target_os = "windows")]
                observed_receiver: state.observed_receiver,
                _thread_handle: state.thread_handle,
                running: state.running,
                blocking_hotkeys: state.blocking_hotkeys,
                standalone_tap_mask: None,
                stop_wakeup: None,
            })
        }
    }

    /// Nonblocking observer with confirmed hook + watcher readiness.
    /// Caller must drain this stream, not recv(), on an observed listener.
    #[cfg(target_os = "windows")]
    pub fn new_observed() -> Result<Self> {
        Self::new_observed_with_tap_mask(StandaloneTapMaskControl::default())
    }
    #[cfg(target_os = "windows")]
    pub fn new_observed_with_tap_mask(control: StandaloneTapMaskControl) -> Result<Self> {
        let state = crate::platform::windows::listener::spawn_observed(Some(control.clone()))?;
        Ok(Self {
            event_receiver: state.event_receiver,
            observed_receiver: state.observed_receiver,
            _thread_handle: state.thread_handle,
            running: state.running,
            blocking_hotkeys: None,
            standalone_tap_mask: Some(control),
            stop_wakeup: None,
        })
    }

    #[cfg(target_os = "windows")]
    pub fn recv_observed_timeout(&self, timeout: Duration) -> Result<ObservedEvent> {
        self.observed_receiver.as_ref().ok_or(Error::EventLoopNotRunning)?
            .recv_timeout(timeout).map_err(|e| match e {
                std::sync::mpsc::RecvTimeoutError::Timeout => Error::Timeout,
                std::sync::mpsc::RecvTimeoutError::Disconnected => Error::EventLoopNotRunning,
            })
    }

    /// Monotonic reset boundary when macOS disables/re-enables the event tap.
    /// Consumers must discard pending gestures rather than treating reconciled
    /// releases as a fresh physical tap.
    #[cfg(target_os = "macos")]
    pub fn macos_reset_epoch(&self) -> u64 { self.reset_epoch.load(Ordering::Acquire) }

    /// Get a reference to the blocking hotkeys set (if blocking is enabled)
    pub fn blocking_hotkeys(&self) -> Option<&BlockingHotkeys> {
        self.blocking_hotkeys.as_ref()
    }

    /// Blocking receive for key events
    ///
    /// Blocks until a key event is received or the listener stops.
    pub fn recv(&self) -> Result<KeyEvent> {
        self.event_receiver
            .recv()
            .map_err(|_| Error::EventLoopNotRunning)
    }

    /// Blocking receive with timeout
    ///
    /// Blocks until a key event is received, the timeout expires, or the listener stops.
    pub fn recv_timeout(&self, timeout: Duration) -> Result<KeyEvent> {
        self.event_receiver
            .recv_timeout(timeout)
            .map_err(|e| match e {
                std::sync::mpsc::RecvTimeoutError::Timeout => Error::Timeout,
                std::sync::mpsc::RecvTimeoutError::Disconnected => Error::EventLoopNotRunning,
            })
    }

    /// Non-blocking receive for key events
    ///
    /// Returns `Some(event)` if an event is available, `None` otherwise.
    pub fn try_recv(&self) -> Option<KeyEvent> {
        match self.event_receiver.try_recv() {
            Ok(event) => Some(event),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => None,
        }
    }
}

impl Drop for KeyboardListener {
    fn drop(&mut self) {
        if let Some(control)=&self.standalone_tap_mask {control.disable();}
        self.running.store(false, Ordering::SeqCst);

        // Wake the listener thread if the backend parks it (macOS stops the
        // tap thread's CFRunLoop). Windows/Linux event loops re-check
        // `running` at least every ~100ms on their own, so the join below is
        // short and shutdown is clean on all platforms.
        if let Some(wake) = &self.stop_wakeup {
            wake();
        }
        if let Some(handle) = self._thread_handle.take() {
            let _ = handle.join();
        }
    }
}
