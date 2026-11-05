//! Where the session's decisions become input on the sharer's machine.

use protocol::{Button, ScrollUnit};

/// A position, 0..1 over the shared surface from its top left.
#[derive(Debug, Clone, Copy, PartialEq)]
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

#[derive(Debug, Clone, PartialEq)]
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
    Text(String),
}

pub trait Injector {
    fn inject(&mut self, action: &Action);

    /// Where the pointer is now, so the session can tell when the sharer moves their own mouse.
    /// `None` if the platform can't say.
    fn pointer(&self) -> Option<Point>;
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
