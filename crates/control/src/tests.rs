//! Scripted traces: who sends what, when it was sent, and when it arrives.

use std::time::{Duration, Instant};

use protocol::{Button, InputEvent, Message, Platform, ScrollUnit};

use crate::{Action, Config, Controller, Effect, EndReason, Guest, Injector, Point, Refusal};

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
    /// Reliable inputs sent so far, as the guest counts them.
    reliable: u64,
}

impl Trace {
    fn new() -> Self {
        Self {
            c: Controller::new(Recorder::default(), Config::default()),
            t0: Instant::now(),
            seq: 0,
            reliable: 0,
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
        let after = self.reliable;
        if !matches!(event, InputEvent::Move { .. }) {
            self.reliable += 1;
        }
        let message = Message::Input {
            seq: self.seq,
            sent_at_us: GUEST_CLOCK_US + sent_ms * 1_000,
            after,
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
        decoders: vec![],
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
    Action::Text { text: s.into() }
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
fn a_hello_is_answered_and_grants_nothing() {
    let ack = |to: &str| {
        vec![Effect::Send {
            to: to.into(),
            message: Message::HelloAck,
        }]
    };
    let mut t = Trace::new();
    // Each hello is answered, to whoever sent it, however often it comes.
    assert_eq!(t.msg(GUEST, hello("Alice", Platform::Macos), 0), ack(GUEST));
    assert_eq!(
        t.msg(GUEST, hello("Alice", Platform::Macos), 250),
        ack(GUEST)
    );
    assert_eq!(t.msg(OTHER, hello("Bob", Platform::Linux), 300), ack(OTHER));
    assert!(t.c.requester().is_none() && t.c.controller().is_none());
    t.send(key("KeyA", true, Some("a")), 400);
    assert!(
        t.take().is_empty(),
        "a hello is no request, let alone a grant"
    );

    // During control, a hello from the controller or anyone else changes nothing about it.
    let mut t = Trace::granted(Platform::Macos);
    assert_eq!(
        t.msg(GUEST, hello("Mallory", Platform::Windows), 10),
        ack(GUEST)
    );
    assert_eq!(
        t.msg(OTHER, hello("Alice", Platform::Macos), 10),
        ack(OTHER)
    );
    let controller = t.c.controller().unwrap();
    assert_eq!(
        (controller.identity.as_str(), controller.name.as_str()),
        (GUEST, "Alice")
    );
    t.send_from(OTHER, key("KeyB", true, Some("b")), 20, 30);
    assert!(t.take().is_empty(), "the other viewer still can't type");
    // A hello sent after a revoke doesn't bring control back.
    t.c.revoke();
    t.msg(GUEST, hello("Alice", Platform::Macos), 50);
    t.send(key("KeyC", true, Some("c")), 60);
    assert!(t.c.controller().is_none());
    assert!(!t.take().contains(&text("c")));
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
        vec![
            Effect::Ended {
                identity: GUEST.into(),
                reason: EndReason::Released
            },
            Effect::Send {
                to: GUEST.into(),
                message: Message::Released
            }
        ]
    );
    assert_eq!(t.take(), vec![left(false)]);
    assert!(t.c.controller().is_none());
}

/// Seen in the network lab on `wifi-bad`: a click's release, resent after a lost packet, arrived
/// after a move the guest made later. Moving first would have dragged with the button down.
#[test]
fn a_move_never_overtakes_a_click_sent_before_it() {
    let mut t = Trace::granted(Platform::Macos);
    t.send(button(true, 0.25, 0.25), 0);
    assert_eq!(t.take(), vec![to(0.25, 0.25), left(true)]);
    // The release is sent at 5 ms but held up; two moves sent after it arrive first.
    t.seq += 1;
    let release_seq = t.seq;
    t.reliable += 1;
    t.send(mv(0.12, 0.11), 10);
    t.send(mv(0.10, 0.10), 18);
    assert!(
        t.take().is_empty(),
        "no move while the button is still down"
    );
    t.msg(
        GUEST,
        Message::Input {
            seq: release_seq,
            sent_at_us: GUEST_CLOCK_US + 5_000,
            after: 1,
            event: button(false, 0.25, 0.25),
        },
        90,
    );
    // The release lands where it was made, then the pointer catches up with the newest move.
    assert_eq!(t.take(), vec![to(0.25, 0.25), left(false), to(0.10, 0.10)]);
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

// Trusted control: the sharer's standing grant for one call (docs/adr/0010-trusted-control.md).

fn grant_to(to: &str) -> Vec<Effect> {
    vec![Effect::Send {
        to: to.into(),
        message: Message::Grant,
    }]
}

fn deny_to(to: &str) -> Vec<Effect> {
    vec![Effect::Send {
        to: to.into(),
        message: Message::Deny,
    }]
}

fn trusted(t: &Trace) -> Vec<String> {
    t.c.trusted().into_iter().map(|g| g.identity).collect()
}

fn asks(effects: &[Effect], who: &str) -> bool {
    matches!(effects, [Effect::Ask(g)] if g.identity == who)
}

impl Trace {
    /// Alice and Bob are in the call, under their tokens' names, and have said hello.
    fn in_call() -> Self {
        let mut t = Self::new();
        t.c.joined(GUEST, "Alice", t.t0);
        t.c.joined(OTHER, "Bob", t.t0);
        t.msg(GUEST, hello("Alice", Platform::Macos), 0);
        t.msg(OTHER, hello("Bob", Platform::Linux), 0);
        t
    }

    fn trust(&mut self, who: &str, ms: u64) -> Vec<Effect> {
        let at = self.at(ms);
        self.c.trust(who, at)
    }
}

#[test]
fn a_trusted_guest_is_granted_without_a_prompt() {
    let mut t = Trace::in_call();
    assert!(
        t.trust(GUEST, 0).is_empty(),
        "trusting grants nothing by itself"
    );
    assert!(t.c.controller().is_none() && t.c.requester().is_none());
    assert_eq!(trusted(&t), [GUEST]);
    // Trust is no grant: input before a request goes nowhere.
    t.send(key("KeyA", true, Some("a")), 5);
    assert!(t.take().is_empty());

    let effects = t.msg(GUEST, Message::RequestControl, 10);
    assert_eq!(effects, grant_to(GUEST), "granted, and nobody asked");
    let controller = t.c.controller().unwrap();
    assert_eq!(
        (controller.identity.as_str(), controller.name.as_str()),
        (GUEST, "Alice")
    );
    t.send(key("KeyB", true, Some("b")), 20);
    assert_eq!(t.take(), vec![text("b")]);
}

#[test]
fn trusting_a_guest_who_is_asking_grants_at_once() {
    let mut t = Trace::in_call();
    assert!(asks(&t.msg(GUEST, Message::RequestControl, 0), GUEST));
    assert_eq!(t.trust(GUEST, 10), grant_to(GUEST));
    assert!(t.c.requester().is_none());
    assert_eq!(t.c.controller().map(|g| g.identity.as_str()), Some(GUEST));
    // The prompt it answered is gone: a grant now changes nothing.
    assert!(t.c.grant(t.at(20)).is_empty());
}

#[test]
fn trusting_is_refused_for_someone_not_in_the_call() {
    let mut t = Trace::new();
    // A hello alone isn't being in the call: the room reports who joined.
    t.msg(GUEST, hello("Alice", Platform::Macos), 0);
    assert_eq!(t.trust(GUEST, 0), vec![Effect::Refused(Refusal::NotInCall)]);
    assert_eq!(
        t.trust("sharer", 0),
        vec![Effect::Refused(Refusal::NotInCall)]
    );
    assert!(trusted(&t).is_empty());
    assert!(asks(&t.msg(GUEST, Message::RequestControl, 10), GUEST));
}

#[test]
fn trusting_is_refused_while_someone_else_controls() {
    let mut t = Trace::in_call();
    t.msg(OTHER, Message::RequestControl, 0);
    t.c.grant(t.at(0));
    assert_eq!(
        t.trust(GUEST, 10),
        vec![Effect::Refused(Refusal::SomeoneElseControls)]
    );
    assert!(trusted(&t).is_empty());
    // Bob gives control back: Alice was never trusted, so she is asked about.
    t.msg(OTHER, Message::ReleaseControl, 20);
    assert!(asks(&t.msg(GUEST, Message::RequestControl, 30), GUEST));

    // Trusting the guest who is in control is fine: they may take it again after giving it back.
    let mut t = Trace::in_call();
    t.msg(GUEST, Message::RequestControl, 0);
    t.c.grant(t.at(0));
    assert!(t.trust(GUEST, 10).is_empty());
    assert_eq!(trusted(&t), [GUEST]);
    t.msg(GUEST, Message::ReleaseControl, 20);
    assert_eq!(t.msg(GUEST, Message::RequestControl, 30), grant_to(GUEST));
}

#[test]
fn one_controller_at_a_time_even_when_trusted() {
    let mut t = Trace::in_call();
    t.trust(GUEST, 0);
    // Bob asks and the sharer lets him in.
    assert!(asks(&t.msg(OTHER, Message::RequestControl, 10), OTHER));
    // Alice's request while Bob is only asking is turned down, as any second request is.
    assert_eq!(t.msg(GUEST, Message::RequestControl, 15), deny_to(GUEST));
    t.c.grant(t.at(20));
    // While Bob controls, trusted Alice is turned down too.
    assert_eq!(t.msg(GUEST, Message::RequestControl, 30), deny_to(GUEST));
    t.send_from(GUEST, key("KeyA", true, Some("a")), 30, 40);
    assert!(t.take().is_empty());
    // Bob gives control back. Nobody is put in control: Alice has to ask herself.
    t.msg(OTHER, Message::ReleaseControl, 50);
    assert!(t.c.controller().is_none() && t.c.requester().is_none());
    t.tick(60);
    assert!(t.c.controller().is_none());
    assert_eq!(t.msg(GUEST, Message::RequestControl, 70), grant_to(GUEST));
}

#[test]
fn giving_control_back_keeps_the_trust() {
    let mut t = Trace::in_call();
    t.trust(GUEST, 0);
    for round in 0..3 {
        let ms = 100 * round;
        assert_eq!(t.msg(GUEST, Message::RequestControl, ms), grant_to(GUEST));
        t.msg(GUEST, Message::ReleaseControl, ms + 50);
        assert!(t.c.controller().is_none());
        assert_eq!(trusted(&t), [GUEST]);
    }
}

#[test]
fn a_request_loop_from_a_trusted_guest_is_granted_once() {
    let mut t = Trace::in_call();
    t.trust(GUEST, 0);
    let grants = (0..200)
        .flat_map(|ms| t.msg(GUEST, Message::RequestControl, ms))
        .collect::<Vec<_>>();
    assert_eq!(grants, grant_to(GUEST));
}

#[test]
fn revoking_takes_back_every_trust_in_every_phase() {
    // Idle: trusted, then revoked before the guest ever asked.
    let mut t = Trace::in_call();
    t.trust(GUEST, 0);
    t.trust(OTHER, 0);
    assert!(t.c.revoke().is_empty());
    assert!(trusted(&t).is_empty());
    assert!(asks(&t.msg(GUEST, Message::RequestControl, 10), GUEST));

    // Someone else asking.
    let mut t = Trace::in_call();
    t.trust(GUEST, 0);
    t.msg(OTHER, Message::RequestControl, 10);
    assert_eq!(t.c.revoke(), deny_to(OTHER));
    assert!(trusted(&t).is_empty());
    assert!(asks(&t.msg(GUEST, Message::RequestControl, 20), GUEST));

    // The trusted guest in control.
    let mut t = Trace::in_call();
    t.trust(GUEST, 0);
    t.msg(GUEST, Message::RequestControl, 10);
    t.send(key("ShiftLeft", true, None), 20);
    t.take();
    assert_eq!(
        t.c.revoke(),
        vec![Effect::Send {
            to: GUEST.into(),
            message: Message::Revoke
        }]
    );
    assert_eq!(t.take(), vec![code("ShiftLeft", false)]);
    assert!(trusted(&t).is_empty());
    // The next request is a prompt, and nothing gets in meanwhile.
    assert!(asks(&t.msg(GUEST, Message::RequestControl, 40), GUEST));
    t.send(key("KeyA", true, Some("a")), 50);
    assert!(t.take().is_empty());
}

#[test]
fn leaving_ends_the_trust_and_a_returning_guest_is_asked() {
    let mut t = Trace::in_call();
    t.trust(GUEST, 0);
    t.c.left(GUEST);
    assert!(trusted(&t).is_empty());
    // The same identity comes back, as a guest with a call pass does.
    t.c.joined(GUEST, "Alice", t.t0);
    t.msg(GUEST, hello("Alice", Platform::Macos), 100);
    assert!(asks(&t.msg(GUEST, Message::RequestControl, 110), GUEST));

    // Leaving while in control, then coming back.
    let mut t = Trace::in_call();
    t.trust(GUEST, 0);
    t.msg(GUEST, Message::RequestControl, 10);
    t.c.left(GUEST);
    t.c.joined(GUEST, "Alice", t.t0);
    assert!(asks(&t.msg(GUEST, Message::RequestControl, 110), GUEST));

    // Someone else leaving while they ask leaves the trusted guest's trust alone, and puts
    // nobody in control.
    let mut t = Trace::in_call();
    t.trust(GUEST, 0);
    t.msg(OTHER, Message::RequestControl, 10);
    t.c.left(OTHER);
    assert!(t.c.controller().is_none() && t.c.requester().is_none());
    assert_eq!(trusted(&t), [GUEST]);
    // And a guest who left can't be trusted until the room says they're back.
    assert_eq!(
        t.trust(OTHER, 20),
        vec![Effect::Refused(Refusal::NotInCall)]
    );
}

#[test]
fn nothing_a_guest_sends_creates_or_restores_trust() {
    let mut t = Trace::in_call();
    // Every message there is, hellos again (as after a reconnect) included, in a tight loop.
    for ms in 0..20 {
        for message in [
            hello("Alice", Platform::Macos),
            Message::HelloAck,
            Message::Grant,
            Message::Deny,
            Message::Revoke,
            Message::Released,
            Message::ReleaseControl,
            Message::Viewport {
                width: 1920,
                height: 1080,
            },
            Message::Heartbeat {
                sent_at_us: GUEST_CLOCK_US,
            },
        ] {
            t.msg(GUEST, message, ms);
        }
    }
    assert!(trusted(&t).is_empty());
    assert!(asks(&t.msg(GUEST, Message::RequestControl, 30), GUEST));

    // A revoked trust doesn't come back with a reconnect's hello either.
    let mut t = Trace::in_call();
    t.trust(GUEST, 0);
    t.c.revoke();
    t.msg(GUEST, hello("Alice", Platform::Macos), 10);
    assert!(trusted(&t).is_empty());
    assert!(asks(&t.msg(GUEST, Message::RequestControl, 20), GUEST));
}

#[test]
fn trust_belongs_to_an_identity_not_a_name() {
    let mut t = Trace::in_call();
    t.trust(GUEST, 0);
    // Bob says he is Alice. He is still Bob, and still asked about.
    t.msg(OTHER, hello("Alice", Platform::Macos), 10);
    let effects = t.msg(OTHER, Message::RequestControl, 20);
    assert!(asks(&effects, OTHER), "{effects:?}");
    t.c.deny();
    // Input from Bob while Alice controls goes nowhere.
    t.msg(GUEST, Message::RequestControl, 30);
    t.send_from(OTHER, key("KeyA", true, Some("a")), 30, 40);
    assert!(t.take().is_empty());
}

// Names.

#[test]
fn the_token_name_is_shown_not_the_hello() {
    let mut t = Trace::new();
    t.c.joined(GUEST, "Alice Account", t.t0);
    // The guest's own app claims another name.
    t.msg(GUEST, hello("Mallory", Platform::Windows), 0);
    let effects = t.msg(GUEST, Message::RequestControl, 0);
    let alice = Guest {
        identity: GUEST.into(),
        name: "Alice Account".into(),
        platform: Platform::Windows,
    };
    assert_eq!(effects, vec![Effect::Ask(alice.clone())]);
    t.c.trust(GUEST, t.at(5));
    assert_eq!(
        t.c.controller(),
        Some(&alice),
        "the label shows the token's name"
    );
    assert_eq!(t.c.trusted(), vec![alice]);

    // A token with no name: the hello's name, then the identity.
    let mut t = Trace::new();
    t.c.joined(GUEST, "", t.t0);
    t.msg(GUEST, hello("Alice", Platform::Macos), 0);
    let Effect::Ask(guest) = &t.msg(GUEST, Message::RequestControl, 0)[0] else {
        panic!()
    };
    assert_eq!(guest.name, "Alice");
    let mut t = Trace::new();
    t.c.joined(GUEST, "", t.t0);
    let Effect::Ask(guest) = &t.msg(GUEST, Message::RequestControl, 0)[0] else {
        panic!()
    };
    assert_eq!(guest.name, GUEST);
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
            after: 0,
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
            after: 0,
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

// Trust on arrival: "Let in with control" for someone not in the call yet.

impl Trace {
    /// Bob is in the call; Alice isn't yet.
    fn bob_only() -> Self {
        let mut t = Self::new();
        t.c.joined(OTHER, "Bob", t.t0);
        t.msg(OTHER, hello("Bob", Platform::Linux), 0);
        t
    }

    fn expect(&mut self, who: &str, ms: u64) -> Vec<Effect> {
        let at = self.at(ms);
        self.c.expect(who, at)
    }

    /// Alice joins at `ms` and says hello.
    fn alice_joins(&mut self, ms: u64) -> Vec<Effect> {
        let at = self.at(ms);
        let effects = self.c.joined(GUEST, "Alice", at);
        self.msg(GUEST, hello("Alice", Platform::Macos), ms);
        effects
    }
}

#[test]
fn an_expected_guest_is_trusted_when_they_join() {
    let mut t = Trace::bob_only();
    assert!(t.expect(GUEST, 0).is_empty());
    assert_eq!(t.c.expected(), [GUEST]);
    assert!(trusted(&t).is_empty(), "not before they join");
    assert!(t.alice_joins(1_000).is_empty());
    assert_eq!(trusted(&t), [GUEST]);
    assert!(t.c.expected().is_empty());
    assert_eq!(
        t.msg(GUEST, Message::RequestControl, 1_100),
        grant_to(GUEST),
        "granted, and nobody asked"
    );
    // Only Alice: Bob is asked about as ever.
    let mut t = Trace::bob_only();
    t.expect(GUEST, 0);
    assert!(asks(&t.msg(OTHER, Message::RequestControl, 10), OTHER));
}

#[test]
fn a_revoke_before_they_join_cancels_the_expectation() {
    // While idle, with nobody else asking: a revoke still counts.
    let mut t = Trace::bob_only();
    t.expect(GUEST, 0);
    assert!(t.c.revoke().is_empty());
    assert!(t.c.expected().is_empty());
    t.alice_joins(1_000);
    assert!(trusted(&t).is_empty());
    assert!(asks(&t.msg(GUEST, Message::RequestControl, 1_100), GUEST));

    // While Bob is asking, and while Bob is in control.
    for bob_in_control in [false, true] {
        let mut t = Trace::bob_only();
        t.expect(GUEST, 0);
        t.msg(OTHER, Message::RequestControl, 10);
        if bob_in_control {
            t.c.grant(t.at(20));
        }
        t.c.revoke();
        assert!(t.c.expected().is_empty());
        t.alice_joins(1_000);
        assert!(trusted(&t).is_empty());
        assert!(asks(&t.msg(GUEST, Message::RequestControl, 1_100), GUEST));
    }
}

#[test]
fn an_expectation_runs_out_after_a_minute() {
    let mut t = Trace::bob_only();
    t.expect(GUEST, 0);
    let late = crate::EXPECT_FOR.as_millis() as u64 + 1_000;
    assert!(t.alice_joins(late).is_empty());
    assert!(trusted(&t).is_empty(), "never trusted late");
    assert!(asks(
        &t.msg(GUEST, Message::RequestControl, late + 100),
        GUEST
    ));

    // The tick drops it too, so a join after that finds nothing.
    let mut t = Trace::bob_only();
    t.expect(GUEST, 0);
    t.tick(crate::EXPECT_FOR.as_millis() as u64);
    assert!(t.c.expected().is_empty());

    // Just in time is in time.
    let mut t = Trace::bob_only();
    t.expect(GUEST, 0);
    t.alice_joins(crate::EXPECT_FOR.as_millis() as u64 - 1);
    assert_eq!(trusted(&t), [GUEST]);
}

#[test]
fn an_expected_guest_isnt_trusted_while_someone_else_controls() {
    let mut t = Trace::bob_only();
    t.expect(GUEST, 0);
    t.msg(OTHER, Message::RequestControl, 10);
    t.c.grant(t.at(20));
    assert_eq!(
        t.alice_joins(1_000),
        vec![Effect::Refused(Refusal::SomeoneElseControls)]
    );
    assert!(trusted(&t).is_empty() && t.c.expected().is_empty());
    // Bob gives control back: Alice was never trusted, so she is asked about.
    t.msg(OTHER, Message::ReleaseControl, 1_100);
    assert!(asks(&t.msg(GUEST, Message::RequestControl, 1_200), GUEST));

    // And expecting someone while another controls is refused at once.
    let mut t = Trace::bob_only();
    t.msg(OTHER, Message::RequestControl, 10);
    t.c.grant(t.at(20));
    assert_eq!(
        t.expect(GUEST, 30),
        vec![Effect::Refused(Refusal::SomeoneElseControls)]
    );
    assert!(t.c.expected().is_empty());
}

#[test]
fn expecting_someone_already_in_the_call_is_trusting_them() {
    let mut t = Trace::in_call();
    assert!(t.expect(GUEST, 0).is_empty());
    assert_eq!(trusted(&t), [GUEST]);
    assert!(t.c.expected().is_empty());
    // Asking right now: granted at once, as with trust.
    let mut t = Trace::in_call();
    t.msg(GUEST, Message::RequestControl, 0);
    assert_eq!(t.expect(GUEST, 10), grant_to(GUEST));
}

#[test]
fn a_new_controller_expects_nobody() {
    // An engine restart is a new controller: what the old one expected is gone.
    let mut t = Trace::bob_only();
    t.expect(GUEST, 0);
    let mut fresh = Trace::bob_only();
    fresh.alice_joins(1_000);
    assert!(trusted(&fresh).is_empty());
    assert!(asks(
        &fresh.msg(GUEST, Message::RequestControl, 1_100),
        GUEST
    ));
}

#[test]
fn nothing_a_guest_sends_creates_an_expectation_or_extends_one() {
    let mut t = Trace::bob_only();
    for message in [
        hello("Alice", Platform::Macos),
        Message::RequestControl,
        Message::ReleaseControl,
        Message::Heartbeat { sent_at_us: 1 },
        Message::Grant,
        Message::HelloAck,
    ] {
        t.msg(GUEST, message.clone(), 10);
        t.msg(OTHER, message, 10);
    }
    assert!(t.c.expected().is_empty());
    t.c.revoke();

    // A hello, or a reconnect hello, near the deadline doesn't push it back.
    let mut t = Trace::bob_only();
    t.expect(GUEST, 0);
    let deadline = crate::EXPECT_FOR.as_millis() as u64;
    t.msg(GUEST, hello("Alice", Platform::Macos), deadline - 100);
    t.msg(GUEST, hello("Alice", Platform::Macos), deadline - 10);
    assert!(t.alice_joins(deadline + 10).is_empty());
    assert!(trusted(&t).is_empty());
}

#[test]
fn an_expected_guest_who_leaves_is_forgotten() {
    let mut t = Trace::bob_only();
    t.expect(GUEST, 0);
    t.c.left(GUEST);
    assert!(t.c.expected().is_empty());
    t.alice_joins(1_000);
    assert!(trusted(&t).is_empty());
}
