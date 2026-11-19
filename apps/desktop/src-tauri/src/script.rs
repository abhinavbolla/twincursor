//! `--script PATH --report PATH`: a test-only mode for `bench/app-e2e.sh`, with nobody at the
//! Mac. The app shares the synthetic pattern with voice off and input recorded (never injected),
//! answers control requests as the script says, and writes everything it sees to the report as
//! JSON lines.
//!
//! This is the one place anything answers a control request without a click, and it only exists
//! behind the flag. Without `--script`, only the Allow button grants control.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{Map, Value};

use crate::engine::{Output, Plan, Quality, Role, TestHooks};

/// How long a step may wait for what it waits for.
pub const STEP_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Script {
    pub call: ScriptCall,
    pub steps: Vec<Step>,
    /// Quit the app once the steps are done. Otherwise it stays, as a person's would.
    #[serde(default)]
    pub quit: bool,
}

/// A share on the synthetic pattern, voice off. Where recorded input goes is required: a script
/// can't make the app inject real input.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScriptCall {
    pub room: String,
    pub name: String,
    pub quality: Quality,
    pub inject_record: PathBuf,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "do", rename_all = "snake_case", deny_unknown_fields)]
pub enum Step {
    /// Wait for the next `control_asked`, and answer that request.
    Answer {
        with: Answer,
    },
    /// Wait for an event with these fields (`{"event": "control", "state": "controlling"}`).
    WaitFor {
        event: Map<String, Value>,
    },
    WaitMs {
        ms: u64,
    },
    /// Send a command that grants nothing.
    Send {
        cmd: Plain,
    },
    /// Wait until the call's engine is gone.
    WaitExit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Answer {
    Allow,
    Deny,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Plain {
    RevokeControl,
    Leave,
    Mute,
    Unmute,
}

impl Plain {
    fn command(self) -> ipc::Command {
        match self {
            Plain::RevokeControl => ipc::Command::RevokeControl,
            Plain::Leave => ipc::Command::Leave,
            Plain::Mute => ipc::Command::Mute,
            Plain::Unmute => ipc::Command::Unmute,
        }
    }
}

impl Script {
    pub fn read(path: &Path) -> Result<Script, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("can't read {}: {e}", path.display()))?;
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn plan(&self) -> Plan {
        Plan {
            role: Role::Share,
            room: self.call.room.clone(),
            name: self.call.name.clone(),
            quality: self.call.quality,
            voice: false,
        }
    }

    pub fn hooks(&self) -> TestHooks {
        TestHooks {
            pattern: true,
            inject_record: Some(self.call.inject_record.clone()),
        }
    }
}

/// The report: one JSON object per line, flushed as it goes so a script can follow it.
pub struct Report {
    file: Mutex<std::fs::File>,
    start: Instant,
}

impl Report {
    pub fn create(path: &Path) -> Result<Report, String> {
        let file = std::fs::File::create(path)
            .map_err(|e| format!("can't write {}: {e}", path.display()))?;
        Ok(Report {
            file: Mutex::new(file),
            start: Instant::now(),
        })
    }

    pub fn write(&self, mut value: Value) {
        if let Some(object) = value.as_object_mut() {
            object.insert(
                "t_ms".into(),
                Value::from(self.start.elapsed().as_millis() as u64),
            );
        }
        let mut file = self.file.lock().unwrap();
        let _ = writeln!(file, "{value}");
        let _ = file.flush();
    }

    pub fn output(&self, output: &Output) {
        self.write(serde_json::to_value(output).unwrap_or(Value::Null));
    }
}

fn fits(event: &Value, want: &Map<String, Value>) -> bool {
    want.iter().all(|(k, v)| event.get(k) == Some(v))
}

/// Plays `steps`, reading the call's output from `outputs` and sending commands with `send`.
/// Stops at the first step that fails.
pub fn play(
    steps: &[Step],
    outputs: &Receiver<Output>,
    send: &dyn Fn(&ipc::Command) -> Result<(), String>,
    report: &dyn Fn(Value),
    timeout: Duration,
) -> Result<(), String> {
    // Waits for the first output `pick` takes, skipping the rest.
    let next = |what: &str, pick: &mut dyn FnMut(&Output) -> bool| -> Result<Output, String> {
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match outputs.recv_timeout(left) {
                Ok(output) if pick(&output) => return Ok(output),
                Ok(Output::Exited { .. }) if what != "exit" => {
                    return Err(format!("the call ended while waiting for {what}"))
                }
                Ok(_) => {}
                Err(RecvTimeoutError::Timeout) => return Err(format!("no {what} in time")),
                Err(RecvTimeoutError::Disconnected) => return Err(format!("no {what}: no call")),
            }
        }
    };
    for (i, step) in steps.iter().enumerate() {
        let result = match step {
            Step::Answer { with } => next(
                "control request",
                &mut |o| matches!(o, Output::Event { event } if event["event"] == "control_asked"),
            )
            .and_then(|asked| {
                let Output::Event { event } = asked else {
                    unreachable!()
                };
                let request = event["request"]
                    .as_u64()
                    .ok_or("a control request without a number")?;
                // The answer names the request it answers, never another.
                send(&match with {
                    Answer::Allow => ipc::Command::AllowControl { request },
                    Answer::Deny => ipc::Command::DenyControl { request },
                })
            }),
            Step::WaitFor { event: want } => next(
                "the event",
                &mut |o| matches!(o, Output::Event { event } if fits(event, want)),
            )
            .map(|_| ()),
            Step::WaitMs { ms } => {
                std::thread::sleep(Duration::from_millis(*ms));
                Ok(())
            }
            Step::Send { cmd } => send(&cmd.command()),
            Step::WaitExit => next("exit", &mut |o| matches!(o, Output::Exited { .. })).map(|_| ()),
        };
        report(serde_json::json!({
            "kind": "script",
            "step": i,
            "what": format!("{step:?}"),
            "ok": result.is_ok(),
            "error": result.as_ref().err(),
        }));
        result.map_err(|e| format!("step {i} ({step:?}): {e}"))?;
    }
    Ok(())
}

/// `--script PATH --report PATH` from the command line, if both are there.
pub fn args_from(mut args: impl Iterator<Item = std::ffi::OsString>) -> Option<(PathBuf, PathBuf)> {
    let (mut script, mut report) = (None, None);
    while let Some(arg) = args.next() {
        if arg == "--script" {
            script = args.next().map(PathBuf::from);
        } else if arg == "--report" {
            report = args.next().map(PathBuf::from);
        }
    }
    script.zip(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::sync::mpsc;

    fn event(json: &str) -> Output {
        Output::Event {
            event: serde_json::from_str(json).unwrap(),
        }
    }

    #[test]
    fn answers_name_the_request_they_answer() {
        let (tx, rx) = mpsc::channel();
        tx.send(event(r#"{"event":"connection","state":"connected"}"#))
            .unwrap();
        tx.send(event(r#"{"event":"control_asked","request":3,"identity":"guest-1","name":"Sam","platform":"macos"}"#)).unwrap();
        tx.send(event(
            r#"{"event":"control","state":"controlling","by":{"identity":"guest-1","name":"Sam"}}"#,
        ))
        .unwrap();
        tx.send(event(r#"{"event":"control_asked","request":4,"identity":"guest-1","name":"Sam","platform":"macos"}"#)).unwrap();
        tx.send(Output::Exited { code: Some(0) }).unwrap();
        let steps: Vec<Step> = serde_json::from_str(
            r#"[{"do":"answer","with":"allow"},
                {"do":"wait_for","event":{"event":"control","state":"controlling"}},
                {"do":"send","cmd":"revoke_control"},
                {"do":"answer","with":"deny"},
                {"do":"wait_exit"}]"#,
        )
        .unwrap();
        let sent = RefCell::new(Vec::new());
        let lines = RefCell::new(Vec::new());
        play(
            &steps,
            &rx,
            &|c| {
                sent.borrow_mut().push(c.clone());
                Ok(())
            },
            &|v| lines.borrow_mut().push(v),
            Duration::from_secs(1),
        )
        .unwrap();
        assert_eq!(
            sent.into_inner(),
            [
                ipc::Command::AllowControl { request: 3 },
                ipc::Command::RevokeControl,
                ipc::Command::DenyControl { request: 4 },
            ]
        );
        assert_eq!(lines.borrow().len(), 5);
    }

    #[test]
    fn nothing_is_answered_without_a_request() {
        let (tx, rx) = mpsc::channel();
        tx.send(event(r#"{"event":"connection","state":"connected"}"#))
            .unwrap();
        let steps = [Step::Answer {
            with: Answer::Allow,
        }];
        let sent = RefCell::new(Vec::new());
        let err = play(
            &steps,
            &rx,
            &|c| {
                sent.borrow_mut().push(c.clone());
                Ok(())
            },
            &|_| {},
            Duration::from_millis(100),
        )
        .unwrap_err();
        assert!(err.contains("no control request in time"), "{err}");
        assert!(sent.borrow().is_empty());
        drop(tx);
    }

    #[test]
    fn scripts_cannot_grant_by_sending() {
        // `send` only takes commands that grant nothing.
        let bad: Result<Step, _> = serde_json::from_str(r#"{"do":"send","cmd":"allow_control"}"#);
        assert!(bad.is_err());
        // And a script must say where recorded input goes.
        let no_record: Result<Script, _> = serde_json::from_str(
            r#"{"call":{"room":"r","name":"Alex","quality":"1080p"},"steps":[]}"#,
        );
        assert!(no_record.is_err());
        let script: Script = serde_json::from_str(
            r#"{"call":{"room":"r","name":"Alex","quality":"4k","inject_record":"/tmp/a.jsonl"},"steps":[],"quit":true}"#,
        )
        .unwrap();
        assert!(!script.plan().voice);
        assert!(script.hooks().pattern);
        assert_eq!(script.hooks().inject_record, Some("/tmp/a.jsonl".into()));
    }

    #[test]
    fn reads_its_flags() {
        let args =
            ["app", "--script", "s.json", "--report", "r.jsonl"].map(std::ffi::OsString::from);
        assert_eq!(
            args_from(args.into_iter().skip(1)),
            Some(("s.json".into(), "r.jsonl".into()))
        );
        let half = ["app", "--script", "s.json"].map(std::ffi::OsString::from);
        assert_eq!(args_from(half.into_iter().skip(1)), None);
    }
}
