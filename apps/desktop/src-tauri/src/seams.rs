//! What the call flows (`calls.rs`) need from the outside world, each behind a small trait so
//! tests and `--script` mode can put their own in: the engine, the clipboard, the permissions,
//! and the clock.

use std::sync::{Arc, Mutex};
#[cfg(test)]
use std::time::Duration;
use std::time::Instant;

use serde::{Deserialize, Serialize};

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

/// The system clipboard (`platform::clipboard_read` and `clipboard_write`).
pub struct SystemClipboard;

impl Clipboard for SystemClipboard {
    fn read(&self, limit: usize) -> Result<Option<String>, String> {
        crate::platform::clipboard_read(limit)
    }

    fn write(&self, text: &str) -> Result<(), String> {
        crate::platform::clipboard_write(text)
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
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

    /// The service's name for `tccutil`.
    pub fn tcc_service(self) -> &'static str {
        match self {
            Permission::ScreenRecording => "ScreenCapture",
            Permission::Accessibility => "Accessibility",
        }
    }
}

/// macOS permissions. Where the platform has none to ask for (Windows), all are granted.
pub trait Permits: Send + Sync {
    fn granted(&self, which: Permission) -> Result<bool, String>;
    /// Asks macOS (its prompt, the first time). Returns whether it is granted afterwards.
    fn request(&self, which: Permission) -> Result<bool, String>;
    /// Clears TwinCursor's own entry for `which` in macOS's privacy list (`tccutil reset`, for
    /// this app's identifier only), so that the next request lists this copy afresh. Returns
    /// tccutil's exit code.
    fn reset(&self, which: Permission) -> Result<i32, String>;
}

/// The engine's view of the permissions, in a fresh `tc-engine permissions` each time: macOS
/// applies a new grant only to processes started after it, and the engine is what needs them.
pub struct EnginePermits;

fn engine_says(args: &[&str], which: Permission) -> Result<bool, String> {
    let exe = engine::engine_path()?;
    let status = engine::run_once(&exe, args)?;
    let answer = status[which.word()].as_str().unwrap_or("unreadable");
    // Which engine answered, so a log shows which copy macOS was asked about. A check that
    // answers as the last one did (the re-check while a permission is missing runs every few
    // seconds) isn't written again; a request always is.
    let said = format!(
        "permission: tc-engine {} says {}={answer} ({})",
        args.join(" "),
        which.word(),
        status["executable"].as_str().unwrap_or("?"),
    );
    static LAST_CHECK: Mutex<Option<String>> = Mutex::new(None);
    let mut last = LAST_CHECK.lock().unwrap_or_else(|e| e.into_inner());
    if args.len() > 1 || last.as_deref() != Some(said.as_str()) {
        crate::applog::line(format!("{said}, engine pid {}", status["pid"]));
        if args.len() == 1 {
            *last = Some(said);
        }
    }
    Ok(answer == "granted" || answer == "not_applicable")
}

impl Permits for EnginePermits {
    fn granted(&self, which: Permission) -> Result<bool, String> {
        if !crate::platform::ASKS_PERMISSIONS {
            return Ok(true);
        }
        engine_says(&["permissions"], which)
    }
    fn request(&self, which: Permission) -> Result<bool, String> {
        if !crate::platform::ASKS_PERMISSIONS {
            return Ok(true);
        }
        engine_says(&["permissions", "--request", which.word()], which)
    }
    fn reset(&self, which: Permission) -> Result<i32, String> {
        if !crate::platform::ASKS_PERMISSIONS {
            return Err(NOTHING_TO_RESET.into());
        }
        crate::platform::reset_permission(which.tcc_service())
    }
}

/// What a reset says where there is no permission list to reset.
pub const NOTHING_TO_RESET: &str = "There's no permission to reset here.";

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
    fn reset(&self, _which: Permission) -> Result<i32, String> {
        Ok(0)
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
                let token = crate::platform::begin_activity(&reason);
                let _ = wait.recv();
                crate::platform::end_activity(token);
            });
        Arc::new(Awake { _stop: stop })
    }
}

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
