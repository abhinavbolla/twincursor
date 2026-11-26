//! A call's engine output, cut down to what a window needs to catch up: for a window opened in
//! the middle of a call (`call_snapshot`). Replayed in order through the window's reducer
//! (`apps/desktop/src/call.ts`), it ends in the same state as a window that heard every output
//! from the start (`call.test.ts` checks that for every point of a recorded call).
//!
//! Kept: the call's first `started` and the latest one, the latest `hello`, `connection`,
//! `participants`, `control`, `voice`, `stats`, `error`, `ended` and `restarting`, a
//! `control_asked` while that request is still open, the last [`MAX_LOGS`] log lines (and events
//! the window can't read, which it shows as log lines), and `exited`. A restart drops what the
//! old engine said about the call, as the window does.

use serde_json::Value;

use crate::engine::Output;

/// Log lines kept, as the window keeps (`MAX_LOGS` in `call.ts`).
pub const MAX_LOGS: usize = 300;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    FirstStarted,
    Started,
    Hello,
    Connection,
    Participants,
    Control,
    ControlAsked,
    Voice,
    Stats,
    Error,
    Ended,
    Restarting,
    Exited,
    /// A log line, or an event the window can't read: both become log lines there.
    Log,
}

/// What one engine knew about the call, dropped when another replaces it.
const PER_ENGINE: [Kind; 8] = [
    Kind::Started,
    Kind::Connection,
    Kind::Participants,
    Kind::Control,
    Kind::ControlAsked,
    Kind::Voice,
    Kind::Stats,
    Kind::Ended,
];

fn event_kind(event: &Value) -> Kind {
    match event.get("event").and_then(Value::as_str).unwrap_or("") {
        "hello" => Kind::Hello,
        "connection" => Kind::Connection,
        "participants" => Kind::Participants,
        "control" => Kind::Control,
        "control_asked" => Kind::ControlAsked,
        "voice" => Kind::Voice,
        "stats" => Kind::Stats,
        "error" => Kind::Error,
        "ended" => Kind::Ended,
        _ => Kind::Log,
    }
}

/// One call's snapshot.
#[derive(Debug, Default, Clone)]
pub struct Snapshot {
    entries: Vec<(Kind, Output)>,
}

impl Snapshot {
    /// Forgets everything: a new call.
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    fn drop_kinds(&mut self, kinds: &[Kind]) {
        self.entries.retain(|(k, _)| !kinds.contains(k));
    }

    fn has(&self, kind: Kind) -> bool {
        self.entries.iter().any(|(k, _)| *k == kind)
    }

    /// Takes in one output, in the order the window hears them.
    pub fn push(&mut self, output: &Output) {
        let kind = match output {
            Output::Started { .. } => {
                if !self.has(Kind::FirstStarted) || self.has(Kind::Exited) {
                    self.clear();
                    Kind::FirstStarted
                } else {
                    self.drop_kinds(&PER_ENGINE);
                    Kind::Started
                }
            }
            Output::Restarting { .. } => {
                self.drop_kinds(&PER_ENGINE[1..]);
                self.drop_kinds(&[Kind::Restarting]);
                Kind::Restarting
            }
            Output::Exited { .. } => Kind::Exited,
            Output::Log { .. } => Kind::Log,
            Output::Event { event } => {
                let kind = event_kind(event);
                if kind == Kind::Control
                    && event.get("state").and_then(Value::as_str) != Some("asked")
                {
                    // Granted, refused, withdrawn or over: the request isn't open any more.
                    self.drop_kinds(&[Kind::ControlAsked]);
                }
                if kind != Kind::Log {
                    self.drop_kinds(&[kind]);
                }
                kind
            }
        };
        self.entries.push((kind, output.clone()));
        if kind == Kind::Log {
            let logs = self.entries.iter().filter(|(k, _)| *k == Kind::Log).count();
            if logs > MAX_LOGS {
                if let Some(oldest) = self.entries.iter().position(|(k, _)| *k == Kind::Log) {
                    self.entries.remove(oldest);
                }
            }
        }
    }

    /// The outputs to replay, in order.
    pub fn outputs(&self) -> Vec<Output> {
        self.entries.iter().map(|(_, o)| o.clone()).collect()
    }

    /// The open control request, if any: its number.
    #[cfg(test)]
    pub fn open_request(&self) -> Option<u64> {
        self.entries.iter().find_map(|(k, o)| match (k, o) {
            (Kind::ControlAsked, Output::Event { event }) => event["request"].as_u64(),
            _ => None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixtures() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures")
    }

    fn stream() -> Vec<Output> {
        std::fs::read_to_string(fixtures().join("call-stream.jsonl"))
            .unwrap()
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    /// The snapshot after each output of the recorded call, one per line. `call.test.ts` replays
    /// each one and compares it with the window that heard the whole stream up to there.
    /// Rewrite the file with `UPDATE_FIXTURES=1 cargo test`.
    #[test]
    fn snapshots_of_the_recorded_call_match_the_fixture() {
        let mut snapshot = Snapshot::default();
        let mut all = Vec::new();
        for output in stream() {
            snapshot.push(&output);
            all.push(snapshot.outputs());
        }
        let text: String = all
            .iter()
            .map(|outputs| serde_json::to_string(outputs).unwrap() + "\n")
            .collect();
        let path = fixtures().join("call-snapshots.jsonl");
        if std::env::var_os("UPDATE_FIXTURES").is_some() {
            std::fs::write(&path, &text).unwrap();
        }
        let want = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(
            want == text,
            "{} is out of date: run UPDATE_FIXTURES=1 cargo test",
            path.display()
        );
    }

    #[test]
    fn ordering_and_restarts() {
        let mut s = Snapshot::default();
        let started = |codec: &str| -> Output {
            serde_json::from_value(serde_json::json!({
                "kind": "started", "role": "share", "codec": codec, "pid": 1, "args": []
            }))
            .unwrap()
        };
        let ev = |v: Value| Output::Event { event: v };
        s.push(&started("h265"));
        s.push(&ev(
            serde_json::json!({"event":"connection","state":"connecting"}),
        ));
        s.push(&ev(
            serde_json::json!({"event":"control_asked","request":1}),
        ));
        assert_eq!(s.open_request(), Some(1));
        s.push(&ev(
            serde_json::json!({"event":"connection","state":"connected"}),
        ));
        let kinds: Vec<Kind> = s.entries.iter().map(|(k, _)| *k).collect();
        assert_eq!(
            kinds,
            [Kind::FirstStarted, Kind::ControlAsked, Kind::Connection],
            "the latest of each kind, in the order they came"
        );
        s.push(&ev(serde_json::json!({"event":"control","state":"none"})));
        assert_eq!(s.open_request(), None);
        s.push(&ev(
            serde_json::json!({"event":"control_asked","request":2}),
        ));
        s.push(&Output::Restarting {
            codec: crate::engine::Codec::H264,
            message: "m".into(),
        });
        assert_eq!(s.open_request(), None, "a restart closes every request");
        s.push(&started("h264"));
        let kinds: Vec<Kind> = s.entries.iter().map(|(k, _)| *k).collect();
        assert_eq!(kinds, [Kind::FirstStarted, Kind::Restarting, Kind::Started]);
        s.push(&Output::Exited { code: Some(0) });
        s.push(&started("h264"));
        assert_eq!(s.entries.len(), 1, "a new call starts a new snapshot");
    }

    #[test]
    fn keeps_the_last_log_lines() {
        let mut s = Snapshot::default();
        for i in 0..(MAX_LOGS + 20) {
            s.push(&Output::Log {
                line: format!("{i}"),
            });
        }
        let outputs = s.outputs();
        assert_eq!(outputs.len(), MAX_LOGS);
        assert_eq!(outputs[0], Output::Log { line: "20".into() });
    }
}
