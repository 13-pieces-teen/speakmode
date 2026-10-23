//! macOS native boundary. No hook, permission prompt or clipboard write on import.
pub(crate) mod ffi;
pub mod permissions;
pub(crate) mod lifecycle;
use objc2::{class, msg_send, runtime::AnyObject};
use objc2_foundation::NSString;
use super::{PlatformError, Result};
pub use permissions::{microphone_granted, status};

pub fn process_lifetime(pid: u32) -> Option<u64> {
    process_info(pid).map(|(_, lifetime)| lifetime)
}
pub(crate) fn process_info(pid: u32) -> Option<(u32, u64)> {
    if pid == 0 || pid > i32::MAX as u32 { return None; }
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of_val(&info) as i32;
    let read = unsafe { libc::proc_pidinfo(pid as i32, libc::PROC_PIDTBSDINFO, 0, (&mut info as *mut libc::proc_bsdinfo).cast(), size) };
    if read != size || info.pbi_pid != pid { return None; }
    let lifetime = info.pbi_start_tvsec.checked_mul(1_000_000)?.checked_add(info.pbi_start_tvusec)?;
    (lifetime > 0).then_some((info.pbi_uid, lifetime))
}

pub fn frontmost_pid() -> Option<u32> {
    objc2::rc::autoreleasepool(|_| unsafe {
        let workspace: *mut AnyObject = msg_send![class!(NSWorkspace), sharedWorkspace];
        let app: *mut AnyObject = msg_send![workspace, frontmostApplication];
        if app.is_null() { return None; }
        let pid: i32 = msg_send![app, processIdentifier];
        (pid > 0).then_some(pid as u32)
    })
}
pub fn activate(pid: u32, lifetime: u64) -> Result<()> {
    if process_lifetime(pid) != Some(lifetime) { return Err(PlatformError::Unavailable("原输入应用已退出或重启".into())); }
    unsafe {
        let app: *mut AnyObject = msg_send![class!(NSRunningApplication), runningApplicationWithProcessIdentifier: pid as i32];
        let accepted: bool = msg_send![app, activateWithOptions: 2usize];
        if !accepted { return Err(PlatformError::Unavailable("未能返回原输入应用".into())); }
    }
    Ok(())
}
pub fn application_key_for(pid: u32, lifetime: u64) -> Option<String> {
    if process_lifetime(pid) != Some(lifetime) { return None; }
    objc2::rc::autoreleasepool(|_| unsafe {
        let app: *mut AnyObject = msg_send![class!(NSRunningApplication), runningApplicationWithProcessIdentifier: pid as i32];
        if app.is_null() { return None; }
        let identifier: *mut NSString = msg_send![app, bundleIdentifier];
        if identifier.is_null() { return None; }
        crate::core::app_writing::application_key(&(*identifier).to_string())
    })
}

#[cfg(debug_assertions)]
pub mod lab;
