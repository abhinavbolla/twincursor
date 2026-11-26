//! The sharer's side of remote control (`tc-spike share --allow-control`, `tc-engine share`).
//!
//! One thread owns the [`Controller`]: it takes link events and the sharer's answers, ticks the
//! deadman, and sends what the controller says to. Where the sharer is asked is the [`Consent`]
//! seam: `tc-spike` asks in its terminal (type `y` to allow), `tc-engine` asks the desktop app
//! with a `control_asked` event and waits for an `allow_control` command naming that request.
//! Nothing else grants control: no answer means no control. The one exception is the sharer's
//! own standing grant, `trust_control` from the app ("Let in with control"): that guest's next
//! request is granted without a prompt, until any revoke or until they leave
//! (docs/adr/0010-trusted-control.md).

use std::io::BufRead;
use std::path::PathBuf;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use control::{
    Config, Controller, Effect, Gated, Guest, Injector, LogInjector, RecordInjector, Refusal,
    RevokeSwitch,
};
use ipc::{ConnectionState, ControlState, Event, Peer, Role};
use media::link::{ControlLink, LinkEvent, LinkSender, Outgoing};
use protocol::Message;

use crate::app::{role_of, Events, People};
use crate::overlay::{OverlayHandle, OverlayState};
use crate::Error;

const TICK: Duration = Duration::from_millis(20);

/// Where accepted input goes.
#[derive(Debug, Clone)]
pub enum Inject {
    /// Log each action, touching nothing.
    Log,
    /// Write each action as a line of JSON to this file (see `tc-spike replay`).
    Record(PathBuf),
    /// Move the real pointer and press real keys (macOS, with the Accessibility permission).
    Real,
}

impl std::str::FromStr for Inject {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.split_once(':') {
            None if s == "log" => Ok(Inject::Log),
            None if s == "real" => Ok(Inject::Real),
            Some(("record", path)) if !path.is_empty() => Ok(Inject::Record(path.into())),
            _ => Err("expected `log`, `real` or `record:PATH`".into()),
        }
    }
}

/// Where the sharer is asked for consent, and told what happens.
pub enum Consent {
    /// In this terminal: a prompt on stdout, answered by typing `y` (anything else is no), and
    /// `r` to take control back. `tc-spike`'s way.
    Terminal,
    /// In the desktop app: a `control_asked` event, answered with [`Sharer::answer`]. Who is in
    /// the call and who has control go to `events` too. With `allow` false, every request is
    /// turned down without asking.
    App { events: Events, allow: bool },
}

/// The app's answer to a `control_asked` event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    Allow { request: u64 },
    Deny { request: u64 },
}

enum In {
    Link(LinkEvent),
    Line(String),
    Answer(Answer),
    /// The app trusts this guest with control. `revokes` is the revoke switch's count when the
    /// command arrived: a revoke after that wins, even if the control thread sees it first.
    Trust {
        identity: String,
        revokes: u64,
        /// `trust_on_arrival`: for someone who may not have joined yet.
        on_arrival: bool,
    },
    Stop,
}

/// The control thread, which owns the controller and the injector.
pub struct Sharer {
    handle: SharerHandle,
    thread: JoinHandle<()>,
}

/// Talks to the control thread from anywhere.
#[derive(Clone)]
pub struct SharerHandle {
    tx: Sender<In>,
    revoke: RevokeSwitch,
}

impl SharerHandle {
    /// Hands the app's answer to the control thread. Only an [`Answer::Allow`] for the request
    /// still open grants control.
    pub fn answer(&self, answer: Answer) {
        let _ = self.tx.send(In::Answer(answer));
    }

    /// The app's `trust_control`: `identity` may take control during this call without a
    /// prompt. Only the desktop app sends it; with [`Consent::Terminal`] it does nothing.
    pub fn trust(&self, identity: String) {
        let revokes = self.revoke.count();
        let _ = self.tx.send(In::Trust {
            identity,
            revokes,
            on_arrival: false,
        });
    }

    /// The app's `trust_on_arrival`: trust `identity` as [`SharerHandle::trust`] does, now if
    /// they are in the call, else if they join within a minute. Any revoke cancels it, the
    /// hotkey included, even one pressed before they join.
    pub fn trust_on_arrival(&self, identity: String) {
        let revokes = self.revoke.count();
        let _ = self.tx.send(In::Trust {
            identity,
            revokes,
            on_arrival: true,
        });
    }

    /// Ends any control and releases everything held, without waiting.
    pub fn stop(&self) {
        let _ = self.tx.send(In::Stop);
    }
}

impl Sharer {
    /// Starts the control thread. Returns the link to hand to `publish::run`. Tripping `revoke`
    /// stops injection at once and ends control.
    pub fn start(
        inject: &Inject,
        revoke: RevokeSwitch,
        overlay: Option<OverlayHandle>,
        consent: Consent,
    ) -> Result<(Self, ControlLink), Error> {
        let injector: Box<dyn Injector + Send> = match inject {
            Inject::Log => Box::new(LogInjector),
            Inject::Record(path) => Box::new(RecordInjector::new(std::fs::File::create(path)?)),
            #[cfg(target_os = "macos")]
            Inject::Real => Box::new(control::macos::MacInjector::new()?),
            #[cfg(not(target_os = "macos"))]
            Inject::Real => return Err(Error::NoRealInjection),
        };
        let (tx, rx) = mpsc::channel();
        let (link, outgoing) = {
            let tx = tx.clone();
            ControlLink::new(protocol::TOPIC, move |event| {
                let _ = tx.send(In::Link(event));
            })
        };
        let terminal = matches!(consent, Consent::Terminal);
        if terminal {
            let tx = tx.clone();
            std::thread::spawn(move || {
                for line in std::io::stdin().lock().lines() {
                    let Ok(line) = line else { break };
                    if tx.send(In::Line(line)).is_err() {
                        break;
                    }
                }
            });
        }
        let handle = SharerHandle {
            tx: tx.clone(),
            revoke: revoke.clone(),
        };
        let thread = std::thread::Builder::new()
            .name("control".into())
            .spawn(move || {
                let injector = Gated::new(injector, revoke.clone());
                let mut seam = match consent {
                    Consent::Terminal => Seam::Terminal,
                    Consent::App { events, allow } => Seam::App(AppSeam::new(events, allow)),
                };
                run(
                    Controller::new(injector, Config::default()),
                    &rx,
                    &outgoing,
                    &revoke,
                    overlay.as_ref(),
                    &mut seam,
                )
            })?;
        if terminal {
            println!(
                "Remote control is allowed to be requested. You'll be asked before anyone gets \
                 it, and {} takes it back at any time.",
                crate::hotkey::REVOKE
            );
        }
        Ok((Self { handle, thread }, link))
    }

    /// For answering and stopping from other threads.
    pub fn handle(&self) -> SharerHandle {
        self.handle.clone()
    }

    /// Ends any control and releases everything held, before the process exits.
    pub fn stop(self) {
        self.handle.stop();
        let _ = self.thread.join();
    }
}

/// How the control thread asks and tells the sharer.
enum Seam {
    Terminal,
    App(AppSeam),
}

/// The desktop app's side of the control thread.
struct AppSeam {
    events: Events,
    /// Whether requests reach the app at all.
    allow: bool,
    /// The last request number handed out.
    requests: u64,
    /// The request the app may answer, and whose it is.
    open: Option<(u64, String)>,
    people: People,
    /// The control state last sent, so each change is sent once.
    shown: Option<(ControlState, Option<Peer>, Vec<Peer>)>,
}

impl AppSeam {
    fn new(events: Events, allow: bool) -> Self {
        Self {
            events,
            allow,
            requests: 0,
            open: None,
            people: People::default(),
            shown: None,
        }
    }

    fn send(&self, event: Event) {
        (self.events)(event);
    }

    /// Sends the control state if it changed: who is asking or in control, and who is trusted.
    fn show(&mut self, requester: Option<&Guest>, controller: Option<&Guest>, trusted: &[Guest]) {
        let peer = |g: &Guest| Peer {
            identity: g.identity.clone(),
            name: g.name.clone(),
        };
        let (state, by) = match (controller, requester) {
            (Some(g), _) => (ControlState::Controlling, Some(peer(g))),
            (None, Some(g)) => (ControlState::Asked, Some(peer(g))),
            (None, None) => (ControlState::None, None),
        };
        let now = (state, by, trusted.iter().map(peer).collect());
        if self.shown.as_ref() != Some(&now) {
            self.shown = Some(now.clone());
            let (state, by, trusted) = now;
            self.send(Event::Control { state, by, trusted });
        }
        // A request that is no longer pending can't be answered.
        if self
            .open
            .as_ref()
            .is_some_and(|(_, who)| requester.is_none_or(|g| &g.identity != who))
        {
            self.open = None;
        }
    }
}

fn run<I: Injector>(
    mut controller: Controller<Gated<I>>,
    rx: &mpsc::Receiver<In>,
    outgoing: &LinkSender,
    revoke: &RevokeSwitch,
    overlay: Option<&OverlayHandle>,
    seam: &mut Seam,
) {
    let mut bad_messages = 0u64;
    if let Seam::App(app) = seam {
        app.show(None, None, &[]);
    }
    loop {
        if controller.injector().pending() {
            if let (Seam::Terminal, Some(guest)) =
                (&seam, controller.controller().or(controller.requester()))
            {
                println!("Revoked: {} no longer has control.", guest.name);
            }
            apply_with(controller.revoke(), outgoing, seam);
            controller.injector_mut().settle();
        }
        let effects = match rx.recv_timeout(TICK) {
            Ok(In::Link(LinkEvent::Data { from, payload })) => match protocol::decode(&payload) {
                Ok(message) => {
                    if let (Seam::App(app), Message::Hello { decoders, .. }) =
                        (&mut *seam, &message)
                    {
                        if let Some(event) = app.people.hello(&from, decoders) {
                            app.send(event);
                        }
                    }
                    controller.handle(&from, message, Instant::now())
                }
                Err(e) => {
                    bad_messages += 1;
                    tracing::debug!(%from, "bad control message: {e}");
                    vec![]
                }
            },
            Ok(In::Link(LinkEvent::Left(identity))) => {
                if let Seam::App(app) = seam {
                    if let Some(event) = app.people.left(&identity) {
                        app.send(event);
                    }
                }
                controller.left(&identity)
            }
            Ok(In::Link(event)) => {
                let effects = match &event {
                    LinkEvent::Joined { identity, name } => {
                        controller.joined(identity, name, Instant::now())
                    }
                    _ => vec![],
                };
                if let Seam::App(app) = seam {
                    link_event(app, event);
                }
                effects
            }
            Ok(In::Line(line)) => answer(&mut controller, line.trim(), revoke),
            Ok(In::Answer(answer)) => match seam {
                Seam::App(app) => app_answer(&mut controller, app, answer),
                Seam::Terminal => vec![],
            },
            Ok(In::Trust {
                identity,
                revokes,
                on_arrival,
            }) => match seam {
                Seam::App(app) => {
                    let stale = revokes != revoke.count();
                    app_trust(&mut controller, app, &identity, stale, on_arrival)
                }
                Seam::Terminal => vec![],
            },
            Ok(In::Stop) | Err(RecvTimeoutError::Disconnected) => {
                apply_with(controller.revoke(), outgoing, seam);
                break;
            }
            Err(RecvTimeoutError::Timeout) => vec![],
        };
        let effects = match seam {
            Seam::Terminal => effects,
            Seam::App(app) => ask_app(&mut controller, app, effects),
        };
        apply_with(effects, outgoing, seam);
        controller.tick(Instant::now());
        if let Seam::App(app) = seam {
            app.show(
                controller.requester(),
                controller.controller(),
                &controller.trusted(),
            );
        }
        if let Some(overlay) = overlay {
            overlay.show(OverlayState {
                asking: controller.requester().map(|g| g.name.clone()),
                controller: controller.controller().map(|g| g.name.clone()),
                pointer: controller.guest_pointer(),
            });
        }
    }
    if let Some(overlay) = overlay {
        overlay.show(OverlayState::default());
    }
    if let Seam::App(app) = seam {
        app.show(None, None, &[]);
    }
    let counters = controller.counters();
    let blocked = controller.injector().blocked();
    tracing::info!(?counters, bad_messages, blocked, "control stopped");
}

fn answer<I: Injector>(
    controller: &mut Controller<Gated<I>>,
    line: &str,
    revoke: &RevokeSwitch,
) -> Vec<Effect> {
    if let Some(guest) = controller.requester().cloned() {
        if matches!(line, "y" | "Y" | "yes") {
            println!(
                "{} now controls this computer. Press {} (or type r and Enter) to take it back.",
                guest.name,
                crate::hotkey::REVOKE
            );
            controller.grant(Instant::now())
        } else {
            println!("Turned down {}.", guest.name);
            controller.deny()
        }
    } else if controller.controller().is_some() && line == "r" {
        // The same switch the hotkey trips, so both take the same path.
        revoke.trip();
        vec![]
    } else {
        vec![]
    }
}

/// Link events the app shows: the connection, and who joined.
fn link_event(app: &mut AppSeam, event: LinkEvent) {
    match event {
        LinkEvent::Connected { .. } | LinkEvent::Reconnected => app.send(Event::Connection {
            state: ConnectionState::Connected,
        }),
        LinkEvent::Reconnecting => app.send(Event::Connection {
            state: ConnectionState::Reconnecting,
        }),
        LinkEvent::Joined { identity, name } => {
            let event = app.people.joined(&identity, &name);
            app.send(event);
        }
        LinkEvent::Sharing(_) | LinkEvent::Data { .. } | LinkEvent::Left(_) => {}
    }
}

/// Turns the controller's [`Effect::Ask`] into a `control_asked` event, numbered so that only an
/// answer to this very request can grant it. Without `allow`, the request is turned down here.
fn ask_app<I: Injector>(
    controller: &mut Controller<Gated<I>>,
    app: &mut AppSeam,
    effects: Vec<Effect>,
) -> Vec<Effect> {
    let mut out = Vec::with_capacity(effects.len());
    for effect in effects {
        let Effect::Ask(guest) = effect else {
            out.push(effect);
            continue;
        };
        if !app.allow {
            tracing::info!(guest = %guest.identity, "control isn't allowed in this call; turned down");
            out.extend(controller.deny());
            continue;
        }
        app.requests += 1;
        let request = app.requests;
        app.open = Some((request, guest.identity.clone()));
        tracing::info!(request, guest = %guest.identity, "asking the app");
        app.send(Event::ControlAsked {
            request,
            identity: guest.identity.clone(),
            name: guest.name.clone(),
            platform: format!("{:?}", guest.platform).to_lowercase(),
        });
    }
    out
}

/// The app answered a request. Only an allow for the request that is still open, from the guest
/// still asking, grants control.
fn app_answer<I: Injector>(
    controller: &mut Controller<Gated<I>>,
    app: &mut AppSeam,
    answer: Answer,
) -> Vec<Effect> {
    let (Answer::Allow { request } | Answer::Deny { request }) = answer;
    let open = match (&app.open, controller.requester()) {
        (Some((open, who)), Some(guest)) => *open == request && *who == guest.identity,
        _ => false,
    };
    if !open {
        app.send(Event::Error {
            message: "That request for control is no longer open.".into(),
        });
        return vec![];
    }
    app.open = None;
    match answer {
        Answer::Allow { .. } => {
            tracing::info!(request, "the app allowed control");
            controller.grant(Instant::now())
        }
        Answer::Deny { .. } => {
            tracing::info!(request, "the app turned control down");
            controller.deny()
        }
    }
}

/// The app trusts `identity` with control for this call (with `on_arrival`, also if they join
/// within a minute). Refused without `--allow-control`, for anyone but a guest, and if control
/// was taken back after the app sent it (`stale`): a revoke always wins over a trust given just
/// before it. [`Controller::trust`] and [`Controller::expect`] check the rest.
fn app_trust<I: Injector>(
    controller: &mut Controller<Gated<I>>,
    app: &mut AppSeam,
    identity: &str,
    stale: bool,
    on_arrival: bool,
) -> Vec<Effect> {
    let refused = if !app.allow {
        Some("This share doesn't allow remote control.")
    } else if stale {
        Some("Control was just taken back, so nobody was given it.")
    } else if role_of(identity) != Role::Guest {
        Some("Only a guest can be given control.")
    } else {
        None
    };
    if let Some(message) = refused {
        tracing::info!(guest = %identity, "{message}");
        app.send(Event::Error {
            message: message.into(),
        });
        return vec![];
    }
    let effects = if on_arrival {
        controller.expect(identity, Instant::now())
    } else {
        controller.trust(identity, Instant::now())
    };
    // A request this granted can't be answered any more.
    if controller.requester().is_none() {
        app.open = None;
    }
    effects
}

fn apply_with(effects: Vec<Effect>, outgoing: &LinkSender, seam: &Seam) {
    let terminal = matches!(seam, Seam::Terminal);
    for effect in effects {
        match effect {
            Effect::Refused(refusal) => refused(&refusal, seam),
            Effect::Send { to, message } => {
                outgoing.send(Outgoing {
                    to: Some(to),
                    payload: protocol::encode(&message),
                    reliable: message.reliable(),
                });
            }
            Effect::Ask(_) | Effect::Ended { .. } if !terminal => {}
            Effect::Ask(guest) => {
                println!(
                    "\n{} ({}, {:?}) asks to control this computer. Allow? [y/N]",
                    guest.name, guest.identity, guest.platform
                );
            }
            Effect::Ended { identity, reason } => {
                println!("Control by {identity} ended: {reason:?}.");
            }
        }
    }
}

/// Tells the sharer why something they asked for wasn't done.
fn refused(refusal: &Refusal, seam: &Seam) {
    tracing::info!("refused: {refusal}");
    match seam {
        Seam::Terminal => println!("{refusal}"),
        Seam::App(app) => app.send(Event::Error {
            message: refusal.to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::{Arc, Mutex};

    type Test = Controller<Gated<RecordInjector<Vec<u8>>>>;

    const SAM: &str = "guest-3f2c8a8e-5b7d-4e8f-9a51-0d6c1e2b7f40";

    /// A controller with Sam in the call, and an app seam whose events land in the list.
    fn setup(allow: bool) -> (Test, AppSeam, Arc<Mutex<Vec<Event>>>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let events: Events = {
            let seen = seen.clone();
            Arc::new(move |e| seen.lock().unwrap().push(e))
        };
        let injector = Gated::new(RecordInjector::new(Vec::new()), RevokeSwitch::default());
        let mut controller = Controller::new(injector, Config::default());
        controller.joined(SAM, "Sam Account", Instant::now());
        controller.joined("sharer-00ff", "Alex", Instant::now());
        controller.joined("recorder-1", "Recorder", Instant::now());
        (controller, AppSeam::new(events, allow), seen)
    }

    fn errors(seen: &Mutex<Vec<Event>>) -> Vec<String> {
        seen.lock()
            .unwrap()
            .drain(..)
            .filter_map(|e| match e {
                Event::Error { message } => Some(message),
                _ => None,
            })
            .collect()
    }

    fn request(controller: &mut Test, from: &str) -> Vec<Effect> {
        controller.handle(from, Message::RequestControl, Instant::now())
    }

    /// Runs the whole control thread over what `inputs` queues, with Sam in the call, until it
    /// stops at the end. Returns the app's events.
    fn control_thread(inputs: impl FnOnce(&SharerHandle, &Sender<In>)) -> Vec<Event> {
        control_thread_from(true, true, inputs)
    }

    /// As [`control_thread`], with Sam in the call from the start or not, and with `stop`
    /// false, ending only when `inputs` sends [`In::Stop`] itself.
    fn control_thread_from(
        sam_first: bool,
        stop: bool,
        inputs: impl FnOnce(&SharerHandle, &Sender<In>),
    ) -> Vec<Event> {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let events: Events = {
            let seen = seen.clone();
            Arc::new(move |e| seen.lock().unwrap().push(e))
        };
        let revoke = RevokeSwitch::default();
        let (tx, rx) = mpsc::channel();
        let (_link, outgoing) = ControlLink::new(protocol::TOPIC, |_| {});
        let injector = Gated::new(RecordInjector::new(Vec::new()), revoke.clone());
        let handle = SharerHandle {
            tx: tx.clone(),
            revoke: revoke.clone(),
        };
        if sam_first {
            tx.send(sam_joins()).unwrap();
        }
        inputs(&handle, &tx);
        if stop {
            tx.send(In::Stop).unwrap();
        }
        let mut seam = Seam::App(AppSeam::new(events, true));
        run(
            Controller::new(injector, Config::default()),
            &rx,
            &outgoing,
            &revoke,
            None,
            &mut seam,
        );
        let seen = seen.lock().unwrap().clone();
        seen
    }

    fn sam_joins() -> In {
        In::Link(LinkEvent::Joined {
            identity: SAM.into(),
            name: "Sam Account".into(),
        })
    }

    fn asked(events: &[Event]) -> bool {
        events
            .iter()
            .any(|e| matches!(e, Event::ControlAsked { identity, .. } if identity == SAM))
    }

    #[test]
    fn trust_on_arrival_trusts_a_guest_who_joins_later() {
        let events = control_thread_from(false, true, |handle, tx| {
            handle.trust_on_arrival(SAM.into());
            tx.send(sam_joins()).unwrap();
            tx.send(from_sam(&Message::RequestControl)).unwrap();
        });
        assert!(
            ever_trusted(&events) && ever_controlling(&events),
            "{events:?}"
        );
        assert!(!asked(&events), "{events:?}");
        // For someone already in the call, it is trust_control.
        let events = control_thread(|handle, tx| {
            handle.trust_on_arrival(SAM.into());
            tx.send(from_sam(&Message::RequestControl)).unwrap();
        });
        assert!(ever_controlling(&events) && !asked(&events), "{events:?}");
    }

    #[test]
    fn any_revoke_before_they_join_cancels_trust_on_arrival() {
        // The hotkey after the command reached the thread, with nobody in control and nobody
        // asking: the switch is tripped from the injector's side, as the hotkey does.
        let events = control_thread_from(false, false, |handle, tx| {
            handle.trust_on_arrival(SAM.into());
            std::thread::spawn({
                let revoke = handle.revoke.clone();
                let tx = tx.clone();
                move || {
                    std::thread::sleep(Duration::from_millis(300));
                    revoke.trip();
                    std::thread::sleep(Duration::from_millis(300));
                    let _ = tx.send(sam_joins());
                    let _ = tx.send(from_sam(&Message::RequestControl));
                    std::thread::sleep(Duration::from_millis(300));
                    let _ = tx.send(In::Stop);
                }
            });
        });
        assert!(!ever_trusted(&events), "{events:?}");
        assert!(!ever_controlling(&events), "{events:?}");
        assert!(asked(&events), "the request is a prompt: {events:?}");

        // A revoke between the app sending it and the thread reading it wins too.
        let events = control_thread_from(false, true, |handle, tx| {
            handle.trust_on_arrival(SAM.into());
            handle.revoke.trip();
            tx.send(sam_joins()).unwrap();
            tx.send(from_sam(&Message::RequestControl)).unwrap();
        });
        assert!(
            !ever_trusted(&events) && !ever_controlling(&events),
            "{events:?}"
        );
        assert!(events.contains(&Event::Error {
            message: "Control was just taken back, so nobody was given it.".into()
        }));
    }

    #[test]
    fn trust_on_arrival_is_refused_as_trust_is() {
        let (mut c, mut app, seen) = setup(false);
        assert!(app_trust(&mut c, &mut app, "guest-later", false, true).is_empty());
        assert_eq!(errors(&seen), ["This share doesn't allow remote control."]);
        assert!(c.expected().is_empty());
        let (mut c, mut app, seen) = setup(true);
        for who in ["sharer-later", "recorder-2"] {
            assert!(app_trust(&mut c, &mut app, who, false, true).is_empty());
            assert_eq!(errors(&seen), ["Only a guest can be given control."]);
        }
        assert!(c.expected().is_empty());
        assert!(app_trust(&mut c, &mut app, "guest-later", false, true).is_empty());
        assert_eq!(c.expected(), ["guest-later"]);
    }

    fn from_sam(message: &Message) -> In {
        In::Link(LinkEvent::Data {
            from: SAM.into(),
            payload: protocol::encode(message),
        })
    }

    fn ever_trusted(events: &[Event]) -> bool {
        events
            .iter()
            .any(|e| matches!(e, Event::Control { trusted, .. } if !trusted.is_empty()))
    }

    fn ever_controlling(events: &[Event]) -> bool {
        events.iter().any(|e| {
            matches!(
                e,
                Event::Control {
                    state: ControlState::Controlling,
                    ..
                }
            )
        })
    }

    #[test]
    fn a_revoke_just_after_a_trust_wins_even_if_the_thread_sees_it_first() {
        // The app sends trust_control, then the sharer presses the hotkey before the control
        // thread got to the trust: the thread carries out the revoke first, then the trust.
        let events = control_thread(|handle, tx| {
            handle.trust(SAM.into());
            handle.revoke.trip();
            tx.send(from_sam(&Message::RequestControl)).unwrap();
        });
        assert!(!ever_trusted(&events), "{events:?}");
        assert!(!ever_controlling(&events), "{events:?}");
        assert!(events.contains(&Event::Error {
            message: "Control was just taken back, so nobody was given it.".into()
        }));
        assert!(events
            .iter()
            .any(|e| matches!(e, Event::ControlAsked { identity, .. } if identity == SAM)));

        // A revoke before the trust doesn't undo it.
        let events = control_thread(|handle, tx| {
            handle.revoke.trip();
            handle.trust(SAM.into());
            tx.send(from_sam(&Message::RequestControl)).unwrap();
        });
        assert!(
            ever_trusted(&events) && ever_controlling(&events),
            "{events:?}"
        );
        assert!(!events
            .iter()
            .any(|e| matches!(e, Event::ControlAsked { .. })));
    }

    #[test]
    fn the_control_thread_grants_a_trusted_request_under_the_tokens_name() {
        let events = control_thread(|handle, tx| {
            tx.send(from_sam(&Message::Hello {
                name: "Mallory".into(),
                platform: protocol::Platform::Macos,
                decoders: vec![],
            }))
            .unwrap();
            handle.trust(SAM.into());
            tx.send(from_sam(&Message::RequestControl)).unwrap();
        });
        let sam = Peer {
            identity: SAM.into(),
            name: "Sam Account".into(),
        };
        assert!(
            events.contains(&Event::Control {
                state: ControlState::Controlling,
                by: Some(sam.clone()),
                trusted: vec![sam],
            }),
            "{events:?}"
        );
        // The thread stops with a revoke: control and trust end, and the app is told.
        assert_eq!(
            events.last(),
            Some(&Event::Control {
                state: ControlState::None,
                by: None,
                trusted: vec![],
            })
        );
    }

    #[test]
    fn the_app_can_only_trust_a_guest_in_a_share_that_allows_control() {
        let (mut c, mut app, seen) = setup(false);
        assert!(app_trust(&mut c, &mut app, SAM, false, false).is_empty());
        assert_eq!(errors(&seen), ["This share doesn't allow remote control."]);
        assert!(c.trusted().is_empty());

        let (mut c, mut app, seen) = setup(true);
        for who in ["sharer-00ff", "recorder-1"] {
            assert!(app_trust(&mut c, &mut app, who, false, false).is_empty());
            assert_eq!(errors(&seen), ["Only a guest can be given control."]);
        }
        // A revoke after the app sent it wins.
        assert!(app_trust(&mut c, &mut app, SAM, true, false).is_empty());
        assert_eq!(
            errors(&seen),
            ["Control was just taken back, so nobody was given it."]
        );
        assert!(c.trusted().is_empty());
        // Someone not in the call: refused by the controller, told to the app as an error.
        assert_eq!(
            app_trust(&mut c, &mut app, "guest-gone", false, false),
            vec![Effect::Refused(Refusal::NotInCall)]
        );
        assert!(c.trusted().is_empty());

        assert!(app_trust(&mut c, &mut app, SAM, false, false).is_empty());
        assert_eq!(c.trusted().len(), 1);
        assert!(errors(&seen).is_empty());
    }

    #[test]
    fn a_trusted_request_skips_the_prompt_and_a_trusted_pending_one_closes_it() {
        // Trusted first: the request is granted with no control_asked and no number.
        let (mut c, mut app, seen) = setup(true);
        app_trust(&mut c, &mut app, SAM, false, false);
        let effects = request(&mut c, SAM);
        let effects = ask_app(&mut c, &mut app, effects);
        assert_eq!(
            effects,
            vec![Effect::Send {
                to: SAM.into(),
                message: Message::Grant
            }]
        );
        assert_eq!(app.requests, 0);
        assert!(!seen
            .lock()
            .unwrap()
            .iter()
            .any(|e| matches!(e, Event::ControlAsked { .. })));

        // Asked first: trusting grants it, and the prompt's number can't be used any more.
        let (mut c, mut app, seen) = setup(true);
        let effects = request(&mut c, SAM);
        ask_app(&mut c, &mut app, effects);
        let asked = seen.lock().unwrap().drain(..).find_map(|e| match e {
            Event::ControlAsked { request, name, .. } => Some((request, name)),
            _ => None,
        });
        assert_eq!(asked, Some((1, "Sam Account".to_owned())));
        assert_eq!(
            app_trust(&mut c, &mut app, SAM, false, false),
            vec![Effect::Send {
                to: SAM.into(),
                message: Message::Grant
            }]
        );
        assert!(app.open.is_none());
        assert!(app_answer(&mut c, &mut app, Answer::Allow { request: 1 }).is_empty());
        assert_eq!(
            errors(&seen),
            ["That request for control is no longer open."]
        );
        assert_eq!(c.controller().map(|g| g.identity.as_str()), Some(SAM));
    }

    #[test]
    fn the_control_event_lists_who_is_trusted() {
        let (mut c, mut app, seen) = setup(true);
        let show =
            |c: &Test, app: &mut AppSeam| app.show(c.requester(), c.controller(), &c.trusted());
        show(&c, &mut app);
        app_trust(&mut c, &mut app, SAM, false, false);
        show(&c, &mut app);
        // Nothing changed: nothing sent.
        show(&c, &mut app);
        c.revoke();
        show(&c, &mut app);
        let sam = Peer {
            identity: SAM.into(),
            name: "Sam Account".into(),
        };
        let none = |trusted: Vec<Peer>| Event::Control {
            state: ControlState::None,
            by: None,
            trusted,
        };
        assert_eq!(
            *seen.lock().unwrap(),
            [none(vec![]), none(vec![sam]), none(vec![])]
        );
    }

    #[test]
    fn parses_inject_modes() {
        assert!(matches!("log".parse(), Ok(Inject::Log)));
        assert!(matches!("real".parse(), Ok(Inject::Real)));
        assert!(
            matches!("record:a.jsonl".parse(), Ok(Inject::Record(p)) if p == std::path::Path::new("a.jsonl"))
        );
        assert!("record:".parse::<Inject>().is_err());
        assert!("enigo".parse::<Inject>().is_err());
    }
}
