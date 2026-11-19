//! Driving `tc-engine`: the flags for a call, its environment, its output, its lifetime, and the
//! switch from HEVC to H.264. Nothing here needs Tauri, so all of it is tested on its own.
//!
//! One engine runs per call. Its stdin stays open for the whole call: the engine exits when it
//! closes, so if this app dies, the kernel closes the pipe and the engine follows. Every line it
//! prints on stdout is an event (`docs/ipc.md`); stderr is its log.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The engine's file name, next to this app's executable.
pub const ENGINE: &str = "tc-engine";
/// How long an engine may take to exit once its stdin closes, before it is killed.
const EXIT_TIMEOUT: Duration = Duration::from_secs(3);
/// How long after `leave` the engine's stdin is closed, if it hasn't gone by itself.
const LEAVE_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Share,
    Join,
}

/// What the sharer picks. Guests get whatever the sharer sends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Quality {
    #[serde(rename = "1080p")]
    P1080,
    #[serde(rename = "1440p")]
    P1440,
    #[serde(rename = "4k")]
    K4,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Codec {
    H264,
    H265,
}

impl Quality {
    fn resolution(self) -> &'static str {
        match self {
            Quality::P1080 => "1080",
            Quality::P1440 => "1440",
            Quality::K4 => "2160",
        }
    }

    /// HEVC for 4K (docs/adr/0008); H.264 below it, which every guest decodes.
    pub fn codec(self) -> Codec {
        match self {
            Quality::K4 => Codec::H265,
            Quality::P1080 | Quality::P1440 => Codec::H264,
        }
    }
}

/// A call as the person set it up.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plan {
    pub role: Role,
    pub room: String,
    pub name: String,
    pub quality: Quality,
    /// Off: no mic and no sound (`--audio off`).
    pub voice: bool,
}

const MAX_NAME: usize = 64;
const MAX_ROOM: usize = 128;

impl Plan {
    /// Trims the name and room and checks they can be used.
    pub fn checked(mut self) -> Result<Plan, String> {
        self.name = self.name.trim().to_owned();
        self.room = self.room.trim().to_owned();
        if self.name.is_empty() {
            return Err("Type your name first.".into());
        }
        if self.room.is_empty() {
            return Err("Make a room code or paste one first.".into());
        }
        if self.name.chars().count() > MAX_NAME {
            return Err(format!("Names can be up to {MAX_NAME} characters."));
        }
        if self.room.chars().count() > MAX_ROOM || self.room.chars().any(char::is_control) {
            return Err("That room code can't be used.".into());
        }
        if self.name.chars().any(char::is_control) {
            return Err("Names can't contain control characters.".into());
        }
        Ok(self)
    }
}

/// Test-only engine flags. They only come from `--script` (see `script.rs`), never from the
/// webview.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TestHooks {
    /// Share the synthetic pattern instead of the screen.
    pub pattern: bool,
    /// Record what a guest in control would inject, instead of injecting it.
    pub inject_record: Option<PathBuf>,
}

/// The engine's arguments for `plan`. The server's settings are not among them: they go through
/// the environment ([`crate::settings::Settings::env`]). Values are attached with `=` so a name
/// that starts with `--` stays a name.
pub fn engine_args(plan: &Plan, codec: Codec, app_pid: u32, hooks: &TestHooks) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();
    let mut push = |a: &str| args.push(a.to_owned());
    match plan.role {
        Role::Share => push("share"),
        Role::Join => push("view"),
    }
    push("--ipc");
    push(&format!("--room={}", plan.room));
    push(&format!("--name={}", plan.name));
    if !plan.voice {
        push("--audio=off");
    }
    if plan.role == Role::Share {
        push(&format!("--resolution={}", plan.quality.resolution()));
        push(match codec {
            Codec::H264 => "--codec=h264",
            Codec::H265 => "--codec=h265",
        });
        // Requests reach this app as `control_asked`; nothing is granted until Allow is clicked.
        push("--allow-control");
        // This window is never in the picture.
        push(&format!("--exclude-app-pid={app_pid}"));
        if hooks.pattern {
            push("--capture=pattern");
        }
        if let Some(path) = &hooks.inject_record {
            push(&format!("--inject=record:{}", path.display()));
        }
    }
    args
}

/// Why the sharer switched to H.264.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Switch {
    /// A guest announced its decoders, and HEVC isn't among them.
    GuestCantDecode { name: String },
    /// The engine sharing with HEVC failed before it sent any video.
    StartFailed,
}

impl Switch {
    pub fn message(&self) -> String {
        match self {
            Switch::GuestCantDecode { name } => {
                format!("{name}'s computer can't decode HEVC; switched to H.264.")
            }
            Switch::StartFailed => "Sharing with HEVC failed; switched to H.264.".into(),
        }
    }
}

/// Whether a `participants` event means switching to H.264: sharing with HEVC, not yet switched
/// in this call, and a guest whose decoders are known and lack `h265`. A guest with no decoders
/// hasn't said yet, so nothing happens.
pub fn guest_fallback(
    codec: Codec,
    switched: bool,
    participants: &[ipc::Participant],
) -> Option<Switch> {
    if codec != Codec::H265 || switched {
        return None;
    }
    participants
        .iter()
        .find(|p| {
            p.role == ipc::Role::Guest
                && !p.decoders.is_empty()
                && !p.decoders.iter().any(|d| d == "h265")
        })
        .map(|p| Switch::GuestCantDecode {
            name: p.name.clone(),
        })
}

/// What is known about an engine when it exits.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ExitFacts {
    /// It sent at least one `stats` event: video went out.
    pub published: bool,
    /// It ended with `failed`.
    pub failed: bool,
    /// Its exit code, if it exited normally.
    pub code: Option<i32>,
    /// The person left, or the app is quitting.
    pub leaving: bool,
}

/// Whether an engine that exited should be started again with H.264: it shared with HEVC, failed
/// before sending any video, and this call hasn't switched already. Once only.
pub fn start_failed_fallback(codec: Codec, switched: bool, facts: ExitFacts) -> Option<Switch> {
    let failed = facts.failed || facts.code != Some(0);
    (codec == Codec::H265 && !switched && !facts.published && !facts.leaving && failed)
        .then_some(Switch::StartFailed)
}

/// What the app hears about a call. Sent to the webview as `{"kind": ..., ...}`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Output {
    /// An engine started. `args` are its flags (never the secret).
    Started {
        role: Role,
        codec: Codec,
        pid: u32,
        args: Vec<String>,
    },
    /// One event from the engine, as it printed it.
    Event { event: Value },
    /// One line of the engine's log.
    Log { line: String },
    /// The engine is being replaced with one sharing in `codec`, for the reason in `message`.
    Restarting { codec: Codec, message: String },
    /// The call's engine is gone, and the call with it.
    Exited { code: Option<i32> },
}

/// Where a call's output goes. Called from the call's own thread.
pub type Sink = Arc<dyn Fn(Output) + Send + Sync>;

/// How to start engines for a call.
#[derive(Clone)]
pub struct Launcher {
    pub exe: PathBuf,
    /// Settings overrides, as `(name, value)`. May hold the secret, so never logged.
    pub env: Vec<(String, String)>,
    pub app_pid: u32,
    pub hooks: TestHooks,
}

impl std::fmt::Debug for Launcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let env: Vec<&str> = self.env.iter().map(|(k, _)| k.as_str()).collect();
        f.debug_struct("Launcher")
            .field("exe", &self.exe)
            .field("env", &env)
            .field("app_pid", &self.app_pid)
            .field("hooks", &self.hooks)
            .finish()
    }
}

enum Msg {
    Line { generation: u64, line: String },
    Log { line: String },
    Eof { generation: u64 },
    Send(String),
    Leave,
    Shutdown,
}

/// A call in progress. Dropping it does nothing; [`Call::shutdown`] ends it.
pub struct Call {
    tx: mpsc::Sender<Msg>,
    thread: Option<JoinHandle<()>>,
    done: Arc<AtomicBool>,
}

impl Call {
    /// Starts the engine for `plan` and returns at once; everything else arrives through `sink`.
    pub fn start(launcher: Launcher, plan: Plan, sink: Sink) -> Result<Call, String> {
        let (tx, rx) = mpsc::channel();
        let mut controller = Controller {
            codec: plan.quality.codec(),
            plan,
            launcher,
            sink,
            tx: tx.clone(),
            switched: false,
            generation: 0,
            engine: None,
            facts: ExitFacts::default(),
            leave_sent: None,
        };
        controller.spawn()?;
        let done = Arc::new(AtomicBool::new(false));
        let finished = done.clone();
        let thread = std::thread::Builder::new()
            .name("call".into())
            .spawn(move || {
                controller.run(rx);
                finished.store(true, Ordering::Release);
            })
            .map_err(|e| format!("couldn't start the call: {e}"))?;
        Ok(Call {
            tx,
            thread: Some(thread),
            done,
        })
    }

    /// The engine is gone and the call is over.
    pub fn is_done(&self) -> bool {
        self.done.load(Ordering::Acquire)
    }

    pub fn send(&self, command: &ipc::Command) -> Result<(), String> {
        let line = serde_json::to_string(command).map_err(|e| e.to_string())?;
        self.tx
            .send(Msg::Send(line))
            .map_err(|_| "The call has ended.".to_owned())
    }

    /// Leaves the call: `leave`, then closing the engine's stdin, then killing it, each if the
    /// one before didn't end it in time. Returns at once.
    pub fn leave(&self) {
        let _ = self.tx.send(Msg::Leave);
    }

    /// Ends the call now (the app is quitting): closes the engine's stdin, kills it if it
    /// lingers, and waits for that.
    pub fn shutdown(mut self) {
        let _ = self.tx.send(Msg::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct Engine {
    generation: u64,
    child: Child,
    stdin: Option<ChildStdin>,
}

struct Controller {
    plan: Plan,
    launcher: Launcher,
    sink: Sink,
    tx: mpsc::Sender<Msg>,
    codec: Codec,
    switched: bool,
    generation: u64,
    engine: Option<Engine>,
    facts: ExitFacts,
    leave_sent: Option<Instant>,
}

fn forward_lines(from: impl Read + Send + 'static, mut each: impl FnMut(String) + Send + 'static) {
    std::thread::spawn(move || {
        for line in BufReader::new(from).lines() {
            let Ok(line) = line else { break };
            each(line);
        }
    });
}

/// Waits up to `timeout` for `child` to exit, then kills it. Its exit code, if it had one.
fn reap(child: &mut Child, timeout: Duration) -> Option<i32> {
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.code(),
            Ok(None) if start.elapsed() < timeout => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                return child.wait().ok().and_then(|s| s.code());
            }
        }
    }
}

impl Controller {
    fn emit(&self, output: Output) {
        (self.sink)(output);
    }

    fn spawn(&mut self) -> Result<(), String> {
        self.generation += 1;
        let generation = self.generation;
        let args = engine_args(
            &self.plan,
            self.codec,
            self.launcher.app_pid,
            &self.launcher.hooks,
        );
        let mut child = Command::new(&self.launcher.exe)
            .args(&args)
            .envs(self.launcher.env.iter().map(|(k, v)| (k, v)))
            .env("NO_COLOR", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| {
                format!(
                    "The engine couldn't start ({}): {e}.",
                    self.launcher.exe.display()
                )
            })?;
        if let Some(stdout) = child.stdout.take() {
            let tx = self.tx.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(stdout).lines() {
                    let Ok(line) = line else { break };
                    if tx.send(Msg::Line { generation, line }).is_err() {
                        return;
                    }
                }
                let _ = tx.send(Msg::Eof { generation });
            });
        }
        if let Some(stderr) = child.stderr.take() {
            let tx = self.tx.clone();
            forward_lines(stderr, move |line| {
                let _ = tx.send(Msg::Log { line });
            });
        }
        let stdin = child.stdin.take();
        self.emit(Output::Started {
            role: self.plan.role,
            codec: self.codec,
            pid: child.id(),
            args,
        });
        self.engine = Some(Engine {
            generation,
            child,
            stdin,
        });
        self.facts = ExitFacts::default();
        Ok(())
    }

    fn write(&mut self, line: &str) {
        let Some(stdin) = self.engine.as_mut().and_then(|e| e.stdin.as_mut()) else {
            return;
        };
        if let Err(e) = writeln!(stdin, "{line}").and_then(|()| stdin.flush()) {
            self.emit(Output::Log {
                line: format!("(couldn't write to the engine: {e})"),
            });
        }
    }

    /// Stops the current engine, as if the app had gone: stdin closed, killed if it lingers.
    fn retire(&mut self) -> Option<i32> {
        let mut engine = self.engine.take()?;
        drop(engine.stdin.take());
        reap(&mut engine.child, EXIT_TIMEOUT)
    }

    /// Replaces the engine with one sharing in H.264, for the rest of the call.
    fn switch(&mut self, why: Switch) -> Result<(), String> {
        self.retire();
        self.switched = true;
        self.codec = Codec::H264;
        self.leave_sent = None;
        self.emit(Output::Restarting {
            codec: self.codec,
            message: why.message(),
        });
        self.spawn()
    }

    fn on_line(&mut self, line: String) -> Result<(), String> {
        let Ok(event) = serde_json::from_str::<Value>(&line) else {
            self.emit(Output::Log {
                line: format!("(stdout) {line}"),
            });
            return Ok(());
        };
        let kind = event.get("event").and_then(Value::as_str).unwrap_or("");
        let mut switch = None;
        match kind {
            "stats" => self.facts.published = true,
            "ended" => {
                self.facts.failed = event.get("reason").and_then(Value::as_str) == Some("failed")
            }
            "participants" => {
                let people: Vec<ipc::Participant> = event
                    .get("participants")
                    .cloned()
                    .and_then(|p| serde_json::from_value(p).ok())
                    .unwrap_or_default();
                switch = guest_fallback(self.codec, self.switched, &people);
            }
            _ => {}
        }
        self.emit(Output::Event { event });
        match switch {
            Some(why) if self.leave_sent.is_none() => self.switch(why),
            _ => Ok(()),
        }
    }

    /// The current engine closed its stdout: it has exited, or is about to.
    fn on_eof(&mut self) -> Result<bool, String> {
        let Some(mut engine) = self.engine.take() else {
            return Ok(true);
        };
        drop(engine.stdin.take());
        self.facts.code = reap(&mut engine.child, EXIT_TIMEOUT);
        self.facts.leaving = self.leave_sent.is_some();
        if let Some(why) = start_failed_fallback(self.codec, self.switched, self.facts) {
            self.switch(why)?;
            return Ok(false);
        }
        self.emit(Output::Exited {
            code: self.facts.code,
        });
        Ok(true)
    }

    fn run(&mut self, rx: mpsc::Receiver<Msg>) {
        loop {
            let msg = match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(msg) => Some(msg),
                Err(mpsc::RecvTimeoutError::Timeout) => None,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            };
            let current = self.engine.as_ref().map(|e| e.generation);
            let result = match msg {
                None => Ok(false),
                Some(Msg::Line { generation, line }) if Some(generation) == current => {
                    self.on_line(line).map(|()| false)
                }
                // A replaced engine's last words: the call goes on without them.
                Some(Msg::Line { .. }) => Ok(false),
                Some(Msg::Log { line }) => {
                    self.emit(Output::Log { line });
                    Ok(false)
                }
                Some(Msg::Eof { generation }) if Some(generation) == current => self.on_eof(),
                Some(Msg::Eof { .. }) => Ok(false),
                Some(Msg::Send(line)) => {
                    self.write(&line);
                    Ok(false)
                }
                Some(Msg::Leave) => {
                    if self.leave_sent.is_none() {
                        self.write(r#"{"cmd":"leave"}"#);
                        self.leave_sent = Some(Instant::now());
                    }
                    Ok(false)
                }
                Some(Msg::Shutdown) => {
                    self.leave_sent.get_or_insert_with(Instant::now);
                    let code = self.retire();
                    self.emit(Output::Exited { code });
                    Ok(true)
                }
            };
            // An engine that ignores `leave` gets its stdin closed; `on_eof` kills it if needed.
            if let (Some(sent), Some(engine)) = (self.leave_sent, self.engine.as_mut()) {
                if sent.elapsed() > LEAVE_TIMEOUT {
                    drop(engine.stdin.take());
                }
            }
            match result {
                Ok(false) => {}
                Ok(true) => break,
                Err(message) => {
                    self.emit(Output::Event {
                        event: serde_json::json!({ "event": "error", "message": message }),
                    });
                    self.emit(Output::Exited { code: None });
                    break;
                }
            }
        }
    }
}

/// Where the engine is: next to this executable (in the bundle, `Contents/MacOS`; in a checkout,
/// the target directory), or the workspace's release build.
pub fn engine_path() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("can't find this app: {e}"))?;
    let mut tried = Vec::new();
    if let Some(dir) = exe.parent() {
        tried.push(dir.join(ENGINE));
    }
    tried.push(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../target/release")
            .join(ENGINE),
    );
    for path in &tried {
        if path.is_file() {
            return Ok(path.canonicalize().unwrap_or_else(|_| path.clone()));
        }
    }
    let tried: Vec<_> = tried.iter().map(|p| p.display().to_string()).collect();
    Err(format!(
        "no {ENGINE} at {} (run `cargo build --release -p twincursor-engine-cli`)",
        tried.join(" or ")
    ))
}

pub fn in_bundle(path: &Path) -> bool {
    path.parent()
        .is_some_and(|dir| dir.ends_with("Contents/MacOS"))
}

/// Runs a short engine command (`permissions`, `config`) and returns the JSON object it printed
/// last. `clear_env` names variables to leave out of its environment.
pub fn run_once(
    exe: &Path,
    args: &[&str],
    env: &[(String, String)],
    clear_env: &[&str],
) -> Result<Value, String> {
    let mut command = Command::new(exe);
    command
        .args(args)
        .envs(env.iter().map(|(k, v)| (k, v)))
        .env("NO_COLOR", "1")
        .stdin(Stdio::null());
    for name in clear_env {
        command.env_remove(name);
    }
    let output = command
        .output()
        .map_err(|e| format!("couldn't run {}: {e}", exe.display()))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    last_json(stdout.lines()).ok_or_else(|| {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let last = stderr.lines().last().unwrap_or("").trim();
        format!(
            "{ENGINE} {} printed nothing ({}). {last}",
            args.join(" "),
            output.status
        )
    })
}

/// The last line that is a JSON object.
pub fn last_json<'a>(lines: impl DoubleEndedIterator<Item = &'a str>) -> Option<Value> {
    lines
        .rev()
        .filter(|l| l.starts_with('{'))
        .find_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(Value::is_object)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{SecretChange, Settings, SettingsChange};

    const SECRET: &str = "s3cr3t-value-that-must-not-leak";

    fn plan(role: Role, quality: Quality, voice: bool) -> Plan {
        Plan {
            role,
            room: "7kq2-m9xd-p4tz-hc3w".into(),
            name: "Alex".into(),
            quality,
            voice,
        }
    }

    fn has(args: &[String], flag: &str) -> bool {
        args.iter().any(|a| a == flag)
    }

    #[test]
    fn flags_for_each_choice() {
        let none = TestHooks::default();
        let share = |q, voice| {
            let p = plan(Role::Share, q, voice);
            engine_args(&p, p.quality.codec(), 4242, &none)
        };
        let a = share(Quality::P1080, true);
        assert_eq!(&a[..2], ["share", "--ipc"]);
        for flag in [
            "--room=7kq2-m9xd-p4tz-hc3w",
            "--name=Alex",
            "--resolution=1080",
            "--codec=h264",
            "--allow-control",
            "--exclude-app-pid=4242",
        ] {
            assert!(has(&a, flag), "{flag} in {a:?}");
        }
        assert!(
            !a.iter().any(|f| f.starts_with("--audio")),
            "voice on is the default"
        );
        assert!(!a
            .iter()
            .any(|f| f.starts_with("--capture") || f.starts_with("--inject")));

        let b = share(Quality::P1440, false);
        assert!(has(&b, "--resolution=1440") && has(&b, "--codec=h264"));
        assert!(has(&b, "--audio=off"));

        let c = share(Quality::K4, true);
        assert!(has(&c, "--resolution=2160") && has(&c, "--codec=h265"));
        let c = engine_args(&plan(Role::Share, Quality::K4, true), Codec::H264, 1, &none);
        assert!(has(&c, "--resolution=2160") && has(&c, "--codec=h264"));

        let j = engine_args(
            &plan(Role::Join, Quality::K4, false),
            Codec::H265,
            4242,
            &none,
        );
        assert_eq!(&j[..2], ["view", "--ipc"]);
        assert!(has(&j, "--audio=off") && has(&j, "--name=Alex"));
        for absent in [
            "--allow-control",
            "--resolution",
            "--codec",
            "--exclude-app-pid",
        ] {
            assert!(
                !j.iter().any(|f| f.starts_with(absent)),
                "{absent} in {j:?}"
            );
        }
        let j = engine_args(
            &plan(Role::Join, Quality::P1080, true),
            Codec::H264,
            1,
            &none,
        );
        assert!(!j.iter().any(|f| f.starts_with("--audio")));
    }

    #[test]
    fn test_hooks_only_add_what_they_say() {
        let hooks = TestHooks {
            pattern: true,
            inject_record: Some("/tmp/actions.jsonl".into()),
        };
        let a = engine_args(
            &plan(Role::Share, Quality::P1080, false),
            Codec::H264,
            1,
            &hooks,
        );
        assert!(has(&a, "--capture=pattern"));
        assert!(has(&a, "--inject=record:/tmp/actions.jsonl"));
    }

    #[test]
    fn a_name_that_looks_like_a_flag_stays_a_name() {
        let mut p = plan(Role::Share, Quality::P1080, true);
        p.name = "--inject=real".into();
        let a = engine_args(&p, Codec::H264, 1, &TestHooks::default());
        assert!(has(&a, "--name=--inject=real"));
        assert!(!has(&a, "--inject=real"));
    }

    #[test]
    fn the_secret_is_never_a_flag() {
        let mut settings = Settings::default();
        settings.apply(SettingsChange {
            url: Some("wss://example.test".into()),
            api_key: Some("key".into()),
            api_secret: SecretChange::Set {
                value: SECRET.into(),
            },
        });
        let env = settings.env();
        assert!(
            env.iter().any(|(_, v)| v == SECRET),
            "it reaches the engine"
        );
        let hooks = [
            TestHooks::default(),
            TestHooks {
                pattern: true,
                inject_record: Some("/tmp/a".into()),
            },
        ];
        for role in [Role::Share, Role::Join] {
            for quality in [Quality::P1080, Quality::P1440, Quality::K4] {
                for voice in [true, false] {
                    for codec in [Codec::H264, Codec::H265] {
                        for h in &hooks {
                            let args = engine_args(&plan(role, quality, voice), codec, 1, h);
                            assert!(!args.iter().any(|a| a.contains(SECRET)), "{args:?}");
                            assert!(!args.iter().any(|a| a.contains("secret")), "{args:?}");
                        }
                    }
                }
            }
        }
        let launcher = Launcher {
            exe: "/x".into(),
            env,
            app_pid: 1,
            hooks: TestHooks::default(),
        };
        assert!(!format!("{launcher:?}").contains(SECRET));
    }

    #[test]
    fn plans_are_checked() {
        let mut p = plan(Role::Join, Quality::P1080, true);
        p.name = "  Sam  ".into();
        assert_eq!(p.clone().checked().unwrap().name, "Sam");
        p.name = " ".into();
        assert!(p.clone().checked().is_err());
        p.name = "Sam".into();
        p.room = String::new();
        assert!(p.clone().checked().is_err());
        p.room = "a\nb".into();
        assert!(p.clone().checked().is_err());
        p.room = "abc".into();
        p.name = "x".repeat(MAX_NAME + 1);
        assert!(p.checked().is_err());
    }

    fn guest(name: &str, decoders: &[&str]) -> ipc::Participant {
        ipc::Participant {
            identity: format!("guest-{name}"),
            name: name.into(),
            role: ipc::Role::Guest,
            decoders: decoders.iter().map(|d| (*d).to_owned()).collect(),
        }
    }

    #[test]
    fn hevc_falls_back_for_a_guest_that_cant_decode_it() {
        let sam = guest("Sam", &["h264", "vp8"]);
        assert_eq!(
            guest_fallback(Codec::H265, false, std::slice::from_ref(&sam)),
            Some(Switch::GuestCantDecode { name: "Sam".into() })
        );
        assert_eq!(
            Switch::GuestCantDecode { name: "Sam".into() }.message(),
            "Sam's computer can't decode HEVC; switched to H.264."
        );
        // Already on H.264, or already switched: nothing.
        assert_eq!(
            guest_fallback(Codec::H264, false, std::slice::from_ref(&sam)),
            None
        );
        assert_eq!(
            guest_fallback(Codec::H265, true, std::slice::from_ref(&sam)),
            None
        );
        // A guest who hasn't said yet, or who decodes HEVC: nothing.
        assert_eq!(
            guest_fallback(Codec::H265, false, &[guest("Ana", &[])]),
            None
        );
        assert_eq!(
            guest_fallback(Codec::H265, false, &[guest("Ana", &["h264", "h265"])]),
            None
        );
        // One guest that can't is enough.
        let both = [guest("Ana", &["h265"]), sam.clone()];
        assert!(guest_fallback(Codec::H265, false, &both).is_some());
        // Only guests count.
        let mut sharer = sam;
        sharer.role = ipc::Role::Sharer;
        assert_eq!(guest_fallback(Codec::H265, false, &[sharer]), None);
    }

    #[test]
    fn hevc_falls_back_once_when_it_fails_to_start() {
        let failed = ExitFacts {
            published: false,
            failed: true,
            code: Some(1),
            leaving: false,
        };
        assert_eq!(
            start_failed_fallback(Codec::H265, false, failed),
            Some(Switch::StartFailed)
        );
        let crashed = ExitFacts {
            failed: false,
            code: None,
            ..failed
        };
        assert!(start_failed_fallback(Codec::H265, false, crashed).is_some());
        assert_eq!(start_failed_fallback(Codec::H265, true, failed), None);
        assert_eq!(start_failed_fallback(Codec::H264, false, failed), None);
        let sent_video = ExitFacts {
            published: true,
            ..failed
        };
        assert_eq!(start_failed_fallback(Codec::H265, false, sent_video), None);
        let left = ExitFacts {
            leaving: true,
            ..failed
        };
        assert_eq!(start_failed_fallback(Codec::H265, false, left), None);
        let fine = ExitFacts {
            failed: false,
            code: Some(0),
            ..failed
        };
        assert_eq!(start_failed_fallback(Codec::H265, false, fine), None);
    }

    #[test]
    fn finds_the_report_among_log_lines() {
        let lines = [
            "INFO starting",
            r#"{"screen_recording":"granted"}"#,
            "{not json",
        ];
        assert_eq!(
            last_json(lines.iter().copied()).unwrap()["screen_recording"],
            "granted"
        );
        assert!(last_json(["[1, 2]"].iter().copied()).is_none());
    }

    #[test]
    fn knows_a_bundled_engine() {
        assert!(in_bundle(Path::new(
            "/Applications/TwinCursor.app/Contents/MacOS/tc-engine"
        )));
        assert!(!in_bundle(Path::new(
            "/src/twincursor/target/release/tc-engine"
        )));
    }

    /// A stand-in engine: says hello, and with `--codec=h265` reports a guest without HEVC (or,
    /// with FAIL_HEVC set, fails at once). Echoes its commands to stderr; `leave` ends it.
    #[cfg(unix)]
    fn fake_engine(name: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let dir =
            std::env::temp_dir().join(format!("twincursor-fake-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("tc-engine");
        std::fs::write(
            &path,
            r#"#!/bin/sh
echo '{"event":"hello","version":1,"role":"sharer","engine":"fake"}'
[ -n "$TWINCURSOR_API_SECRET" ] && echo "a secret is set" >&2
case "$*" in
  *codec=h265*)
    if [ -n "$FAIL_HEVC" ]; then echo '{"event":"ended","reason":"failed"}'; exit 1; fi
    echo '{"event":"participants","participants":[{"identity":"guest-1","name":"Sam","role":"guest","decoders":["h264","vp8"]}]}' ;;
  *) echo '{"event":"stats","width":1,"height":1,"fps":1.0,"kbps":1.0,"rtt_ms":null,"codec":"h264","shown_pct":null}' ;;
esac
while read -r line; do
  echo "got $line" >&2
  case "$line" in *leave*) echo '{"event":"ended","reason":"left"}'; exit 0 ;; esac
done
"#,
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[cfg(unix)]
    fn run_fake(name: &str, env: Vec<(String, String)>, leave: bool) -> Vec<Output> {
        let (tx, rx) = mpsc::channel();
        let sink: Sink = Arc::new(move |o| {
            let _ = tx.send(o);
        });
        let launcher = Launcher {
            exe: fake_engine(name),
            env,
            app_pid: 7,
            hooks: TestHooks::default(),
        };
        let call = Call::start(launcher, plan(Role::Share, Quality::K4, false), sink).unwrap();
        let mut seen = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut left = false;
        while Instant::now() < deadline {
            let Ok(output) = rx.recv_timeout(Duration::from_millis(50)) else {
                continue;
            };
            let exited = matches!(output, Output::Exited { .. });
            let h264_running =
                matches!(&output, Output::Event { event } if event["event"] == "stats");
            seen.push(output);
            if exited {
                break;
            }
            if h264_running && leave && !left {
                call.send(&ipc::Command::Mute).unwrap();
                call.leave();
                left = true;
            }
        }
        assert!(call.is_done() || seen.iter().any(|o| matches!(o, Output::Exited { .. })));
        seen
    }

    #[cfg(unix)]
    #[test]
    fn a_call_switches_to_h264_for_a_guest_and_leaves() {
        let env = vec![("TWINCURSOR_API_SECRET".to_owned(), SECRET.to_owned())];
        let seen = run_fake("guest", env, true);
        let started: Vec<_> = seen
            .iter()
            .filter_map(|o| match o {
                Output::Started { codec, args, .. } => Some((*codec, args.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(started.len(), 2, "{seen:#?}");
        assert_eq!(started[0].0, Codec::H265);
        assert_eq!(started[1].0, Codec::H264);
        assert!(started[1].1.iter().any(|a| a == "--codec=h264"));
        assert!(seen.iter().any(|o| matches!(o,
            Output::Restarting { codec: Codec::H264, message } if message.starts_with("Sam's computer"))));
        // The secret reached the engine's environment, and nothing it printed carries it.
        assert!(seen
            .iter()
            .any(|o| matches!(o, Output::Log { line } if line == "a secret is set")));
        let all = format!("{seen:?}");
        assert!(!all.contains(SECRET));
        // Commands went to the engine that's running now, and leave ended it.
        assert!(seen
            .iter()
            .any(|o| matches!(o, Output::Log { line } if line == r#"got {"cmd":"mute"}"#)));
        assert!(seen.iter().any(
            |o| matches!(o, Output::Event { event } if event["event"] == "ended" && event["reason"] == "left")
        ));
        assert_eq!(seen.last(), Some(&Output::Exited { code: Some(0) }));
        // The replaced engine's `ended` never reached the app: one only, from the second.
        let ended = seen
            .iter()
            .filter(|o| matches!(o, Output::Event { event } if event["event"] == "ended"))
            .count();
        assert_eq!(ended, 1);
    }

    #[cfg(unix)]
    #[test]
    fn a_call_switches_to_h264_when_hevc_fails_to_start() {
        let env = vec![("FAIL_HEVC".to_owned(), "1".to_owned())];
        let seen = run_fake("fails", env, true);
        assert!(seen.iter().any(|o| matches!(o,
            Output::Restarting { codec: Codec::H264, message } if message.starts_with("Sharing with HEVC failed"))));
        assert_eq!(
            seen.iter()
                .filter(|o| matches!(o, Output::Started { .. }))
                .count(),
            2
        );
        assert_eq!(seen.last(), Some(&Output::Exited { code: Some(0) }));
    }

    #[cfg(unix)]
    #[test]
    fn shutdown_closes_the_engine() {
        let (tx, rx) = mpsc::channel();
        let sink: Sink = Arc::new(move |o| {
            let _ = tx.send(o);
        });
        let launcher = Launcher {
            exe: fake_engine("shutdown"),
            env: Vec::new(),
            app_pid: 7,
            hooks: TestHooks::default(),
        };
        let call = Call::start(launcher, plan(Role::Join, Quality::P1080, true), sink).unwrap();
        // Wait for the engine to be up.
        while !matches!(
            rx.recv_timeout(Duration::from_secs(5)),
            Ok(Output::Event { .. })
        ) {}
        let start = Instant::now();
        call.shutdown();
        assert!(start.elapsed() < EXIT_TIMEOUT, "closing stdin was enough");
        let rest: Vec<Output> = rx.try_iter().collect();
        assert!(rest.iter().any(|o| matches!(o, Output::Exited { .. })));
    }
}
