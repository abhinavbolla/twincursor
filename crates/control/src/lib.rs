//! The sharer's side of remote control: who may control, and which input gets through.
//!
//! [`Controller`] has no network or platform code. The caller feeds it decoded messages, the
//! sharer's decisions and the time, and it drives an [`Injector`] and returns what to send or
//! show. That keeps the rules in `docs/threat-model.md` ("How control works") testable with
//! scripted traces, in `tests.rs`.

pub mod guest;
mod held;
mod injector;
mod keyboard;
mod lateness;
#[cfg(target_os = "macos")]
pub mod macos;
mod revoke;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::time::{Duration, Instant};

use held::Held;
pub use injector::{Action, Injector, LogInjector, Point, RecordInjector};
use keyboard::{Keyboard, Press};
use lateness::Lateness;
use protocol::{InputEvent, Message, Platform};
pub use revoke::{Gated, RevokeSwitch};

#[derive(Debug, Clone)]
pub struct Config {
    /// Moves later than this are dropped (ADR 0003).
    pub max_lateness: Duration,
    /// Held keys and buttons are released after this long without hearing from the guest.
    pub deadman: Duration,
    /// After the sharer moves their own mouse, the guest's moves stop driving the pointer for
    /// this long. Only one pointer exists, and the sharer wins.
    pub sharer_priority: Duration,
    /// Turn a Windows or Linux guest's Ctrl into ⌘, so Ctrl+C copies (ADR 0004).
    pub swap_ctrl_cmd: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            max_lateness: Duration::from_millis(150),
            deadman: Duration::from_secs(1),
            sharer_priority: Duration::from_millis(500),
            swap_ctrl_cmd: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Guest {
    /// The LiveKit identity, which the server vouches for.
    pub identity: String,
    /// What the guest calls themself. Not verified.
    pub name: String,
    pub platform: Platform,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    /// Send this message to one participant.
    Send { to: String, message: Message },
    /// A guest asks for control. Ask the sharer, then call [`Controller::grant`] or
    /// [`Controller::deny`].
    Ask(Guest),
    /// Control, or a pending request, ended without the sharer deciding it.
    Ended { identity: String, reason: EndReason },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndReason {
    /// The guest left the call.
    Left,
    /// The guest gave control back, or withdrew the request.
    Released,
}

/// What the controller turned away, for logs, the HUD and tests.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counters {
    /// Input from someone without control.
    pub rejected: u64,
    /// Pointer moves that were reordered or too late.
    pub stale_moves: u64,
    /// Button presses dropped because they arrived late after a deadman release.
    pub stale_presses: u64,
    /// Times the deadman let go of held input.
    pub deadman_releases: u64,
}

enum Phase {
    Idle,
    Requested(Guest),
    Granted(Guest, Box<Control>),
}

/// State that lives for one grant.
struct Control {
    held: Held,
    lateness: Lateness,
    keyboard: Keyboard,
    last_heard: Instant,
    /// The deadman fired and the backlog hasn't caught up yet.
    stalled: bool,
    last_pointer_seq: Option<u64>,
    /// Where the pointer should be if only the guest moved it.
    expected: Option<Point>,
    sharer_until: Option<Instant>,
    guest_pointer: Option<Point>,
    /// Reliable inputs applied in this grant, to compare with a move's `after`.
    reliable_seen: u64,
    /// The newest move that arrived ahead of a click or key sent before it: (seq, to, after).
    held_move: Option<(u64, Point, u64)>,
}

pub struct Controller<I> {
    injector: I,
    config: Config,
    phase: Phase,
    hellos: HashMap<String, (String, Platform)>,
    counters: Counters,
}

impl<I: Injector> Controller<I> {
    pub fn new(injector: I, config: Config) -> Self {
        Self {
            injector,
            config,
            phase: Phase::Idle,
            hellos: HashMap::new(),
            counters: Counters::default(),
        }
    }

    /// Handles a message from `from`, the sender's identity as LiveKit reports it.
    pub fn handle(&mut self, from: &str, message: Message, now: Instant) -> Vec<Effect> {
        match message {
            Message::Hello { name, platform } => {
                self.hellos.insert(from.to_owned(), (name, platform));
                vec![]
            }
            Message::RequestControl => self.on_request(from),
            Message::ReleaseControl => self.end_by_guest(from, EndReason::Released),
            Message::Heartbeat { sent_at_us } => {
                let max_lateness = self.config.max_lateness;
                if let Some(control) = self.control_of(from) {
                    control.heard(sent_at_us, now, max_lateness);
                }
                vec![]
            }
            Message::Input {
                seq,
                sent_at_us,
                after,
                event,
            } => {
                self.on_input(from, seq, sent_at_us, after, event, now);
                vec![]
            }
            // Only the sharer sends these.
            Message::Grant | Message::Deny | Message::Revoke | Message::Released => vec![],
        }
    }

    /// The sharer accepts the pending request.
    pub fn grant(&mut self, now: Instant) -> Vec<Effect> {
        match std::mem::replace(&mut self.phase, Phase::Idle) {
            Phase::Requested(guest) => {
                let swap = self.config.swap_ctrl_cmd && guest.platform != Platform::Macos;
                let control = Control {
                    held: Held::default(),
                    lateness: Lateness::new(now),
                    keyboard: Keyboard {
                        swap_ctrl_cmd: swap,
                    },
                    last_heard: now,
                    stalled: false,
                    last_pointer_seq: None,
                    expected: None,
                    sharer_until: None,
                    guest_pointer: None,
                    reliable_seen: 0,
                    held_move: None,
                };
                let to = guest.identity.clone();
                self.phase = Phase::Granted(guest, Box::new(control));
                vec![send(to, Message::Grant)]
            }
            other => {
                self.phase = other;
                vec![]
            }
        }
    }

    /// The sharer turns down the pending request.
    pub fn deny(&mut self) -> Vec<Effect> {
        match std::mem::replace(&mut self.phase, Phase::Idle) {
            Phase::Requested(guest) => vec![send(guest.identity, Message::Deny)],
            other => {
                self.phase = other;
                vec![]
            }
        }
    }

    /// The sharer ends control, or turns down a pending request. Releases everything held.
    pub fn revoke(&mut self) -> Vec<Effect> {
        match std::mem::replace(&mut self.phase, Phase::Idle) {
            Phase::Idle => vec![],
            Phase::Requested(guest) => vec![send(guest.identity, Message::Deny)],
            Phase::Granted(guest, mut control) => {
                self.release(&mut control);
                vec![send(guest.identity, Message::Revoke)]
            }
        }
    }

    /// A participant left the call.
    pub fn left(&mut self, identity: &str) -> Vec<Effect> {
        self.hellos.remove(identity);
        self.end_by_guest(identity, EndReason::Left)
    }

    /// Call often, every 100 ms or so: it runs the deadman.
    pub fn tick(&mut self, now: Instant) {
        let Phase::Granted(_, control) = &mut self.phase else {
            return;
        };
        if !control.held.is_empty()
            && now.saturating_duration_since(control.last_heard) >= self.config.deadman
        {
            tracing::warn!("no word from the guest; releasing held input");
            for action in control.held.release_all() {
                self.injector.inject(&action);
            }
            control.stalled = true;
            self.counters.deadman_releases += 1;
        }
    }

    pub fn requester(&self) -> Option<&Guest> {
        match &self.phase {
            Phase::Requested(guest) => Some(guest),
            _ => None,
        }
    }

    pub fn controller(&self) -> Option<&Guest> {
        match &self.phase {
            Phase::Granted(guest, _) => Some(guest),
            _ => None,
        }
    }

    /// Where the guest is pointing, for their cursor overlay. It can differ from the real pointer
    /// while the sharer has it.
    pub fn guest_pointer(&self) -> Option<Point> {
        match &self.phase {
            Phase::Granted(_, control) => control.guest_pointer,
            _ => None,
        }
    }

    pub fn counters(&self) -> Counters {
        self.counters
    }

    pub fn injector(&self) -> &I {
        &self.injector
    }

    pub fn injector_mut(&mut self) -> &mut I {
        &mut self.injector
    }

    fn on_request(&mut self, from: &str) -> Vec<Effect> {
        match &self.phase {
            Phase::Idle => {
                let (name, platform) = self.hellos.get(from).cloned().unwrap_or_default();
                let guest = Guest {
                    identity: from.to_owned(),
                    name: if name.is_empty() {
                        from.to_owned()
                    } else {
                        name
                    },
                    platform,
                };
                self.phase = Phase::Requested(guest.clone());
                vec![Effect::Ask(guest)]
            }
            Phase::Requested(guest) | Phase::Granted(guest, _) if guest.identity == from => vec![],
            // One guest at a time.
            _ => vec![send(from.to_owned(), Message::Deny)],
        }
    }

    fn end_by_guest(&mut self, identity: &str, reason: EndReason) -> Vec<Effect> {
        let ours = match &self.phase {
            Phase::Requested(guest) | Phase::Granted(guest, _) => guest.identity == identity,
            Phase::Idle => false,
        };
        if !ours {
            return vec![];
        }
        if let Phase::Granted(_, mut control) = std::mem::replace(&mut self.phase, Phase::Idle) {
            self.release(&mut control);
        }
        let mut effects = vec![Effect::Ended {
            identity: identity.to_owned(),
            reason,
        }];
        // Everything the guest sent before its release has been applied: tell it, so it can
        // leave without cutting off its last input.
        if reason == EndReason::Released {
            effects.push(send(identity.to_owned(), Message::Released));
        }
        effects
    }

    fn control_of(&mut self, from: &str) -> Option<&mut Control> {
        match &mut self.phase {
            Phase::Granted(guest, control) if guest.identity == from => Some(control),
            _ => None,
        }
    }

    fn release(&mut self, control: &mut Control) {
        for action in control.held.release_all() {
            self.injector.inject(&action);
        }
    }

    fn on_input(
        &mut self,
        from: &str,
        seq: u64,
        sent_at_us: u64,
        after: u64,
        event: InputEvent,
        now: Instant,
    ) {
        let config = &self.config;
        let Phase::Granted(guest, control) = &mut self.phase else {
            self.counters.rejected += 1;
            return;
        };
        if guest.identity != from {
            self.counters.rejected += 1;
            return;
        }
        let fresh = control.heard(sent_at_us, now, config.max_lateness);
        let injector = &mut self.injector;

        let reliable = !matches!(event, InputEvent::Move { .. });
        'apply: {
            match event {
                InputEvent::Move { x, y } => {
                    let reordered = control.last_pointer_seq.is_some_and(|last| seq <= last);
                    if reordered || !fresh {
                        self.counters.stale_moves += 1;
                        break 'apply;
                    }
                    control.last_pointer_seq = Some(seq);
                    let to = Point { x, y };
                    if after > control.reliable_seen {
                        // It overtook a click or key the guest sent first. Moving now could drag
                        // with a button the guest has already let go: wait for it.
                        control.held_move = Some((seq, to, after));
                        break 'apply;
                    }
                    control.held_move = None;
                    control.move_pointer(injector, to, now, config.sharer_priority);
                }
                InputEvent::Button {
                    button,
                    pressed,
                    x,
                    y,
                } => {
                    if pressed {
                        // A late press after a deadman release would start a drag the guest
                        // has already finished.
                        if control.stalled {
                            self.counters.stale_presses += 1;
                            break 'apply;
                        }
                        if !control.held.press_button(button) {
                            break 'apply;
                        }
                    } else if !control.held.release_button(button) {
                        break 'apply;
                    }
                    control.last_pointer_seq =
                        Some(control.last_pointer_seq.map_or(seq, |s| s.max(seq)));
                    // A click takes the pointer back from the sharer.
                    let at = Point { x, y };
                    control.guest_pointer = Some(at);
                    control.expected = Some(at);
                    control.sharer_until = None;
                    injector.inject(&Action::MoveTo(at));
                    injector.inject(&Action::Button { button, pressed });
                }
                InputEvent::Scroll { dx, dy, unit } => {
                    injector.inject(&Action::Scroll { dx, dy, unit });
                }
                InputEvent::Key {
                    code,
                    pressed,
                    text,
                } => {
                    let code = control.keyboard.sharer_code(&code);
                    if pressed {
                        match control.keyboard.press(code, text, &control.held) {
                            Press::Text(text) => injector.inject(&Action::Text { text }),
                            Press::Key(code) => {
                                control.held.press_key(&code);
                                injector.inject(&Action::Key {
                                    code,
                                    pressed: true,
                                });
                            }
                        }
                    } else if control.held.release_key(&code) {
                        injector.inject(&Action::Key {
                            code,
                            pressed: false,
                        });
                    }
                }
                InputEvent::ReleaseAll => {
                    for action in control.held.release_all() {
                        injector.inject(&action);
                    }
                }
            }
        }
        // Everything but a move is reliable, applied or not. A move held back for it may go now,
        // unless a click since then carried a newer position.
        if !reliable {
            return;
        }
        control.reliable_seen += 1;
        if let Some((held_seq, to, held_after)) = control.held_move {
            if held_after <= control.reliable_seen {
                control.held_move = None;
                if control.last_pointer_seq.is_none_or(|last| held_seq >= last) {
                    control.move_pointer(injector, to, now, config.sharer_priority);
                }
            }
        }
    }
}

impl Control {
    fn move_pointer<I: Injector>(
        &mut self,
        injector: &mut I,
        to: Point,
        now: Instant,
        priority: Duration,
    ) {
        self.guest_pointer = Some(to);
        if self.sharer_has_pointer(injector.pointer(), now, priority) {
            return;
        }
        injector.inject(&Action::MoveTo(to));
        self.expected = Some(to);
    }

    /// Notes that the guest is alive, and returns whether this message is on time.
    fn heard(&mut self, sent_at_us: u64, now: Instant, max_lateness: Duration) -> bool {
        self.last_heard = now;
        let fresh = self.lateness.observe(sent_at_us, now) <= max_lateness;
        if fresh {
            self.stalled = false;
        }
        fresh
    }

    /// Whether the sharer is using their own mouse, judged by the pointer moving somewhere the
    /// guest didn't put it.
    fn sharer_has_pointer(&mut self, pointer: Option<Point>, now: Instant, hold: Duration) -> bool {
        if let (Some(pointer), Some(expected)) = (pointer, self.expected) {
            if !pointer.near(expected) {
                self.sharer_until = Some(now + hold);
                self.expected = Some(pointer);
            }
        }
        self.sharer_until.is_some_and(|until| now < until)
    }
}

fn send(to: String, message: Message) -> Effect {
    Effect::Send { to, message }
}
