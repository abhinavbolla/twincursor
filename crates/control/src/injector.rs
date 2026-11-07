//! Where the session's decisions become input on the sharer's machine.

use protocol::{Button, ScrollUnit};
use serde::{Deserialize, Serialize};

/// A position, 0..1 over the shared surface from its top left.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    /// About a pixel on a large display.
    const SAME: f64 = 1e-3;

    pub(crate) fn near(self, other: Point) -> bool {
        (self.x - other.x).abs() <= Self::SAME && (self.y - other.y).abs() <= Self::SAME
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Action {
    MoveTo(Point),
    Button {
        button: Button,
        pressed: bool,
    },
    Scroll {
        dx: f64,
        dy: f64,
        unit: ScrollUnit,
    },
    /// A physical key on the sharer's keyboard, named as `KeyboardEvent.code` does.
    Key {
        code: String,
        pressed: bool,
    },
    /// Text to type as-is, whatever the sharer's keyboard layout.
    Text {
        text: String,
    },
}

pub trait Injector {
    fn inject(&mut self, action: &Action);

    /// Where the pointer is now, so the session can tell when the sharer moves their own mouse.
    /// `None` if the platform can't say.
    fn pointer(&self) -> Option<Point>;
}

impl<I: Injector + ?Sized> Injector for Box<I> {
    fn inject(&mut self, action: &Action) {
        (**self).inject(action);
    }

    fn pointer(&self) -> Option<Point> {
        (**self).pointer()
    }
}

/// Logs what it would do, for trying the session without touching the machine.
#[derive(Debug, Default)]
pub struct LogInjector;

impl Injector for LogInjector {
    fn inject(&mut self, action: &Action) {
        tracing::info!(?action, "inject");
    }

    fn pointer(&self) -> Option<Point> {
        None
    }
}

/// Writes each action as a line of JSON, for checking what reached the sharer (`tc-spike
/// replay`). It reports the pointer where it last put it, as if nobody else moved the mouse.
pub struct RecordInjector<W> {
    out: W,
    pointer: Option<Point>,
}

impl<W: std::io::Write> RecordInjector<W> {
    pub fn new(out: W) -> Self {
        Self { out, pointer: None }
    }

    pub fn into_inner(self) -> W {
        self.out
    }
}

impl<W: std::io::Write> Injector for RecordInjector<W> {
    fn inject(&mut self, action: &Action) {
        if let Action::MoveTo(p) = action {
            self.pointer = Some(*p);
        }
        let line = serde_json::to_string(action).expect("actions serialize");
        if let Err(e) = writeln!(self.out, "{line}").and_then(|()| self.out.flush()) {
            tracing::error!("recording an action failed: {e}");
        }
    }

    fn pointer(&self) -> Option<Point> {
        self.pointer
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_actions_as_json_lines() {
        let mut rec = RecordInjector::new(Vec::new());
        rec.inject(&Action::MoveTo(Point { x: 0.5, y: 0.25 }));
        rec.inject(&Action::Text { text: "é".into() });
        rec.inject(&Action::Key {
            code: "KeyC".into(),
            pressed: true,
        });
        assert_eq!(rec.pointer(), Some(Point { x: 0.5, y: 0.25 }));
        let out = String::from_utf8(rec.into_inner()).unwrap();
        assert_eq!(
            out,
            "{\"action\":\"move_to\",\"x\":0.5,\"y\":0.25}\n\
             {\"action\":\"text\",\"text\":\"é\"}\n\
             {\"action\":\"key\",\"code\":\"KeyC\",\"pressed\":true}\n"
        );
        for line in out.lines() {
            let _: Action = serde_json::from_str(line).unwrap();
        }
    }
}
