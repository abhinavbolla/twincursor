//! The guest's side of remote control in the viewer.
//!
//! One thread owns the [`GuestSession`]. The window (or a script) hands it input, the link hands
//! it the sharer's answers, and it sends to the sharer: the participant whose identity says so
//! (`app::role_of`), found when they join, or failing that the one whose screen we receive.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use control::guest::{GuestSession, GuestState};
use ipc::{ConnectionState, ControlState, Event, Peer, Role};
use media::link::{ControlLink, LinkEvent, LinkSender, Outgoing};
use protocol::{InputEvent, Message, Platform};
use serde::Deserialize;

use crate::app::{role_of, Events, People};
use crate::{Error, ScriptError};

const TICK: Duration = Duration::from_millis(4);
/// A viewport has to hold this long before it's reported, so dragging a window's edge sends
/// one report after the drag rather than one per step.
pub const VIEWPORT_SETTLE: Duration = Duration::from_millis(300);
/// The viewport is sent again this often. Nothing confirms it arrived; a repeat is a few dozen
/// bytes.
pub const VIEWPORT_REPEAT: Duration = Duration::from_secs(2);
/// Until the sharer acknowledges the hello, it's offered again this often...
const HELLO_RETRY: Duration = Duration::from_millis(250);
/// ...for this long, and then every [`HELLO_SLOW_RETRY`]: a sharer that predates
/// `Message::HelloAck` never answers.
const HELLO_FAST_FOR: Duration = Duration::from_secs(15);
const HELLO_SLOW_RETRY: Duration = Duration::from_secs(2);

/// What the window or a script asks of the guest session.
#[derive(Debug, Clone)]
pub enum Ui {
    Input(InputEvent),
    /// Ask for control, or give it back.
    Toggle,
    Request,
    Release,
    /// The largest area, in physical pixels, the picture can take here. Reported to the sharer
    /// once it has held for [`VIEWPORT_SETTLE`], and at once to a sharer just found.
    Viewport((u32, u32)),
}

/// What this viewer has told the sharer about its viewport, and what it will tell next.
#[derive(Debug, Default)]
struct ViewportReport {
    /// The newest viewport, and since when.
    latest: Option<((u32, u32), Instant)>,
    /// The last one sent to the current sharer, and when.
    sent: Option<((u32, u32), Instant)>,
}

impl ViewportReport {
    fn set(&mut self, viewport: (u32, u32), now: Instant) {
        let clamp = |v: u32| v.clamp(protocol::MIN_VIEWPORT, protocol::MAX_VIEWPORT);
        let viewport = (clamp(viewport.0), clamp(viewport.1));
        if self.latest.map(|(v, _)| v) != Some(viewport) {
            self.latest = Some((viewport, now));
        }
    }

    /// A viewport to send now: a new one that has held long enough, or the same one again
    /// after [`VIEWPORT_REPEAT`].
    fn due(&mut self, now: Instant) -> Option<Message> {
        let (viewport, since) = self.latest?;
        let repeat = match self.sent {
            Some((sent, at)) if sent == viewport => now.duration_since(at) >= VIEWPORT_REPEAT,
            _ => now.duration_since(since) >= VIEWPORT_SETTLE,
        };
        repeat.then(|| self.send(viewport, now))
    }

    /// A sharer was just found: tell it the newest viewport without waiting.
    fn fresh(&mut self, now: Instant) -> Option<Message> {
        let (viewport, _) = self.latest?;
        Some(self.send(viewport, now))
    }

    /// The last viewport sent, again: with each offer of the hello, and once the sharer is known
    /// to hear us, since what went before may have been dropped. A newer one still waits to
    /// settle.
    fn again(&mut self, now: Instant) -> Option<Message> {
        let (viewport, _) = self.sent?;
        Some(self.send(viewport, now))
    }

    fn send(&mut self, viewport: (u32, u32), now: Instant) -> Message {
        self.sent = Some((viewport, now));
        viewport_message(viewport)
    }

    /// The sharer left: whoever shares next is told again.
    fn lost(&mut self) {
        self.sent = None;
    }
}

fn viewport_message((width, height): (u32, u32)) -> Message {
    Message::Viewport { width, height }
}

/// Whether the current sharer has this viewer's hello, and when to offer it again.
///
/// A sharer drops what a viewer sends before the sharer's room has heard that viewer joined:
/// LiveKit hands such packets over without a sender, and the control link only takes messages
/// it can attribute. On a local server that window is about 2 to 3 s after a viewer joins, and
/// the viewer can't see it from its side. So the hello is offered until the sharer answers with
/// `Message::HelloAck`. The answer is about the hello only; it says nothing about control.
#[derive(Debug, Default)]
struct HelloOffer {
    /// While unanswered: when offering to this sharer started, and when it was last sent.
    pending: Option<(Instant, Instant)>,
}

impl HelloOffer {
    /// Starts offering: the caller sends the hello now. For a sharer just found, or after the
    /// connection came back, when the sharer may have forgotten us.
    fn start(&mut self, now: Instant) {
        self.pending = Some((now, now));
    }

    /// Whether to send the hello again now.
    fn due(&mut self, now: Instant) -> bool {
        let Some((since, last)) = self.pending else {
            return false;
        };
        let gap = if now.duration_since(since) < HELLO_FAST_FOR {
            HELLO_RETRY
        } else {
            HELLO_SLOW_RETRY
        };
        let due = now.duration_since(last) >= gap;
        if due {
            self.pending = Some((since, now));
        }
        due
    }

    /// The sharer answered. Returns whether that's news: the first answer since offering began.
    fn acknowledged(&mut self) -> bool {
        self.pending.take().is_some()
    }

    /// Whether the sharer may not have the hello yet.
    fn unanswered(&self) -> bool {
        self.pending.is_some()
    }

    /// The sharer left: nothing to offer until the next one is found.
    fn lost(&mut self) {
        self.pending = None;
    }
}

enum In {
    Link(LinkEvent),
    Ui(Ui),
    /// Give control back and end, after the sharer confirms it has applied everything (or
    /// this long passes).
    Stop(Duration),
}

/// The guest session's state, readable from any thread.
#[derive(Clone, Default)]
pub struct Status(Arc<AtomicU8>);

impl Status {
    pub fn get(&self) -> GuestState {
        match self.0.load(Ordering::Acquire) {
            1 => GuestState::Requested,
            2 => GuestState::InControl,
            _ => GuestState::Idle,
        }
    }

    fn set(&self, state: GuestState) {
        let v = match state {
            GuestState::Idle => 0,
            GuestState::Requested => 1,
            GuestState::InControl => 2,
        };
        self.0.store(v, Ordering::Release);
    }
}

#[derive(Clone)]
pub struct Guest {
    tx: Sender<In>,
    pub status: Status,
    sender: LinkSender,
}

impl Guest {
    /// Starts the guest thread. Returns the link to hand to `subscribe::run`, and the thread
    /// to join after [`Guest::stop`]. With `sent_log`, every message sent is also written there
    /// as a line of JSON (see `tc-spike replay`). With `events`, the connection, who is in the
    /// call and this guest's control state go there too, for the desktop app.
    pub fn start(
        name: String,
        sent_log: Option<&Path>,
        events: Option<Events>,
    ) -> Result<(Self, ControlLink, JoinHandle<()>), Error> {
        let (tx, rx) = mpsc::channel();
        let (link, outgoing) = {
            let tx = tx.clone();
            ControlLink::new(protocol::TOPIC, move |event| {
                let _ = tx.send(In::Link(event));
            })
        };
        let log = sent_log.map(std::fs::File::create).transpose()?;
        let status = Status::default();
        let thread = {
            let status = status.clone();
            std::thread::Builder::new().name("guest".into()).spawn({
                let outgoing = outgoing.clone();
                move || run(name, &rx, &outgoing, &status, log, events.map(AppSeam::new))
            })?
        };
        Ok((
            Self {
                tx,
                status,
                sender: outgoing,
            },
            link,
            thread,
        ))
    }

    pub fn send(&self, ui: Ui) {
        let _ = self.tx.send(In::Ui(ui));
    }

    /// Gives control back and ends the thread once the sharer confirms it has applied
    /// everything sent, so leaving the room doesn't cut off the guest's last input. Waits at most
    /// `limit` for that. Join the thread to wait for it.
    pub fn stop(&self, limit: Duration) {
        let _ = self.tx.send(In::Stop(limit));
    }

    /// Messages handed to the link but not sent yet.
    pub fn unsent(&self) -> usize {
        self.sender.pending()
    }
}

/// What the desktop app is told from the guest thread.
struct AppSeam {
    events: Events,
    people: People,
    /// This guest, once connected.
    me: Option<Peer>,
    shown: Option<GuestState>,
}

impl AppSeam {
    fn new(events: Events) -> Self {
        Self {
            events,
            people: People::default(),
            me: None,
            shown: None,
        }
    }

    fn link(&mut self, event: &LinkEvent, name: &str) {
        let event = match event {
            LinkEvent::Connected { identity } => {
                self.me = Some(Peer {
                    identity: identity.clone(),
                    name: name.to_owned(),
                });
                Event::Connection {
                    state: ConnectionState::Connected,
                }
            }
            LinkEvent::Reconnected => Event::Connection {
                state: ConnectionState::Connected,
            },
            LinkEvent::Reconnecting => Event::Connection {
                state: ConnectionState::Reconnecting,
            },
            LinkEvent::Joined { identity, name } => self.people.joined(identity, name),
            LinkEvent::Left(identity) => match self.people.left(identity) {
                Some(event) => event,
                None => return,
            },
            LinkEvent::Sharing(_) | LinkEvent::Data { .. } => return,
        };
        (self.events)(event);
    }

    fn show(&mut self, state: GuestState) {
        if self.shown == Some(state) {
            return;
        }
        self.shown = Some(state);
        let (state, by) = match state {
            GuestState::Idle => (ControlState::None, None),
            GuestState::Requested => (ControlState::Asked, self.me.clone()),
            GuestState::InControl => (ControlState::Controlling, self.me.clone()),
        };
        (self.events)(Event::Control {
            state,
            by,
            trusted: Vec::new(),
        });
    }
}

fn hello(name: &str) -> Message {
    Message::Hello {
        name: name.to_owned(),
        platform: this_platform(),
        decoders: crate::app::decoders(),
    }
}

fn run(
    name: String,
    rx: &mpsc::Receiver<In>,
    outgoing: &LinkSender,
    status: &Status,
    mut log: Option<std::fs::File>,
    mut app: Option<AppSeam>,
) {
    let mut session = GuestSession::new(Instant::now());
    let mut sharer: Option<String> = None;
    let mut viewport = ViewportReport::default();
    let mut offer = HelloOffer::default();
    // A release the sharer hasn't confirmed yet, and when we must leave anyway.
    let mut awaiting_release = false;
    let mut leave_by: Option<Instant> = None;
    let mut send = |sharer: &Option<String>, message: Message| {
        let Some(to) = sharer else {
            tracing::debug!(?message, "no sharer yet; not sent");
            return;
        };
        if let Some(log) = log.as_mut() {
            let line = serde_json::to_string(&message).expect("messages serialize");
            let _ = writeln!(log, "{line}");
        }
        outgoing.send(Outgoing {
            to: Some(to.clone()),
            payload: protocol::encode(&message),
            reliable: message.reliable(),
        });
    };
    loop {
        let now = Instant::now();
        let mut out = Vec::new();
        let received = rx.recv_timeout(TICK);
        if let (Some(app), Ok(In::Link(event))) = (app.as_mut(), &received) {
            app.link(event, &name);
        }
        // The sharer is found as soon as they are in the room, by the role in their identity, not
        // when their screen arrives: a guest that can't decode the sharer's codec never receives
        // it, and the decoders in its hello are what make the sharer switch. A sharer joining
        // again (already known) is offered the hello again; their screen arriving after that is
        // no news. The screen still finds a sharer whose identity carries no role.
        let found = match &received {
            Ok(In::Link(LinkEvent::Joined { identity, .. })) => {
                (role_of(identity) == Role::Sharer).then(|| identity.clone())
            }
            Ok(In::Link(LinkEvent::Sharing(identity))) => {
                (sharer.as_ref() != Some(identity)).then(|| identity.clone())
            }
            _ => None,
        };
        if let Some(identity) = found {
            tracing::info!(%identity, "sharer found");
            sharer = Some(identity);
            out.push(hello(&name));
            offer.start(now);
            out.extend(viewport.fresh(now));
            // A request made before the sharer was found went nowhere: make it now.
            if session.state() == GuestState::Requested {
                out.push(Message::RequestControl);
            }
        }
        match received {
            Ok(In::Link(LinkEvent::Data { from, payload })) if Some(&from) == sharer.as_ref() => {
                match protocol::decode(&payload) {
                    Ok(Message::HelloAck) => {
                        if offer.acknowledged() {
                            tracing::debug!("the sharer has our hello");
                            out.extend(viewport.again(now));
                        }
                    }
                    Ok(message) => {
                        if matches!(message, Message::Released | Message::Revoke) {
                            awaiting_release = false;
                        }
                        if session.on_message(&message) {
                            tracing::info!(?message, state = ?session.state(), "sharer answered");
                            // A turn-down that isn't the sharer's choice says why: show it.
                            if let Message::Deny {
                                reason: Some(reason),
                            } = &message
                            {
                                tracing::warn!("control turned down: {reason}");
                                if let Some(app) = app.as_ref() {
                                    (app.events)(Event::Error {
                                        message: reason.clone(),
                                    });
                                }
                            }
                        }
                    }
                    Err(e) => tracing::debug!("bad control message: {e}"),
                }
            }
            Ok(In::Link(LinkEvent::Left(identity))) if Some(&identity) == sharer.as_ref() => {
                session.on_message(&Message::Revoke);
                sharer = None;
                viewport.lost();
                offer.lost();
                awaiting_release = false;
            }
            // After a full reconnect we are a new participant to the sharer, which forgot the
            // hello when the old one left.
            Ok(In::Link(LinkEvent::Reconnected)) if sharer.is_some() => {
                out.push(hello(&name));
                offer.start(now);
                out.extend(viewport.fresh(now));
            }
            Ok(In::Link(_)) => {}
            Ok(In::Ui(ui)) => match ui {
                Ui::Input(event) => out.extend(session.input(event, now)),
                Ui::Request => out.extend(session.request(now)),
                Ui::Release => out.extend(session.release()),
                Ui::Viewport(v) => viewport.set(v, now),
                Ui::Toggle => out.extend(match session.state() {
                    GuestState::Idle => session.request(now),
                    _ => session.release(),
                }),
            },
            Ok(In::Stop(limit)) => {
                out.extend(session.release());
                leave_by = Some(now + limit);
            }
            Err(RecvTimeoutError::Disconnected) => {
                if let Some(m) = session.release() {
                    send(&sharer, m);
                }
                status.set(session.state());
                if let Some(app) = app.as_mut() {
                    app.show(session.state());
                }
                return;
            }
            Err(RecvTimeoutError::Timeout) => {}
        }
        if sharer.is_some() {
            if offer.due(now) {
                out.push(hello(&name));
                out.extend(viewport.again(now));
            }
            out.extend(viewport.due(now));
        }
        for message in session.tick(now) {
            // A repeated request goes with a repeated hello, in case the sharer lost track of
            // us some way the offer doesn't see.
            if message == Message::RequestControl {
                out.push(hello(&name));
            }
            out.push(message);
        }
        // A request names who is asking from the hello: while the sharer may not have it, it
        // goes first.
        if offer.unanswered() {
            if let Some(i) = out.iter().position(|m| *m == Message::RequestControl) {
                if !out[..i].iter().any(|m| matches!(m, Message::Hello { .. })) {
                    out.insert(i, hello(&name));
                }
            }
        }
        for message in out {
            if message == Message::ReleaseControl && sharer.is_some() {
                awaiting_release = true;
            }
            send(&sharer, message);
        }
        status.set(session.state());
        if let Some(app) = app.as_mut() {
            app.show(session.state());
        }
        if let Some(deadline) = leave_by {
            if !awaiting_release {
                return;
            }
            if Instant::now() >= deadline {
                tracing::warn!("the sharer didn't confirm the release; leaving anyway");
                return;
            }
        }
    }
}

fn this_platform() -> Platform {
    if cfg!(target_os = "macos") {
        Platform::Macos
    } else if cfg!(target_os = "windows") {
        Platform::Windows
    } else if cfg!(target_os = "linux") {
        Platform::Linux
    } else {
        Platform::Unknown
    }
}

/// One step of a control script for a headless viewer (`view --headless --control-script`).
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "do", rename_all = "snake_case")]
pub enum Step {
    /// Ask for control and wait until it's granted.
    Request,
    /// Wait until control is granted, without asking: the request comes from elsewhere, such
    /// as the desktop app's `request_control` command.
    AwaitControl,
    /// Wait until control ends, by the sharer taking it back or the guest giving it back.
    AwaitEnd,
    Release,
    Input {
        event: InputEvent,
    },
    /// Type text one key at a time, as a guest's keyboard would send it.
    Type {
        text: String,
        #[serde(default)]
        gap_ms: u64,
    },
    Wait {
        ms: u64,
    },
    /// Hold here, sending nothing, until `file` exists. If it says `go` the script carries on;
    /// anything else, or no file within `timeout_ms`, gives control back and stops the script.
    /// Lets a test check what the first input did before it sends the rest
    /// (`bench/control-inject.sh`).
    Gate {
        file: PathBuf,
        #[serde(default = "gate_timeout_ms")]
        timeout_ms: u64,
    },
}

fn gate_timeout_ms() -> u64 {
    15_000
}

/// What a gate file says: `Some(true)` to go on, `Some(false)` to stop, `None` while there is
/// nothing (or nothing complete) there yet.
fn gate_says(contents: Option<&str>) -> Option<bool> {
    let word = contents?.trim();
    if word.is_empty() {
        return None;
    }
    Some(word == "go")
}

/// How long `request` waits for the sharer.
const GRANT_TIMEOUT: Duration = Duration::from_secs(60);

pub fn read_script(path: &PathBuf) -> Result<Vec<Step>, Error> {
    let text = std::fs::read_to_string(path)?;
    Ok(serde_json::from_str(&text)?)
}

/// Plays a script through the guest session. Fails if the sharer doesn't grant control.
pub fn play(guest: &Guest, steps: &[Step]) -> Result<(), ScriptError> {
    for step in steps {
        match step {
            Step::Request => {
                guest.send(Ui::Request);
                let asked = Instant::now();
                let mut pending = false;
                loop {
                    match guest.status.get() {
                        GuestState::InControl => break,
                        GuestState::Requested => pending = true,
                        // Idle before the session took the request, or after a "no".
                        GuestState::Idle if pending => return Err(ScriptError::Denied),
                        GuestState::Idle => {}
                    }
                    if asked.elapsed() >= GRANT_TIMEOUT {
                        return Err(ScriptError::NoAnswer);
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                tracing::info!("control granted");
            }
            Step::AwaitControl => {
                let asked = Instant::now();
                while guest.status.get() != GuestState::InControl {
                    if asked.elapsed() >= GRANT_TIMEOUT {
                        return Err(ScriptError::NoAnswer);
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                tracing::info!("control granted");
            }
            Step::AwaitEnd => {
                let since = Instant::now();
                while guest.status.get() == GuestState::InControl {
                    if since.elapsed() >= GRANT_TIMEOUT {
                        return Err(ScriptError::NoAnswer);
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                tracing::info!("control ended");
            }
            Step::Release => guest.send(Ui::Release),
            Step::Input { event } => guest.send(Ui::Input(event.clone())),
            Step::Type { text, gap_ms } => {
                for c in text.chars() {
                    for event in key_events(c) {
                        guest.send(Ui::Input(event));
                    }
                    if *gap_ms > 0 {
                        std::thread::sleep(Duration::from_millis(*gap_ms));
                    }
                }
            }
            Step::Wait { ms } => std::thread::sleep(Duration::from_millis(*ms)),
            Step::Gate { file, timeout_ms } => {
                let since = Instant::now();
                let open = loop {
                    let contents = std::fs::read_to_string(file).ok();
                    if let Some(go) = gate_says(contents.as_deref()) {
                        break go;
                    }
                    if since.elapsed() >= Duration::from_millis(*timeout_ms) {
                        break false;
                    }
                    std::thread::sleep(Duration::from_millis(10));
                };
                if !open {
                    guest.send(Ui::Release);
                    return Err(ScriptError::Gated(format!(
                        "{} didn't say go",
                        file.display()
                    )));
                }
                tracing::info!(gate = %file.display(), "gate open");
            }
        }
        if matches!(step, Step::Input { .. } | Step::Type { .. })
            && guest.status.get() != GuestState::InControl
        {
            return Err(ScriptError::Ended);
        }
    }
    Ok(())
}

/// The key down and up a US keyboard sends for `c`. Characters off that keyboard go as text on
/// an unidentified key, as a layout or input method would produce them.
pub fn key_events(c: char) -> [InputEvent; 2] {
    let (code, text) = match c {
        '\n' => ("Enter".to_owned(), None),
        '\t' => ("Tab".to_owned(), None),
        ' ' => ("Space".to_owned(), Some(" ".to_owned())),
        'a'..='z' | 'A'..='Z' => (
            format!("Key{}", c.to_ascii_uppercase()),
            Some(c.to_string()),
        ),
        '0'..='9' => (format!("Digit{c}"), Some(c.to_string())),
        _ => ("Unidentified".to_owned(), Some(c.to_string())),
    };
    [
        InputEvent::Key {
            code: code.clone(),
            pressed: true,
            text,
        },
        InputEvent::Key {
            code,
            pressed: false,
            text: None,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripts_parse() {
        let steps: Vec<Step> = serde_json::from_str(
            r#"[{"do":"request"},{"do":"input","event":{"kind":"move","x":0.5,"y":0.5}},
                {"do":"type","text":"hi"},{"do":"wait","ms":5},{"do":"release"},
                {"do":"await_control"},{"do":"await_end"},
                {"do":"gate","file":"/tmp/x/gate-canary"},
                {"do":"gate","file":"g","timeout_ms":5}]"#,
        )
        .unwrap();
        assert_eq!(steps.len(), 9);
        assert!(matches!(
            &steps[7],
            Step::Gate {
                timeout_ms: 15_000,
                ..
            }
        ));
    }

    #[test]
    fn a_gate_opens_only_on_go() {
        assert_eq!(gate_says(None), None);
        assert_eq!(gate_says(Some("")), None, "written but not yet filled");
        assert_eq!(gate_says(Some("go\n")), Some(true));
        assert_eq!(gate_says(Some("stop\n")), Some(false));
        assert_eq!(gate_says(Some("gone")), Some(false));
    }

    #[test]
    fn viewports_are_reported_once_settled() {
        let t0 = Instant::now();
        let at = |ms| t0 + Duration::from_millis(ms);
        let mut r = ViewportReport::default();
        assert_eq!(r.due(t0), None, "nothing known yet");
        assert_eq!(r.fresh(t0), None);
        // A drag: a new size every 20 ms for a second, then still.
        for i in 0..50 {
            r.set((1000 + i * 10, 800), at(u64::from(i) * 20));
            assert_eq!(r.due(at(u64::from(i) * 20 + 10)), None, "mid-drag");
        }
        assert_eq!(r.due(at(980 + 299)), None);
        assert_eq!(r.due(at(1280)), Some(viewport_message((1490, 800))));
        assert_eq!(r.due(at(1290)), None, "sent once");
        // The same size again is no news, but it's repeated now and then.
        r.set((1490, 800), at(2000));
        assert_eq!(r.due(at(3000)), None);
        assert_eq!(r.due(at(3280)), Some(viewport_message((1490, 800))));
        assert_eq!(r.due(at(3290)), None);
        // A new sharer hears it at once, then every 2 s. (Sooner repeats ride with the hello
        // until the sharer answers it: see `HelloOffer`.)
        r.lost();
        assert_eq!(r.fresh(at(4000)), Some(viewport_message((1490, 800))));
        let sent: Vec<u64> = (4001..12_000)
            .step_by(10)
            .filter(|&ms| r.due(at(ms)).is_some())
            .collect();
        assert_eq!(sent, [6001, 8001, 10_001]);
        // Sending it again with an offer of the hello restarts the wait.
        assert_eq!(r.again(at(10_500)), Some(viewport_message((1490, 800))));
        assert_eq!(r.due(at(12_001)), None);
        assert!(r.due(at(12_500)).is_some());
        // Mid-drag, an offer repeats the last settled viewport, not the one under the pointer.
        r.set((1600, 900), at(13_000));
        assert_eq!(r.again(at(13_100)), Some(viewport_message((1490, 800))));
        assert_eq!(r.due(at(13_300)), Some(viewport_message((1600, 900))));
        assert_eq!(r.again(at(13_400)), Some(viewport_message((1600, 900))));
        // A sharer that left takes nothing to repeat with it.
        r.lost();
        assert_eq!(r.again(at(14_000)), None);
    }

    #[test]
    fn the_hello_is_offered_until_the_sharer_answers() {
        let t0 = Instant::now();
        let at = |ms| t0 + Duration::from_millis(ms);
        let offers = |o: &mut HelloOffer, from: u64, to: u64| -> Vec<u64> {
            (from..to).step_by(10).filter(|&ms| o.due(at(ms))).collect()
        };
        let mut o = HelloOffer::default();
        assert!(!o.due(t0) && !o.unanswered(), "no sharer, nothing to offer");
        // Found a sharer: offered every 250 ms...
        o.start(t0);
        assert!(o.unanswered());
        assert_eq!(offers(&mut o, 10, 1000), [250, 500, 750]);
        // ...until it answers. A second answer (to an offer already on its way) is no news.
        assert!(o.acknowledged());
        assert!(!o.acknowledged());
        assert!(!o.unanswered());
        assert_eq!(offers(&mut o, 1000, 30_000), [] as [u64; 0]);
        // The connection came back: offered again, from the start.
        o.start(at(30_000));
        assert_eq!(offers(&mut o, 30_010, 30_600), [30_250, 30_500]);
        // The sharer left; the next one gets its own offers.
        o.lost();
        assert!(!o.unanswered());
        assert_eq!(offers(&mut o, 30_600, 40_000), [] as [u64; 0]);
        o.start(at(40_000));
        assert_eq!(offers(&mut o, 40_010, 40_300), [40_250]);
    }

    #[test]
    fn a_sharer_that_never_answers_hears_the_hello_less_often() {
        let t0 = Instant::now();
        let at = |ms| t0 + Duration::from_millis(ms);
        let mut o = HelloOffer::default();
        o.start(t0);
        let sent: Vec<u64> = (10..25_000)
            .step_by(10)
            .filter(|&ms| o.due(at(ms)))
            .collect();
        // 59 offers in the first 15 s (every 250 ms), then every 2 s.
        assert_eq!(sent.iter().filter(|&&ms| ms < 15_000).count(), 59);
        assert_eq!(sent[59..], [16_750, 18_750, 20_750, 22_750, 24_750]);
        assert!(o.unanswered());
    }

    /// The guest thread, fed by hand, and what it sends, as (to, message).
    struct Harness {
        tx: Sender<In>,
        link: ControlLink,
        thread: Option<JoinHandle<()>>,
    }

    impl Harness {
        fn start() -> Self {
            let (tx, rx) = mpsc::channel();
            let (link, sender) = ControlLink::new(protocol::TOPIC, |_| {});
            let thread = std::thread::spawn(move || {
                run("Sam".into(), &rx, &sender, &Status::default(), None, None)
            });
            Self {
                tx,
                link,
                thread: Some(thread),
            }
        }

        fn link(&self, event: LinkEvent) {
            self.tx.send(In::Link(event)).unwrap();
        }

        fn from(&self, who: &str, message: &Message) {
            self.link(LinkEvent::Data {
                from: who.into(),
                payload: protocol::encode(message),
            });
        }

        /// Everything sent in the next `ms`.
        fn sent(&mut self, ms: u64) -> Vec<(String, Message)> {
            std::thread::sleep(Duration::from_millis(ms));
            let mut out = Vec::new();
            while let Ok(o) = self.link.outgoing.try_recv() {
                out.push((o.to.unwrap(), protocol::decode(&o.payload).unwrap()));
            }
            out
        }
    }

    impl Drop for Harness {
        fn drop(&mut self) {
            let (tx, _) = mpsc::channel();
            drop(std::mem::replace(&mut self.tx, tx));
            let _ = self.thread.take().unwrap().join();
        }
    }

    fn hellos(sent: &[(String, Message)], to: &str) -> usize {
        sent.iter()
            .filter(|(who, m)| who == to && matches!(m, Message::Hello { .. }))
            .count()
    }

    #[test]
    fn the_guest_offers_its_hello_until_its_sharer_answers() {
        let mut h = Harness::start();
        h.tx.send(In::Ui(Ui::Viewport((1920, 1080)))).unwrap();
        h.link(LinkEvent::Sharing("sharer-a".into()));
        let sent = h.sent(700);
        assert!(hellos(&sent, "sharer-a") >= 2, "{sent:?}");
        assert!(sent.iter().all(|(who, _)| who == "sharer-a"));
        // The viewport rides with each offer.
        let viewports = sent
            .iter()
            .filter(|(_, m)| *m == viewport_message((1920, 1080)))
            .count();
        assert_eq!(viewports, hellos(&sent, "sharer-a"), "{sent:?}");

        // An answer from anyone but the sharer counts for nothing.
        h.from("guest-b", &Message::HelloAck);
        assert!(hellos(&h.sent(600), "sharer-a") >= 1);

        // The sharer's answer stops the offers, and the viewport goes once more.
        h.from("sharer-a", &Message::HelloAck);
        let sent = h.sent(200);
        assert!(sent.contains(&("sharer-a".into(), viewport_message((1920, 1080)))));
        assert_eq!(hellos(&h.sent(800), "sharer-a"), 0);
        // A second answer changes nothing.
        h.from("sharer-a", &Message::HelloAck);
        assert_eq!(h.sent(300), []);

        // Once the sharer has the hello, a request goes alone.
        h.tx.send(In::Ui(Ui::Request)).unwrap();
        let sent = h.sent(100);
        assert_eq!(sent, [("sharer-a".into(), Message::RequestControl)]);
        h.tx.send(In::Ui(Ui::Release)).unwrap();
        h.sent(100);

        // After a full reconnect the sharer may have forgotten us: offered again.
        h.link(LinkEvent::Reconnected);
        assert!(hellos(&h.sent(600), "sharer-a") >= 2);
        h.from("sharer-a", &Message::HelloAck);
        h.sent(200);

        // The sharer leaves and another comes: the new one is offered the hello, until it answers.
        h.link(LinkEvent::Left("sharer-a".into()));
        assert_eq!(h.sent(400), []);
        h.link(LinkEvent::Sharing("sharer-c".into()));
        let sent = h.sent(600);
        assert!(hellos(&sent, "sharer-c") >= 2, "{sent:?}");
        assert!(sent.iter().all(|(who, _)| who == "sharer-c"));
        // A request while it may not have the hello goes after one, so the sharer is asked
        // by name.
        h.tx.send(In::Ui(Ui::Request)).unwrap();
        let sent = h.sent(100);
        let request = sent
            .iter()
            .position(|(_, m)| *m == Message::RequestControl)
            .expect("request sent");
        assert!(sent[..request]
            .iter()
            .any(|(_, m)| matches!(m, Message::Hello { .. })));
        // Withdrawn, so no repeat of the request (which carries a hello) lands in the wait below.
        h.tx.send(In::Ui(Ui::Release)).unwrap();
        h.sent(100);
        h.from("sharer-c", &Message::HelloAck);
        h.sent(200);
        assert_eq!(hellos(&h.sent(600), "sharer-c"), 0);
    }

    fn joined(identity: &str) -> LinkEvent {
        LinkEvent::Joined {
            identity: identity.into(),
            name: "Alex".into(),
        }
    }

    /// A guest that can't decode the sharer's codec never receives the screen: the sharer must
    /// still hear its decoders, or it never learns to switch.
    #[test]
    fn a_sharer_whose_screen_never_arrives_still_gets_the_hello() {
        let mut h = Harness::start();
        h.tx.send(In::Ui(Ui::Viewport((1920, 1080)))).unwrap();
        // Others in the room are no sharer: a guest, and an identity with no role.
        h.link(joined("guest-b"));
        h.link(joined("Sam"));
        assert_eq!(h.sent(400), []);

        h.link(joined("sharer-a"));
        let sent = h.sent(700);
        assert!(hellos(&sent, "sharer-a") >= 2, "{sent:?}");
        assert!(sent.iter().all(|(who, _)| who == "sharer-a"));
        let decoders = sent.iter().find_map(|(_, m)| match m {
            Message::Hello { decoders, .. } => Some(decoders.clone()),
            _ => None,
        });
        assert_eq!(decoders, Some(crate::app::decoders()));
        assert!(sent.contains(&("sharer-a".into(), viewport_message((1920, 1080)))));

        // The answer stops the offers, and the screen arriving later is no news.
        h.from("sharer-a", &Message::HelloAck);
        h.sent(200);
        h.link(LinkEvent::Sharing("sharer-a".into()));
        assert_eq!(hellos(&h.sent(800), "sharer-a"), 0);

        // The sharer joining again (a new connection under the same identity) may have forgotten
        // us: offered again, until it answers.
        h.link(joined("sharer-a"));
        assert!(hellos(&h.sent(600), "sharer-a") >= 2);
        h.from("sharer-a", &Message::HelloAck);
        h.sent(200);
        assert_eq!(hellos(&h.sent(600), "sharer-a"), 0);
    }

    #[test]
    fn a_request_made_before_the_sharer_joins_waits_for_them() {
        let mut h = Harness::start();
        h.tx.send(In::Ui(Ui::Request)).unwrap();
        h.link(joined("guest-b"));
        assert_eq!(h.sent(300), [], "nobody to ask yet");

        h.link(joined("sharer-a"));
        let sent = h.sent(100);
        let request = sent
            .iter()
            .position(|(_, m)| *m == Message::RequestControl)
            .expect("request sent once the sharer is found");
        assert!(sent[..request]
            .iter()
            .any(|(_, m)| matches!(m, Message::Hello { .. })));
        assert!(sent.iter().all(|(who, _)| who == "sharer-a"));
        h.tx.send(In::Ui(Ui::Release)).unwrap();
        h.sent(100);
    }

    #[test]
    fn viewports_are_kept_in_the_range_the_sharer_accepts() {
        let mut r = ViewportReport::default();
        r.set((10, 100_000), Instant::now());
        let message = r.fresh(Instant::now()).unwrap();
        assert_eq!(
            message,
            viewport_message((protocol::MIN_VIEWPORT, protocol::MAX_VIEWPORT))
        );
        protocol::decode(&protocol::encode(&message)).expect("valid");
    }

    #[test]
    fn typed_keys_pass_validation() {
        for c in "Hello, world!\n\tÅß1 ".chars() {
            for event in key_events(c) {
                let message = Message::Input {
                    seq: 1,
                    sent_at_us: 0,
                    after: 0,
                    event,
                };
                protocol::decode(&protocol::encode(&message)).expect("valid");
            }
        }
    }
}
