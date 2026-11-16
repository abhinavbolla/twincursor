//! The guest's side of remote control in the viewer.
//!
//! One thread owns the [`GuestSession`]. The window (or a script) hands it input, the link hands
//! it the sharer's answers, and it sends to the sharer whose screen we're receiving.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use control::guest::{GuestSession, GuestState};
use media::link::{ControlLink, LinkEvent, LinkSender, Outgoing};
use protocol::{InputEvent, Message, Platform};
use serde::Deserialize;

use crate::{Error, ScriptError};

const TICK: Duration = Duration::from_millis(4);

/// What the window or a script asks of the guest session.
#[derive(Debug, Clone)]
pub enum Ui {
    Input(InputEvent),
    /// Ask for control, or give it back.
    Toggle,
    Request,
    Release,
}

enum In {
    Link(LinkEvent),
    Ui(Ui),
    /// Give control back and end, after the sharer confirms it has applied everything (or
    /// this long passes).
    Stop(Duration),
}

/// The guest session's state, readable from any thread.
#[derive(Clone, Default)]
pub struct Status(Arc<AtomicU8>);

impl Status {
    pub fn get(&self) -> GuestState {
        match self.0.load(Ordering::Acquire) {
            1 => GuestState::Requested,
            2 => GuestState::InControl,
            _ => GuestState::Idle,
        }
    }

    fn set(&self, state: GuestState) {
        let v = match state {
            GuestState::Idle => 0,
            GuestState::Requested => 1,
            GuestState::InControl => 2,
        };
        self.0.store(v, Ordering::Release);
    }
}

#[derive(Clone)]
pub struct Guest {
    tx: Sender<In>,
    pub status: Status,
    sender: LinkSender,
}

impl Guest {
    /// Starts the guest thread. Returns the link to hand to `subscribe::run`, and the thread
    /// to join after [`Guest::stop`]. With `sent_log`, every message sent is also written there
    /// as a line of JSON (see `tc-spike replay`).
    pub fn start(
        name: String,
        sent_log: Option<&Path>,
    ) -> Result<(Self, ControlLink, JoinHandle<()>), Error> {
        let (tx, rx) = mpsc::channel();
        let (link, outgoing) = {
            let tx = tx.clone();
            ControlLink::new(protocol::TOPIC, move |event| {
                let _ = tx.send(In::Link(event));
            })
        };
        let log = sent_log.map(std::fs::File::create).transpose()?;
        let status = Status::default();
        let thread = {
            let status = status.clone();
            std::thread::Builder::new().name("guest".into()).spawn({
                let outgoing = outgoing.clone();
                move || run(name, &rx, &outgoing, &status, log)
            })?
        };
        Ok((
            Self {
                tx,
                status,
                sender: outgoing,
            },
            link,
            thread,
        ))
    }

    pub fn send(&self, ui: Ui) {
        let _ = self.tx.send(In::Ui(ui));
    }

    /// Gives control back and ends the thread once the sharer confirms it has applied
    /// everything sent, so leaving the room doesn't cut off the guest's last input. Waits at most
    /// `limit` for that. Join the thread to wait for it.
    pub fn stop(&self, limit: Duration) {
        let _ = self.tx.send(In::Stop(limit));
    }

    /// Messages handed to the link but not sent yet.
    pub fn unsent(&self) -> usize {
        self.sender.pending()
    }
}

fn run(
    name: String,
    rx: &mpsc::Receiver<In>,
    outgoing: &LinkSender,
    status: &Status,
    mut log: Option<std::fs::File>,
) {
    let mut session = GuestSession::new(Instant::now());
    let mut sharer: Option<String> = None;
    // A release the sharer hasn't confirmed yet, and when we must leave anyway.
    let mut awaiting_release = false;
    let mut leave_by: Option<Instant> = None;
    let mut send = |sharer: &Option<String>, message: Message| {
        let Some(to) = sharer else {
            tracing::debug!(?message, "no sharer yet; not sent");
            return;
        };
        if let Some(log) = log.as_mut() {
            let line = serde_json::to_string(&message).expect("messages serialize");
            let _ = writeln!(log, "{line}");
        }
        outgoing.send(Outgoing {
            to: Some(to.clone()),
            payload: protocol::encode(&message),
            reliable: message.reliable(),
        });
    };
    loop {
        let now = Instant::now();
        let mut out = Vec::new();
        match rx.recv_timeout(TICK) {
            Ok(In::Link(LinkEvent::Sharing(identity))) => {
                tracing::info!(%identity, "sharer found");
                sharer = Some(identity);
                out.push(Message::Hello {
                    name: name.clone(),
                    platform: this_platform(),
                });
                // A request made before the sharer was found went nowhere: make it now.
                if session.state() == GuestState::Requested {
                    out.push(Message::RequestControl);
                }
            }
            Ok(In::Link(LinkEvent::Data { from, payload })) if Some(&from) == sharer.as_ref() => {
                match protocol::decode(&payload) {
                    Ok(message) => {
                        if matches!(message, Message::Released | Message::Revoke) {
                            awaiting_release = false;
                        }
                        if session.on_message(&message) {
                            tracing::info!(?message, state = ?session.state(), "sharer answered");
                        }
                    }
                    Err(e) => tracing::debug!("bad control message: {e}"),
                }
            }
            Ok(In::Link(LinkEvent::Left(identity))) if Some(&identity) == sharer.as_ref() => {
                session.on_message(&Message::Revoke);
                sharer = None;
                awaiting_release = false;
            }
            Ok(In::Link(_)) => {}
            Ok(In::Ui(ui)) => match ui {
                Ui::Input(event) => out.extend(session.input(event, now)),
                Ui::Request => out.extend(session.request(now)),
                Ui::Release => out.extend(session.release()),
                Ui::Toggle => out.extend(match session.state() {
                    GuestState::Idle => session.request(now),
                    _ => session.release(),
                }),
            },
            Ok(In::Stop(limit)) => {
                out.extend(session.release());
                leave_by = Some(now + limit);
            }
            Err(RecvTimeoutError::Disconnected) => {
                if let Some(m) = session.release() {
                    send(&sharer, m);
                }
                status.set(session.state());
                return;
            }
            Err(RecvTimeoutError::Timeout) => {}
        }
        for message in session.tick(now) {
            // A repeated request goes with a repeated hello, in case that was lost too.
            if message == Message::RequestControl {
                out.push(Message::Hello {
                    name: name.clone(),
                    platform: this_platform(),
                });
            }
            out.push(message);
        }
        for message in out {
            if message == Message::ReleaseControl && sharer.is_some() {
                awaiting_release = true;
            }
            send(&sharer, message);
        }
        status.set(session.state());
        if let Some(deadline) = leave_by {
            if !awaiting_release {
                return;
            }
            if Instant::now() >= deadline {
                tracing::warn!("the sharer didn't confirm the release; leaving anyway");
                return;
            }
        }
    }
}

fn this_platform() -> Platform {
    if cfg!(target_os = "macos") {
        Platform::Macos
    } else if cfg!(target_os = "windows") {
        Platform::Windows
    } else if cfg!(target_os = "linux") {
        Platform::Linux
    } else {
        Platform::Unknown
    }
}

/// One step of a control script for a headless viewer (`view --headless --control-script`).
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "do", rename_all = "snake_case")]
pub enum Step {
    /// Ask for control and wait until it's granted.
    Request,
    Release,
    Input {
        event: InputEvent,
    },
    /// Type text one key at a time, as a guest's keyboard would send it.
    Type {
        text: String,
        #[serde(default)]
        gap_ms: u64,
    },
    Wait {
        ms: u64,
    },
}

/// How long `request` waits for the sharer.
const GRANT_TIMEOUT: Duration = Duration::from_secs(60);

pub fn read_script(path: &PathBuf) -> Result<Vec<Step>, Error> {
    let text = std::fs::read_to_string(path)?;
    Ok(serde_json::from_str(&text)?)
}

/// Plays a script through the guest session. Fails if the sharer doesn't grant control.
pub fn play(guest: &Guest, steps: &[Step]) -> Result<(), ScriptError> {
    for step in steps {
        match step {
            Step::Request => {
                guest.send(Ui::Request);
                let asked = Instant::now();
                let mut pending = false;
                loop {
                    match guest.status.get() {
                        GuestState::InControl => break,
                        GuestState::Requested => pending = true,
                        // Idle before the session took the request, or after a "no".
                        GuestState::Idle if pending => return Err(ScriptError::Denied),
                        GuestState::Idle => {}
                    }
                    if asked.elapsed() >= GRANT_TIMEOUT {
                        return Err(ScriptError::NoAnswer);
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                tracing::info!("control granted");
            }
            Step::Release => guest.send(Ui::Release),
            Step::Input { event } => guest.send(Ui::Input(event.clone())),
            Step::Type { text, gap_ms } => {
                for c in text.chars() {
                    for event in key_events(c) {
                        guest.send(Ui::Input(event));
                    }
                    if *gap_ms > 0 {
                        std::thread::sleep(Duration::from_millis(*gap_ms));
                    }
                }
            }
            Step::Wait { ms } => std::thread::sleep(Duration::from_millis(*ms)),
        }
        if matches!(step, Step::Input { .. } | Step::Type { .. })
            && guest.status.get() != GuestState::InControl
        {
            return Err(ScriptError::Ended);
        }
    }
    Ok(())
}

/// The key down and up a US keyboard sends for `c`. Characters off that keyboard go as text on
/// an unidentified key, as a layout or input method would produce them.
pub fn key_events(c: char) -> [InputEvent; 2] {
    let (code, text) = match c {
        '\n' => ("Enter".to_owned(), None),
        '\t' => ("Tab".to_owned(), None),
        ' ' => ("Space".to_owned(), Some(" ".to_owned())),
        'a'..='z' | 'A'..='Z' => (
            format!("Key{}", c.to_ascii_uppercase()),
            Some(c.to_string()),
        ),
        '0'..='9' => (format!("Digit{c}"), Some(c.to_string())),
        _ => ("Unidentified".to_owned(), Some(c.to_string())),
    };
    [
        InputEvent::Key {
            code: code.clone(),
            pressed: true,
            text,
        },
        InputEvent::Key {
            code,
            pressed: false,
            text: None,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripts_parse() {
        let steps: Vec<Step> = serde_json::from_str(
            r#"[{"do":"request"},{"do":"input","event":{"kind":"move","x":0.5,"y":0.5}},
                {"do":"type","text":"hi"},{"do":"wait","ms":5},{"do":"release"}]"#,
        )
        .unwrap();
        assert_eq!(steps.len(), 5);
    }

    #[test]
    fn typed_keys_pass_validation() {
        for c in "Hello, world!\n\tÅß1 ".chars() {
            for event in key_events(c) {
                let message = Message::Input {
                    seq: 1,
                    sent_at_us: 0,
                    after: 0,
                    event,
                };
                protocol::decode(&protocol::encode(&message)).expect("valid");
            }
        }
    }
}
