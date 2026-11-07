//! Revoking without waiting for anything.
//!
//! The sharer's hotkey trips a [`RevokeSwitch`] from whatever thread sees it: no lock, no
//! channel, no async. The injector is wrapped in [`Gated`], which checks the switch before every
//! action, so injection stops at once even if the control thread is busy or behind. The control
//! thread then notices, revokes properly (telling the guest and releasing everything held), and
//! settles the gate.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::{Action, Injector, Point};

#[derive(Debug, Clone, Default)]
pub struct RevokeSwitch(Arc<AtomicU64>);

impl RevokeSwitch {
    /// Asks for control to end. Safe to call from any thread, any number of times.
    pub fn trip(&self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }

    fn generation(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

/// An injector that lets nothing but releases through while a revoke is pending.
pub struct Gated<I> {
    inner: I,
    switch: RevokeSwitch,
    seen: u64,
    blocked: u64,
}

impl<I: Injector> Gated<I> {
    pub fn new(inner: I, switch: RevokeSwitch) -> Self {
        let seen = switch.generation();
        Self {
            inner,
            switch,
            seen,
            blocked: 0,
        }
    }

    /// Whether the switch was tripped since the last [`Gated::settle`].
    pub fn pending(&self) -> bool {
        self.switch.generation() != self.seen
    }

    /// Opens the gate again, once the revoke has been carried out.
    pub fn settle(&mut self) {
        self.seen = self.switch.generation();
    }

    /// Actions dropped because a revoke was pending.
    pub fn blocked(&self) -> u64 {
        self.blocked
    }

    pub fn inner(&self) -> &I {
        &self.inner
    }

    pub fn inner_mut(&mut self) -> &mut I {
        &mut self.inner
    }
}

impl<I: Injector> Injector for Gated<I> {
    fn inject(&mut self, action: &Action) {
        let release = matches!(
            action,
            Action::Key { pressed: false, .. } | Action::Button { pressed: false, .. }
        );
        if self.pending() && !release {
            self.blocked += 1;
            return;
        }
        self.inner.inject(action);
    }

    fn pointer(&self) -> Option<Point> {
        self.inner.pointer()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RecordInjector;
    use protocol::Button;

    fn key(code: &str, pressed: bool) -> Action {
        Action::Key {
            code: code.into(),
            pressed,
        }
    }

    fn recorded(g: Gated<RecordInjector<Vec<u8>>>) -> Vec<Action> {
        let Gated { inner, .. } = g;
        String::from_utf8(inner.into_inner())
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    #[test]
    fn a_trip_stops_presses_at_once_but_lets_releases_through() {
        let switch = RevokeSwitch::default();
        let mut g = Gated::new(RecordInjector::new(Vec::new()), switch.clone());
        g.inject(&key("ShiftLeft", true));
        // The hotkey fires on another thread, before the control thread has caught up.
        std::thread::spawn({
            let switch = switch.clone();
            move || switch.trip()
        })
        .join()
        .unwrap();
        assert!(g.pending());
        g.inject(&key("KeyA", true));
        g.inject(&Action::Text { text: "a".into() });
        g.inject(&Action::MoveTo(Point { x: 0.1, y: 0.1 }));
        g.inject(&Action::Button {
            button: Button::Left,
            pressed: true,
        });
        g.inject(&key("ShiftLeft", false));
        g.inject(&Action::Button {
            button: Button::Left,
            pressed: false,
        });
        assert_eq!(g.blocked(), 4);
        g.settle();
        assert!(!g.pending());
        g.inject(&key("KeyB", true));
        assert_eq!(
            recorded(g),
            [
                key("ShiftLeft", true),
                key("ShiftLeft", false),
                Action::Button {
                    button: Button::Left,
                    pressed: false
                },
                key("KeyB", true),
            ]
        );
    }
}
