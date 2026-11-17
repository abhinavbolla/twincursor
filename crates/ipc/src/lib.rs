//! What the desktop app and the engine say to each other.
//!
//! The app starts the engine (`tc-engine share --ipc` or `tc-engine view --ipc`) and talks to it
//! over the engine's stdin and stdout, one JSON object per line: [`Command`]s in, [`Event`]s out.
//! The first event is always [`Event::Hello`], which carries [`VERSION`]. Logs go to stderr, so
//! stdout carries nothing but events.
//!
//! Every field is always present (`null` when there is nothing to say), so the TypeScript side
//! can describe each message as one fixed shape. `fixtures/*.jsonl` holds one example of every
//! command and event; the tests here read them back, and the app's tests can read the same files.

use serde::{Deserialize, Serialize};

/// Bumped on any change the other side would misread. The app checks it in [`Event::Hello`].
pub const VERSION: u16 = 1;

/// Which side of the call this engine is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Shares their screen, and may let a guest control it.
    Sharer,
    /// Watches the sharer's screen, and may ask for control.
    Guest,
    /// Someone the engine can't place, such as a participant from a newer version.
    #[serde(other)]
    Unknown,
}

/// What the app tells the engine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    /// Mute this side's microphone.
    Mute,
    /// Unmute it.
    Unmute,
    /// Leave the call. The engine sends [`Event::Ended`] and exits.
    Leave,
    /// Sharer: let the guest who asked take control. `request` is the number from
    /// [`Event::ControlAsked`]; an allow for any other request does nothing, so a click meant
    /// for one request can never grant another.
    AllowControl { request: u64 },
    /// Sharer: turn the request down.
    DenyControl { request: u64 },
    /// Sharer: take control back at once. The same as the revoke hotkey.
    RevokeControl,
    /// Guest: ask the sharer for control.
    RequestControl,
    /// Guest: give control back, or withdraw a request.
    ReleaseControl,
    /// Use this microphone: its name as `tc-engine devices` lists it, or `null` for the system
    /// default.
    SetMic { device: Option<String> },
    /// Use these speakers or headphones, as for [`Command::SetMic`].
    SetSpeaker { device: Option<String> },
}

/// What the engine tells the app.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    /// Always the first line.
    Hello {
        /// [`VERSION`] of the engine that wrote it.
        version: u16,
        role: Role,
        /// The engine's own version, for logs and bug reports.
        engine: String,
    },
    /// Where the connection to the server is.
    Connection { state: ConnectionState },
    /// Everyone else in the call, after anyone joins or leaves, or says more about themself.
    Participants { participants: Vec<Participant> },
    /// Sharer: a guest asks for control. Nothing is granted until the app answers with
    /// [`Command::AllowControl`] for this `request`.
    ControlAsked {
        /// Counts up from 1 for every request this engine sees.
        request: u64,
        identity: String,
        /// The name the guest typed. Not proof of who they are.
        name: String,
        platform: String,
    },
    /// Who is in control, or asking, or nobody.
    Control {
        state: ControlState,
        /// Sharer: the guest asking or in control. Guest: this guest. `null` when the state is
        /// `none`.
        by: Option<Peer>,
    },
    /// This side's voice.
    Voice {
        /// A voice track is published (the call was started with a microphone).
        sending: bool,
        muted: bool,
        /// The devices in use, when the voice is on.
        mic: Option<String>,
        speaker: Option<String>,
    },
    /// About once a second: the video this side sends (sharer) or receives (guest).
    Stats {
        width: u32,
        height: u32,
        fps: f64,
        kbps: f64,
        /// Network round trip to the server and back, in milliseconds.
        rtt_ms: Option<f64>,
        /// The codec negotiated, such as `h264` or `h265`.
        codec: Option<String>,
        /// Guest with a window: how the picture is drawn, in percent of its size (100 is 1:1).
        shown_pct: Option<u32>,
    },
    /// Something went wrong, in a sentence a person can read. The call may go on.
    Error { message: String },
    /// The call is over for this engine; it exits next.
    Ended { reason: EndReason },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionState {
    Connecting,
    Connected,
    /// The connection dropped and the engine is getting it back.
    Reconnecting,
    Disconnected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlState {
    None,
    /// A guest asked and the sharer hasn't answered yet.
    Asked,
    Controlling,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndReason {
    /// The app sent [`Command::Leave`].
    Left,
    /// The server ended the call or the connection was lost for good.
    Disconnected,
    /// The engine couldn't go on; an [`Event::Error`] before this says why.
    Failed,
}

/// Someone in the call, as shown to the person.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Participant {
    /// Unique in the call; the server vouches for it.
    pub identity: String,
    /// What they typed as their name. Not proof of who they are.
    pub name: String,
    pub role: Role,
    /// Guests: the video codecs they can decode, as they announced them (`h264`, `h265`, ...).
    /// Empty until they say.
    pub decoders: Vec<String>,
}

/// One participant, in a control event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Peer {
    pub identity: String,
    /// Not proof of who they are.
    pub name: String,
}

/// The line an event is written as, without the newline.
pub fn encode(event: &Event) -> String {
    serde_json::to_string(event).expect("events serialize")
}

/// Reads one line from the app. Blank lines are `None`.
pub fn decode_command(line: &str) -> Option<Result<Command, String>> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    Some(serde_json::from_str(line).map_err(|e| format!("That command couldn't be read ({e}).")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    /// One of every command. The match makes adding a command without a sample a build error.
    fn every_command() -> Vec<Command> {
        let all = vec![
            Command::Mute,
            Command::Unmute,
            Command::Leave,
            Command::AllowControl { request: 1 },
            Command::DenyControl { request: 1 },
            Command::RevokeControl,
            Command::RequestControl,
            Command::ReleaseControl,
            Command::SetMic {
                device: Some("MacBook Pro Microphone".into()),
            },
            Command::SetSpeaker { device: None },
        ];
        for c in &all {
            match c {
                Command::Mute
                | Command::Unmute
                | Command::Leave
                | Command::AllowControl { .. }
                | Command::DenyControl { .. }
                | Command::RevokeControl
                | Command::RequestControl
                | Command::ReleaseControl
                | Command::SetMic { .. }
                | Command::SetSpeaker { .. } => {}
            }
        }
        all
    }

    fn tag(value: &Value, key: &str) -> String {
        value[key].as_str().expect("tagged").to_owned()
    }

    fn lines(name: &str) -> Vec<String> {
        let path = format!("{}/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
        std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("{path}: {e}"))
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(str::to_owned)
            .collect()
    }

    #[test]
    fn command_fixtures_round_trip() {
        let mut seen = Vec::new();
        for line in lines("commands.jsonl") {
            let command = decode_command(&line).unwrap().unwrap();
            let again: Value = serde_json::to_value(&command).unwrap();
            assert_eq!(
                again,
                serde_json::from_str::<Value>(&line).unwrap(),
                "{line}"
            );
            seen.push(tag(&again, "cmd"));
        }
        for command in every_command() {
            let name = tag(&serde_json::to_value(&command).unwrap(), "cmd");
            assert!(seen.contains(&name), "commands.jsonl has no `{name}`");
        }
    }

    #[test]
    fn event_fixtures_round_trip() {
        let mut seen = Vec::new();
        for line in lines("events.jsonl") {
            let event: Event = serde_json::from_str(&line).unwrap();
            assert_eq!(
                encode(&event),
                line,
                "fixtures are written as the engine writes them"
            );
            seen.push(tag(&serde_json::to_value(&event).unwrap(), "event"));
        }
        let every = [
            "hello",
            "connection",
            "participants",
            "control_asked",
            "control",
            "voice",
            "stats",
            "error",
            "ended",
        ];
        for name in every {
            assert!(
                seen.contains(&name.to_owned()),
                "events.jsonl has no `{name}`"
            );
        }
        // Any new event must be added to the list above, and so to the fixtures.
        let sample = Event::Error {
            message: String::new(),
        };
        match sample {
            Event::Hello { .. }
            | Event::Connection { .. }
            | Event::Participants { .. }
            | Event::ControlAsked { .. }
            | Event::Control { .. }
            | Event::Voice { .. }
            | Event::Stats { .. }
            | Event::Error { .. }
            | Event::Ended { .. } => {}
        }
    }

    #[test]
    fn the_first_fixture_event_is_hello_with_this_version() {
        let first = lines("events.jsonl").remove(0);
        let event: Event = serde_json::from_str(&first).unwrap();
        assert!(
            matches!(
                event,
                Event::Hello {
                    version: VERSION,
                    ..
                }
            ),
            "{first}"
        );
    }

    #[test]
    fn bad_and_blank_command_lines() {
        assert!(decode_command("   ").is_none());
        let err = decode_command(r#"{"cmd":"format_disk"}"#)
            .unwrap()
            .unwrap_err();
        assert!(err.starts_with("That command couldn't be read"), "{err}");
        // An allow must say which request it answers.
        assert!(decode_command(r#"{"cmd":"allow_control"}"#)
            .unwrap()
            .is_err());
    }

    #[test]
    fn unknown_roles_still_read() {
        let p: Participant =
            serde_json::from_str(r#"{"identity":"x","name":"x","role":"recorder","decoders":[]}"#)
                .unwrap();
        assert_eq!(p.role, Role::Unknown);
    }
}
