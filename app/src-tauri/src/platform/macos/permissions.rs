use super::ffi::*;
use objc2::{class, msg_send, runtime::AnyObject};
use objc2_foundation::{NSDictionary, NSNumber, NSString};
use serde::Serialize;
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionStatus { pub platform: &'static str, pub microphone: &'static str, pub accessibility: bool, pub input_monitoring: bool, pub secure_input: bool }
pub fn microphone_status() -> &'static str {
    unsafe {
        let media = AVMediaTypeAudio as *const AnyObject;
        let value: isize = msg_send![class!(AVCaptureDevice), authorizationStatusForMediaType: media];
        match value { 0 => "notDetermined", 1 => "restricted", 2 => "denied", 3 => "granted", _ => "unknown" }
    }
}
pub fn microphone_granted() -> bool { microphone_status() == "granted" }
pub fn status() -> PermissionStatus {
    PermissionStatus { platform: "macos", microphone: microphone_status(), accessibility: unsafe { AXIsProcessTrusted() }, input_monitoring: unsafe { CGPreflightListenEventAccess() }, secure_input: unsafe { IsSecureEventInputEnabled() } }
}
pub fn request_microphone() {
    if microphone_status() == "notDetermined" {
        let completion = block2::RcBlock::new(|_: objc2::runtime::Bool| {});
        unsafe { let _: () = msg_send![class!(AVCaptureDevice), requestAccessForMediaType: AVMediaTypeAudio as *const AnyObject, completionHandler: &*completion]; }
    } else if !microphone_granted() { open_settings("microphone"); }
}
fn accessibility_trusted(prompt: bool) -> bool {
    unsafe {
        // Use Apple's exported key and a retaining, value-comparing dictionary.
        // A raw CFDictionary with NULL callbacks compares keys by address; a
        // freshly allocated NSString then cannot be found by the system API.
        let key = &*kAXTrustedCheckOptionPrompt.cast::<NSString>();
        let value = NSNumber::new_bool(prompt);
        let options = NSDictionary::from_slices(&[key], &[&*value]);
        AXIsProcessTrustedWithOptions((&*options as *const NSDictionary<NSString, NSNumber>).cast())
    }
}
pub fn request_accessibility() {
    accessibility_trusted(true);
    open_settings("accessibility");
}
pub fn request_input_monitoring() { unsafe { CGRequestListenEventAccess(); } open_settings("inputMonitoring"); }
pub fn open_settings(permission: &str) {
    let pane = match permission { "microphone" => "Privacy_Microphone", "inputMonitoring" => "Privacy_ListenEvent", _ => "Privacy_Accessibility" };
    // Fixed system URL; no IPC-provided URL or shell interpolation.
    let _ = std::process::Command::new("/usr/bin/open").arg(format!("x-apple.systempreferences:com.apple.preference.security?{pane}")).spawn();
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn accessibility_options_are_valid_on_a_worker_thread_without_prompting() {
        // Exercise the exact native call that crashed, including dictionary
        // creation/release. Never prompt or alter the user's permission state.
        std::thread::spawn(|| {
            for _ in 0..16 {
                assert_eq!(accessibility_trusted(false), unsafe { AXIsProcessTrusted() });
            }
        }).join().unwrap();
    }
}
