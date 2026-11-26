//! What the call flows (`calls.rs`) need from the outside world, each behind a small trait so
//! tests and `--script` mode can put their own in: the engine, the clipboard, the permissions,
//! and the clock.

use std::sync::{Arc, Mutex};
#[cfg(test)]
use std::time::Duration;
use std::time::Instant;

use serde::Serialize;

use crate::engine::{self, EnvHook, Launcher, Plan, Sink, TestHooks};

/// One running call engine (`engine::Call` in the app).
pub trait EngineHandle: Send {
    fn send(&self, command: &ipc::Command) -> Result<(), String>;
    /// Only to the engine process `engine` (counting `Started` outputs from 1), never to one
    /// that replaced it.
    fn send_for(&self, command: &ipc::Command, engine: u64) -> Result<(), String>;
    fn leave(&self);
    fn is_done(&self) -> bool;
    /// Ends it now and waits. Never called from the engine's own thread.
    fn shutdown(self: Box<Self>);
}

impl EngineHandle for engine::Call {
    fn send(&self, command: &ipc::Command) -> Result<(), String> {
        engine::Call::send(self, command)
    }
    fn send_for(&self, command: &ipc::Command, engine: u64) -> Result<(), String> {
        engine::Call::send_for(self, command, engine)
    }
    fn leave(&self) {
        engine::Call::leave(self);
    }
    fn is_done(&self) -> bool {
        engine::Call::is_done(self)
    }
    fn shutdown(self: Box<Self>) {
        engine::Call::shutdown(*self);
    }
}

/// How to start an engine for a call.
pub struct EngineSpec {
    pub plan: Plan,
    /// May hold the secret or a pass: never logged.
    pub env: Vec<(String, String)>,
    /// A fresh environment for a replacement engine (a new pass).
    pub refresh_env: Option<EnvHook>,
}

/// Starts engines.
pub trait Engines: Send + Sync {
    fn start(&self, spec: EngineSpec, sink: Sink) -> Result<Box<dyn EngineHandle>, String>;
}

/// The real `tc-engine`, with the test hooks `--script` gives (none otherwise).
pub struct RealEngines {
    pub hooks: TestHooks,
}

impl Engines for RealEngines {
    fn start(&self, spec: EngineSpec, sink: Sink) -> Result<Box<dyn EngineHandle>, String> {
        let launcher = Launcher {
            exe: engine::engine_path()?,
            env: spec.env,
            app_pid: std::process::id(),
            hooks: self.hooks.clone(),
            refresh_env: spec.refresh_env,
        };
        Ok(Box::new(engine::Call::start(launcher, spec.plan, sink)?))
    }
}

/// The system clipboard, as text.
pub trait Clipboard: Send + Sync {
    /// The clipboard's text, if it holds text. Longer text than `limit` bytes is an error: it
    /// can't be a call link, and isn't kept.
    fn read(&self, limit: usize) -> Result<Option<String>, String>;
    fn write(&self, text: &str) -> Result<(), String>;
}

/// The general pasteboard, through AppKit.
pub struct SystemClipboard;

#[cfg(target_os = "macos")]
impl Clipboard for SystemClipboard {
    fn read(&self, limit: usize) -> Result<Option<String>, String> {
        use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};
        let board = NSPasteboard::generalPasteboard();
        // SAFETY: an AppKit constant, valid for the life of the process.
        let kind = unsafe { NSPasteboardTypeString };
        let Some(text) = board.stringForType(kind) else {
            return Ok(None);
        };
        if text.length() > limit {
            return Err("What's on the clipboard is too long to be a call link.".into());
        }
        Ok(Some(text.to_string()))
    }

    fn write(&self, text: &str) -> Result<(), String> {
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
}

#[cfg(not(target_os = "macos"))]
impl Clipboard for SystemClipboard {
    fn read(&self, _limit: usize) -> Result<Option<String>, String> {
        Err("The clipboard is only used on macOS so far.".into())
    }
    fn write(&self, _text: &str) -> Result<(), String> {
        Err("The clipboard is only used on macOS so far.".into())
    }
}

/// A clipboard in memory, for `--script` mode and tests: the real one is never touched.
#[derive(Default)]
pub struct MemoryClipboard {
    text: Mutex<Option<String>>,
}

impl MemoryClipboard {
    pub fn text(&self) -> Option<String> {
        self.text.lock().unwrap().clone()
    }
    pub fn set(&self, text: Option<&str>) {
        *self.text.lock().unwrap() = text.map(str::to_owned);
    }
}

impl Clipboard for MemoryClipboard {
    fn read(&self, limit: usize) -> Result<Option<String>, String> {
        match self.text() {
            Some(t) if t.len() > limit => {
                Err("What's on the clipboard is too long to be a call link.".into())
            }
            t => Ok(t),
        }
    }
    fn write(&self, text: &str) -> Result<(), String> {
        self.set(Some(text));
        Ok(())
    }
}

/// A permission the flows ask for when they first need it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    /// To share the screen.
    ScreenRecording,
    /// To give someone control.
    Accessibility,
}

impl Permission {
    pub fn word(self) -> &'static str {
        match self {
            Permission::ScreenRecording => "screen_recording",
            Permission::Accessibility => "accessibility",
        }
    }
}

/// macOS permissions.
pub trait Permits: Send + Sync {
    fn granted(&self, which: Permission) -> Result<bool, String>;
    /// Asks macOS (its prompt, the first time). Returns whether it is granted afterwards.
    fn request(&self, which: Permission) -> Result<bool, String>;
}

/// The engine's view of the permissions, in a fresh `tc-engine permissions` each time: macOS
/// applies a new grant only to processes started after it, and the engine is what needs them.
pub struct EnginePermits;

fn engine_says(args: &[&str], which: Permission) -> Result<bool, String> {
    let exe = engine::engine_path()?;
    let status = engine::run_once(&exe, args)?;
    Ok(status[which.word()] == "granted" || status[which.word()] == "not_applicable")
}

impl Permits for EnginePermits {
    fn granted(&self, which: Permission) -> Result<bool, String> {
        engine_says(&["permissions"], which)
    }
    fn request(&self, which: Permission) -> Result<bool, String> {
        engine_says(&["permissions", "--request", which.word()], which)
    }
}

/// Everything granted, for `--script` mode: it shares the synthetic pattern (no Screen Recording
/// needed) and records input instead of injecting it (no Accessibility needed).
pub struct ScriptPermits;

impl Permits for ScriptPermits {
    fn granted(&self, _which: Permission) -> Result<bool, String> {
        Ok(true)
    }
    fn request(&self, _which: Permission) -> Result<bool, String> {
        Ok(true)
    }
}

/// The time, for the arming delay and the 60 s window for giving control on arrival.
pub trait Clock: Send + Sync {
    fn now(&self) -> Instant;
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

/// A clock tests move by hand.
#[cfg(test)]
pub struct ManualClock {
    start: Instant,
    offset: Mutex<Duration>,
}

#[cfg(test)]
impl Default for ManualClock {
    fn default() -> ManualClock {
        ManualClock {
            start: Instant::now(),
            offset: Mutex::new(Duration::ZERO),
        }
    }
}

#[cfg(test)]
impl ManualClock {
    pub fn advance(&self, by: Duration) {
        *self.offset.lock().unwrap() += by;
    }
}

#[cfg(test)]
impl Clock for ManualClock {
    fn now(&self) -> Instant {
        self.start + *self.offset.lock().unwrap()
    }
}

/// Keeps macOS from napping this app (App Nap throttles the timers of an app whose window is
/// hidden) while it holds a call open: the poll is the host's heartbeat and the guest's knock.
/// Dropping it lets the app nap again. Elsewhere it does nothing.
pub struct Awake {
    _stop: std::sync::mpsc::Sender<()>,
}

impl Awake {
    pub fn hold(reason: &str) -> Arc<Awake> {
        let (stop, wait) = std::sync::mpsc::channel::<()>();
        let reason = reason.to_owned();
        // The activity token isn't Send, so one thread begins and ends it.
        let _ = std::thread::Builder::new()
            .name("awake".into())
            .spawn(move || {
                let token = begin_activity(&reason);
                let _ = wait.recv();
                end_activity(token);
            });
        Arc::new(Awake { _stop: stop })
    }
}

#[cfg(target_os = "macos")]
type ActivityToken =
    objc2::rc::Retained<objc2::runtime::ProtocolObject<dyn objc2::runtime::NSObjectProtocol>>;

#[cfg(target_os = "macos")]
fn begin_activity(reason: &str) -> ActivityToken {
    use objc2_foundation::{NSActivityOptions, NSProcessInfo, NSString};
    NSProcessInfo::processInfo().beginActivityWithOptions_reason(
        NSActivityOptions::UserInitiatedAllowingIdleSystemSleep,
        &NSString::from_str(reason),
    )
}

#[cfg(target_os = "macos")]
fn end_activity(token: ActivityToken) {
    use objc2_foundation::NSProcessInfo;
    // SAFETY: the token is the one beginActivityWithOptions returned, ended once.
    unsafe { NSProcessInfo::processInfo().endActivity(&token) };
}

#[cfg(not(target_os = "macos"))]
fn begin_activity(_reason: &str) {}

#[cfg(not(target_os = "macos"))]
fn end_activity(_token: ()) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_memory_clipboard_keeps_what_it_is_given() {
        let clip = MemoryClipboard::default();
        assert_eq!(clip.read(10).unwrap(), None);
        clip.write("hello").unwrap();
        assert_eq!(clip.read(10).unwrap().as_deref(), Some("hello"));
        assert!(clip.read(3).is_err());
    }

    #[test]
    fn the_manual_clock_moves_when_told() {
        let clock = ManualClock::default();
        let a = clock.now();
        clock.advance(Duration::from_millis(1500));
        assert_eq!(clock.now() - a, Duration::from_millis(1500));
    }

    #[test]
    fn staying_awake_starts_and_stops() {
        let awake = Awake::hold("test");
        drop(awake);
    }
}
