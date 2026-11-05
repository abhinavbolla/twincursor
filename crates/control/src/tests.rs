//! Scripted traces: who sends what, when it was sent, and when it arrives.

use std::time::{Duration, Instant};

use protocol::{Button, InputEvent, Message, Platform, ScrollUnit};

use crate::{Action, Config, Controller, Effect, EndReason, Guest, Injector, Point};

const GUEST: &str = "alice";
const OTHER: &str = "bob";
/// The guest's clock started long before ours: nothing may assume a shared clock.
const GUEST_CLOCK_US: u64 = 9_000_000_000;
/// Normal one-way delay in these traces.
const NET_MS: u64 = 20;

#[derive(Default)]
struct Recorder {
    actions: Vec<Action>,
    pointer: Option<Point>,
}

impl Injector for Recorder {
    fn inject(&mut self, action: &Action) {
        if let Action::MoveTo(p) = action {
            self.pointer = Some(*p);
        }
        self.actions.push(action.clone());
    }

    fn pointer(&self) -> Option<Point> {
        self.pointer
    }
}

struct Trace {
    c: Controller<Recorder>,
    t0: Instant,
    seq: u64,
}

impl Trace {
    fn new() -> Self {
        Self {
            c: Controller::new(Recorder::default(), Config::default()),
            t0: Instant::now(),
            seq: 0,
        }
    }

    /// Alice, on `platform`, asks for control and the sharer grants it at 0 ms.
    fn granted(platform: Platform) -> Self {
        let mut t = Self::new();
        t.msg(GUEST, hello("Alice", platform), 0);
        t.msg(GUEST, Message::RequestControl, 0);
        t.c.grant(t.at(0));
        t
    }

    fn at(&self, ms: u64) -> Instant {
        self.t0 + Duration::from_millis(ms)
    }

    fn msg(&mut self, from: &str, message: Message, arrive_ms: u64) -> Vec<Effect> {
        self.c.handle(from, message, self.at(arrive_ms))
    }

    /// Input sent at `sent_ms` that takes the normal delay.
    fn send(&mut self, event: InputEvent, sent_ms: u64) {
        self.send_from(GUEST, event, sent_ms, sent_ms + NET_MS);
    }

    fn send_from(&mut self, from: &str, event: InputEvent, sent_ms: u64, arrive_ms: u64) {
        self.seq += 1;
        let message = Message::Input {
            seq: self.seq,
            sent_at_us: GUEST_CLOCK_US + sent_ms * 1_000,
            event,
        };
        self.msg(from, message, arrive_ms);
    }

    fn heartbeat(&mut self, sent_ms: u64) {
        let message = Message::Heartbeat {
            sent_at_us: GUEST_CLOCK_US + sent_ms * 1_000,
        };
        self.msg(GUEST, message, sent_ms + NET_MS);
    }

    fn tick(&mut self, ms: u64) {
        self.c.tick(self.at(ms));
    }

    fn take(&mut self) -> Vec<Action> {
        std::mem::take(&mut self.c.injector_mut().actions)
    }
}

fn hello(name: &str, platform: Platform) -> Message {
    Message::Hello {
        name: name.into(),
        platform,
    }
}

fn mv(x: f64, y: f64) -> InputEvent {
    InputEvent::Move { x, y }
}

fn button(pressed: bool, x: f64, y: f64) -> InputEvent {
    InputEvent::Button {
        button: Button::Left,
        pressed,
        x,
        y,
    }
}

fn key(code: &str, pressed: bool, text: Option<&str>) -> InputEvent {
    InputEvent::Key {
        code: code.into(),
        pressed,
        text: text.map(Into::into),
    }
}

fn to(x: f64, y: f64) -> Action {
    Action::MoveTo(Point { x, y })
}

fn left(pressed: bool) -> Action {
    Action::Button {
        button: Button::Left,
        pressed,
    }
}

fn code(code: &str, pressed: bool) -> Action {
    Action::Key {
        code: code.into(),
        pressed,
    }
}

fn text(s: &str) -> Action {
    Action::Text(s.into())
}

// Consent.

#[test]
fn control_starts_off() {
    let mut t = Trace::new();
    t.msg(GUEST, hello("Alice", Platform::Macos), 0);
    t.send(mv(0.5, 0.5), 0);
    t.send(key("KeyA", true, Some("a")), 10);
    assert!(t.take().is_empty());
    assert_eq!(t.c.counters().rejected, 2);
}

#[test]
fn a_request_asks_the_sharer_and_a_grant_answers_the_guest() {
    let mut t = Trace::new();
    t.msg(GUEST, hello("Alice", Platform::Windows), 0);
    let effects = t.msg(GUEST, Message::RequestControl, 0);
    let alice = Guest {
        identity: GUEST.into(),
        name: "Alice".into(),
        platform: Platform::Windows,
    };
    assert_eq!(effects, vec![Effect::Ask(alice.clone())]);
    // Asking again doesn't ask the sharer twice.
    assert!(t.msg(GUEST, Message::RequestControl, 5).is_empty());

    let effects = t.c.grant(t.at(10));
    assert_eq!(
        effects,
        vec![Effect::Send {
            to: GUEST.into(),
            message: Message::Grant
        }]
    );
    assert_eq!(t.c.controller(), Some(&alice));
    t.send(mv(0.5, 0.5), 20);
    assert_eq!(t.take(), vec![to(0.5, 0.5)]);
}

#[test]
fn a_guest_without_a_hello_is_named_by_identity() {
    let mut t = Trace::new();
    let effects = t.msg(GUEST, Message::RequestControl, 0);
    let Effect::Ask(guest) = &effects[0] else {
        panic!("{effects:?}")
    };
    assert_eq!(guest.name, GUEST);
    assert_eq!(guest.platform, Platform::Unknown);
}

#[test]
fn denied_guests_cannot_inject() {
    let mut t = Trace::new();
    t.msg(GUEST, Message::RequestControl, 0);
    let effects = t.c.deny();
    assert_eq!(
        effects,
        vec![Effect::Send {
            to: GUEST.into(),
            message: Message::Deny
        }]
    );
    t.send(mv(0.5, 0.5), 10);
    assert!(t.take().is_empty());
    assert!(t.c.requester().is_none());
}

#[test]
fn only_the_granted_guest_can_inject_and_one_at_a_time() {
    let mut t = Trace::granted(Platform::Macos);
    t.send_from(OTHER, mv(0.1, 0.1), 0, 20);
    t.send_from(OTHER, key("KeyA", true, Some("a")), 0, 20);
    assert!(t.take().is_empty());
    assert_eq!(t.c.counters().rejected, 2);

    let effects = t.msg(OTHER, Message::RequestControl, 30);
    assert_eq!(
        effects,
        vec![Effect::Send {
            to: OTHER.into(),
            message: Message::Deny
        }]
    );
    // Another participant can't end someone else's control.
    assert!(t.msg(OTHER, Message::ReleaseControl, 40).is_empty());
    assert!(t.c.left(OTHER).is_empty());
    assert_eq!(t.c.controller().map(|g| g.identity.as_str()), Some(GUEST));
}

#[test]
fn revoking_mid_drag_lets_go_of_everything() {
    let mut t = Trace::granted(Platform::Macos);
    t.send(key("ShiftLeft", true, None), 0);
    t.send(button(true, 0.2, 0.2), 10);
    t.send(mv(0.3, 0.3), 20);
    t.take();

    let effects = t.c.revoke();
    assert_eq!(
        effects,
        vec![Effect::Send {
            to: GUEST.into(),
            message: Message::Revoke
        }]
    );
    assert_eq!(t.take(), vec![left(false), code("ShiftLeft", false)]);

    // The rest of the drag arrives after the revoke and goes nowhere.
    t.send(mv(0.4, 0.4), 30);
    t.send(button(false, 0.4, 0.4), 40);
    assert!(t.take().is_empty());
}

#[test]
fn leaving_ends_control_and_rejoining_needs_a_new_grant() {
    let mut t = Trace::granted(Platform::Macos);
    t.send(key("KeyA", true, Some("a")), 0);
    t.send(key("AltLeft", true, None), 10);
    t.take();

    let effects = t.c.left(GUEST);
    assert_eq!(
        effects,
        vec![Effect::Ended {
            identity: GUEST.into(),
            reason: EndReason::Left
        }]
    );
    assert_eq!(t.take(), vec![code("AltLeft", false)]);

    // Same identity, back in the call.
    t.msg(GUEST, hello("Alice", Platform::Macos), 100);
    t.send(mv(0.5, 0.5), 110);
    assert!(t.take().is_empty());
}

#[test]
fn the_guest_can_give_control_back() {
    let mut t = Trace::granted(Platform::Macos);
    t.send(button(true, 0.5, 0.5), 0);
    t.take();
    let effects = t.msg(GUEST, Message::ReleaseControl, 30);
    assert_eq!(
        effects,
        vec![Effect::Ended {
            identity: GUEST.into(),
            reason: EndReason::Released
        }]
    );
    assert_eq!(t.take(), vec![left(false)]);
    assert!(t.c.controller().is_none());
}

#[test]
fn guests_cannot_send_sharer_messages() {
    let mut t = Trace::new();
    t.msg(GUEST, Message::RequestControl, 0);
    for message in [Message::Grant, Message::Revoke, Message::Deny] {
        assert!(t.msg(GUEST, message, 10).is_empty());
    }
    assert!(t.c.controller().is_none());
    assert!(t.c.requester().is_some());
}

// Held input and stalls.

#[test]
fn focus_loss_releases_held_input() {
    let mut t = Trace::granted(Platform::Macos);
    t.send(key("MetaLeft", true, None), 0);
    t.send(button(true, 0.5, 0.5), 10);
    t.take();
    t.send(InputEvent::ReleaseAll, 20);
    assert_eq!(t.take(), vec![left(false), code("MetaLeft", false)]);
    // The guest's real key-up arrives later and does nothing.
    t.send(key("MetaLeft", false, None), 30);
    assert!(t.take().is_empty());
}

#[test]
fn heartbeats_keep_held_input_alive() {
    let mut t = Trace::granted(Platform::Macos);
    t.send(key("ShiftLeft", true, None), 0);
    t.take();
    for ms in (250..=3_000).step_by(250) {
        t.heartbeat(ms);
        t.tick(ms + NET_MS + 50);
    }
    assert!(t.take().is_empty());
    assert_eq!(t.c.counters().deadman_releases, 0);
}

#[test]
fn the_deadman_releases_after_a_second_of_silence() {
    let mut t = Trace::granted(Platform::Macos);
    t.send(key("ShiftLeft", true, None), 0);
    t.take();
    t.tick(NET_MS + 900);
    assert!(t.take().is_empty());
    t.tick(NET_MS + 1_000);
    assert_eq!(t.take(), vec![code("ShiftLeft", false)]);
    // Nothing is held now, so silence is fine.
    t.tick(NET_MS + 5_000);
    assert_eq!(t.c.counters().deadman_releases, 1);
}

/// A 1.2 s stall in the middle of a drag, shaped like the network lab's `stall-1.2s` profile:
/// everything sent during the stall arrives together, in order, when it clears.
#[test]
fn a_stall_mid_drag_releases_and_the_backlog_cannot_restart_it() {
    let mut t = Trace::granted(Platform::Macos);
    t.send(button(true, 0.2, 0.2), 0);
    t.send(mv(0.25, 0.25), 50);
    assert_eq!(t.take(), vec![to(0.2, 0.2), left(true), to(0.25, 0.25)]);

    // The link stalls from 100 ms to 1,300 ms. The deadman fires one second after the last
    // message arrived.
    t.tick(1_000);
    assert!(t.take().is_empty());
    t.tick(NET_MS + 50 + 1_000);
    assert_eq!(t.take(), vec![left(false)]);

    // The backlog: the rest of the drag, a new click-and-drag, and some typing.
    let clear = 1_300;
    t.send_from(GUEST, mv(0.3, 0.3), 100, clear);
    t.send_from(GUEST, button(false, 0.35, 0.35), 200, clear);
    t.send_from(GUEST, button(true, 0.5, 0.5), 300, clear);
    t.send_from(GUEST, mv(0.6, 0.6), 400, clear);
    t.send_from(GUEST, key("KeyH", true, Some("h")), 500, clear);
    t.send_from(GUEST, key("KeyH", false, Some("h")), 550, clear);
    t.send_from(GUEST, key("KeyI", true, Some("i")), 600, clear);
    t.send_from(GUEST, button(false, 0.6, 0.6), 700, clear);
    // Keys typed during the stall still apply, in order. Nothing touches the pointer or buttons.
    assert_eq!(t.take(), vec![text("h"), text("i")]);
    assert_eq!(t.c.counters().stale_moves, 2);
    assert_eq!(t.c.counters().stale_presses, 1);

    // Once input is on time again, it works normally.
    t.send(mv(0.7, 0.7), 1_310);
    t.send(button(true, 0.7, 0.7), 1_320);
    assert_eq!(t.take(), vec![to(0.7, 0.7), to(0.7, 0.7), left(true)]);
}

/// A 0.6 s stall with nothing held: no deadman, but stale moves still mustn't replay.
#[test]
fn a_short_stall_drops_old_moves_but_keeps_clicks_and_keys() {
    let mut t = Trace::granted(Platform::Macos);
    t.send(mv(0.1, 0.1), 0);
    t.take();
    let clear = 700;
    t.send_from(GUEST, mv(0.2, 0.2), 100, clear);
    t.send_from(GUEST, button(true, 0.3, 0.3), 200, clear);
    t.send_from(GUEST, button(false, 0.3, 0.3), 250, clear);
    t.send_from(GUEST, mv(0.4, 0.4), 600, clear);
    t.send_from(GUEST, key("KeyX", true, Some("x")), 650, clear);
    assert_eq!(
        t.take(),
        vec![
            to(0.3, 0.3),
            left(true),
            to(0.3, 0.3),
            left(false),
            to(0.4, 0.4),
            text("x")
        ]
    );
    assert_eq!(t.c.counters().stale_moves, 1);
}

#[test]
fn reordered_moves_are_dropped() {
    let mut t = Trace::granted(Platform::Macos);
    t.send(mv(0.1, 0.1), 0);
    t.send(mv(0.2, 0.2), 10);
    // seq 1 again: overtaken by seq 2.
    t.c.handle(
        GUEST,
        Message::Input {
            seq: 1,
            sent_at_us: GUEST_CLOCK_US,
            event: mv(0.9, 0.9),
        },
        t.at(35),
    );
    assert_eq!(t.take(), vec![to(0.1, 0.1), to(0.2, 0.2)]);
    assert_eq!(t.c.counters().stale_moves, 1);
}

#[test]
fn a_move_older_than_a_click_cannot_drag_the_pointer_back() {
    let mut t = Trace::granted(Platform::Macos);
    t.send(button(true, 0.5, 0.5), 0);
    // A move sent before the click, which overtook it on the lossy channel.
    t.c.handle(
        GUEST,
        Message::Input {
            seq: 0,
            sent_at_us: GUEST_CLOCK_US,
            event: mv(0.1, 0.1),
        },
        t.at(25),
    );
    assert_eq!(t.take(), vec![to(0.5, 0.5), left(true)]);
}

#[test]
fn clock_drift_over_an_hour_does_not_drop_moves() {
    let mut t = Trace::granted(Platform::Macos);
    // The guest's clock runs 100 ppm slow: 360 ms behind after an hour, well past the limit.
    for s in 0..3_600 {
        let arrive_ms = s * 1_000 + NET_MS;
        let sent_ms = s * 1_000 - s / 10;
        t.send_from(GUEST, mv(0.5, 0.5), sent_ms, arrive_ms);
    }
    assert_eq!(t.c.counters().stale_moves, 0);
}

// The pointer and the keyboard.

#[test]
fn the_sharer_wins_the_pointer_until_they_stop() {
    let mut t = Trace::granted(Platform::Macos);
    t.send(mv(0.1, 0.1), 0);
    t.take();

    // The sharer grabs their mouse.
    t.c.injector_mut().pointer = Some(Point { x: 0.8, y: 0.8 });
    t.send(mv(0.2, 0.2), 100);
    t.send(mv(0.3, 0.3), 300);
    assert!(t.take().is_empty());
    // The guest's overlay cursor still follows the guest.
    assert_eq!(t.c.guest_pointer(), Some(Point { x: 0.3, y: 0.3 }));

    // The sharer keeps moving, which extends their hold.
    t.c.injector_mut().pointer = Some(Point { x: 0.7, y: 0.7 });
    t.send(mv(0.4, 0.4), 500);
    assert!(t.take().is_empty());

    // Half a second after the sharer's last move, the guest drives again.
    t.send(mv(0.5, 0.5), 1_000);
    assert_eq!(t.take(), vec![to(0.5, 0.5)]);
}

#[test]
fn a_guest_click_takes_the_pointer_back() {
    let mut t = Trace::granted(Platform::Macos);
    t.send(mv(0.1, 0.1), 0);
    t.c.injector_mut().pointer = Some(Point { x: 0.8, y: 0.8 });
    t.send(mv(0.2, 0.2), 100);
    t.take();
    t.send(button(true, 0.25, 0.25), 150);
    t.send(mv(0.3, 0.3), 200);
    assert_eq!(t.take(), vec![to(0.25, 0.25), left(true), to(0.3, 0.3)]);
}

#[test]
fn typing_sends_the_guests_text() {
    let mut t = Trace::granted(Platform::Windows);
    t.send(key("ShiftLeft", true, None), 0);
    t.send(key("KeyA", true, Some("A")), 10);
    t.send(key("KeyA", false, Some("A")), 20);
    t.send(key("ShiftLeft", false, None), 30);
    // A German layout's ö on the guest, whatever the sharer's layout.
    t.send(key("Semicolon", true, Some("ö")), 40);
    t.send(key("Semicolon", false, None), 50);
    assert_eq!(
        t.take(),
        vec![
            code("ShiftLeft", true),
            text("A"),
            code("ShiftLeft", false),
            text("ö")
        ]
    );
}

#[test]
fn keys_that_produce_control_characters_are_pressed() {
    let mut t = Trace::granted(Platform::Macos);
    t.send(key("Enter", true, Some("\r")), 0);
    t.send(key("Enter", false, None), 10);
    t.send(key("Backspace", true, Some("\u{8}")), 20);
    t.send(key("Backspace", false, None), 30);
    assert_eq!(
        t.take(),
        vec![
            code("Enter", true),
            code("Enter", false),
            code("Backspace", true),
            code("Backspace", false)
        ]
    );
}

#[test]
fn a_windows_guests_ctrl_c_copies_on_the_mac() {
    let mut t = Trace::granted(Platform::Windows);
    t.send(key("ControlLeft", true, None), 0);
    t.send(key("KeyC", true, Some("c")), 10);
    t.send(key("KeyC", false, Some("c")), 20);
    t.send(key("ControlLeft", false, None), 30);
    assert_eq!(
        t.take(),
        vec![
            code("MetaLeft", true),
            code("KeyC", true),
            code("KeyC", false),
            code("MetaLeft", false)
        ]
    );
}

#[test]
fn a_mac_guests_keys_are_not_swapped() {
    let mut t = Trace::granted(Platform::Macos);
    t.send(key("MetaLeft", true, None), 0);
    t.send(key("KeyC", true, Some("c")), 10);
    t.send(key("ControlLeft", true, None), 20);
    assert_eq!(
        t.take(),
        vec![
            code("MetaLeft", true),
            code("KeyC", true),
            code("ControlLeft", true)
        ]
    );
}

#[test]
fn the_swap_can_be_turned_off() {
    let config = Config {
        swap_ctrl_cmd: false,
        ..Config::default()
    };
    let mut t = Trace::new();
    t.c = Controller::new(Recorder::default(), config);
    t.msg(GUEST, hello("Alice", Platform::Windows), 0);
    t.msg(GUEST, Message::RequestControl, 0);
    t.c.grant(t.at(0));
    t.send(key("ControlLeft", true, None), 0);
    assert_eq!(t.take(), vec![code("ControlLeft", true)]);
}

#[test]
fn key_repeats_press_again() {
    let mut t = Trace::granted(Platform::Macos);
    for ms in [0, 30, 60] {
        t.send(key("ArrowDown", true, None), ms);
    }
    t.send(key("ArrowDown", false, None), 90);
    for ms in [100, 130] {
        t.send(key("KeyA", true, Some("a")), ms);
    }
    let pressed = code("ArrowDown", true);
    assert_eq!(
        t.take(),
        vec![
            pressed.clone(),
            pressed.clone(),
            pressed,
            code("ArrowDown", false),
            text("a"),
            text("a")
        ]
    );
}

#[test]
fn scrolls_pass_through() {
    let mut t = Trace::granted(Platform::Macos);
    let scroll = InputEvent::Scroll {
        dx: 0.0,
        dy: -42.5,
        unit: ScrollUnit::Pixel,
    };
    t.send(scroll, 0);
    assert_eq!(
        t.take(),
        vec![Action::Scroll {
            dx: 0.0,
            dy: -42.5,
            unit: ScrollUnit::Pixel
        }]
    );
}
