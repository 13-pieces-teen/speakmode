//! Small audited C ABI surface for Accessibility/CoreGraphics/CoreFoundation.
use std::ffi::{c_char, c_void};
pub type Ref = *const c_void;
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Range { pub location: isize, pub length: isize }
#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    pub fn AXIsProcessTrusted() -> bool;
    pub fn AXIsProcessTrustedWithOptions(options: Ref) -> bool;
    pub static kAXTrustedCheckOptionPrompt: Ref;
    pub fn AXUIElementCreateApplication(pid: i32) -> Ref;
    pub fn AXUIElementCopyAttributeValue(element: Ref, attribute: Ref, value: *mut Ref) -> i32;
    pub fn AXUIElementCopyParameterizedAttributeValue(element: Ref, attribute: Ref, parameter: Ref, value: *mut Ref) -> i32;
    pub fn AXUIElementIsAttributeSettable(element: Ref, attribute: Ref, value: *mut bool) -> i32;
    pub fn AXUIElementSetMessagingTimeout(element: Ref, seconds: f32) -> i32;
    pub fn AXUIElementGetPid(element: Ref, pid: *mut i32) -> i32;
    pub fn AXValueGetTypeID() -> usize;
    pub fn AXValueCreate(kind: u32, value: *const c_void) -> Ref;
    pub fn AXValueGetValue(value: Ref, kind: u32, result: *mut c_void) -> bool;
    pub fn AXValueGetType(value: Ref) -> u32;
    pub fn CGEventSourceCreate(state: i32) -> Ref;
    pub fn CGEventCreateKeyboardEvent(source: Ref, key: u16, down: bool) -> Ref;
    pub fn CGEventSetFlags(event: Ref, flags: u64);
    pub fn CGEventSetIntegerValueField(event: Ref, field: u32, value: i64);
    pub fn CGEventPost(location: u32, event: Ref);
    pub fn CGEventSourceFlagsState(state: i32) -> u64;
    pub fn CGEventSourceKeyState(state: i32, key: u16) -> bool;
    pub fn CGPreflightListenEventAccess() -> bool;
    pub fn CGRequestListenEventAccess() -> bool;
    pub fn CGPreflightPostEventAccess() -> bool;
}
#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    pub fn CFRelease(value: Ref);
    pub fn CFEqual(a: Ref, b: Ref) -> bool;
    pub fn CFGetTypeID(value: Ref) -> usize;
    pub fn CFStringGetTypeID() -> usize;
    pub fn CFNumberGetTypeID() -> usize;
    pub fn CFNumberGetValue(value: Ref, kind: isize, result: *mut c_void) -> bool;
    pub fn CFBooleanGetTypeID() -> usize;
    pub fn CFBooleanGetValue(value: Ref) -> bool;
    pub fn CFStringGetLength(value: Ref) -> isize;
    pub fn CFStringGetCString(value: Ref, buffer: *mut c_char, size: isize, encoding: u32) -> bool;
}
#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" { pub fn IsSecureEventInputEnabled() -> bool; }
#[link(name = "AppKit", kind = "framework")]
unsafe extern "C" {}
#[link(name = "AVFoundation", kind = "framework")]
unsafe extern "C" { pub static AVMediaTypeAudio: Ref; }

/// Owned Create/Copy reference. Only CF/AX objects cross the worker boundary.
pub struct Owned(pub Ref);
impl Owned { pub fn new(value: Ref) -> Option<Self> { (!value.is_null()).then(|| Self(value)) } }
impl Drop for Owned { fn drop(&mut self) { unsafe { CFRelease(self.0) } } }
unsafe impl Send for Owned {}
// Access to retained AX identities is serialized by their registry mutex.
unsafe impl Sync for Owned {}
pub fn string(value: Ref, max_utf16: usize) -> Option<String> {
    if value.is_null() { return None; }
    unsafe {
        if CFGetTypeID(value) != CFStringGetTypeID() { return None; }
        let length = usize::try_from(CFStringGetLength(value)).ok()?;
        if length > max_utf16 { return None; }
        let mut buffer = vec![0u8; length.checked_mul(4)?.checked_add(1)?];
        if !CFStringGetCString(value, buffer.as_mut_ptr().cast(), buffer.len() as isize, 0x0800_0100) { return None; }
        let end = buffer.iter().position(|byte| *byte == 0)?;
        String::from_utf8(buffer[..end].to_vec()).ok()
    }
}
pub fn attribute(element: Ref, name: &str) -> Option<Owned> {
    attribute_result(element, name).ok()
}
pub fn attribute_result(element: Ref, name: &str) -> std::result::Result<Owned, i32> {
    let name = objc2_foundation::NSString::from_str(name);
    let mut value = std::ptr::null();
    let error = unsafe { AXUIElementCopyAttributeValue(element, (&*name as *const objc2_foundation::NSString).cast(), &mut value) };
    if error == 0 { Owned::new(value).ok_or(-25200) } else { Err(error) }
}
pub fn settable(element: Ref, name: &str) -> bool {
    let name = objc2_foundation::NSString::from_str(name);
    let mut result = false;
    unsafe { AXUIElementIsAttributeSettable(element, (&*name as *const objc2_foundation::NSString).cast(), &mut result) == 0 && result }
}
pub fn boolean(value: Ref) -> Option<bool> {
    unsafe { (!value.is_null() && CFGetTypeID(value) == CFBooleanGetTypeID()).then(|| CFBooleanGetValue(value)) }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn null_native_result_never_constructs_or_releases_an_owned_reference() {
        // then_some(Self(value)) eagerly constructs and drops Owned(NULL),
        // which calls CFRelease(NULL). Failure paths must return None directly.
        assert!(Owned::new(std::ptr::null()).is_none());
    }
}
