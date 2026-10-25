//! Read only the executable basename for an already captured process lifetime.
//! Failure leaves the writing scene unknown; it never changes input permission.
use super::delivery::TargetSnapshot;

#[cfg(windows)]
pub fn name_for_target(target: &TargetSnapshot) -> Option<String> {
    use windows::{core::PWSTR, Win32::{Foundation::{CloseHandle, FILETIME, HANDLE},
        System::Threading::{OpenProcess, GetProcessTimes, QueryFullProcessImageNameW,
            PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_NAME_WIN32}}};
    struct Owned(HANDLE);
    impl Drop for Owned { fn drop(&mut self) { unsafe { let _ = CloseHandle(self.0); } } }
    if target.process_id == 0 || target.process_created_at == 0 { return None; }
    unsafe {
        let process = Owned(OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, target.process_id).ok()?);
        let (mut creation, mut exit, mut kernel, mut user) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
        GetProcessTimes(process.0, &mut creation, &mut exit, &mut kernel, &mut user).ok()?;
        let created = u64::from(creation.dwHighDateTime) << 32 | u64::from(creation.dwLowDateTime);
        if created != target.process_created_at { return None; }
        let mut path = vec![0u16; 32768]; let mut len = path.len() as u32;
        QueryFullProcessImageNameW(process.0, PROCESS_NAME_WIN32, PWSTR(path.as_mut_ptr()), &mut len).ok()?;
        let path = String::from_utf16(&path[..len as usize]).ok()?;
        crate::core::app_writing::application_key(path.rsplit(['\\', '/']).next()?)
    }
}
#[cfg(not(windows))]
pub fn name_for_target(_: &TargetSnapshot) -> Option<String> { None }

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use windows::Win32::{Foundation::FILETIME, System::Threading::{GetCurrentProcess,GetProcessTimes}};
    #[test]
    fn own_process_basename_requires_the_same_process_lifetime() {
        let (mut created,mut exit,mut kernel,mut user)=(FILETIME::default(),FILETIME::default(),FILETIME::default(),FILETIME::default());
        unsafe { GetProcessTimes(GetCurrentProcess(),&mut created,&mut exit,&mut kernel,&mut user).unwrap(); }
        let mut target = TargetSnapshot { hwnd:0,process_id:std::process::id(),focused_hwnd:0,runtime_id:vec![],selection:None,
            process_created_at:u64::from(created.dwHighDateTime)<<32|u64::from(created.dwLowDateTime),compatibility:None };
        let expected=std::env::current_exe().unwrap().file_name().unwrap().to_str().unwrap().to_ascii_lowercase();
        assert_eq!(name_for_target(&target),Some(expected));
        target.process_created_at+=1; assert!(name_for_target(&target).is_none());
        target.process_id=0; assert!(name_for_target(&target).is_none());
    }
}
