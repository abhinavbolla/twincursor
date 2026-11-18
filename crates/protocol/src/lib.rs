//! Remote-control messages between a guest's viewer and the sharer.
//!
//! Every message travels as a frame: JSON with the protocol version beside the message, so the
//! sharer can turn away a viewer it doesn't understand before reading anything else. [`decode`]
//! also validates, so nothing out of range reaches the control session: the viewer isn't trusted.

use serde::{Deserialize, Serialize};

/// Bumped on any change an older peer would misread.
pub const VERSION: u16 = 1;

/// Data-packet topic for every control message, both ways.
pub const TOPIC: &str = "tc.control";

/// Payloads longer than this are rejected before parsing.
pub const MAX_LEN: usize = 1024;
pub const MAX_NAME_CHARS: usize = 64;
pub const MAX_TEXT_CHARS: usize = 32;
pub const MAX_KEY_CODE_LEN: usize = 32;
/// Largest scroll step accepted, in either unit.
pub const MAX_SCROLL: f64 = 10_000.0;
/// Smallest and largest viewport side accepted, in pixels. 16384 is past any display made,
/// and the sharer never sends more than its own setting anyway.
pub const MIN_VIEWPORT: u32 = 64;
pub const MAX_VIEWPORT: u32 = 16_384;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Message {
    // Guest to sharer.
    /// Sent on joining, so the sharer can see who is asking.
    Hello {
        name: String,
        platform: Platform,
        /// The video codecs this viewer can decode, so the sharer can pick one it shows.
        /// Viewers before this field send none, which means "not said".
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        decoders: Vec<Codec>,
    },
    RequestControl,
    /// The guest gives control back, or withdraws a request.
    ReleaseControl,
    /// Keeps held keys and buttons alive during control. Viewers send one every 250 ms.
    Heartbeat {
        sent_at_us: u64,
    },
    /// `seq` counts up across all input from one viewer. `sent_at_us` is the viewer's monotonic
    /// clock, from any fixed start: the sharer only compares it with itself. `after` is how many
    /// reliable inputs (everything but moves) the viewer had sent in this grant before this one:
    /// a move waits until the sharer has applied that many, so it can't overtake a click or key
    /// sent before it.
    Input {
        seq: u64,
        sent_at_us: u64,
        #[serde(default)]
        after: u64,
        event: InputEvent,
    },
    /// The largest area, in physical pixels, the picture can occupy on this viewer, so the
    /// sharer sends no more than that. Sent on joining and when it changes. A sharer that
    /// predates it turns it away as a message it doesn't know, and a viewer that never sends it
    /// gets the sharer's own size.
    Viewport {
        width: u32,
        height: u32,
    },

    // Sharer to guest.
    /// Answers every `Hello`: the sharer has it. Information only: it grants nothing and says
    /// nothing about control. A viewer offers its hello until this arrives, because a sharer
    /// drops what a viewer sends before the sharer's room has heard that viewer joined. Viewers
    /// that predate it turn it away as a message they don't know, and a sharer that never sends
    /// it gets the hello offered again every few seconds.
    HelloAck,
    Grant,
    Deny,
    Revoke,
    /// Answers the guest's `ReleaseControl`. Everything the guest sent before it has been
    /// applied, so the guest can leave without cutting off its last input.
    Released,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InputEvent {
    /// Pointer position, 0..1 over the shared surface from its top left.
    Move { x: f64, y: f64 },
    /// Carries its own position, so it can't land in the wrong place by overtaking a move.
    Button {
        button: Button,
        pressed: bool,
        x: f64,
        y: f64,
    },
    /// Positive values scroll right and down, as on the web: the content moves left and up.
    Scroll { dx: f64, dy: f64, unit: ScrollUnit },
    /// `code` names the physical key as `KeyboardEvent.code` does (`KeyA`, `ShiftLeft`), whatever
    /// the guest's layout. `text` is what the key produced in that layout, if anything.
    Key {
        code: String,
        pressed: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
    },
    /// Release everything held, for example when the viewer window loses focus.
    ReleaseAll,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Button {
    Left,
    Right,
    Middle,
    Back,
    Forward,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScrollUnit {
    /// Trackpads and smooth scrolling.
    Pixel,
    /// Mouse wheels with notches.
    Line,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Platform {
    Macos,
    Windows,
    Linux,
    #[default]
    #[serde(other)]
    Unknown,
}

/// A video codec, as viewers announce what they decode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Codec {
    H264,
    H265,
    Vp8,
    Vp9,
    Av1,
    /// A codec this version doesn't know.
    #[serde(other)]
    Unknown,
}

impl Codec {
    /// The name used in logs and in the app's events.
    pub fn name(self) -> &'static str {
        match self {
            Codec::H264 => "h264",
            Codec::H265 => "h265",
            Codec::Vp8 => "vp8",
            Codec::Vp9 => "vp9",
            Codec::Av1 => "av1",
            Codec::Unknown => "unknown",
        }
    }
}

/// Most codecs a hello may list.
pub const MAX_DECODERS: usize = 16;

#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    #[error("message is {0} bytes, more than {MAX_LEN}")]
    TooLong(usize),
    #[error("malformed message: {0}")]
    Malformed(#[from] serde_json::Error),
    #[error("protocol version {0}, expected {VERSION}")]
    UnsupportedVersion(u16),
    #[error("invalid message: {0}")]
    Invalid(&'static str),
}

impl Message {
    /// Whether the message must arrive, in order. Only pointer moves may be dropped: a newer one
    /// replaces them (ADR 0003).
    pub fn reliable(&self) -> bool {
        !matches!(
            self,
            Message::Input {
                event: InputEvent::Move { .. },
                ..
            }
        )
    }

    /// Checks every value against what a well-behaved viewer could send.
    pub fn validate(&self) -> Result<(), &'static str> {
        match self {
            Message::Hello { name, .. }
                if name.chars().count() > MAX_NAME_CHARS || name.chars().any(char::is_control) =>
            {
                Err("bad name")
            }
            Message::Hello { decoders, .. } if decoders.len() > MAX_DECODERS => {
                Err("too many codecs")
            }
            Message::Input { event, .. } => event.validate(),
            Message::Viewport { width, height }
                if ![width, height]
                    .iter()
                    .all(|v| (MIN_VIEWPORT..=MAX_VIEWPORT).contains(*v)) =>
            {
                Err("viewport out of range")
            }
            _ => Ok(()),
        }
    }
}

impl InputEvent {
    fn validate(&self) -> Result<(), &'static str> {
        match self {
            InputEvent::Move { x, y } | InputEvent::Button { x, y, .. } => {
                if !unit(*x) || !unit(*y) {
                    return Err("position outside the shared surface");
                }
            }
            InputEvent::Scroll { dx, dy, .. } => {
                if !(dx.abs() <= MAX_SCROLL && dy.abs() <= MAX_SCROLL) {
                    return Err("scroll out of range");
                }
            }
            InputEvent::Key { code, text, .. } => {
                if code.is_empty()
                    || code.len() > MAX_KEY_CODE_LEN
                    || !code.bytes().all(|b| b.is_ascii_alphanumeric())
                {
                    return Err("bad key code");
                }
                if text
                    .as_ref()
                    .is_some_and(|t| t.is_empty() || t.chars().count() > MAX_TEXT_CHARS)
                {
                    return Err("bad key text");
                }
            }
            InputEvent::ReleaseAll => {}
        }
        Ok(())
    }
}

fn unit(v: f64) -> bool {
    (0.0..=1.0).contains(&v)
}

#[derive(Serialize)]
struct FrameOut<'a> {
    v: u16,
    msg: &'a Message,
}

#[derive(Deserialize)]
struct Head {
    v: u16,
}

#[derive(Deserialize)]
struct FrameIn {
    msg: Message,
}

pub fn encode(message: &Message) -> Vec<u8> {
    serde_json::to_vec(&FrameOut {
        v: VERSION,
        msg: message,
    })
    .expect("messages serialize")
}

/// Parses and validates a frame. The version is checked before the message is read.
pub fn decode(payload: &[u8]) -> Result<Message, DecodeError> {
    if payload.len() > MAX_LEN {
        return Err(DecodeError::TooLong(payload.len()));
    }
    let Head { v } = serde_json::from_slice(payload)?;
    if v != VERSION {
        return Err(DecodeError::UnsupportedVersion(v));
    }
    let FrameIn { msg } = serde_json::from_slice(payload)?;
    msg.validate().map_err(DecodeError::Invalid)?;
    Ok(msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(event: InputEvent) -> Message {
        Message::Input {
            seq: 7,
            sent_at_us: 1_234_567,
            after: 3,
            event,
        }
    }

    #[test]
    fn round_trips_every_message() {
        let messages = [
            Message::Hello {
                name: "André".into(),
                platform: Platform::Windows,
                decoders: vec![],
            },
            Message::Hello {
                name: "Sam".into(),
                platform: Platform::Macos,
                decoders: vec![Codec::H264, Codec::H265],
            },
            Message::RequestControl,
            Message::ReleaseControl,
            Message::Heartbeat { sent_at_us: 42 },
            input(InputEvent::Move { x: 0.0, y: 1.0 }),
            input(InputEvent::Button {
                button: Button::Right,
                pressed: true,
                x: 0.25,
                y: 0.75,
            }),
            input(InputEvent::Scroll {
                dx: -3.5,
                dy: 120.0,
                unit: ScrollUnit::Pixel,
            }),
            input(InputEvent::Key {
                code: "KeyA".into(),
                pressed: true,
                text: Some("å".into()),
            }),
            input(InputEvent::Key {
                code: "ShiftLeft".into(),
                pressed: false,
                text: None,
            }),
            input(InputEvent::ReleaseAll),
            Message::Viewport {
                width: 2872,
                height: 1728,
            },
            Message::HelloAck,
            Message::Grant,
            Message::Deny,
            Message::Revoke,
            Message::Released,
        ];
        for message in messages {
            assert_eq!(decode(&encode(&message)).unwrap(), message);
        }
    }

    #[test]
    fn wire_format_is_readable() {
        let bytes = encode(&input(InputEvent::Move { x: 0.5, y: 0.25 }));
        assert_eq!(
            std::str::from_utf8(&bytes).unwrap(),
            r#"{"v":1,"msg":{"type":"input","seq":7,"sent_at_us":1234567,"after":3,"event":{"kind":"move","x":0.5,"y":0.25}}}"#
        );
    }

    #[test]
    fn rejects_other_versions_before_reading_the_message() {
        let err = decode(br#"{"v":2,"msg":{"type":"from_the_future"}}"#).unwrap_err();
        assert!(matches!(err, DecodeError::UnsupportedVersion(2)), "{err}");
    }

    #[test]
    fn rejects_malformed_and_oversized_payloads() {
        for bad in [
            &b"not json"[..],
            br#"{"msg":{"type":"grant"}}"#,
            br#"{"v":1}"#,
        ] {
            assert!(matches!(decode(bad), Err(DecodeError::Malformed(_))));
        }
        let long = vec![b' '; MAX_LEN + 1];
        assert!(matches!(decode(&long), Err(DecodeError::TooLong(_))));
    }

    #[test]
    fn rejects_values_a_viewer_could_not_send() {
        let bad = [
            input(InputEvent::Move { x: 1.5, y: 0.5 }),
            input(InputEvent::Move { x: 0.5, y: -0.1 }),
            input(InputEvent::Button {
                button: Button::Left,
                pressed: true,
                x: 2.0,
                y: 0.0,
            }),
            input(InputEvent::Scroll {
                dx: 0.0,
                dy: 1e9,
                unit: ScrollUnit::Line,
            }),
            input(InputEvent::Key {
                code: String::new(),
                pressed: true,
                text: None,
            }),
            input(InputEvent::Key {
                code: "Key A".into(),
                pressed: true,
                text: None,
            }),
            input(InputEvent::Key {
                code: "KeyA".into(),
                pressed: true,
                text: Some("x".repeat(MAX_TEXT_CHARS + 1)),
            }),
            Message::Hello {
                name: "a\u{7}b".into(),
                platform: Platform::Macos,
                decoders: vec![],
            },
            Message::Hello {
                name: "x".into(),
                platform: Platform::Macos,
                decoders: vec![Codec::H264; MAX_DECODERS + 1],
            },
            Message::Viewport {
                width: 0,
                height: 1080,
            },
            Message::Viewport {
                width: 1920,
                height: MIN_VIEWPORT - 1,
            },
            Message::Viewport {
                width: MAX_VIEWPORT + 1,
                height: 1080,
            },
            Message::Viewport {
                width: u32::MAX,
                height: u32::MAX,
            },
        ];
        for message in bad {
            let err = decode(&encode(&message)).unwrap_err();
            assert!(matches!(err, DecodeError::Invalid(_)), "{message:?}: {err}");
        }
    }

    #[test]
    fn unknown_platform_still_decodes() {
        let msg = decode(br#"{"v":1,"msg":{"type":"hello","name":"x","platform":"haiku"}}"#);
        assert_eq!(
            msg.unwrap(),
            Message::Hello {
                name: "x".into(),
                platform: Platform::Unknown,
                decoders: vec![],
            }
        );
    }

    #[test]
    fn hellos_with_and_without_codecs_read_both_ways() {
        // A hello from a viewer before codecs were announced reads as "not said".
        let old = decode(br#"{"v":1,"msg":{"type":"hello","name":"x","platform":"macos"}}"#);
        assert!(matches!(old.unwrap(), Message::Hello { decoders, .. } if decoders.is_empty()));
        // A hello without codecs is written exactly as before, so older sharers read it.
        let bytes = encode(&Message::Hello {
            name: "x".into(),
            platform: Platform::Macos,
            decoders: vec![],
        });
        assert_eq!(
            std::str::from_utf8(&bytes).unwrap(),
            r#"{"v":1,"msg":{"type":"hello","name":"x","platform":"macos"}}"#
        );
        // Codecs from a newer viewer that this version doesn't know still read.
        let new = decode(
            br#"{"v":1,"msg":{"type":"hello","name":"x","platform":"macos","decoders":["h265","h266"]}}"#,
        );
        assert!(matches!(
            new.unwrap(),
            Message::Hello { decoders, .. } if decoders == [Codec::H265, Codec::Unknown]
        ));
    }

    #[test]
    fn viewports_read_both_ways() {
        // The wire format, which a sharer of this version reads.
        let bytes = encode(&Message::Viewport {
            width: 1920,
            height: 1080,
        });
        assert_eq!(
            std::str::from_utf8(&bytes).unwrap(),
            r#"{"v":1,"msg":{"type":"viewport","width":1920,"height":1080}}"#
        );
        // Fields a newer viewer might add are ignored.
        let newer =
            decode(br#"{"v":1,"msg":{"type":"viewport","width":1920,"height":1080,"scale":2.0}}"#);
        assert_eq!(
            newer.unwrap(),
            Message::Viewport {
                width: 1920,
                height: 1080
            }
        );
        // Negative or missing sizes don't parse at all.
        for bad in [
            &br#"{"v":1,"msg":{"type":"viewport","width":-1,"height":1080}}"#[..],
            br#"{"v":1,"msg":{"type":"viewport","width":1920}}"#,
        ] {
            assert!(matches!(decode(bad), Err(DecodeError::Malformed(_))));
        }
        // The edges of the range are accepted.
        for (width, height) in [(MIN_VIEWPORT, MIN_VIEWPORT), (MAX_VIEWPORT, MAX_VIEWPORT)] {
            let m = Message::Viewport { width, height };
            assert_eq!(decode(&encode(&m)).unwrap(), m);
        }
    }

    #[test]
    fn hello_acks_read_both_ways() {
        // The wire format, which a viewer of this version reads.
        assert_eq!(
            std::str::from_utf8(&encode(&Message::HelloAck)).unwrap(),
            r#"{"v":1,"msg":{"type":"hello_ack"}}"#
        );
        // Fields a newer sharer might add are ignored.
        let newer = decode(br#"{"v":1,"msg":{"type":"hello_ack","codec":"h265"}}"#);
        assert_eq!(newer.unwrap(), Message::HelloAck);
        // A viewer that predates it reads it as a message it doesn't know, as it would any
        // other, and goes on.
        let older = decode(br#"{"v":1,"msg":{"type":"from_a_newer_sharer"}}"#);
        assert!(matches!(older, Err(DecodeError::Malformed(_))));
        assert!(Message::HelloAck.reliable());
    }

    #[test]
    fn only_moves_are_unreliable() {
        assert!(!input(InputEvent::Move { x: 0.0, y: 0.0 }).reliable());
        assert!(input(InputEvent::ReleaseAll).reliable());
        assert!(Message::Heartbeat { sent_at_us: 0 }.reliable());
    }
}
