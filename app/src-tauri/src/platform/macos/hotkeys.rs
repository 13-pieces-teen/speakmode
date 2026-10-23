use super::*;
use crate::platform::macos::ffi::{CGEventSourceKeyState, AXIsProcessTrusted, IsSecureEventInputEnabled};
use handy_keys::KeyboardListener;

pub(super) fn run(shortcut: Hotkey, maximum: Duration, control: Receiver<Command>, stopping: Arc<AtomicBool>,
    ready: Sender<std::result::Result<(), String>>, callback: impl Fn(HotkeyEvent)) {
    // Standalone modifiers remain available for normal Option/Command chords.
    // Only key-bearing configured shortcuts are blocked by the event tap.
    let blocking = handy_keys::BlockingHotkeys::default();
    let listener = match KeyboardListener::new_with_blocking(blocking.clone()) {
        Ok(listener) => listener,
        Err(error) => { let _ = ready.send(Err(error.to_string())); return; }
    };
    let observers = crate::platform::macos::lifecycle::Observers::new();
    let mut tap = ShortcutTap::new(shortcut, maximum);
    let mut enabled = true;
    let mut capture = false;
    let mut recorded: Option<Hotkey> = None;
    let update_blocking = |shortcut: Hotkey, enabled: bool| {
        if let Ok(mut set) = blocking.lock() { set.clear(); if enabled && shortcut.key.is_some() { set.insert(shortcut); } }
    };
    update_blocking(shortcut, true);
    if stopping.load(Ordering::SeqCst) { return; }
    let _ = ready.send(Ok(()));
    callback(HotkeyEvent::Ready);
    let mut tap_epoch = listener.macos_reset_epoch();
    let mut last_tick = Instant::now();
    let mut was_secure = unsafe { IsSecureEventInputEnabled() };
    while !stopping.load(Ordering::SeqCst) {
        while let Ok(command) = control.try_recv() {
            match command {
                Command::Replace(shortcut, ack) => ack.apply(|| { tap = ShortcutTap::new(shortcut, maximum); update_blocking(shortcut, enabled && !capture); Ok(()) }),
                Command::Capture(value, ack) => ack.apply(|| { capture = value; recorded = None; tap.reset(); update_blocking(tap.shortcut, enabled && !capture); Ok(()) }),
                Command::Enable(value, ack) => ack.apply(|| { enabled = value; tap.reset(); update_blocking(tap.shortcut, enabled && !capture); Ok(()) }),
                Command::Reset => { tap.reset(); recorded = None; },
            }
        }
        while let Ok(reason) = observers.events.try_recv() {
            tap.reset(); recorded = None;
            while listener.try_recv().is_some() {}
            callback(HotkeyEvent::LifecycleReset(reason.into()));
        }
        if !unsafe { AXIsProcessTrusted() } {
            callback(HotkeyEvent::Unavailable("辅助功能权限已撤销".into())); break;
        }
        let now = Instant::now();
        let secure = unsafe { IsSecureEventInputEnabled() };
        if now.duration_since(last_tick) > Duration::from_secs(2) || secure != was_secure {
            tap.reset(); recorded = None;
            callback(HotkeyEvent::LifecycleReset(if secure { "SecureInput" } else { "Resumed" }.into()));
        }
        last_tick = now; was_secure = secure;
        let event = listener.recv_timeout(Duration::from_millis(50));
        let epoch = listener.macos_reset_epoch();
        if epoch != tap_epoch {
            tap_epoch = epoch; tap.reset(); recorded = None;
            while listener.try_recv().is_some() {}
            callback(HotkeyEvent::LifecycleReset("HooksReinstalled".into()));
            continue;
        }
        match event {
            Ok(_) if secure => { tap.reset(); recorded = None; },
            Ok(event) if capture => {
                if event.is_key_down {
                    if let Ok(value) = event.as_hotkey() { recorded = Some(value); }
                } else if (event.modifiers.is_empty() && event.key.is_none()) ||
                    recorded.is_some_and(|value| value.key.is_some() && value.key == event.key) {
                    if let Some(value) = recorded.take() {
                        if parse_shortcut(&value.to_handy_string()).is_ok() { callback(HotkeyEvent::Captured(value.to_handy_string())); }
                    }
                }
            },
            Ok(event) if enabled => {
                if tap.feed(event, Instant::now()) && (tap.shortcut.key.is_some() || !unrelated_key_is_held()) { callback(HotkeyEvent::Toggle); }
            },
            Ok(_) | Err(handy_keys::Error::Timeout) => {},
            Err(error) => { callback(HotkeyEvent::Unavailable(error.to_string())); break; },
        }
    }
    update_blocking(tap.shortcut, false);
    // KeyboardListener's Drop wakes and joins the CFRunLoop owner before this
    // service thread completes; shutdown ACK cannot precede native release.
}
fn unrelated_key_is_held() -> bool {
    (0..128).filter(|key| !matches!(key, 0x36..=0x3f)).any(|key| unsafe { CGEventSourceKeyState(1, key) })
}
