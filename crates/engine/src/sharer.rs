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

/// What the share shows, which decides where granted input may go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Surface {
    /// A whole display, by id (`CGDirectDisplayID`): the one being captured
    /// ([`crate::screen::choose`]). Real input is mapped onto it and no other.
    Display(u32),
    /// One window, or one app's windows. Input would be mapped onto a whole display, not onto
    /// the window in the picture, so no control: every request is turned down and every trust
    /// refused with [`Refusal::OneWindowShared`], and nothing is injected.
    Window,
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
        surface: Surface,
        revoke: RevokeSwitch,
        overlay: Option<OverlayHandle>,
        consent: Consent,
    ) -> Result<(Self, ControlLink), Error> {
        // Logging and recording need no permission, so they start now. Real input waits for the
        // first grant: sharing with --allow-control needs no Accessibility until then.
        let injector = match inject {
            Inject::Log => Lazy::now(Box::new(LogInjector)),
            Inject::Record(path) => {
                Lazy::now(Box::new(RecordInjector::new(std::fs::File::create(path)?)))
            }
            #[cfg(target_os = "macos")]
            Inject::Real => Lazy::later(Box::new(move || {
                use control::macos::{MacError, MacInjector};
                // A window share is never granted control, so this is never made for one.
                let Surface::Display(display) = surface else {
                    return Err(Refusal::OneWindowShared.to_string());
                };
                match MacInjector::new(display) {
                    Ok(injector) => Ok(Box::new(injector) as Box<dyn Injector + Send>),
                    Err(MacError::NoPermission) => Err(NEEDS_ACCESSIBILITY.to_owned()),
                    Err(e) => Err(format!("Remote control couldn't start: {e}.")),
                }
            })),
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
                    controller_for(surface, injector),
                    &rx,
                    &outgoing,
                    &revoke,
                    overlay.as_ref(),
                    &mut seam,
                )
            })?;
        if terminal {
            match surface {
                Surface::Display(_) => println!(
                    "Remote control is allowed to be requested. You'll be asked before anyone \
                     gets it, and {} takes it back at any time.",
                    crate::hotkey::REVOKE
                ),
                Surface::Window => println!("{}", Refusal::OneWindowShared),
            }
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

/// The controller for a share of `surface`: a window share's is closed from the start.
fn controller_for<I: Injector>(surface: Surface, injector: I) -> Controller<I> {
    let mut controller = Controller::new(injector, Config::default());
    if surface == Surface::Window {
        // Nobody has joined yet, so there is nothing to send or show.
        let _ = controller.close(Refusal::OneWindowShared);
    }
    controller
}

/// Why a grant didn't happen, when real input can't be injected yet.
pub const NEEDS_ACCESSIBILITY: &str =
    "TwinCursor needs Accessibility to let someone control this Mac.";

/// Makes the injector, or says in a sentence why it can't.
type MakeInjector = Box<dyn FnMut() -> Result<Box<dyn Injector + Send>, String> + Send>;

/// The injector, made the first time control is given (or a guest is trusted with it) rather
/// than at start, so that sharing with `--allow-control` asks for no permission until then.
/// Every grant first calls [`Lazy::ensure`] and doesn't happen if it fails; once made, the
/// injector stays for the rest of the share. Revoking never depends on it: the revoke switch and
/// hotkey are set up at start, as before.
struct Lazy {
    injector: Option<Box<dyn Injector + Send>>,
    make: Option<MakeInjector>,
}

impl Lazy {
    fn now(injector: Box<dyn Injector + Send>) -> Self {
        Self {
            injector: Some(injector),
            make: None,
        }
    }

    // Real input is made later only where it exists (macOS); the tests make one too.
    #[cfg(any(target_os = "macos", test))]
    fn later(make: MakeInjector) -> Self {
        Self {
            injector: None,
            make: Some(make),
        }
    }

    /// Makes the injector if it isn't made yet. Tried again on the next grant if it fails, so a
    /// permission given mid-call works without starting again.
    fn ensure(&mut self) -> Result<(), String> {
        if self.injector.is_some() {
            return Ok(());
        }
        let make = self
            .make
            .as_mut()
            .ok_or_else(|| "Remote control couldn't start.".to_owned())?;
        let injector = make()?;
        tracing::info!("input injection ready");
        self.injector = Some(injector);
        Ok(())
    }

    fn ready(&self) -> bool {
        self.injector.is_some()
    }
}

impl Injector for Lazy {
    fn inject(&mut self, action: &control::Action) {
        match &mut self.injector {
            Some(injector) => injector.inject(action),
            // Can't happen: nothing is granted before the injector exists.
            None => tracing::error!(?action, "no injector yet; dropped"),
        }
    }

    fn pointer(&self) -> Option<control::Point> {
        self.injector.as_ref().and_then(|i| i.pointer())
    }

    fn display_gone(&mut self) -> bool {
        self.injector.as_mut().is_some_and(|i| i.display_gone())
    }
}

/// Makes sure input can be injected before a grant.
fn can_inject(controller: &mut Controller<Gated<Lazy>>) -> Result<(), String> {
    controller.injector_mut().inner_mut().ensure()
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

fn run(
    mut controller: Controller<Gated<Lazy>>,
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
        // Every grant comes after `can_inject` (a trusted guest's too: trusting needs it, and so
        // does trust on arrival, before the guest joins). Should one ever not, it is taken back
        // before the guest or the app hears of it, however the guest came to be granted.
        let effects = if controller.controller().is_some() && !controller.injector().inner().ready()
        {
            tracing::error!("control granted with no injector; taken back");
            controller.revoke()
        } else {
            effects
        };
        apply_with(effects, outgoing, seam);
        // The shared display going away takes control back here, and the sharer is told.
        let effects = controller.tick(Instant::now());
        apply_with(effects, outgoing, seam);
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

fn answer(
    controller: &mut Controller<Gated<Lazy>>,
    line: &str,
    revoke: &RevokeSwitch,
) -> Vec<Effect> {
    if let Some(guest) = controller.requester().cloned() {
        if matches!(line, "y" | "Y" | "yes") {
            if let Err(message) = can_inject(controller) {
                println!(
                    "{message} Allow it in System Settings → Privacy & Security → \
                     Accessibility (for tc-spike, the terminal you run it from). Turned down {}.",
                    guest.name
                );
                return controller.deny();
            }
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
fn ask_app(
    controller: &mut Controller<Gated<Lazy>>,
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
fn app_answer(
    controller: &mut Controller<Gated<Lazy>>,
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
            if let Err(message) = can_inject(controller) {
                tracing::warn!(request, "can't inject: {message}");
                app.send(Event::Error { message });
                return controller.deny();
            }
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
fn app_trust(
    controller: &mut Controller<Gated<Lazy>>,
    app: &mut AppSeam,
    identity: &str,
    stale: bool,
    on_arrival: bool,
) -> Vec<Effect> {
    let refused = if !app.allow {
        Some("This share doesn't allow remote control.".to_owned())
    } else if let Some(why) = controller.unavailable() {
        // Before making the injector: a window share never needs one.
        Some(why.to_string())
    } else if stale {
        Some("Control was just taken back, so nobody was given it.".to_owned())
    } else if role_of(identity) != Role::Guest {
        Some("Only a guest can be given control.".to_owned())
    } else {
        None
    };
    if let Some(message) = refused {
        tracing::info!(guest = %identity, "{message}");
        app.send(Event::Error { message });
        return vec![];
    }
    // Trust is a grant made in advance, and trust on arrival a trust made in advance: both need
    // injection to be possible as much as a grant does. Without it, nobody is trusted or
    // expected, and a request this would have granted is turned down.
    if let Err(message) = can_inject(controller) {
        tracing::warn!(guest = %identity, "can't inject: {message}");
        app.send(Event::Error { message });
        if controller
            .requester()
            .is_some_and(|g| g.identity == identity)
        {
            app.open = None;
            return controller.deny();
        }
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

    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    type Test = Controller<Gated<Lazy>>;

    const SAM: &str = "guest-3f2c8a8e-5b7d-4e8f-9a51-0d6c1e2b7f40";

    fn recorder() -> Lazy {
        Lazy::now(Box::new(RecordInjector::new(Vec::new())))
    }

    /// An injector made on the first grant, as real input is, which can only be made while
    /// `allowed` (Accessibility, in a real one). `made` counts how often it was made.
    fn later(allowed: &Arc<AtomicBool>, made: &Arc<AtomicUsize>) -> Lazy {
        let (allowed, made) = (allowed.clone(), made.clone());
        Lazy::later(Box::new(move || {
            if !allowed.load(Ordering::SeqCst) {
                return Err(NEEDS_ACCESSIBILITY.to_owned());
            }
            made.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(RecordInjector::new(Vec::new())) as Box<dyn Injector + Send>)
        }))
    }

    /// A controller with Sam in the call, and an app seam whose events land in the list.
    fn setup(allow: bool) -> (Test, AppSeam, Arc<Mutex<Vec<Event>>>) {
        setup_with(allow, recorder())
    }

    fn setup_with(allow: bool, injector: Lazy) -> (Test, AppSeam, Arc<Mutex<Vec<Event>>>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let events: Events = {
            let seen = seen.clone();
            Arc::new(move |e| seen.lock().unwrap().push(e))
        };
        let injector = Gated::new(injector, RevokeSwitch::default());
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
        control_thread_in(recorder(), sam_first, stop, inputs)
    }

    /// As [`control_thread_from`] with `stop`, over `injector`.
    fn control_thread_with(
        injector: Lazy,
        sam_first: bool,
        inputs: impl FnOnce(&SharerHandle, &Sender<In>),
    ) -> Vec<Event> {
        control_thread_in(injector, sam_first, true, inputs)
    }

    fn control_thread_in(
        injector: Lazy,
        sam_first: bool,
        stop: bool,
        inputs: impl FnOnce(&SharerHandle, &Sender<In>),
    ) -> Vec<Event> {
        control_thread_on(Surface::Display(1), injector, sam_first, stop, inputs)
    }

    /// As [`control_thread_in`], for a share of `surface`.
    fn control_thread_on(
        surface: Surface,
        injector: Lazy,
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
        let injector = Gated::new(injector, revoke.clone());
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
            controller_for(surface, injector),
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
    fn the_app_knows_this_sentence() {
        // The call bar tells this refusal apart from its own (apps/desktop/src/permissions.ts).
        let ui = include_str!("../../../apps/desktop/src/permissions.ts");
        assert!(ui.contains(&format!("\"{NEEDS_ACCESSIBILITY}\"")));
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

    fn deny_to_sam() -> Vec<Effect> {
        vec![Effect::Send {
            to: SAM.into(),
            message: Message::Deny { reason: None },
        }]
    }

    fn grant_to_sam() -> Vec<Effect> {
        vec![Effect::Send {
            to: SAM.into(),
            message: Message::Grant,
        }]
    }

    fn ask(c: &mut Test, app: &mut AppSeam) -> u64 {
        let effects = request(c, SAM);
        assert!(ask_app(c, app, effects).is_empty());
        app.requests
    }

    #[test]
    fn real_input_is_set_up_at_the_first_grant_not_at_start() {
        let (allowed, made) = (
            Arc::new(AtomicBool::new(true)),
            Arc::new(AtomicUsize::new(0)),
        );
        let (mut c, mut app, _seen) = setup_with(true, later(&allowed, &made));
        // Sharing, asking and turning down need no injector.
        let n = ask(&mut c, &mut app);
        app_answer(&mut c, &mut app, Answer::Deny { request: n });
        assert_eq!(made.load(Ordering::SeqCst), 0);
        // The first allow makes it, and later grants use the same one.
        let n = ask(&mut c, &mut app);
        assert_eq!(
            app_answer(&mut c, &mut app, Answer::Allow { request: n }),
            grant_to_sam()
        );
        assert_eq!(made.load(Ordering::SeqCst), 1);
        c.revoke();
        let n = ask(&mut c, &mut app);
        app_answer(&mut c, &mut app, Answer::Allow { request: n });
        assert_eq!(made.load(Ordering::SeqCst), 1);

        // Trusting makes it too, before anyone asks.
        let (allowed, made) = (
            Arc::new(AtomicBool::new(true)),
            Arc::new(AtomicUsize::new(0)),
        );
        let (mut c, mut app, _seen) = setup_with(true, later(&allowed, &made));
        app_trust(&mut c, &mut app, SAM, false, false);
        assert_eq!(made.load(Ordering::SeqCst), 1);
        assert_eq!(request(&mut c, SAM), grant_to_sam());
    }

    #[test]
    fn without_accessibility_nothing_is_granted_or_trusted() {
        let (allowed, made) = (
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicUsize::new(0)),
        );
        let (mut c, mut app, seen) = setup_with(true, later(&allowed, &made));

        // An allow: the guest is turned down and the app told why.
        let n = ask(&mut c, &mut app);
        errors(&seen);
        assert_eq!(
            app_answer(&mut c, &mut app, Answer::Allow { request: n }),
            deny_to_sam()
        );
        assert_eq!(errors(&seen), [NEEDS_ACCESSIBILITY]);
        assert!(c.controller().is_none() && c.requester().is_none());
        // That request is over: allowing it again does nothing.
        assert!(app_answer(&mut c, &mut app, Answer::Allow { request: n }).is_empty());
        assert_eq!(
            errors(&seen),
            ["That request for control is no longer open."]
        );

        // A trust with nobody asking: nobody is trusted.
        assert!(app_trust(&mut c, &mut app, SAM, false, false).is_empty());
        assert_eq!(errors(&seen), [NEEDS_ACCESSIBILITY]);
        assert!(c.trusted().is_empty());
        let asked = request(&mut c, SAM);
        assert!(!ask_app(&mut c, &mut app, asked).iter().any(|e| matches!(
            e,
            Effect::Send {
                message: Message::Grant,
                ..
            }
        )));
        assert!(c.requester().is_some(), "a normal prompt, not a grant");

        // A trust for the guest who is asking: they are turned down, and the prompt is over.
        let n = app.requests;
        assert_eq!(
            app_trust(&mut c, &mut app, SAM, false, false),
            deny_to_sam()
        );
        assert_eq!(errors(&seen), [NEEDS_ACCESSIBILITY]);
        assert!(c.trusted().is_empty() && c.requester().is_none());
        assert!(app.open.is_none());
        assert!(app_answer(&mut c, &mut app, Answer::Allow { request: n }).is_empty());
        assert_eq!(made.load(Ordering::SeqCst), 0);

        // The sharer gives the permission mid-call: the next allow works.
        allowed.store(true, Ordering::SeqCst);
        let n = ask(&mut c, &mut app);
        assert_eq!(
            app_answer(&mut c, &mut app, Answer::Allow { request: n }),
            grant_to_sam()
        );
        assert_eq!(made.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_grant_with_no_injector_is_taken_back_before_anyone_hears_of_it() {
        // Not reachable through the app (every grant checks first); the control thread checks
        // anyway. Granting straight through the controller skips the check.
        let (allowed, made) = (
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicUsize::new(0)),
        );
        let seen = Arc::new(Mutex::new(Vec::new()));
        let events: Events = {
            let seen = seen.clone();
            Arc::new(move |e| seen.lock().unwrap().push(e))
        };
        let revoke = RevokeSwitch::default();
        let (tx, rx) = mpsc::channel();
        let (_link, outgoing) = ControlLink::new(protocol::TOPIC, |_| {});
        let mut controller = Controller::new(
            Gated::new(later(&allowed, &made), revoke.clone()),
            Config::default(),
        );
        controller.joined(SAM, "Sam Account", Instant::now());
        request(&mut controller, SAM);
        controller.grant(Instant::now());
        tx.send(In::Stop).unwrap();
        run(
            controller,
            &rx,
            &outgoing,
            &revoke,
            None,
            &mut Seam::App(AppSeam::new(events, true)),
        );
        let seen = seen.lock().unwrap();
        assert!(!ever_controlling(&seen), "{seen:?}");
    }

    /// Sam joining, then asking, on a control thread whose injector `later` makes, with
    /// `before` run on the controller first. Returns the app's events.
    fn arrive_and_ask(injector: Lazy, before: impl FnOnce(&mut Test)) -> Vec<Event> {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let events: Events = {
            let seen = seen.clone();
            Arc::new(move |e| seen.lock().unwrap().push(e))
        };
        let revoke = RevokeSwitch::default();
        let (tx, rx) = mpsc::channel();
        let (_link, outgoing) = ControlLink::new(protocol::TOPIC, |_| {});
        let mut controller =
            Controller::new(Gated::new(injector, revoke.clone()), Config::default());
        before(&mut controller);
        tx.send(sam_joins()).unwrap();
        tx.send(from_sam(&Message::RequestControl)).unwrap();
        tx.send(In::Stop).unwrap();
        run(
            controller,
            &rx,
            &outgoing,
            &revoke,
            None,
            &mut Seam::App(AppSeam::new(events, true)),
        );
        let seen = seen.lock().unwrap().clone();
        seen
    }

    #[test]
    fn without_accessibility_trust_on_arrival_expects_nobody() {
        let (allowed, made) = (
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicUsize::new(0)),
        );
        let (mut c, mut app, seen) = setup_with(true, later(&allowed, &made));
        assert!(app_trust(&mut c, &mut app, "guest-later", false, true).is_empty());
        assert_eq!(errors(&seen), [NEEDS_ACCESSIBILITY]);
        assert!(c.expected().is_empty() && c.trusted().is_empty());
        // They join and ask: a normal prompt, not a grant.
        assert!(c.joined("guest-later", "Later", Instant::now()).is_empty());
        assert!(c.trusted().is_empty());
        let asked = request(&mut c, "guest-later");
        assert!(!ask_app(&mut c, &mut app, asked).iter().any(|e| matches!(
            e,
            Effect::Send {
                message: Message::Grant,
                ..
            }
        )));
        assert!(c.requester().is_some() && c.controller().is_none());
        // For someone already in the call (trust_control by another name), the same.
        assert!(app_trust(&mut c, &mut app, SAM, false, true).is_empty());
        assert_eq!(errors(&seen), [NEEDS_ACCESSIBILITY]);
        assert!(c.trusted().is_empty());
        assert_eq!(made.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn trust_on_arrival_makes_the_injector_before_the_guest_joins() {
        let (allowed, made) = (
            Arc::new(AtomicBool::new(true)),
            Arc::new(AtomicUsize::new(0)),
        );
        let (mut c, mut app, seen) = setup_with(true, later(&allowed, &made));
        assert!(app_trust(&mut c, &mut app, "guest-later", false, true).is_empty());
        assert!(errors(&seen).is_empty());
        assert_eq!(c.expected(), ["guest-later"]);
        // Made now, while the sharer is at the screen, not when the guest turns up.
        assert_eq!(made.load(Ordering::SeqCst), 1);
        assert!(c.injector().inner().ready());
        assert!(c.joined("guest-later", "Later", Instant::now()).is_empty());
        assert!(c.trusted().iter().any(|g| g.identity == "guest-later"));
        assert_eq!(
            request(&mut c, "guest-later"),
            vec![Effect::Send {
                to: "guest-later".into(),
                message: Message::Grant,
            }]
        );
        assert_eq!(made.load(Ordering::SeqCst), 1);

        // The same over the whole control thread.
        let (allowed, made) = (
            Arc::new(AtomicBool::new(true)),
            Arc::new(AtomicUsize::new(0)),
        );
        let events = control_thread_with(later(&allowed, &made), false, |handle, tx| {
            handle.trust_on_arrival(SAM.into());
            tx.send(sam_joins()).unwrap();
            tx.send(from_sam(&Message::RequestControl)).unwrap();
        });
        assert!(ever_controlling(&events) && !asked(&events), "{events:?}");
        assert_eq!(made.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_guest_trusted_on_arrival_with_no_injector_is_never_granted() {
        // Not reachable through the app (trust on arrival checks first); expecting straight
        // through the controller skips the check, and the control thread takes the grant back.
        let (allowed, made) = (
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicUsize::new(0)),
        );
        let events = arrive_and_ask(later(&allowed, &made), |c| {
            assert!(c.expect(SAM, Instant::now()).is_empty());
        });
        assert!(!ever_controlling(&events), "{events:?}");
        assert_eq!(made.load(Ordering::SeqCst), 0);
    }

    const ONE_WINDOW: &str = "Control isn't available while one window is shared.";

    #[test]
    fn a_window_share_gives_nobody_control_and_says_why() {
        let (allowed, made) = (
            Arc::new(AtomicBool::new(true)),
            Arc::new(AtomicUsize::new(0)),
        );
        let events = control_thread_on(
            Surface::Window,
            later(&allowed, &made),
            true,
            true,
            |handle, tx| {
                tx.send(from_sam(&Message::RequestControl)).unwrap();
                handle.trust(SAM.into());
                handle.trust_on_arrival(SAM.into());
                handle.trust_on_arrival("guest-later".into());
                tx.send(from_sam(&Message::RequestControl)).unwrap();
                // An allow for the request that was never asked about grants nothing.
                handle.answer(Answer::Allow { request: 1 });
            },
        );
        assert!(!asked(&events), "nobody is asked: {events:?}");
        assert!(
            !ever_controlling(&events) && !ever_trusted(&events),
            "{events:?}"
        );
        let errors: Vec<&str> = events
            .iter()
            .filter_map(|e| match e {
                Event::Error { message } => Some(message.as_str()),
                _ => None,
            })
            .collect();
        // Two requests, two trusts and a trust on arrival for each: the same sentence each time.
        // The stale allow is the usual error.
        assert_eq!(
            errors,
            [
                ONE_WINDOW,
                ONE_WINDOW,
                ONE_WINDOW,
                ONE_WINDOW,
                ONE_WINDOW,
                "That request for control is no longer open."
            ],
            "{events:?}"
        );
        assert_eq!(
            made.load(Ordering::SeqCst),
            0,
            "no injector, so no Accessibility"
        );
    }

    #[test]
    fn a_window_share_refuses_trust_before_anything_else_but_allow() {
        let (mut c, mut app, seen) = setup(true);
        let _ = c.close(Refusal::OneWindowShared);
        for on_arrival in [false, true] {
            assert!(app_trust(&mut c, &mut app, SAM, false, on_arrival).is_empty());
            assert_eq!(errors(&seen), [ONE_WINDOW]);
        }
        // A request is denied to Sam with the reason, and the app hears the same.
        let effects = request(&mut c, SAM);
        assert_eq!(
            effects,
            vec![
                Effect::Send {
                    to: SAM.into(),
                    message: Message::Deny {
                        reason: Some(ONE_WINDOW.into())
                    },
                },
                Effect::Refused(Refusal::OneWindowShared),
            ]
        );
        // Without --allow-control, that is still what the app hears about trust.
        let (mut c, mut app, seen) = setup(false);
        let _ = c.close(Refusal::OneWindowShared);
        app_trust(&mut c, &mut app, SAM, false, false);
        assert_eq!(errors(&seen), ["This share doesn't allow remote control."]);
    }

    /// Records into `actions`, and loses its display when `gone` is set.
    struct Vanishing {
        gone: Arc<AtomicBool>,
        actions: Arc<Mutex<Vec<control::Action>>>,
    }

    impl Injector for Vanishing {
        fn inject(&mut self, action: &control::Action) {
            self.actions.lock().unwrap().push(action.clone());
        }

        fn pointer(&self) -> Option<control::Point> {
            None
        }

        fn display_gone(&mut self) -> bool {
            self.gone.load(Ordering::SeqCst)
        }
    }

    #[test]
    fn the_shared_display_going_away_takes_control_back_and_the_app_is_told() {
        let gone = Arc::new(AtomicBool::new(false));
        let actions = Arc::new(Mutex::new(Vec::new()));
        let injector = Lazy::now(Box::new(Vanishing {
            gone: gone.clone(),
            actions: actions.clone(),
        }));
        let typed = |code: &str, pressed: bool, seq: u64| {
            from_sam(&Message::Input {
                seq,
                sent_at_us: seq * 1_000,
                after: 0,
                event: protocol::InputEvent::Key {
                    code: code.into(),
                    pressed,
                    text: None,
                },
            })
        };
        let events = control_thread_in(injector, true, false, |handle, tx| {
            handle.trust(SAM.into());
            tx.send(from_sam(&Message::RequestControl)).unwrap();
            tx.send(typed("ShiftLeft", true, 1)).unwrap();
            let (tx, gone) = (tx.clone(), gone.clone());
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(200));
                gone.store(true, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(200));
                // Late input, a new request and a trust: none of it does anything.
                let _ = tx.send(typed("KeyA", true, 2));
                let _ = tx.send(from_sam(&Message::RequestControl));
                std::thread::sleep(Duration::from_millis(100));
                let _ = tx.send(In::Stop);
            });
        });
        let controlling = events
            .iter()
            .position(|e| {
                matches!(
                    e,
                    Event::Control {
                        state: ControlState::Controlling,
                        ..
                    }
                )
            })
            .expect("Sam had control");
        let none = Event::Control {
            state: ControlState::None,
            by: None,
            trusted: vec![],
        };
        assert!(events[controlling..].contains(&none), "{events:?}");
        let gone_sentence = "The shared display is gone, so control isn't available.";
        let told = events
            .iter()
            .filter(|e| matches!(e, Event::Error { message } if message == gone_sentence))
            .count();
        assert_eq!(
            told, 2,
            "once when it went, once for the later request: {events:?}"
        );
        assert!(!asked(&events), "{events:?}");
        // Shift went down while the display was there, and came up when control was taken
        // back. Nothing after.
        let key = |code: &str, pressed| control::Action::Key {
            code: code.into(),
            pressed,
        };
        assert_eq!(
            *actions.lock().unwrap(),
            [key("ShiftLeft", true), key("ShiftLeft", false)]
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
