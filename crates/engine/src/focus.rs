//! Whether this process is the app in front, the one that gets the keyboard.

/// Whether this process is the frontmost app and active, so keys typed now reach its key
/// window. `None` where the platform can't say.
#[cfg(target_os = "macos")]
pub fn frontmost() -> Option<bool> {
    use objc2::msg_send;
    use objc2::runtime::{AnyObject, Bool};

    let ours = i32::try_from(std::process::id()).ok()?;
    // SAFETY: `+[NSWorkspace sharedWorkspace]`, `-frontmostApplication` (which may be nil),
    // `-processIdentifier` (a pid_t) and `+[NSApplication sharedApplication]`/`-isActive` (a
    // BOOL) take no arguments. The objects are autoreleased or shared; nothing is kept.
    unsafe {
        let workspace: *mut AnyObject = msg_send![objc2::class!(NSWorkspace), sharedWorkspace];
        if workspace.is_null() {
            return None;
        }
        let app: *mut AnyObject = msg_send![workspace, frontmostApplication];
        if app.is_null() {
            return Some(false);
        }
        let pid: i32 = msg_send![app, processIdentifier];
        let nsapp: *mut AnyObject = msg_send![objc2::class!(NSApplication), sharedApplication];
        let active: Bool = if nsapp.is_null() {
            Bool::NO
        } else {
            msg_send![nsapp, isActive]
        };
        Some(pid == ours && active.as_bool())
    }
}

/// Whether this process is the frontmost app: unknown off macOS.
#[cfg(not(target_os = "macos"))]
pub fn frontmost() -> Option<bool> {
    None
}
