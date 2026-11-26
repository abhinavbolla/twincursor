//! `--script PATH --report PATH`: a test-only mode for the benches (`bench/app-e2e.sh`,
//! `bench/app-accounts-e2e.sh`), with nobody at the Mac. The app shares the synthetic pattern
//! with voice off and input recorded (never injected), uses a clipboard in memory (the real one
//! is never read or written) and treats the permissions as granted (the pattern needs no Screen
//! Recording, recorded input no Accessibility). It plays the script's steps and writes
//! everything it sees to the report as JSON lines: engine output, every `home` view, and what
//! the call flows did (`trace`).
//!
//! A script is either a room-code call (`call`, started at once, as before accounts) or the
//! account flows (`accounts`, signed in with the session file in `TWINCURSOR_SESSION_FILE`).
//!
//! This is the one place anything answers a knock or a control request without a click, and it
//! only exists behind the flag. It goes through the same checks as a click: an answer that
//! comes too soon is refused, so the steps wait for the arming delay like a person would.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{Map, Value};

use crate::calls::{ARMING, TOO_SOON};
use crate::engine::{Output, Plan, Quality, Role, TestHooks};

/// How long a step may wait for what it waits for.
pub const STEP_TIMEOUT: Duration = Duration::from_secs(60);
/// How often a step looks at the home view while it waits.
const HOME_EVERY: Duration = Duration::from_millis(100);

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Script {
    /// A room-code share, started at once.
    #[serde(default)]
    pub call: Option<ScriptCall>,
    /// The account flows instead: the steps share, join and answer.
    #[serde(default)]
    pub accounts: Option<ScriptAccounts>,
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

/// The account flows, on the synthetic pattern with voice off and input recorded.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScriptAccounts {
    #[serde(default = "best")]
    pub quality: Quality,
    pub inject_record: PathBuf,
}

fn best() -> Quality {
    Quality::K4
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "do", rename_all = "snake_case", deny_unknown_fields)]
pub enum Step {
    /// Wait for the next `control_asked`, and answer that request. An allow waits out the
    /// arming delay; with `early`, it first tries at once and must be refused.
    Answer {
        with: Answer,
        #[serde(default)]
        early: bool,
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
    /// Share my screen.
    Share,
    /// Join with a call link (or a bare code).
    JoinLink {
        link: String,
    },
    /// Stop waiting to be let in.
    CancelKnock,
    /// Wait until someone with this name knocks on this app's call.
    WaitKnock {
        name: String,
    },
    /// Let them in (with control, or not) or turn them away. Letting in waits until the window
    /// shows the knock armed; with `try_early`, it first tries at once and must be refused.
    AnswerKnock {
        name: String,
        let_in: bool,
        #[serde(default)]
        control: bool,
        #[serde(default)]
        try_early: bool,
    },
    /// Give control to the guest in the call with this name.
    GiveControl {
        name: String,
    },
    /// Ring the contact with this name.
    Ring {
        name: String,
    },
    /// Wait until the person with this name rings this app.
    WaitRing {
        name: String,
    },
    /// Answer the call ringing this app.
    AnswerRing {
        accept: bool,
    },
    /// Wait until the home view contains these fields (objects in part, each array element
    /// somewhere in the array).
    WaitHome {
        home: Map<String, Value>,
    },
    /// Stop sharing, or leave.
    End,
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

/// A call flow a step runs, as the window's commands do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Act {
    Share,
    JoinLink(String),
    CancelKnock,
    AnswerKnock {
        user_id: String,
        let_in: bool,
        control: bool,
    },
    GiveControl {
        name: String,
    },
    Ring {
        user_id: String,
    },
    AnswerRing {
        call_id: String,
        accept: bool,
    },
    End,
}

impl Script {
    pub fn read(path: &Path) -> Result<Script, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("can't read {}: {e}", path.display()))?;
        let script: Script =
            serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        script.check()?;
        Ok(script)
    }

    fn check(&self) -> Result<(), String> {
        match (&self.call, &self.accounts) {
            (Some(_), None) | (None, Some(_)) => Ok(()),
            _ => Err("a script has either `call` or `accounts`".into()),
        }
    }

    /// The room-code call to start at once, if this script has one.
    pub fn plan(&self) -> Option<Plan> {
        self.call.as_ref().map(|call| Plan {
            role: Role::Share,
            room: call.room.clone(),
            name: call.name.clone(),
            quality: call.quality,
            voice: false,
            pass: false,
        })
    }

    pub fn quality(&self) -> Quality {
        match (&self.call, &self.accounts) {
            (Some(call), _) => call.quality,
            (_, Some(accounts)) => accounts.quality,
            _ => best(),
        }
    }

    pub fn hooks(&self) -> TestHooks {
        let record = match (&self.call, &self.accounts) {
            (Some(call), _) => Some(call.inject_record.clone()),
            (_, Some(accounts)) => Some(accounts.inject_record.clone()),
            _ => None,
        };
        TestHooks {
            pattern: true,
            inject_record: record,
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

/// Whether `want` is in `have`: objects in part, each element of a wanted array matching some
/// element of the array there, anything else equal.
pub fn contains(have: &Value, want: &Value) -> bool {
    match (have, want) {
        (Value::Object(h), Value::Object(w)) => w
            .iter()
            .all(|(k, v)| h.get(k).is_some_and(|hv| contains(hv, v))),
        (Value::Array(h), Value::Array(w)) => {
            w.iter().all(|wv| h.iter().any(|hv| contains(hv, wv)))
        }
        _ => have == want,
    }
}

/// What the steps play against.
pub struct Host<'a> {
    /// The call's engine output.
    pub outputs: &'a Receiver<Output>,
    /// A command for the engine, through the same checks as the window's.
    pub send: &'a dyn Fn(&ipc::Command) -> Result<(), String>,
    pub report: &'a dyn Fn(Value),
    /// The home view, as the window would read it.
    pub home: &'a dyn Fn() -> Value,
    /// Runs a call flow; the home view after it.
    pub act: &'a dyn Fn(Act) -> Result<Value, String>,
    pub timeout: Duration,
}

impl Host<'_> {
    /// Waits for the first output `pick` takes, skipping the rest.
    fn next(&self, what: &str, pick: &mut dyn FnMut(&Output) -> bool) -> Result<Output, String> {
        let deadline = Instant::now() + self.timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.outputs.recv_timeout(left) {
                Ok(output) if pick(&output) => return Ok(output),
                Ok(Output::Exited { .. }) if what != "exit" => {
                    return Err(format!("the call ended while waiting for {what}"))
                }
                Ok(_) => {}
                Err(RecvTimeoutError::Timeout) => return Err(format!("no {what} in time")),
                Err(RecvTimeoutError::Disconnected) => return Err(format!("no {what}: no call")),
            }
        }
    }

    /// Waits until `find` finds something in the home view.
    fn in_home<T>(&self, what: &str, find: &dyn Fn(&Value) -> Option<T>) -> Result<T, String> {
        let deadline = Instant::now() + self.timeout;
        loop {
            if let Some(found) = find(&(self.home)()) {
                return Ok(found);
            }
            if Instant::now() >= deadline {
                return Err(format!("no {what} in time"));
            }
            std::thread::sleep(HOME_EVERY);
        }
    }

    /// Runs `act` once now and expects it to be refused as too soon.
    fn early(&self, act: Act) -> Result<(), String> {
        match (self.act)(act) {
            Err(e) if e == TOO_SOON => {
                (self.report)(serde_json::json!({ "kind": "script", "early_refused": e }));
                Ok(())
            }
            Err(e) => Err(format!(
                "an early answer was refused, but not as too soon: {e}"
            )),
            Ok(_) => Err("an early answer was accepted".into()),
        }
    }

    fn answer(&self, with: Answer, early: bool) -> Result<(), String> {
        let asked = self.next(
            "control request",
            &mut |o| matches!(o, Output::Event { event } if event["event"] == "control_asked"),
        )?;
        let heard = Instant::now();
        let Output::Event { event } = asked else {
            unreachable!()
        };
        let request = event["request"]
            .as_u64()
            .ok_or("a control request without a number")?;
        // The answer names the request it answers, never another.
        match with {
            Answer::Deny => (self.send)(&ipc::Command::DenyControl { request }),
            Answer::Allow => {
                let allow = ipc::Command::AllowControl { request };
                if early {
                    match (self.send)(&allow) {
                        Err(e) if e == TOO_SOON => (self.report)(
                            serde_json::json!({ "kind": "script", "early_refused": e }),
                        ),
                        other => {
                            return Err(format!(
                                "an allow sent at once wasn't refused as too soon: {other:?}"
                            ))
                        }
                    }
                }
                // Like a person: the button arms a second after the request shows.
                std::thread::sleep(ARMING.saturating_sub(heard.elapsed()));
                let deadline = Instant::now() + Duration::from_secs(2);
                loop {
                    match (self.send)(&allow) {
                        Err(e) if e == TOO_SOON && Instant::now() < deadline => {
                            std::thread::sleep(Duration::from_millis(100))
                        }
                        other => return other,
                    }
                }
            }
        }
    }

    fn knock_by(&self, name: &str) -> Result<Value, String> {
        self.in_home(&format!("knock from {name}"), &|home| {
            home["knocks"]
                .as_array()?
                .iter()
                .find(|k| k["name"] == name)
                .cloned()
        })
    }

    fn answer_knock(
        &self,
        name: &str,
        let_in: bool,
        control: bool,
        early: bool,
    ) -> Result<(), String> {
        let knock = self.knock_by(name)?;
        let user_id = knock["user_id"]
            .as_str()
            .ok_or("a knock without an id")?
            .to_owned();
        let act = Act::AnswerKnock {
            user_id: user_id.clone(),
            let_in,
            control,
        };
        if let_in {
            if early {
                self.early(act.clone())?;
            }
            // Like a person: Let in can be clicked once the window shows it armed.
            self.in_home("an armed knock", &|home| {
                home["knocks"]
                    .as_array()?
                    .iter()
                    .find(|k| k["user_id"] == user_id.as_str() && k["armed"] == true)
                    .map(|_| ())
            })?;
        }
        (self.act)(act).map(drop)
    }

    fn step(&self, step: &Step) -> Result<(), String> {
        match step {
            Step::Answer { with, early } => self.answer(*with, *early),
            Step::WaitFor { event: want } => self
                .next(
                    "the event",
                    &mut |o| matches!(o, Output::Event { event } if fits(event, want)),
                )
                .map(drop),
            Step::WaitMs { ms } => {
                std::thread::sleep(Duration::from_millis(*ms));
                Ok(())
            }
            Step::Send { cmd } => (self.send)(&cmd.command()),
            Step::WaitExit => self
                .next("exit", &mut |o| matches!(o, Output::Exited { .. }))
                .map(drop),
            Step::Share => (self.act)(Act::Share).map(drop),
            Step::JoinLink { link } => (self.act)(Act::JoinLink(link.clone())).map(drop),
            Step::CancelKnock => (self.act)(Act::CancelKnock).map(drop),
            Step::WaitKnock { name } => self.knock_by(name).map(drop),
            Step::AnswerKnock {
                name,
                let_in,
                control,
                try_early,
            } => self.answer_knock(name, *let_in, *control, *try_early),
            Step::GiveControl { name } => {
                (self.act)(Act::GiveControl { name: name.clone() }).map(drop)
            }
            Step::Ring { name } => {
                let user_id = self.in_home(&format!("contact {name}"), &|home| {
                    home["contacts"]
                        .as_array()?
                        .iter()
                        .find(|c| c["name"] == name.as_str())
                        .and_then(|c| c["id"].as_str().map(str::to_owned))
                })?;
                (self.act)(Act::Ring { user_id }).map(drop)
            }
            Step::WaitRing { name } => self.in_home(&format!("ring from {name}"), &|home| {
                home["rings"]
                    .as_array()?
                    .iter()
                    .find(|r| r["host"]["name"] == name.as_str())
                    .map(|_| ())
            }),
            Step::AnswerRing { accept } => {
                let call_id = self.in_home("a ring", &|home| {
                    home["rings"].as_array()?.first()?["call_id"]
                        .as_str()
                        .map(str::to_owned)
                })?;
                (self.act)(Act::AnswerRing {
                    call_id,
                    accept: *accept,
                })
                .map(drop)
            }
            Step::WaitHome { home } => {
                let want = Value::Object(home.clone());
                self.in_home("that home view", &|have| {
                    contains(have, &want).then_some(())
                })
            }
            Step::End => (self.act)(Act::End).map(drop),
        }
    }
}

/// Plays `steps` against `host`. Stops at the first step that fails.
pub fn play(steps: &[Step], host: &Host<'_>) -> Result<(), String> {
    for (i, step) in steps.iter().enumerate() {
        let result = host.step(step);
        (host.report)(serde_json::json!({
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

    fn no_home() -> Value {
        Value::Null
    }

    fn no_act(_: Act) -> Result<Value, String> {
        Err("no flows here".into())
    }

    #[test]
    fn answers_name_the_request_they_answer_and_wait_to_allow() {
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
        let start = Instant::now();
        let send = |c: &ipc::Command| {
            sent.borrow_mut().push((c.clone(), start.elapsed()));
            Ok(())
        };
        let report = |v| lines.borrow_mut().push(v);
        let host = Host {
            outputs: &rx,
            send: &send,
            report: &report,
            home: &no_home,
            act: &no_act,
            timeout: Duration::from_secs(1),
        };
        play(&steps, &host).unwrap();
        let sent = sent.into_inner();
        assert_eq!(
            sent.iter().map(|(c, _)| c.clone()).collect::<Vec<_>>(),
            [
                ipc::Command::AllowControl { request: 3 },
                ipc::Command::RevokeControl,
                ipc::Command::DenyControl { request: 4 },
            ]
        );
        assert!(sent[0].1 >= ARMING, "the allow waited: {:?}", sent[0].1);
        assert!(sent[2].1 - sent[1].1 < ARMING, "a deny doesn't wait");
        assert_eq!(lines.borrow().len(), 5);
    }

    #[test]
    fn an_early_allow_must_be_refused() {
        let (tx, rx) = mpsc::channel();
        tx.send(event(r#"{"event":"control_asked","request":1,"identity":"guest-1","name":"Sam","platform":"macos"}"#)).unwrap();
        let heard = Instant::now();
        let sent = RefCell::new(Vec::new());
        // Refuses as the shell does until a second has passed.
        let send = |c: &ipc::Command| {
            sent.borrow_mut().push(c.clone());
            if heard.elapsed() < ARMING {
                Err(TOO_SOON.to_owned())
            } else {
                Ok(())
            }
        };
        let lines = RefCell::new(Vec::new());
        let report = |v| lines.borrow_mut().push(v);
        let host = Host {
            outputs: &rx,
            send: &send,
            report: &report,
            home: &no_home,
            act: &no_act,
            timeout: Duration::from_secs(2),
        };
        let steps = [Step::Answer {
            with: Answer::Allow,
            early: true,
        }];
        play(&steps, &host).unwrap();
        assert!(sent.borrow().len() >= 2);
        assert!(lines
            .borrow()
            .iter()
            .any(|l| l["early_refused"] == TOO_SOON));

        // A shell that accepted at once fails the step.
        tx.send(event(r#"{"event":"control_asked","request":2,"identity":"guest-1","name":"Sam","platform":"macos"}"#)).unwrap();
        let accept = |_: &ipc::Command| Ok(());
        let host = Host {
            send: &accept,
            ..host
        };
        assert!(play(&steps, &host).unwrap_err().contains("wasn't refused"));
    }

    #[test]
    fn nothing_is_answered_without_a_request() {
        let (tx, rx) = mpsc::channel();
        tx.send(event(r#"{"event":"connection","state":"connected"}"#))
            .unwrap();
        let steps = [Step::Answer {
            with: Answer::Allow,
            early: false,
        }];
        let sent = RefCell::new(Vec::new());
        let send = |c: &ipc::Command| {
            sent.borrow_mut().push(c.clone());
            Ok(())
        };
        let host = Host {
            outputs: &rx,
            send: &send,
            report: &|_| {},
            home: &no_home,
            act: &no_act,
            timeout: Duration::from_millis(100),
        };
        let err = play(&steps, &host).unwrap_err();
        assert!(err.contains("no control request in time"), "{err}");
        assert!(sent.borrow().is_empty());
        drop(tx);
    }

    /// Letting in waits until the view shows the knock armed, and `try_early` proves an answer
    /// before that is refused.
    #[test]
    fn knocks_are_let_in_once_armed() {
        let (_tx, rx) = mpsc::channel();
        let start = Instant::now();
        let armed = move || start.elapsed() >= Duration::from_millis(300);
        let home = move || serde_json::json!({ "knocks": [{ "user_id": "u2", "name": "Sam", "armed": armed() }] });
        let acts = RefCell::new(Vec::new());
        let act = |a: Act| {
            acts.borrow_mut().push((a.clone(), armed()));
            match a {
                Act::AnswerKnock { let_in: true, .. } if !armed() => Err(TOO_SOON.to_owned()),
                _ => Ok(Value::Null),
            }
        };
        let host = Host {
            outputs: &rx,
            send: &|_| Ok(()),
            report: &|_| {},
            home: &home,
            act: &act,
            timeout: Duration::from_secs(2),
        };
        let steps: Vec<Step> = serde_json::from_str(
            r#"[{"do":"wait_knock","name":"Sam"},
                {"do":"answer_knock","name":"Sam","let_in":true,"control":true,"try_early":true}]"#,
        )
        .unwrap();
        play(&steps, &host).unwrap();
        let acts = acts.into_inner();
        assert_eq!(acts.len(), 2);
        assert!(!acts[0].1 && acts[1].1, "{acts:?}");
        assert_eq!(
            acts[1].0,
            Act::AnswerKnock {
                user_id: "u2".into(),
                let_in: true,
                control: true
            }
        );
    }

    #[test]
    fn the_home_view_is_matched_in_part() {
        let have = serde_json::json!({
            "stage": { "kind": "hosting", "call_id": "c", "link": "l" },
            "members": [{ "name": "Sam", "state": "admitted" }, { "name": "Kim", "state": "ringing" }],
            "notice": null
        });
        assert!(contains(
            &have,
            &serde_json::json!({ "stage": { "kind": "hosting" } })
        ));
        assert!(contains(
            &have,
            &serde_json::json!({ "members": [{ "name": "Kim" }], "notice": null })
        ));
        assert!(!contains(
            &have,
            &serde_json::json!({ "members": [{ "name": "Lee" }] })
        ));
        assert!(!contains(
            &have,
            &serde_json::json!({ "stage": { "kind": "idle" } })
        ));
    }

    #[test]
    fn scripts_cannot_grant_by_sending() {
        // `send` only takes commands that grant nothing.
        let bad: Result<Step, _> = serde_json::from_str(r#"{"do":"send","cmd":"allow_control"}"#);
        assert!(bad.is_err());
        let bad: Result<Step, _> = serde_json::from_str(r#"{"do":"send","cmd":"trust_control"}"#);
        assert!(bad.is_err());
        // And a script must say where recorded input goes.
        let no_record: Result<Script, _> = serde_json::from_str(
            r#"{"call":{"room":"r","name":"Alex","quality":"1080p"},"steps":[]}"#,
        );
        assert!(no_record.is_err());
        let no_record: Result<Script, _> =
            serde_json::from_str(r#"{"accounts":{"quality":"4k"},"steps":[]}"#);
        assert!(no_record.is_err());
        let script: Script = serde_json::from_str(
            r#"{"call":{"room":"r","name":"Alex","quality":"4k","inject_record":"/tmp/a.jsonl"},"steps":[],"quit":true}"#,
        )
        .unwrap();
        script.check().unwrap();
        assert!(!script.plan().unwrap().voice);
        assert!(script.hooks().pattern);
        assert_eq!(script.hooks().inject_record, Some("/tmp/a.jsonl".into()));
        let script: Script = serde_json::from_str(
            r#"{"accounts":{"inject_record":"/tmp/b.jsonl"},"steps":[{"do":"share"},{"do":"end"}]}"#,
        )
        .unwrap();
        script.check().unwrap();
        assert!(script.plan().is_none());
        assert_eq!(script.quality(), Quality::K4);
        assert_eq!(script.hooks().inject_record, Some("/tmp/b.jsonl".into()));
        let both: Script = serde_json::from_str(
            r#"{"call":{"room":"r","name":"A","quality":"4k","inject_record":"/a"},"accounts":{"inject_record":"/b"},"steps":[]}"#,
        )
        .unwrap();
        assert!(both.check().is_err());
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
