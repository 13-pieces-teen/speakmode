# Desktop platform implementation

The shared engine uses the same ownership and delivery policy on Windows and
macOS. The macOS boundary lives in `macos/`: CPAL/CoreAudio input, native keyring,
Accessibility target capture and bounded context, tagged Cmd+V dispatch, AppKit
overlay windows, lifecycle observers, and a peer-checked Unix maintenance socket.
Default standalone right Alt is presented as right Option on Mac. Native paste
reports dispatch without claiming confirmed insertion. Secure input, focus/selection
changes and unavailable accessibility permissions fail closed. See
[macOS build and acceptance notes](../../../../docs/macos.md) for permission setup,
native probes, actual test results and remaining acceptance work.

The following sections describe the Windows implementation and the shared
ownership contracts; Windows packaging settings live in `tauri.windows.conf.json`.

No module import performs input capture or clipboard mutation. Native operations
are explicit. The desktop coordinator owns user intent, durable state, generation
and system admission policy; this module owns native resources and mechanics.

## Dependencies

- handy-keys **0.3.4**, patched from exact upstream commit
  `e7d1b8e6de8258f21ee1d60608d16f8d195157bf`, MIT.
- CPAL **0.16.0** / WASAPI, Rubato **0.16.2**, crossbeam-channel **0.5**.
- windows **0.58** features: `Win32_Foundation`, `Win32_UI_Accessibility`,
  `Win32_UI_Input_KeyboardAndMouse`, `Win32_UI_WindowsAndMessaging`,
  `Win32_System_Com`, `Win32_System_Memory`, `Win32_System_DataExchange`,
  `Win32_System_Threading`, `Win32_Security`, `Win32_System_Ole`,
  `Win32_System_Variant`. Root may enable additional shell features.
- Serde derive and thiserror. No Python/audio subprocess is part of runtime.

Root Cargo patches crates.io handy-keys to `src/platform/vendor/handy-keys`.
The original audited upstream checkout is unchanged. The patch adds actual
constructor hook/watcher readiness, a single ordered observed key/lifecycle
stream, physical/reconciled origin, suspend/lock/resume notifications, hook
reinstall status, and injected-event filtering. Existing non-observed APIs remain.
The Windows constructor also joins its native hook thread before returning a
readiness error/timeout. SpeakMode calls it only inside its dedicated outer
startup owner thread, so this cleanup cannot block the UI or an engine lock.
See the [local-source modification record](../../../../docs/third-party/LOCAL-SOURCE-MODIFICATIONS.md) for the local-source boundary.
The app observes native chords without blocking them. In 0.1.9 a configured
independent short Alt/Win tap replaces only its release with one ordered inert
menu-mask/modifier-up batch, and only after the full batch is accepted. Injected
events never produce recording toggles. Chords, AltGr, long holds, Capture and
disabled policies retain native handling. Physical-key/menu behavior remains a
separate real Windows acceptance item; synthetic injected keys cannot prove it.

## Hotkeys

`HotkeyService::start(shortcut, maximum_tap, callback)` returns
`std::result::Result<HotkeyService, HotkeyStartError>`. It creates an owner thread
and waits at most six seconds for hook + watcher readiness. Run this startup on
a dedicated startup thread, outside UI/engine locks. The controller is Send.
Callbacks must be fast and must not synchronously call controller methods.

`HotkeyStartError { error: PlatformError, owner: Option<HotkeyService> }` and
`into_parts()` preserve ownership on every post-spawn failure. Parse/spawn errors
have no owner; readiness timeout/error/disconnect return a stopped-admission owner,
which the desktop must retain in its resource slot until retryable `shutdown`
completes. Do not clear a startup/resource gate merely because readiness failed.
A late successful start during exit must likewise be retained until cleanup ACK.

`owner_released()` observes actual thread completion, not the stop flag. Bounded
`shutdown` keeps its JoinHandle on timeout; success follows resource destruction
and join. A thread panic is reported once after join, and a later shutdown is
idempotent. Failed-start/stopping services reject capture/register/resume/reset.
Drop only requests stop and is not a release receipt. The nested vendored native
thread now remains owned until join even if its five-second readiness wait fails.

`HotkeyEvent` is Ready / Toggle / Captured / LifecycleReset / Unavailable.
`replace_shortcut`, `set_capture`, `suspend`, `resume`, `reset`, and bounded
`shutdown` are explicit. A timeout cancels the queued update under the same
transaction lock that applies it, preventing a failed settings save from silently
changing the native shortcut later. Resume/replace reject unavailable hooks.

Tap means complete down/up, not the manager's Pressed edge. Other held keys,
extra modifiers, repeat and AltGr character chords do not produce extra toggles.
Reconciliation releases reset the gesture rather than completing it. Startup-held
ordinary keys are also checked for modifier-only taps. Capture settings disable
Toggle without changing the active stored shortcut. `maximum_tap` is a caller
engineering parameter; no real keyboard threshold has been accepted here.

## Audio

`enumerate_inputs()` reads descriptors only. `CaptureHandle::start(Option<&str>)`
is the explicit microphone action and starts an independent CPAL owner thread.
`CaptureHandle` contains channels/atomic controls/JoinHandle, not CPAL Stream;
its Send bound is compile-tested.

- `frames: Receiver<AudioFrame>` has **44** 100ms frames of native capacity.
  Root bridge has 5, provider may have 1 in flight: **44 + 5 + 1 = 50**, the
  shared five-second ASR backlog bound. No extra unbounded frame bridge is allowed.
- `events: Receiver<AudioEvent>` carries FirstSamples, Level (max20Hz), Error.
- Native raw input has a two-second sample budget and a 64-message secondary
  ceiling. Callback only normalizes/downmixes and try-sends. It does no network,
  disk or resampling; overflow stops acceptance and marks incomplete.
- `request_stop()` atomically fixes acceptance cutoff. The owner drops CPAL
  Stream and waits for its native callback thread to terminate, then drains all
  already accepted input and all FrameResampler.finish emissions.
- `cancel()` has an independent atomic flag so a previously queued Stop cannot
  suppress cancellation. No successful completed summary is issued on cancellation.
- `wait_drained(Duration)` permits short polling while root keeps draining frames.
  A returned summary means owner processing finished; root must still drain its
  bridge/provider queue before sending provider Finish. Timeout is not release.
- `capture_resources_released()` stays false if a late startup/cleanup thread has
  not really exited. Native start independently rejects a second owner.

`CaptureSummary` includes completeness, cancellation, true captured mono sample
count/native rate, first sample, emitted frames and error. Duration excludes
resampler/frame padding. AudioFrame includes sequence, PCM16 samples, padding
count and `to_le_bytes()`; each successful emission increments sequence once.

CPAL 0.16 does not expose stable WASAPI endpoint identity in its public Device
API. This implementation uses unique friendly names as settings IDs; duplicate
names reject explicit selection, and disappearing devices do not silently switch.
This is a documented compatibility gap for a later IMMDevice endpoint-ID adapter.

## Targets, clipboard and dispatch

`capture_foreground_anchor()` takes a cheap native foreground/process/focus
snapshot. The coordinator binds it when stopping dictation, before the heavier
input inspection. `capture_target_for_anchor(&ForegroundAnchor)` checks that
identity before and after capture; `validate_target(&TargetSnapshot)` checks the
target again before delivery. Verified standard Edit/RichEdit controls use native
style and selection queries. Other supported controls use UIA on a separate MTA
thread with 200ms UIA connection/transaction limits and a one-second caller
deadline. Snapshot stores HWND/PID/process-creation time, focused HWND, runtime ID
and UTF-16 selection offsets. No target content/title is returned or persisted.
To count the caret offset, UIA transiently reads the selected input's bounded
prefix (<=65,536 UTF-16 units); it is never logged or sent to the provider.

Own window, password, disabled, read-only, unknown editable capability, higher
integrity and unobservable selection targets reject automatic delivery. Different
input fields under the same browser window are compared by UIA identity. The
caret/process/control must still match. Unknown host capabilities use copy.
There is no SetForegroundWindow/force-refocus or fallback paste into the current
foreground. Validation and SendInput are inherently not fully atomic.

`copy_text(&str)` uses UTF-16/CF_UNICODETEXT and an owned hidden clipboard window,
with a 200ms clipboard contention bound. It keeps the result in clipboard and
does not restore old formats. Invalid embedded NUL/oversized content rejects.

Core creates `DeliveryPermit::from_committed_attempt(id, generation)` **only after
the unique attempt's durable write-ahead commit**. The permit is consumed once;
it is not the DB ledger. `cancel_before_injection()` can revoke prepared/claimed
attempts, and cannot undo a native injection already entered.

`dispatch_once(&permit, &target, text, final_gate)` returns DispatchReceipt:
Skipped / Failed / Unconfirmed, clipboard_changed, input_events, reason. It
checks generation/cancel/quit gate and target before clipboard and again before
the native chord. Cancellation after clipboard reports that local side effect.
Physical modifiers have a bounded 350ms release wait, with cancellation/gate
checks; a timeout rejects dispatch rather than forcibly releasing user keys.
There is one Ctrl+V chord. Partial SendInput only releases keys introduced by
the chord; it never repeats V-down. Consumed permits never repeat input.

**Product delivery does not perform insertion readback.** Even SendInput count=4
returns Unconfirmed; the module cannot claim Confirmed insertion. UIA Edit,
Document and Group candidates require a positive writable-capability check plus
an observable TextPattern selection. A readable Group/Document alone is not
editable proof. The Group path covers Chromium contenteditable using a strict
boolean false IsReadOnly text attribute. Unknown/mixed/read-only values reject.
Controlled editor/browser test readback is separate from this runtime receipt;
it cannot establish compatibility with every application or every input field.

## Verification

The clean repository uses the production Cargo library target. Earlier isolated
harnesses, GNU build caches and machine-local receipts are not included, and
their old test counts are not current validation of this export. See
[repository validation](../../../../docs/validation.md) for this run's status,
historical Windows editor/browser evidence and remaining acceptance work.

Run from the repository's `app/` directory on Windows with Rust/MSVC and the
desktop build prerequisites available:

```powershell
cargo test --locked --manifest-path src-tauri/Cargo.toml --features desktop --target x86_64-pc-windows-msvc --lib
cargo test --locked --manifest-path src-tauri/Cargo.toml --features desktop --target x86_64-pc-windows-msvc --lib platform:: -- --test-threads=1
```

The filtered command tests this application's platform policy, ownership and
resampler behavior. Hotkey owner tests cover timeout, initialization failure,
readiness disconnect, retryable cleanup, actual owner-thread release, panic and
idempotent join. They include an inner resource-owner join that blocks outer
startup cleanup. Those tests do not install real OS hooks. Library compilation
and fake-operation dispatch tests do not prove real microphone capture,
physical AltGr/tap/system recovery, arbitrary-host paste, latency or installation.

Source notices are retained as [handy-keys LICENSE](vendor/handy-keys/LICENSE)
and [HANDY-LICENSE.txt](HANDY-LICENSE.txt); the extracted resampler's fixed source
commit/path remains in [resampler.rs](resampler.rs). The broader
[third-party record](../../../../docs/third-party/README.md) distinguishes these
materials from full release/source-provenance verification.
