//! Notification observers are owned by the hotkey service and removed on shutdown.
use objc2::{class, msg_send, rc::Retained, runtime::AnyObject};
use objc2_foundation::NSString;
use std::sync::mpsc::{channel, Receiver};
pub fn prepare_notification_centers() {
    unsafe {
        let workspace: *mut AnyObject = msg_send![class!(NSWorkspace), sharedWorkspace];
        let _: *mut AnyObject = msg_send![workspace, notificationCenter];
        let _: *mut AnyObject = msg_send![class!(NSDistributedNotificationCenter), defaultCenter];
    }
}
pub struct Observers { registrations: Vec<(Retained<AnyObject>, Retained<AnyObject>)>, pub events: Receiver<&'static str> }
impl Observers {
    pub fn new() -> Self {
        let (tx, events) = channel();
        let mut registrations = Vec::new();
        objc2::rc::autoreleasepool(|_| unsafe {
            let queue: *mut AnyObject = msg_send![class!(NSOperationQueue), mainQueue];
            let workspace: *mut AnyObject = msg_send![class!(NSWorkspace), sharedWorkspace];
            let workspace_center: *mut AnyObject = msg_send![workspace, notificationCenter];
            let distributed: *mut AnyObject = msg_send![class!(NSDistributedNotificationCenter), defaultCenter];
            for (center, name, reason) in [
                (workspace_center, "NSWorkspaceWillSleepNotification", "Suspended"),
                (workspace_center, "NSWorkspaceDidWakeNotification", "Resumed"),
                (workspace_center, "NSWorkspaceSessionDidResignActiveNotification", "Locked"),
                (workspace_center, "NSWorkspaceSessionDidBecomeActiveNotification", "Connected"),
                (distributed, "com.apple.screenIsLocked", "Locked"),
                (distributed, "com.apple.screenIsUnlocked", "Connected"),
            ] {
                if center.is_null() { continue; }
                let sender = tx.clone();
                let callback = block2::RcBlock::new(move |_: *mut AnyObject| { let _ = sender.send(reason); });
                let name = NSString::from_str(name);
                let token: *mut AnyObject = msg_send![center, addObserverForName: &*name, object: std::ptr::null::<AnyObject>(), queue: queue, usingBlock: &*callback];
                if let (Some(center), Some(token)) = (Retained::retain(center), Retained::retain(token)) { registrations.push((center, token)); }
            }
        });
        Self { registrations, events }
    }
}
impl Drop for Observers {
    fn drop(&mut self) { for (center, token) in &self.registrations { unsafe { let _: () = msg_send![&**center, removeObserver: &**token]; } } }
}
