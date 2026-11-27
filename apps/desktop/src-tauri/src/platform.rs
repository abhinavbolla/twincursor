//! What the app needs from the operating system besides its windows (those are `native.rs`):
//! what this platform can do, opening a page in the browser, the System Settings panes, the
//! clipboard, keeping the app awake during a call, where the session and the log go, and how
//! the engine process is started and tied to the app.
//!
//! Each platform has a module with the same functions: `mac` (the app as it ships), `win` (a
//! guest: it joins calls and takes control, but can't share its screen) and `other` (builds, so
//! the crate compiles, and says what isn't there). The parts that are plain data in, data out
//! (the browser's command line on Windows, clipboard text in UTF-16) are at the top of this
//! file, so they are tested on every platform.

use std::path::{Path, PathBuf};

use serde::Serialize;

/// What the window learns about the platform (`platform_info`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct PlatformInfo {
    /// `macos`, `windows` or `linux`.
    pub os: &'static str,
    /// Whether this app can share its screen (and so start a call or ring someone). Only on
    /// macOS: screen capture and input injection exist only there.
    pub can_share: bool,
}

pub const INFO: PlatformInfo = PlatformInfo {
    os: std::env::consts::OS,
    can_share: cfg!(target_os = "macos"),
};

/// What `share` says where sharing isn't built.
pub const CANT_SHARE: &str =
    "Sharing your screen from Windows isn't available yet. You can join a call with a link.";

#[cfg_attr(not(windows), allow(dead_code))]
/// The app's identifier (`identifier` in tauri.conf.json), for paths worked out before Tauri
/// has started.
pub const IDENTIFIER: &str = "dev.twincursor.desktop";

#[cfg_attr(target_os = "macos", allow(dead_code))]
/// A page the app may open in the browser: http or https, and nothing in it that a command
/// line could split or quote (whitespace, quotes, control characters). The sign-in page's
/// address and the guide's never have any; one that does is refused rather than mangled.
pub fn checked_url(url: &str) -> Result<&str, String> {
    let scheme_ok = url.starts_with("https://") || url.starts_with("http://");
    let clean = !url
        .chars()
        .any(|c| c.is_whitespace() || c.is_control() || c == '"' || c == '\'' || c == '^');
    if scheme_ok && clean && url.len() > "https://".len() {
        Ok(url)
    } else {
        Err("That address can't be opened.".into())
    }
}

#[cfg_attr(not(windows), allow(dead_code))]
/// The command that opens `url` in the default browser on Windows: `rundll32.exe
/// url.dll,FileProtocolHandler <url>`, from the System32 folder (never one found on PATH). No
/// shell sits in between, so `&`, `?` and `%` in the address mean nothing special; the address
/// is one argument, which Rust passes unquoted because [`checked_url`] lets no space or quote
/// through.
pub fn windows_open_command(url: &str, system_root: &Path) -> (PathBuf, Vec<String>) {
    (
        system_root.join("System32").join("rundll32.exe"),
        vec!["url.dll,FileProtocolHandler".to_owned(), url.to_owned()],
    )
}

#[cfg_attr(not(windows), allow(dead_code))]
/// Clipboard text as Windows keeps it (UTF-16, up to a NUL), if it is at most `limit` bytes of
/// UTF-8. Longer text can't be a call link and is refused without being kept.
pub fn text_from_utf16(units: &[u16], limit: usize) -> Result<Option<String>, String> {
    let end = units.iter().position(|&u| u == 0).unwrap_or(units.len());
    let units = &units[..end];
    // Every UTF-16 unit is at least one byte of UTF-8: checked before decoding anything.
    if units.len() > limit {
        return Err(TOO_LONG.into());
    }
    let text = String::from_utf16_lossy(units);
    if text.len() > limit {
        return Err(TOO_LONG.into());
    }
    Ok(Some(text))
}

#[cfg_attr(not(windows), allow(dead_code))]
/// Text for the Windows clipboard: UTF-16 with its NUL.
pub fn utf16_with_nul(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

pub const TOO_LONG: &str = "What's on the clipboard is too long to be a call link.";

/// Which System Settings privacy pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Pane {
    ScreenRecording,
    Accessibility,
    Microphone,
}

#[cfg(target_os = "macos")]
mod mac {
    use std::path::PathBuf;
    use std::process::{Child, Command};

    use tauri::{AppHandle, Manager};

    use super::Pane;

    /// Opens a page in the default browser with `open`.
    pub fn open_url(url: &str) -> Result<(), String> {
        std::process::Command::new("/usr/bin/open")
            .arg(url)
            .status()
            .map_err(|e| e.to_string())
            .and_then(|s| {
                s.success()
                    .then_some(())
                    .ok_or_else(|| "open failed".to_owned())
            })
    }

    pub fn open_settings_pane(pane: Pane) -> Result<(), String> {
        let anchor = match pane {
            Pane::ScreenRecording => "Privacy_ScreenCapture",
            Pane::Accessibility => "Privacy_Accessibility",
            Pane::Microphone => "Privacy_Microphone",
        };
        let url = format!("x-apple.systempreferences:com.apple.preference.security?{anchor}");
        std::process::Command::new("/usr/bin/open")
            .arg(url)
            .status()
            .map_err(|e| format!("System Settings didn't open: {e}."))
            .map(|_| ())
    }

    /// The general pasteboard, through AppKit.
    pub fn clipboard_read(limit: usize) -> Result<Option<String>, String> {
        use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};
        let board = NSPasteboard::generalPasteboard();
        // SAFETY: an AppKit constant, valid for the life of the process.
        let kind = unsafe { NSPasteboardTypeString };
        let Some(text) = board.stringForType(kind) else {
            return Ok(None);
        };
        if text.length() > limit {
            return Err(super::TOO_LONG.into());
        }
        Ok(Some(text.to_string()))
    }

    pub fn clipboard_write(text: &str) -> Result<(), String> {
        use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};
        use objc2_foundation::NSString;
        let board = NSPasteboard::generalPasteboard();
        board.clearContents();
        // SAFETY: as above.
        let kind = unsafe { NSPasteboardTypeString };
        if board.setString_forType(&NSString::from_str(text), kind) {
            Ok(())
        } else {
            Err("The link couldn't be put on the clipboard.".into())
        }
    }

    pub type ActivityToken =
        objc2::rc::Retained<objc2::runtime::ProtocolObject<dyn objc2::runtime::NSObjectProtocol>>;

    /// Keeps App Nap away (it throttles the timers of an app whose window is hidden).
    pub fn begin_activity(reason: &str) -> ActivityToken {
        use objc2_foundation::{NSActivityOptions, NSProcessInfo, NSString};
        NSProcessInfo::processInfo().beginActivityWithOptions_reason(
            NSActivityOptions::UserInitiatedAllowingIdleSystemSleep,
            &NSString::from_str(reason),
        )
    }

    pub fn end_activity(token: ActivityToken) {
        use objc2_foundation::NSProcessInfo;
        // SAFETY: the token is the one beginActivityWithOptions returned, ended once.
        unsafe { NSProcessInfo::processInfo().endActivity(&token) };
    }

    /// The session file's folder: the app's config folder (~/Library/Application Support/…).
    pub fn session_dir(app: &AppHandle) -> Result<PathBuf, String> {
        app.path()
            .app_config_dir()
            .map_err(|e| format!("No config folder: {e}."))
    }

    /// No log file unless `TWINCURSOR_LOG_FILE` asks for one.
    pub fn default_log_file() -> Option<PathBuf> {
        None
    }

    pub fn prepare_engine(_command: &mut Command) {}

    /// Nothing to do: the engine exits when its stdin closes, which the kernel does when this
    /// app dies.
    pub fn adopt_engine(_child: &Child) {}

    /// The shell asks macOS for Screen Recording and Accessibility through the engine.
    pub const ASKS_PERMISSIONS: bool = true;
}

#[cfg(windows)]
mod win {
    use std::os::windows::io::AsRawHandle;
    use std::os::windows::process::CommandExt;
    use std::path::PathBuf;
    use std::process::{Child, Command};
    use std::sync::OnceLock;
    use std::time::Duration;

    use tauri::{AppHandle, Manager};
    use windows_sys::Win32::Foundation::{GlobalFree, HANDLE};
    use windows_sys::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable,
        OpenClipboard, SetClipboardData,
    };
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, IsProcessInJob,
        JobObjectExtendedLimitInformation, SetInformationJobObject,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows_sys::Win32::System::Memory::{
        GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE,
    };
    use windows_sys::Win32::System::Ole::CF_UNICODETEXT;

    use super::Pane;

    /// No console window for the engine (it is a console program; the app isn't).
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    fn system_root() -> PathBuf {
        std::env::var_os("SystemRoot")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
    }

    /// Opens a page in the default browser (see [`super::windows_open_command`]).
    pub fn open_url(url: &str) -> Result<(), String> {
        let url = super::checked_url(url)?;
        let (program, args) = super::windows_open_command(url, &system_root());
        Command::new(program)
            .args(args)
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    pub fn open_settings_pane(_pane: Pane) -> Result<(), String> {
        Err("There are no permissions to turn on in Windows Settings for joining a call.".into())
    }

    /// The clipboard, held open for as short a time as possible. Another app may have it open
    /// for a moment, so opening is tried a few times.
    struct Open;

    impl Open {
        fn new() -> Result<Open, String> {
            for _ in 0..10 {
                // SAFETY: no owner window; closed again in Drop.
                if unsafe { OpenClipboard(std::ptr::null_mut()) } != 0 {
                    return Ok(Open);
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err("The clipboard is busy. Try again.".into())
        }
    }

    impl Drop for Open {
        fn drop(&mut self) {
            // SAFETY: opened by this thread in `Open::new`.
            unsafe { CloseClipboard() };
        }
    }

    pub fn clipboard_read(limit: usize) -> Result<Option<String>, String> {
        let _open = Open::new()?;
        // SAFETY: the clipboard is open (above). The handle belongs to the clipboard and is
        // only read while locked; its size bounds the read.
        unsafe {
            if IsClipboardFormatAvailable(u32::from(CF_UNICODETEXT)) == 0 {
                return Ok(None);
            }
            let handle = GetClipboardData(u32::from(CF_UNICODETEXT));
            if handle.is_null() {
                return Ok(None);
            }
            let bytes = GlobalSize(handle);
            let data = GlobalLock(handle) as *const u16;
            if data.is_null() {
                return Ok(None);
            }
            // Never more than the limit's worth of units (and one for the NUL) is looked at.
            let units = (bytes / 2).min(limit.saturating_add(1));
            let text = super::text_from_utf16(std::slice::from_raw_parts(data, units), limit);
            GlobalUnlock(handle);
            text
        }
    }

    pub fn clipboard_write(text: &str) -> Result<(), String> {
        let units = super::utf16_with_nul(text);
        let _open = Open::new()?;
        // SAFETY: the clipboard is open. The memory is allocated, filled while locked, and
        // handed to the clipboard, which owns it from then on; freed here only if it refused.
        unsafe {
            EmptyClipboard();
            let memory = GlobalAlloc(GMEM_MOVEABLE, units.len() * 2);
            if memory.is_null() {
                return Err(FAILED.into());
            }
            let dest = GlobalLock(memory) as *mut u16;
            if dest.is_null() {
                GlobalFree(memory);
                return Err(FAILED.into());
            }
            std::ptr::copy_nonoverlapping(units.as_ptr(), dest, units.len());
            GlobalUnlock(memory);
            if SetClipboardData(u32::from(CF_UNICODETEXT), memory as HANDLE).is_null() {
                GlobalFree(memory);
                return Err(FAILED.into());
            }
        }
        Ok(())
    }

    const FAILED: &str = "The link couldn't be put on the clipboard.";

    /// Windows doesn't throttle a hidden app's timers the way App Nap does.
    pub struct ActivityToken;
    pub fn begin_activity(_reason: &str) -> ActivityToken {
        ActivityToken
    }
    pub fn end_activity(_token: ActivityToken) {}

    /// The session file's folder: `%LOCALAPPDATA%\dev.twincursor.desktop`, which is this
    /// user's and stays on this PC (unlike the roaming folder).
    pub fn session_dir(app: &AppHandle) -> Result<PathBuf, String> {
        app.path()
            .app_local_data_dir()
            .map_err(|e| format!("No app data folder: {e}."))
    }

    /// `%LOCALAPPDATA%\dev.twincursor.desktop\logs\twincursor.log` (where Tauri's
    /// `app_log_dir` is), worked out before Tauri starts so start-up is logged too.
    pub fn default_log_file() -> Option<PathBuf> {
        let local = std::env::var_os("LOCALAPPDATA").filter(|v| !v.is_empty())?;
        Some(
            PathBuf::from(local)
                .join(super::IDENTIFIER)
                .join("logs")
                .join("twincursor.log"),
        )
    }

    pub fn prepare_engine(command: &mut Command) {
        command.creation_flags(CREATE_NO_WINDOW);
    }

    /// The job every engine joins: when this app ends, however it ends, Windows closes the
    /// job's last handle and ends what is in it. The engine also exits when its stdin closes;
    /// this covers the case where it is stuck and doesn't read it.
    fn job() -> Option<usize> {
        static JOB: OnceLock<Option<usize>> = OnceLock::new();
        *JOB.get_or_init(|| {
            // SAFETY: plain Win32 calls with owned, initialised arguments. The handle is kept
            // for the life of the process on purpose.
            unsafe {
                let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if job.is_null() {
                    return None;
                }
                let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                let ok = SetInformationJobObject(
                    job,
                    JobObjectExtendedLimitInformation,
                    &limits as *const _ as *const core::ffi::c_void,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                );
                (ok != 0).then_some(job as usize)
            }
        })
    }

    pub fn adopt_engine(child: &Child) {
        let Some(job) = job() else {
            crate::applog::line("engine: no job object; it relies on its stdin closing");
            return;
        };
        let process = child.as_raw_handle() as HANDLE;
        // SAFETY: both handles are live: the job's for the process's life, the child's while
        // `child` is borrowed.
        let ok = unsafe { AssignProcessToJobObject(job as HANDLE, process) };
        if ok == 0 {
            crate::applog::line(format!(
                "engine: couldn't join the job object ({})",
                std::io::Error::last_os_error()
            ));
        }
        // Read back, rather than trusting the call above.
        let mut inside = 0;
        // SAFETY: as above; `inside` is written by the call.
        let asked = unsafe { IsProcessInJob(process, job as HANDLE, &mut inside) };
        crate::applog::line(match (asked != 0, inside != 0) {
            (true, true) => "engine: in the app's job object (Windows ends it with the app)",
            (true, false) => "engine: NOT in the app's job object; it relies on its stdin closing",
            (false, _) => "engine: couldn't tell whether it is in the app's job object",
        });
    }

    /// A Windows guest needs only the microphone, which Windows asks about itself.
    pub const ASKS_PERMISSIONS: bool = false;
}

#[cfg(not(any(target_os = "macos", windows)))]
mod other {
    use std::path::PathBuf;
    use std::process::{Child, Command};

    use tauri::{AppHandle, Manager};

    use super::Pane;

    pub fn open_url(url: &str) -> Result<(), String> {
        let url = super::checked_url(url)?;
        Command::new("xdg-open")
            .arg(url)
            .status()
            .map_err(|e| e.to_string())
            .and_then(|s| {
                s.success()
                    .then_some(())
                    .ok_or_else(|| "xdg-open failed".to_owned())
            })
    }

    pub fn open_settings_pane(_pane: Pane) -> Result<(), String> {
        Err("Permissions are only needed on macOS.".into())
    }

    pub fn clipboard_read(_limit: usize) -> Result<Option<String>, String> {
        Err("The clipboard isn't built for this platform yet.".into())
    }

    pub fn clipboard_write(_text: &str) -> Result<(), String> {
        Err("The clipboard isn't built for this platform yet.".into())
    }

    pub struct ActivityToken;
    pub fn begin_activity(_reason: &str) -> ActivityToken {
        ActivityToken
    }
    pub fn end_activity(_token: ActivityToken) {}

    pub fn session_dir(app: &AppHandle) -> Result<PathBuf, String> {
        app.path()
            .app_config_dir()
            .map_err(|e| format!("No config folder: {e}."))
    }

    pub fn default_log_file() -> Option<PathBuf> {
        None
    }

    pub fn prepare_engine(_command: &mut Command) {}
    pub fn adopt_engine(_child: &Child) {}

    pub const ASKS_PERMISSIONS: bool = false;
}

#[cfg(target_os = "macos")]
pub use mac::*;
#[cfg(not(any(target_os = "macos", windows)))]
pub use other::*;
#[cfg(windows)]
pub use win::*;

#[cfg(test)]
mod tests {
    use super::*;

    const SIGN_IN: &str = "https://abc.supabase.co/auth/v1/authorize?provider=github&redirect_to=http%3A%2F%2F127.0.0.1%3A53682%2Fcallback&code_challenge=Zx_9-a%2Bb&code_challenge_method=s256";

    #[test]
    fn the_windows_browser_command_keeps_the_address_whole() {
        let url = checked_url(SIGN_IN).unwrap();
        let (program, args) = windows_open_command(url, Path::new(r"C:\Windows"));
        assert_eq!(
            program,
            Path::new(r"C:\Windows")
                .join("System32")
                .join("rundll32.exe")
        );
        // One argument for the address, byte for byte: & ? % = survive, nothing is added.
        assert_eq!(args, ["url.dll,FileProtocolHandler", SIGN_IN]);
        assert!(args[1].contains('&') && args[1].contains('?') && args[1].contains('%'));
    }

    #[test]
    fn only_plain_web_addresses_are_opened() {
        assert!(checked_url("https://github.com/abhinavbolla/twincursor").is_ok());
        assert!(checked_url("http://127.0.0.1:54321/auth").is_ok());
        for bad in [
            "",
            "https://",
            "file:///C:/Windows/System32/calc.exe",
            "javascript:alert(1)",
            "https://a.test/x y",
            "https://a.test/\"&calc",
            "https://a.test/'x",
            "https://a.test/^x",
            "https://a.test/\tx",
            "https://a.test/\nx",
            "ms-settings:privacy",
        ] {
            assert!(checked_url(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn clipboard_text_round_trips_through_utf16() {
        let link = "twincursor://call/0123456789abcdef — Zoë 王";
        let units = utf16_with_nul(link);
        assert_eq!(units.last(), Some(&0));
        assert_eq!(text_from_utf16(&units, 200).unwrap().as_deref(), Some(link));
        // Whatever follows the NUL isn't text.
        let mut more = utf16_with_nul("abc");
        more.extend([0x41, 0x42]);
        assert_eq!(text_from_utf16(&more, 10).unwrap().as_deref(), Some("abc"));
        // No NUL at all: the units given.
        assert_eq!(
            text_from_utf16(&[0x61, 0x62], 10).unwrap().as_deref(),
            Some("ab")
        );
    }

    #[test]
    fn long_clipboard_text_is_refused() {
        let units = utf16_with_nul(&"a".repeat(11));
        assert_eq!(text_from_utf16(&units, 10).unwrap_err(), TOO_LONG);
        assert!(text_from_utf16(&units, 11).is_ok());
        // Ten units, but more than ten bytes of UTF-8.
        let wide = utf16_with_nul(&"王".repeat(10));
        assert_eq!(text_from_utf16(&wide, 10).unwrap_err(), TOO_LONG);
    }

    #[test]
    fn the_identifier_is_the_apps() {
        let conf: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        assert_eq!(conf["identifier"], IDENTIFIER);
    }

    #[test]
    fn only_macos_shares() {
        assert_eq!(INFO.can_share, cfg!(target_os = "macos"));
        assert_eq!(
            serde_json::to_value(INFO).unwrap(),
            serde_json::json!({ "os": std::env::consts::OS, "can_share": cfg!(target_os = "macos") })
        );
    }
}
