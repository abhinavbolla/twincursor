//! What macOS lets this process do: the microphone check a call makes before it starts, and the
//! Screen Recording, Accessibility and Microphone report that `tc-spike permissions` and
//! `tc-engine permissions` print.

use serde::Serialize;

use crate::Error;

/// Microphone access, as macOS records it for this app (the terminal, when run from one).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MicPermission {
    /// macOS hasn't asked yet; it will the first time the mic opens.
    NotAsked,
    /// Device management blocks the microphone.
    Restricted,
    /// The user turned access off.
    Denied,
    /// Access is on.
    Granted,
}

impl MicPermission {
    /// From `AVAuthorizationStatus`.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    fn from_status(status: isize) -> Self {
        match status {
            0 => Self::NotAsked,
            1 => Self::Restricted,
            2 => Self::Denied,
            _ => Self::Granted,
        }
    }
}

/// Stops with a clear message when macOS won't let us record; without access, CoreAudio quietly
/// records silence instead of failing.
pub fn check_mic_permission() -> Result<(), Error> {
    match mic_permission() {
        Some(MicPermission::Denied) => return Err(Error::MicDenied),
        Some(MicPermission::Restricted) => return Err(Error::MicRestricted),
        Some(MicPermission::NotAsked) => {
            tracing::info!("macOS will ask whether this app may use the microphone")
        }
        Some(MicPermission::Granted) | None => {}
    }
    Ok(())
}

/// The microphone permission, or `None` where the platform has no such permission.
#[cfg(target_os = "macos")]
pub fn mic_permission() -> Option<MicPermission> {
    use objc2::runtime::{AnyClass, AnyObject};
    use objc2::{class, msg_send};

    #[link(name = "AVFoundation", kind = "framework")]
    extern "C" {
        static AVMediaTypeAudio: &'static AnyObject;
    }
    // Make sure the class is loaded before asking for it by name.
    let _: Option<&AnyClass> = AnyClass::get(c"AVCaptureDevice");
    // SAFETY: `authorizationStatusForMediaType:` is a class method that takes an
    // AVMediaType (an NSString constant AVFoundation exports) and returns an NSInteger.
    let status: isize = unsafe {
        msg_send![class!(AVCaptureDevice), authorizationStatusForMediaType: AVMediaTypeAudio]
    };
    Some(MicPermission::from_status(status))
}

/// The microphone permission, or `None` where the platform has no such permission.
#[cfg(not(target_os = "macos"))]
pub fn mic_permission() -> Option<MicPermission> {
    None
}

/// Asks macOS for microphone access and waits for the answer, up to `timeout`. Shows the system
/// prompt only while the permission is [`MicPermission::NotAsked`]; otherwise it returns at once.
#[cfg(target_os = "macos")]
pub fn request_mic_permission(timeout: std::time::Duration) {
    use block2::RcBlock;
    use objc2::runtime::{AnyClass, AnyObject, Bool};
    use objc2::{class, msg_send};

    #[link(name = "AVFoundation", kind = "framework")]
    extern "C" {
        static AVMediaTypeAudio: &'static AnyObject;
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let done = RcBlock::new(move |_granted: Bool| {
        let _ = tx.send(());
    });
    let _: Option<&AnyClass> = AnyClass::get(c"AVCaptureDevice");
    // SAFETY: `requestAccessForMediaType:completionHandler:` takes an AVMediaType and a
    // block taking a BOOL; AVFoundation copies the block before returning.
    unsafe {
        let _: () = msg_send![
            class!(AVCaptureDevice),
            requestAccessForMediaType: AVMediaTypeAudio,
            completionHandler: &*done
        ];
    }
    let _ = rx.recv_timeout(timeout);
}

/// Asks macOS for microphone access: nothing to ask where the platform has no such permission.
#[cfg(not(target_os = "macos"))]
pub fn request_mic_permission(_timeout: std::time::Duration) {}

/// One permission, as macOS reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Access {
    Granted,
    /// Not allowed, or not asked yet: macOS doesn't tell the two apart for this one.
    NotGranted,
    NotAsked,
    Denied,
    Restricted,
    /// Nothing to allow on this platform.
    NotApplicable,
}

impl From<Option<MicPermission>> for Access {
    fn from(mic: Option<MicPermission>) -> Self {
        match mic {
            Some(MicPermission::Granted) => Self::Granted,
            Some(MicPermission::NotAsked) => Self::NotAsked,
            Some(MicPermission::Denied) => Self::Denied,
            Some(MicPermission::Restricted) => Self::Restricted,
            None => Self::NotApplicable,
        }
    }
}

/// Screen Recording, Accessibility and Microphone access, as `tc-spike permissions` and
/// `tc-engine permissions` print them, with who is asking.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Status {
    pub screen_recording: Access,
    pub accessibility: Access,
    pub microphone: Access,
    pub pid: u32,
    pub parent_pid: u32,
    pub executable: String,
}

/// What macOS lets this process do right now. Never prompts.
pub fn status() -> Status {
    Status {
        screen_recording: sys::screen_recording(),
        accessibility: sys::accessibility(),
        microphone: mic_permission().into(),
        pid: std::process::id(),
        parent_pid: parent_pid(),
        executable: std::env::current_exe()
            .map(|p| p.display().to_string())
            .unwrap_or_default(),
    }
}

fn parent_pid() -> u32 {
    #[cfg(unix)]
    {
        std::os::unix::process::parent_id()
    }
    #[cfg(not(unix))]
    {
        0
    }
}

/// Asks macOS for what `before` shows is missing. Screen Recording and Accessibility show their
/// prompts and return at once; the microphone waits for its answer, up to two minutes.
pub fn request_missing(before: &Status) {
    if before.screen_recording != Access::Granted {
        sys::request_screen_recording();
    }
    if before.accessibility != Access::Granted {
        sys::request_accessibility();
    }
    if before.microphone == Access::NotAsked {
        request_mic_permission(std::time::Duration::from_secs(120));
    }
}

#[cfg(target_os = "macos")]
mod sys {
    use std::ffi::c_void;

    use super::Access;

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGPreflightScreenCaptureAccess() -> bool;
        fn CGRequestScreenCaptureAccess() -> bool;
    }

    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXIsProcessTrusted() -> bool;
        fn AXIsProcessTrustedWithOptions(options: *const c_void) -> bool;
        static kAXTrustedCheckOptionPrompt: *const c_void;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        static kCFBooleanTrue: *const c_void;
        static kCFTypeDictionaryKeyCallBacks: u8;
        static kCFTypeDictionaryValueCallBacks: u8;
        fn CFDictionaryCreate(
            allocator: *const c_void,
            keys: *const *const c_void,
            values: *const *const c_void,
            count: isize,
            key_callbacks: *const u8,
            value_callbacks: *const u8,
        ) -> *const c_void;
        fn CFRelease(cf: *const c_void);
    }

    fn granted(yes: bool) -> Access {
        if yes {
            Access::Granted
        } else {
            Access::NotGranted
        }
    }

    pub fn screen_recording() -> Access {
        // SAFETY: plain CoreGraphics call without arguments; it never prompts.
        granted(unsafe { CGPreflightScreenCaptureAccess() })
    }

    pub fn accessibility() -> Access {
        // SAFETY: plain call without arguments; it never prompts.
        granted(unsafe { AXIsProcessTrusted() })
    }

    pub fn request_screen_recording() {
        // SAFETY: as above. Shows the system prompt once per app; the answer only takes effect
        // in processes started after it.
        unsafe { CGRequestScreenCaptureAccess() };
    }

    pub fn request_accessibility() {
        // SAFETY: a one-entry CFDictionary of CF constants, released after the call.
        unsafe {
            let keys = [kAXTrustedCheckOptionPrompt];
            let values = [kCFBooleanTrue];
            let options = CFDictionaryCreate(
                std::ptr::null(),
                keys.as_ptr(),
                values.as_ptr(),
                1,
                &raw const kCFTypeDictionaryKeyCallBacks,
                &raw const kCFTypeDictionaryValueCallBacks,
            );
            if options.is_null() {
                return;
            }
            AXIsProcessTrustedWithOptions(options);
            CFRelease(options);
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod sys {
    use super::Access;

    pub fn screen_recording() -> Access {
        Access::NotApplicable
    }

    pub fn accessibility() -> Access {
        Access::NotApplicable
    }

    pub fn request_screen_recording() {}

    pub fn request_accessibility() {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_status_values() {
        assert_eq!(MicPermission::from_status(0), MicPermission::NotAsked);
        assert_eq!(MicPermission::from_status(1), MicPermission::Restricted);
        assert_eq!(MicPermission::from_status(2), MicPermission::Denied);
        assert_eq!(MicPermission::from_status(3), MicPermission::Granted);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn permission_can_be_read() {
        // Reading the status never prompts; it only has to come back as a known value.
        assert!(mic_permission().is_some());
    }

    #[test]
    fn status_serializes_as_snake_case() {
        let json = serde_json::to_value(Status {
            screen_recording: Access::Granted,
            accessibility: Access::NotGranted,
            microphone: Access::NotAsked,
            pid: 1,
            parent_pid: 2,
            executable: "x".into(),
        })
        .unwrap();
        assert_eq!(json["screen_recording"], "granted");
        assert_eq!(json["accessibility"], "not_granted");
        assert_eq!(json["microphone"], "not_asked");
    }
}
