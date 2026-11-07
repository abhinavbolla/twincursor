//! The guest's side: asking for control, and turning the viewer's input into messages.
//!
//! Like [`crate::Controller`], it has no network or window code. The viewer feeds it input and
//! the time, and sends what it returns.

use std::time::{Duration, Instant};

use protocol::{InputEvent, Message};

/// Pointer moves go out at most this often (120 a second). Newer positions replace older ones
/// that haven't gone out yet.
pub const MOVE_EVERY: Duration = Duration::from_micros(8_333);
/// While in control, the viewer sends at least this often, so the sharer's deadman knows the
/// guest is still there (ADR 0003).
pub const HEARTBEAT_EVERY: Duration = Duration::from_millis(250);
/// An unanswered request is made again this often. Packets sent just after joining can be lost,
/// and the sharer treats a repeated request as the same one.
pub const REQUEST_RETRY: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuestState {
    Idle,
    /// Asked, waiting for the sharer.
    Requested,
    InControl,
}

#[derive(Debug)]
pub struct GuestSession {
    state: GuestState,
    epoch: Instant,
    seq: u64,
    last_move: Option<Instant>,
    pending_move: Option<(f64, f64)>,
    last_sent: Option<Instant>,
    asked_at: Option<Instant>,
    /// Reliable inputs sent in this grant: every input but moves.
    reliable: u64,
}

impl GuestSession {
    /// `epoch` is the start of the clock that stamps every message. Any fixed instant will do:
    /// the sharer only compares the stamps with each other.
    pub fn new(epoch: Instant) -> Self {
        Self {
            state: GuestState::Idle,
            epoch,
            seq: 0,
            last_move: None,
            pending_move: None,
            last_sent: None,
            asked_at: None,
            reliable: 0,
        }
    }

    pub fn state(&self) -> GuestState {
        self.state
    }

    /// Asks for control. Does nothing unless idle.
    pub fn request(&mut self, now: Instant) -> Option<Message> {
        (self.state == GuestState::Idle).then(|| {
            self.state = GuestState::Requested;
            self.asked_at = Some(now);
            Message::RequestControl
        })
    }

    /// Gives control back, or withdraws the request. The sharer releases anything still held.
    pub fn release(&mut self) -> Option<Message> {
        (self.state != GuestState::Idle).then(|| {
            self.reset();
            Message::ReleaseControl
        })
    }

    /// Handles a message from the sharer. Returns whether the state changed.
    pub fn on_message(&mut self, message: &Message) -> bool {
        let next = match (self.state, message) {
            (GuestState::Requested, Message::Grant) => GuestState::InControl,
            (GuestState::Requested, Message::Deny)
            | (GuestState::Requested | GuestState::InControl, Message::Revoke) => GuestState::Idle,
            _ => return false,
        };
        if next == GuestState::Idle {
            self.reset();
        } else {
            // A new grant starts a new count on the sharer too.
            self.reliable = 0;
            self.state = next;
        }
        true
    }

    /// Turns one input event into what to send now. Input outside control is dropped.
    pub fn input(&mut self, event: InputEvent, now: Instant) -> Vec<Message> {
        if self.state != GuestState::InControl {
            return vec![];
        }
        let mut out = Vec::with_capacity(2);
        match event {
            InputEvent::Move { x, y } => {
                if self.move_due(now) {
                    self.pending_move = None;
                    out.push(self.send_move(x, y, now));
                } else {
                    self.pending_move = Some((x, y));
                }
            }
            other => {
                // Keep the order the guest made them in.
                if let Some((x, y)) = self.pending_move.take() {
                    out.push(self.send_move(x, y, now));
                }
                out.push(self.stamp(other, now));
            }
        }
        out
    }

    /// Call often, every 10 ms or so: it repeats an unanswered request, sends the newest
    /// held-back move, and heartbeats.
    pub fn tick(&mut self, now: Instant) -> Vec<Message> {
        if self.state == GuestState::Requested {
            let due = self
                .asked_at
                .is_none_or(|t| now.saturating_duration_since(t) >= REQUEST_RETRY);
            if due {
                self.asked_at = Some(now);
                return vec![Message::RequestControl];
            }
        }
        if self.state != GuestState::InControl {
            return vec![];
        }
        if self.pending_move.is_some() && self.move_due(now) {
            let (x, y) = self.pending_move.take().expect("checked");
            return vec![self.send_move(x, y, now)];
        }
        let quiet = self
            .last_sent
            .is_none_or(|t| now.saturating_duration_since(t) >= HEARTBEAT_EVERY);
        if quiet {
            self.last_sent = Some(now);
            return vec![Message::Heartbeat {
                sent_at_us: self.clock_us(now),
            }];
        }
        vec![]
    }

    fn reset(&mut self) {
        self.state = GuestState::Idle;
        self.pending_move = None;
        self.last_move = None;
    }

    fn move_due(&self, now: Instant) -> bool {
        self.last_move
            .is_none_or(|t| now.saturating_duration_since(t) >= MOVE_EVERY)
    }

    fn send_move(&mut self, x: f64, y: f64, now: Instant) -> Message {
        self.last_move = Some(now);
        self.stamp(InputEvent::Move { x, y }, now)
    }

    fn stamp(&mut self, event: InputEvent, now: Instant) -> Message {
        self.seq += 1;
        self.last_sent = Some(now);
        let after = self.reliable;
        if !matches!(event, InputEvent::Move { .. }) {
            self.reliable += 1;
        }
        Message::Input {
            seq: self.seq,
            sent_at_us: self.clock_us(now),
            after,
            event,
        }
    }

    fn clock_us(&self, now: Instant) -> u64 {
        u64::try_from(now.saturating_duration_since(self.epoch).as_micros()).unwrap_or(u64::MAX)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::Button;

    fn ms(t0: Instant, ms: u64) -> Instant {
        t0 + Duration::from_millis(ms)
    }

    fn granted(t0: Instant) -> GuestSession {
        let mut g = GuestSession::new(t0);
        assert_eq!(g.request(t0), Some(Message::RequestControl));
        assert!(g.on_message(&Message::Grant));
        g
    }

    fn event(m: &Message) -> &InputEvent {
        match m {
            Message::Input { event, .. } => event,
            other => panic!("not input: {other:?}"),
        }
    }

    #[test]
    fn asks_once_and_follows_the_sharer() {
        let t0 = Instant::now();
        let mut g = GuestSession::new(t0);
        assert!(!g.on_message(&Message::Grant), "a grant nobody asked for");
        assert_eq!(g.request(t0), Some(Message::RequestControl));
        assert_eq!(g.request(t0), None, "already asked");
        assert!(g.on_message(&Message::Deny));
        assert_eq!(g.state(), GuestState::Idle);

        g.request(t0);
        assert!(g.on_message(&Message::Grant));
        assert_eq!(g.state(), GuestState::InControl);
        assert!(!g.on_message(&Message::Deny), "deny only answers a request");
        assert!(g.on_message(&Message::Revoke));
        assert_eq!(g.state(), GuestState::Idle);
        assert_eq!(g.release(), None);
    }

    #[test]
    fn an_unanswered_request_is_repeated_every_second() {
        let t0 = Instant::now();
        let mut g = GuestSession::new(t0);
        g.request(t0);
        assert!(g.tick(ms(t0, 900)).is_empty());
        assert_eq!(g.tick(ms(t0, 1000)), vec![Message::RequestControl]);
        assert!(g.tick(ms(t0, 1500)).is_empty());
        assert_eq!(g.tick(ms(t0, 2000)), vec![Message::RequestControl]);
        g.on_message(&Message::Grant);
        assert!(
            !g.tick(ms(t0, 3000)).contains(&Message::RequestControl),
            "answered"
        );
    }

    #[test]
    fn input_outside_control_goes_nowhere() {
        let t0 = Instant::now();
        let mut g = GuestSession::new(t0);
        g.request(t0);
        assert!(g.input(InputEvent::ReleaseAll, t0).is_empty());
        assert!(
            g.tick(ms(t0, 500)).is_empty(),
            "no heartbeat before a grant"
        );
    }

    #[test]
    fn moves_are_capped_at_120_a_second_and_the_last_one_arrives() {
        let t0 = Instant::now();
        let mut g = granted(t0);
        let mut sent = Vec::new();
        // A 1 kHz mouse for one second, ticked every millisecond.
        for i in 0..1000u64 {
            let now = t0 + Duration::from_micros(i * 1000);
            sent.extend(g.input(
                InputEvent::Move {
                    x: i as f64 / 1000.0,
                    y: 0.5,
                },
                now,
            ));
            sent.extend(g.tick(now));
        }
        sent.extend(g.tick(ms(t0, 1010)));
        let moves: Vec<_> = sent
            .iter()
            .filter(|m| matches!(m, Message::Input { .. }))
            .collect();
        assert!(
            (110..=125).contains(&moves.len()),
            "{} moves in a second",
            moves.len()
        );
        assert_eq!(
            event(moves.last().unwrap()),
            &InputEvent::Move { x: 0.999, y: 0.5 },
            "the newest position always goes out"
        );
    }

    #[test]
    fn a_click_sends_the_held_back_move_first() {
        let t0 = Instant::now();
        let mut g = granted(t0);
        g.input(InputEvent::Move { x: 0.1, y: 0.1 }, t0);
        assert!(g
            .input(InputEvent::Move { x: 0.2, y: 0.2 }, ms(t0, 1))
            .is_empty());
        let click = InputEvent::Button {
            button: Button::Left,
            pressed: true,
            x: 0.2,
            y: 0.2,
        };
        let out = g.input(click.clone(), ms(t0, 2));
        assert_eq!(out.len(), 2);
        assert_eq!(event(&out[0]), &InputEvent::Move { x: 0.2, y: 0.2 });
        assert_eq!(event(&out[1]), &click);
    }

    #[test]
    fn sequence_clock_and_reliable_count_go_up() {
        let t0 = Instant::now();
        let mut g = granted(t0);
        let a = g.input(InputEvent::ReleaseAll, ms(t0, 5)).remove(0);
        let b = g
            .input(InputEvent::Move { x: 0.5, y: 0.5 }, ms(t0, 9))
            .remove(0);
        let c = g.input(InputEvent::ReleaseAll, ms(t0, 20)).remove(0);
        let d = g
            .input(InputEvent::Move { x: 0.6, y: 0.5 }, ms(t0, 30))
            .remove(0);
        let stamp = |m: &Message| match m {
            Message::Input {
                seq,
                sent_at_us,
                after,
                ..
            } => (*seq, *sent_at_us, *after),
            other => panic!("{other:?}"),
        };
        // Moves don't count as reliable, and carry how many reliable inputs went before.
        assert_eq!(
            [a, b, c, d].iter().map(stamp).collect::<Vec<_>>(),
            [(1, 5_000, 0), (2, 9_000, 1), (3, 20_000, 1), (4, 30_000, 2)]
        );
        // A new grant starts counting again, as the sharer does.
        g.on_message(&Message::Revoke);
        g.request(ms(t0, 40));
        g.on_message(&Message::Grant);
        let e = g
            .input(InputEvent::Move { x: 0.1, y: 0.1 }, ms(t0, 50))
            .remove(0);
        assert_eq!(stamp(&e).2, 0);
    }

    #[test]
    fn heartbeats_only_when_quiet() {
        let t0 = Instant::now();
        let mut g = granted(t0);
        assert_eq!(
            g.tick(t0),
            vec![Message::Heartbeat { sent_at_us: 0 }],
            "one right away"
        );
        assert!(g.tick(ms(t0, 200)).is_empty());
        g.input(InputEvent::ReleaseAll, ms(t0, 240));
        assert!(
            g.tick(ms(t0, 400)).is_empty(),
            "input counts as a sign of life"
        );
        assert_eq!(
            g.tick(ms(t0, 490)),
            vec![Message::Heartbeat {
                sent_at_us: 490_000
            }]
        );
    }

    #[test]
    fn releasing_drops_held_back_moves() {
        let t0 = Instant::now();
        let mut g = granted(t0);
        g.input(InputEvent::Move { x: 0.1, y: 0.1 }, t0);
        g.input(InputEvent::Move { x: 0.3, y: 0.3 }, ms(t0, 1));
        assert_eq!(g.release(), Some(Message::ReleaseControl));
        assert!(g.tick(ms(t0, 100)).is_empty());
    }
}
