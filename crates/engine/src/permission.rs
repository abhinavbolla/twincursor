//! The macOS microphone permission check.

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
}
