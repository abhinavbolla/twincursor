//! The sharer's side of remote control (`tc-spike share --allow-control`, `tc-engine share`).
//!
//! One thread owns the [`Controller`]: it takes link events and the sharer's answers, ticks the
//! deadman, and sends what the controller says to. Where the sharer is asked is the [`Consent`]
//! seam: `tc-spike` asks in its terminal (type `y` to allow), `tc-engine` asks the desktop app
//! with a `control_asked` event and waits for an `allow_control` command naming that request.
//! Nothing else grants control: no answer means no control.

use std::io::BufRead;
use std::path::PathBuf;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use control::{
    Config, Controller, Effect, Gated, Guest, Injector, LogInjector, RecordInjector, RevokeSwitch,
};
use ipc::{ConnectionState, ControlState, Event, Peer};
use media::link::{ControlLink, LinkEvent, LinkSender, Outgoing};
use protocol::Message;

use crate::app::{Events, People};
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
}

impl SharerHandle {
    /// Hands the app's answer to the control thread. Only an [`Answer::Allow`] for the request
    /// still open grants control.
    pub fn answer(&self, answer: Answer) {
        let _ = self.tx.send(In::Answer(answer));
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
        Ok((
            Self {
                handle: SharerHandle { tx },
                thread,
            },
            link,
        ))
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
    shown: Option<(ControlState, Option<Peer>)>,
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

    /// Sends the control state if it changed.
    fn show(&mut self, requester: Option<&Guest>, controller: Option<&Guest>) {
        let peer = |g: &Guest| Peer {
            identity: g.identity.clone(),
            name: g.name.clone(),
        };
        let now = match (controller, requester) {
            (Some(g), _) => (ControlState::Controlling, Some(peer(g))),
            (None, Some(g)) => (ControlState::Asked, Some(peer(g))),
            (None, None) => (ControlState::None, None),
        };
        if self.shown.as_ref() != Some(&now) {
            self.shown = Some(now.clone());
            let (state, by) = now;
            self.send(Event::Control { state, by });
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
        app.show(None, None);
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
                if let Seam::App(app) = seam {
                    link_event(app, event);
                }
                vec![]
            }
            Ok(In::Line(line)) => answer(&mut controller, line.trim(), revoke),
            Ok(In::Answer(answer)) => match seam {
                Seam::App(app) => app_answer(&mut controller, app, answer),
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
            app.show(controller.requester(), controller.controller());
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
        app.show(None, None);
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

fn apply_with(effects: Vec<Effect>, outgoing: &LinkSender, seam: &Seam) {
    let terminal = matches!(seam, Seam::Terminal);
    for effect in effects {
        match effect {
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

#[cfg(test)]
mod tests {
    use super::*;

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
