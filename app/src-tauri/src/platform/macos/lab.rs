//! Explicit debug-only native acceptance lab. No business credentials or user documents.
use std::{path::{Path, PathBuf}, time::{Duration, Instant}, process::{Command, Child}};
use objc2::{class, msg_send, sel, rc::Retained, runtime::{AnyObject, Sel}};
use objc2_foundation::{NSString, NSPoint, NSSize, NSRect, NSRange};
use crate::platform::{delivery, macos};
use serde_json::json;
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
pub fn run(args: impl Iterator<Item=String>) -> Result<()> {
    let args: Vec<_> = args.collect();
    match args.first().map(String::as_str) {
        Some("--host") => host(Path::new(args.get(1).ok_or("missing fixture root")?)),
        Some("--delivery") => delivery_test(args.get(1).map(Path::new)),
        Some("--keychain") => keychain_test(),
        Some("--audio") => audio_test(),
        Some("--request-accessibility") => {
            macos::permissions::request_accessibility();
            println!("{}", json!({"accessibilityRequest":"PASS", "status":macos::status()}));
            Ok(())
        },
        None => { println!("{}", serde_json::to_string(&macos::status())?); Ok(()) },
        _ => Err("usage: macos_native_probe [--delivery [fixture-executable]|--keychain|--audio|--request-accessibility]".into()),
    }
}
fn audio_test() -> Result<()> {
    use crate::platform::audio;
    if !macos::microphone_granted() { return Err("microphone permission required".into()); }
    let mut capture = audio::CaptureHandle::start(None)?;
    let deadline = Instant::now() + Duration::from_secs(1);
    let mut frames = 0u64;
    while Instant::now() < deadline {
        if capture.frames.recv_timeout(Duration::from_millis(50)).is_ok() { frames += 1; }
        while capture.events.try_recv().is_ok() {}
    }
    capture.request_stop();
    while capture.frames.recv_timeout(Duration::from_millis(100)).is_ok() { frames += 1; }
    let summary = capture.wait_drained(Duration::from_secs(3))?;
    if !summary.complete || !summary.first_samples_received || frames == 0 || !audio::capture_resources_released() { return Err("native audio capture/drain failed".into()); }
    println!("{}", json!({"coreAudio":"PASS","frames":frames,"sampleRate":summary.input_sample_rate,"durationMs":summary.duration_ms(),"ownerReleased":true,"audioSaved":false}));
    Ok(())
}
fn keychain_test() -> Result<()> {
    let service = format!("SpeakMode.Test.{}", uuid::Uuid::new_v4());
    let entry = keyring::Entry::new(&service, "fixture")?;
    let value = format!("synthetic-{}", uuid::Uuid::new_v4());
    entry.set_password(&value)?;
    let read = entry.get_password();
    let deleted = entry.delete_credential();
    let matches = read.as_ref().is_ok_and(|saved| saved == &value);
    deleted?;
    if !matches || !matches!(entry.get_password(), Err(keyring::Error::NoEntry)) { return Err("native Keychain round trip failed".into()); }
    println!("{}", json!({"keychain":"PASS","credentialRemoved":true}));
    Ok(())
}
struct ClipboardBackup { items: objc2::rc::Retained<AnyObject>, owned_change: Option<isize> }
impl ClipboardBackup {
    fn new() -> Result<Self> {
        objc2::rc::autoreleasepool(|_| unsafe {
            let pasteboard: *mut AnyObject = msg_send![class!(NSPasteboard), generalPasteboard];
            let items: *mut AnyObject = msg_send![pasteboard, pasteboardItems];
            let copies: *mut AnyObject = msg_send![class!(NSMutableArray), array];
            let count: usize = if items.is_null() { 0 } else { msg_send![items, count] };
            let mut bytes = 0usize;
            for index in 0..count {
                let item: *mut AnyObject = msg_send![items, objectAtIndex: index];
                let types: *mut AnyObject = msg_send![item, types];
                let count: usize = msg_send![types, count];
                let copy: *mut AnyObject = msg_send![class!(NSPasteboardItem), new];
                let copy = objc2::rc::Retained::from_raw(copy).ok_or("clipboard snapshot unavailable")?;
                for index in 0..count {
                    let kind: *mut AnyObject = msg_send![types, objectAtIndex: index];
                    let data: *mut AnyObject = msg_send![item, dataForType: kind];
                    if data.is_null() { return Err("cannot snapshot a promised clipboard item; lab skipped".into()); }
                    let length: usize = msg_send![data, length];
                    bytes = bytes.checked_add(length).ok_or("clipboard snapshot too large")?;
                    if bytes > 16 * 1024 * 1024 { return Err("clipboard snapshot too large; lab skipped".into()); }
                    let ok: bool = msg_send![&*copy, setData: data, forType: kind];
                    if !ok { return Err("clipboard snapshot unavailable".into()); }
                }
                let _: () = msg_send![copies, addObject: &*copy];
            }
            Ok(Self { items: objc2::rc::Retained::retain(copies).ok_or("clipboard snapshot unavailable")?, owned_change: None })
        })
    }
    fn mark_owned(&mut self) { unsafe {
        let pasteboard: *mut AnyObject = msg_send![class!(NSPasteboard), generalPasteboard];
        self.owned_change = Some(msg_send![pasteboard, changeCount]);
    } }
}
impl Drop for ClipboardBackup {
    fn drop(&mut self) { objc2::rc::autoreleasepool(|_| unsafe {
        let pasteboard: *mut AnyObject = msg_send![class!(NSPasteboard), generalPasteboard];
        let current: isize = msg_send![pasteboard, changeCount];
        if self.owned_change == Some(current) {
            let _: isize = msg_send![pasteboard, clearContents];
            let _: bool = msg_send![pasteboard, writeObjects: &*self.items];
        }
    }); }
}
struct Fixture { child: Child, root: PathBuf }
impl Drop for Fixture { fn drop(&mut self) { let _ = self.child.kill(); let _ = self.child.wait(); let _ = std::fs::remove_dir_all(&self.root); } }
fn wait_for(mut check: impl FnMut() -> bool) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !check() { if Instant::now() > deadline { return Err("fixture timeout".into()); } std::thread::sleep(Duration::from_millis(30)); }
    Ok(())
}
fn fixture_diagnostics(pid: u32) -> serde_json::Value {
    // Only the freshly spawned fixture's AX metadata. Never read AXValue text.
    use super::ffi::*;
    unsafe {
        let Some(app) = Owned::new(AXUIElementCreateApplication(pid as i32)) else { return json!({"application":"unavailable"}); };
        AXUIElementSetMessagingTimeout(app.0, 0.08);
        let name = NSString::from_str("AXFocusedUIElement");
        let mut value = std::ptr::null();
        let error = AXUIElementCopyAttributeValue(app.0, (&*name as *const NSString).cast(), &mut value);
        if error != 0 { return json!({"focusedElementError":error}); }
        let Some(element) = Owned::new(value) else { return json!({"focusedElement":"null"}); };
        AXUIElementSetMessagingTimeout(element.0, 0.08);
        let range = attribute(element.0, "AXSelectedTextRange");
        let enabled_error = attribute_result(element.0, "AXEnabled").err();
        json!({
            "role":attribute(element.0,"AXRole").and_then(|v|string(v.0,64)),
            "subrole":attribute(element.0,"AXSubrole").and_then(|v|string(v.0,64)),
            "enabled":attribute(element.0,"AXEnabled").and_then(|v|boolean(v.0)),
            "enabledError":enabled_error,
            "valueSettable":settable(element.0,"AXValue"),
            "selectedTextSettable":settable(element.0,"AXSelectedText"),
            "rangePresent":range.is_some(),
            "rangeType":range.as_ref().filter(|v|CFGetTypeID(v.0)==AXValueGetTypeID()).map(|v|AXValueGetType(v.0)),
            "windowPresent":attribute(element.0,"AXWindow").is_some(),
            "topLevelPresent":attribute(element.0,"AXTopLevelUIElement").is_some(),
        })
    }
}
fn delivery_test(fixture_executable: Option<&Path>) -> Result<()> {
    let permissions = macos::status();
    if !permissions.accessibility || permissions.secure_input {
        println!("{}", json!({"delivery":"NOT_RUN","reason":"accessibility permission required; secure input must be off"}));
        return Err("native delivery prerequisites unavailable".into());
    }
    let root = std::env::temp_dir().join(format!("speakmode-native-fixture-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&root)?;
    // An explicit external fixture lets its UI evolve without rebuilding the
    // authorized driver. Default remains the self-contained debug app.
    let executable = match fixture_executable { Some(path)=>path.canonicalize()?,None=>std::env::current_exe()? };
    let mut command=Command::new(executable);
    if fixture_executable.is_none() && std::env::args().nth(1).as_deref()==Some("--native-probe") { command.arg("--native-probe"); }
    let child = command.arg("--host").arg(&root).spawn()?;
    let fixture = Fixture { child, root };
    wait_for(|| fixture.root.join("ready").exists() && macos::frontmost_pid() == Some(fixture.child.id()))?;
    let mut clipboard = ClipboardBackup::new()?;
    let mut target = None;
    let ready = wait_for(|| {
        if macos::frontmost_pid() != Some(fixture.child.id()) { return false; }
        target = delivery::capture_target().ok();
        target.is_some()
    });
    if let Err(error) = ready {
        eprintln!("fixture AX metadata: {}", fixture_diagnostics(fixture.child.id()));
        return Err(error);
    }
    let target = target.ok_or("fixture target unavailable")?;
    if target.process_id != fixture.child.id() { return Err("fixture does not own target".into()); }
    let permit = delivery::DeliveryPermit::from_committed_attempt("native-fixture".into(), 1);
    let receipt = delivery::dispatch_once(&permit, &target, "SpeakMode 中文🙂", || true);
    if receipt.clipboard_changed { clipboard.mark_owned(); }
    if receipt.input_events != 4 { return Err("paste dispatch failed".into()); }
    wait_for(|| std::fs::read_to_string(fixture.root.join("text")).is_ok_and(|text| text == "SpeakMode 中文🙂"))?;
    // Consumed permits must not dispatch twice, even when supplied the new caret.
    let current = delivery::capture_target()?;
    let context = delivery::capture_input_context(&current)?;
    if context.before_text != "SpeakMode 中文🙂" || !context.selected_text.is_empty() || !context.after_text.is_empty() {
        return Err("fixture bounded Unicode context mismatch".into());
    }
    let duplicate = delivery::dispatch_once(&permit, &current, "DUPLICATE", || true);
    if duplicate.input_events != 0 { return Err("duplicate dispatch".into()); }
    let current = delivery::capture_target()?;
    std::fs::write(fixture.root.join("select-start"), b"1")?;
    wait_for(|| fixture.root.join("selected").exists())?;
    let moved = delivery::DeliveryPermit::from_committed_attempt("moved".into(), 1);
    if delivery::dispatch_once(&moved, &current, "WRONG", || true).input_events != 0 { return Err("caret change was not rejected".into()); }
    let cancelled = delivery::DeliveryPermit::from_committed_attempt("cancelled".into(), 1);
    cancelled.cancel_before_injection();
    if delivery::dispatch_once(&cancelled, &current, "WRONG", || true).input_events != 0 { return Err("cancelled dispatch".into()); }
    std::fs::write(fixture.root.join("readonly"), b"1")?;
    wait_for(|| fixture.root.join("readonly-ready").exists())?;
    if delivery::capture_target().is_ok() { return Err("read-only text view was accepted".into()); }
    std::fs::write(fixture.root.join("secure"), b"1")?;
    wait_for(|| fixture.root.join("secure-ready").exists())?;
    if delivery::capture_target().is_ok() { return Err("secure text field was accepted".into()); }
    println!("{}", json!({"nativeDelivery":"PASS","unicodePaste":true,"boundedUnicodeContext":true,"singleUse":true,"changedCaretRejected":true,"cancelledRejected":true,"readonlyRejected":true,"passwordRejected":true}));
    Ok(())
}
fn host(root: &Path) -> Result<()> {
    // This process owns only its blank fixture editor; it never opens an
    // existing document, changes another app's selection, or reads user text.
    unsafe {
        let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
        let _: bool = msg_send![app, setActivationPolicy: 0isize];
        // Cmd+V is a menu key equivalent in AppKit. A bare NSTextView without
        // a standard Edit/Paste item receives no paste action from NSApp.
        let menubar = cocoa_menu("");
        let app_menu = cocoa_menu("SpeakMode");
        let app_item = cocoa_menu_item("SpeakMode", None, "");
        let _: () = msg_send![&*app_item, setSubmenu: &*app_menu];
        let _: () = msg_send![&*menubar, addItem: &*app_item];
        let edit_menu = cocoa_menu("Edit");
        let edit_item = cocoa_menu_item("Edit", None, "");
        let paste_item = cocoa_menu_item("Paste", Some(sel!(paste:)), "v");
        let _: () = msg_send![&*paste_item, setKeyEquivalentModifierMask: 1usize << 20];
        let _: () = msg_send![&*edit_menu, addItem: &*paste_item];
        let _: () = msg_send![&*edit_item, setSubmenu: &*edit_menu];
        let _: () = msg_send![&*menubar, addItem: &*edit_item];
        let _: () = msg_send![app, setMainMenu: &*menubar];
        let _: () = msg_send![app, finishLaunching];
        let window: *mut AnyObject = msg_send![class!(NSWindow), alloc];
        let rect = NSRect::new(NSPoint::new(100.0, 150.0), NSSize::new(520.0, 220.0));
        let window: *mut AnyObject = msg_send![window, initWithContentRect: rect, styleMask: 15usize, backing: 2usize, defer: false];
        let title = NSString::from_str("SpeakMode native test fixture");
        let _: () = msg_send![window, setTitle: &*title];
        let text: *mut AnyObject = msg_send![class!(NSTextView), alloc];
        let text: *mut AnyObject = msg_send![text, initWithFrame: NSRect::new(NSPoint::new(0.0,0.0),NSSize::new(520.0,220.0))];
        let _: () = msg_send![text, setRichText: false];
        let _: () = msg_send![text, setEditable: true];
        let _: () = msg_send![text, setSelectable: true];
        let _: () = msg_send![text, setSelectedRange: NSRange::new(0,0)];
        let _: () = msg_send![window, setContentView: text];
        let _: bool = msg_send![window, makeFirstResponder: text];
        let _: () = msg_send![window, makeKeyAndOrderFront: std::ptr::null::<AnyObject>()];
        let _: () = msg_send![app, activateIgnoringOtherApps: true];
        std::fs::write(root.join("ready"), b"1")?;
        let deadline = Instant::now() + Duration::from_secs(30);
        let mode = NSString::from_str("kCFRunLoopDefaultMode");
        while Instant::now() < deadline {
            objc2::rc::autoreleasepool(|_| {
                let date: *mut AnyObject = msg_send![class!(NSDate), dateWithTimeIntervalSinceNow: 0.02f64];
                let event: *mut AnyObject = msg_send![app, nextEventMatchingMask: usize::MAX, untilDate: date, inMode: &*mode, dequeue: true];
                if !event.is_null() { let _: () = msg_send![app, sendEvent: event]; }
                let _: () = msg_send![app, updateWindows];
                if root.join("select-start").exists() && !root.join("selected").exists() {
                    let _: () = msg_send![text, setSelectedRange: NSRange::new(0,0)];
                    let _ = std::fs::write(root.join("selected"), b"1");
                }
                if root.join("secure").exists() && !root.join("secure-ready").exists() {
                    let secure: *mut AnyObject = msg_send![class!(NSSecureTextField), alloc];
                    let secure: *mut AnyObject = msg_send![secure, initWithFrame: NSRect::new(NSPoint::new(20.0,20.0),NSSize::new(400.0,40.0))];
                    let _: () = msg_send![window, setContentView: secure];
                    let _: bool = msg_send![window, makeFirstResponder: secure];
                    let _ = std::fs::write(root.join("secure-ready"), b"1");
                }
                if root.join("readonly").exists() && !root.join("readonly-ready").exists() {
                    let _: () = msg_send![text, setEditable: false];
                    let _ = std::fs::write(root.join("readonly-ready"), b"1");
                }
                let value: *mut NSString = msg_send![text, string];
                if !value.is_null() { let _ = std::fs::write(root.join("text"), (*value).to_string()); }
            });
        }
        let _: () = msg_send![window, orderOut: std::ptr::null::<AnyObject>()];
    }
    Ok(())
}
fn cocoa_menu(title: &str) -> Retained<AnyObject> {
    unsafe {
        let title = NSString::from_str(title);
        let menu: *mut AnyObject = msg_send![class!(NSMenu), alloc];
        Retained::from_raw(msg_send![menu, initWithTitle: &*title]).expect("fixture menu")
    }
}
fn cocoa_menu_item(title: &str, action: Option<Sel>, key: &str) -> Retained<AnyObject> {
    unsafe {
        let title=NSString::from_str(title); let key=NSString::from_str(key);
        let item: *mut AnyObject=msg_send![class!(NSMenuItem), alloc];
        Retained::from_raw(msg_send![item, initWithTitle: &*title, action: action, keyEquivalent: &*key]).expect("fixture menu item")
    }
}
