//! Calls with accounts: sharing, joining by link, letting people in, ringing, and the engine
//! that runs each call. This is the app's one source of truth about them: it keeps a
//! [`HomeView`] and sends it to the window whenever it changes (the `home` event; `home_state`
//! returns it too). The window never sees a token, a pass or the clipboard's text.
//!
//! The backend decides who may join a call (`supabase/README.md`); this side decides what this
//! Mac does: when its engine starts and stops, and who it gives control to. Consent rules
//! enforced here, on top of the engine's own (`docs/calls.md`):
//!
//! - **Arming.** A knock can't be let in, and a control request can't be allowed, until
//!   [`ARMING`] after it could first be seen: the later of when it was first delivered to the
//!   window and when the call bar came on screen ([`Sight`]), so a click meant for something
//!   else can't answer it. Turning away and denying are never delayed.
//! - **Control on arrival** ("Let in with control"): once the backend has let them in, the
//!   engine running the call is sent `trust_on_arrival` for that identity, once. The engine keeps
//!   the promise, so every revoke cancels it, the hotkey included (ADR 0010).
//! - **The window can't trust anyone directly**: `trust_control` and `trust_on_arrival` from
//!   the window are refused; only Let in with control and Give control send them, after the
//!   Accessibility check.
//!
//! Nothing here needs Tauri: `main.rs` connects it to the window and the tests drive it with a
//! fake backend and a fake engine.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use account::{Account, CallPass, Error as AccountError, Inbox, Member, MemberState, Profile};
use serde::Serialize;
use serde_json::{json, Value};

use crate::accounts::{AccountShell, AccountView, Me};
use crate::engine::{EnvHook, Output, Plan, Quality, Role, Sink};
use crate::link;
use crate::seams::{
    Awake, Clipboard, Clock, EngineHandle, EngineSpec, Engines, Permission, Permits,
};
use crate::snapshot::Snapshot;

/// How long after a knock or a control request could first be seen it can be accepted.
pub const ARMING: Duration = Duration::from_millis(1000);

/// Whether the call bar, where knocks and control requests show, can be seen. Without a window
/// to track (tests, scripts that show none) it is `Always`, and arming counts from delivery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sight {
    Always,
    Hidden,
    /// On screen since then.
    Since(Instant),
}

/// When the arming delay starts for something first delivered at `delivered`: once it can be
/// seen. `None` while it can't.
pub fn arming_from(delivered: Instant, sight: Sight) -> Option<Instant> {
    match sight {
        Sight::Always => Some(delivered),
        Sight::Hidden => None,
        Sight::Since(shown) => Some(delivered.max(shown)),
    }
}

/// Whether something first delivered at `delivered` may be accepted at `now`.
fn armed(now: Instant, delivered: Instant, sight: Sight) -> bool {
    arming_from(delivered, sight).is_some_and(|from| now.saturating_duration_since(from) >= ARMING)
}
/// How often the backend is asked, normally and after repeated network failures.
pub const POLL_EVERY: Duration = Duration::from_secs(2);
pub const POLL_BACKOFF: Duration = Duration::from_secs(10);
/// Network failures in a row before the poll backs off and says the server can't be reached.
pub const FAILURES_BEFORE_BACKOFF: u32 = 3;
/// How long a knock keeps trying while the server can't be reached (the backend forgets a knock
/// not repeated for 45 s).
pub const KNOCK_GIVE_UP: Duration = Duration::from_secs(45);
/// How long leaving waits for the engine to go before stopping it.
const ENGINE_GONE_TIMEOUT: Duration = Duration::from_secs(7);
/// How often a missing permission is checked again while the window says it is needed, so the
/// message goes by itself once it is turned on in System Settings.
pub const RECHECK_EVERY: Duration = Duration::from_secs(2);

pub const NOT_SIGNED_IN: &str = "Sign in first.";
pub const IN_CALL: &str = "You're already in a call. Leave it first.";
pub const NEEDS_SCREEN_RECORDING: &str =
    "TwinCursor needs Screen Recording to share your screen. Turn it on in System Settings, then try again.";
pub const NEEDS_ACCESSIBILITY: &str =
    "TwinCursor needs Accessibility to give control. Turn it on in System Settings, then try again.";
pub const TOO_SOON: &str = "Too soon. Try again.";
pub const LINK_COPIED: &str = "The call link is on your clipboard.";
pub const LINK_NOT_COPIED: &str = "The call link couldn't be put on the clipboard.";
pub const CALL_ENDED: &str = "The call ended.";
pub const KNOCK_GAVE_UP: &str = "Can't reach the server, so you stopped waiting to join.";
pub const CANT_REACH: &str = "Can't reach the server.";
pub const NOT_HOSTING: &str = "You aren't sharing a call.";
pub const NOT_KNOCKING: &str = "You aren't waiting to join a call.";
pub const NOT_WAITING: &str = "That person isn't waiting to join any more.";
pub const NOT_IN_THE_CALL: &str = "That person isn't in the call.";
pub const NO_RING: &str = "That call isn't ringing you any more.";
pub const REQUEST_NOT_OPEN: &str = "That control request is no longer open.";
pub const NO_CALL: &str = "No call is running.";
pub const STOPPED: &str = "Stopped.";
pub const ENGINE_STOPPED: &str = "The call stopped unexpectedly.";
pub const NO_LONGER_WAITING: &str = "You're no longer waiting to join that call.";
pub const TRUST_FROM_WINDOW: &str = "Control is given with Let in with control or Give control.";
pub const ACCESSIBILITY_ON: &str = "Accessibility is on. You can give control now.";
pub const NOTHING_TO_RESET: &str = "macOS isn't refusing that permission now.";
pub const NOBODY_OWED: &str = "Nobody is waiting to be given control.";

fn reset_failed(code: i32) -> String {
    format!(
        "The reset didn't work (tccutil exited with {code}). In System Settings, select TwinCursor in the list, remove it with −, then try again."
    )
}

fn turned_away(host: &str) -> String {
    format!("{host} didn't let you in.")
}

fn let_in_without_control(name: &str) -> String {
    format!("{name} was let in without control: TwinCursor needs Accessibility to give control.")
}

fn not_given_control(name: &str) -> String {
    format!("{name} wasn't given control: TwinCursor needs Accessibility.")
}

fn restarted(message: &str) -> String {
    format!("{message} Control was reset: anyone you gave it to has to be given it again.")
}

/// A person, as the window shows them.
pub type Person = Me;

/// Where this app is with calls.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StageView {
    Idle,
    /// Setting up a share or a join.
    Starting,
    /// Sharing this call; `link` is what others join with.
    Hosting {
        call_id: String,
        link: String,
    },
    /// Waiting for `host` to let this person in.
    Knocking {
        host: Person,
    },
    /// In `host`'s call, as a guest.
    Joined {
        host: Person,
    },
    /// Leaving or ending a call.
    Ending,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RingView {
    pub call_id: String,
    pub host: Person,
}

/// Someone waiting to be let into this app's call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct KnockView {
    pub user_id: String,
    pub name: String,
    pub handle: Option<String>,
    pub avatar_url: Option<String>,
    /// Letting them in is possible now ([`ARMING`] after they were first shown).
    pub armed: bool,
}

/// A guest who wasn't given control for want of Accessibility, now that it is on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OwedView {
    pub identity: String,
    pub name: String,
    /// Giving control is possible now ([`ARMING`] after this was first shown).
    pub armed: bool,
}

/// Someone else on this app's call, as the backend has them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MemberView {
    pub user_id: String,
    pub name: String,
    pub handle: Option<String>,
    pub avatar_url: Option<String>,
    pub state: MemberState,
}

/// Everything the window shows about accounts and calls, outside the call screen itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HomeView {
    pub account: AccountView,
    pub stage: StageView,
    pub contacts: Vec<Person>,
    /// Calls ringing this person.
    pub rings: Vec<RingView>,
    /// People waiting to be let into this app's call.
    pub knocks: Vec<KnockView>,
    /// Everyone else on this app's call: rung, let in, turned away, gone.
    pub members: Vec<MemberView>,
    /// One plain sentence about something the app did, or didn't do and why.
    pub notice: Option<String>,
    pub error: Option<String>,
    /// A permission needed right now, for "Open Settings".
    pub needs: Option<Permission>,
    /// `needs` is still refused after macOS was asked and the person went to System Settings
    /// or tried again: the window offers to reset TwinCursor's entry and ask again.
    pub stuck: bool,
    /// Someone who would have been given control but for Accessibility, once it is on: one
    /// click gives it, armed like a knock.
    pub owed: Option<OwedView>,
    /// "Can't reach the server." while the poll keeps failing.
    pub server_problem: Option<String>,
}

/// How this app's engines run.
#[derive(Debug, Clone, Copy)]
pub struct Options {
    pub quality: Quality,
    pub voice: bool,
    /// This platform can share its screen (`platform::INFO`). Without it, sharing and ringing
    /// (which shares first) are refused before anything starts.
    pub can_share: bool,
}

/// What the flows are wired to.
pub struct Setup {
    pub account: Arc<AccountShell>,
    pub engines: Arc<dyn Engines>,
    pub clipboard: Arc<dyn Clipboard>,
    pub permits: Arc<dyn Permits>,
    pub clock: Arc<dyn Clock>,
    pub options: Options,
    /// The home view, each time it changes.
    pub on_home: Arc<dyn Fn(&HomeView) + Send + Sync>,
    /// Every engine output, as the window hears it (`engine` event).
    pub on_engine: Sink,
    /// What the flows did that a test checks: passes fetched, control given. Never a token.
    pub trace: Arc<dyn Fn(Value) + Send + Sync>,
    pub runtime: tokio::runtime::Handle,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Stage {
    Idle,
    Starting,
    Hosting {
        call_id: String,
        code: String,
    },
    Knocking {
        call_id: String,
        code: String,
        host: Person,
    },
    Joined {
        call_id: String,
        host: Person,
    },
    Ending,
}

/// A guest owed control ([`OwedView`]); `shown` is when the window first showed it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Owed {
    identity: String,
    name: String,
    shown: Option<Instant>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CallRef {
    call_id: String,
    host: bool,
}

struct Running {
    handle: Box<dyn EngineHandle>,
    gen: u64,
    /// The account call it is in; `None` for a room-code call, or once the backend side is
    /// taken care of.
    call: Option<CallRef>,
}

struct State {
    account: AccountView,
    stage: Stage,
    /// Changes whenever the stage does by someone's action, so a slow answer from the backend
    /// for an older one is dropped.
    flow: u64,
    contacts: Vec<Person>,
    rings: Vec<RingView>,
    /// The hosted call's members, from the last poll.
    members: Vec<Member>,
    /// When each knock was first shown to the window.
    knock_seen: HashMap<String, Instant>,
    notice: Option<String>,
    error: Option<String>,
    needs: Option<Permission>,
    failures: u32,
    knock_trouble_since: Option<Instant>,
    /// Permissions macOS was asked for in this run (its prompt shows once).
    requested: HashSet<Permission>,
    /// See [`HomeView::stuck`].
    stuck: bool,
    /// Permissions the window's Open Settings was used for in this run.
    sent_to_settings: HashSet<Permission>,
    /// The re-check timer is running.
    rechecking: bool,
    /// What the last re-check found, so the log has changes rather than every tick.
    rechecked: Option<(Permission, bool)>,
    owed: Option<Owed>,
    /// The notice that says a permission is missing, so it can go once it isn't.
    notice_needs: Option<String>,
    engine: Option<Running>,
    /// The engine whose output counts; one per `Engines::start`.
    gen: u64,
    /// `Started` outputs seen from it: 1 for its first process, 2 after a restart.
    proc: u64,
    /// Which control requests the window was shown, when first, by which engine process, and
    /// who asked.
    asked_seen: HashMap<u64, (Instant, u64, String)>,
    /// Whether the call bar can be seen, for arming.
    sight: Sight,
    participants: Vec<ipc::Participant>,
    engine_error: Option<String>,
    engine_failed: bool,
    /// An engine that exited before it was stored.
    early_exit: Option<u64>,
    snapshot: Snapshot,
    last_home: Option<HomeView>,
    arm_timer: bool,
    awake: Option<Arc<Awake>>,
    leftover: Option<String>,
}

impl State {
    fn free(&self) -> bool {
        self.stage == Stage::Idle && self.engine.as_ref().is_none_or(|r| r.handle.is_done())
    }

    fn hosting_id(&self) -> Option<String> {
        match &self.stage {
            Stage::Hosting { call_id, .. } => Some(call_id.clone()),
            _ => None,
        }
    }

    fn engine_alive(&self, gen: u64) -> bool {
        self.engine
            .as_ref()
            .is_some_and(|r| r.gen == gen && !r.handle.is_done())
    }

    /// Back to idle after a flow, with what to say.
    fn idle(&mut self, notice: Option<String>, error: Option<String>) {
        self.stage = Stage::Idle;
        self.flow += 1;
        self.members.clear();
        self.knock_seen.clear();
        self.knock_trouble_since = None;
        self.awake = None;
        self.owed = None;
        if notice.is_some() {
            self.notice = notice;
        }
        if error.is_some() {
            self.error = error;
        }
    }
}

/// What one poll did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Polled {
    Done,
    /// Another poll was running.
    Busy,
    SignedOut,
    NotConfigured,
    Failed,
}

/// How long to wait before the next poll.
pub fn poll_delay(failures: u32) -> Duration {
    if failures >= FAILURES_BEFORE_BACKOFF {
        POLL_BACKOFF
    } else {
        POLL_EVERY
    }
}

fn is_network(e: &AccountError) -> bool {
    matches!(e, AccountError::Unreachable | AccountError::Server(_))
}

fn person(p: &Profile) -> Person {
    Me::from(p.clone())
}

fn pass_env(pass: &CallPass) -> Vec<(String, String)> {
    vec![
        ("TWINCURSOR_SERVER_URL".to_owned(), pass.url.clone()),
        ("TWINCURSOR_PASS".to_owned(), pass.token.expose().to_owned()),
    ]
}

/// The call flows. Cheap to clone; every clone is the same app state.
#[derive(Clone)]
pub struct Calls {
    inner: Arc<Inner>,
}

struct Inner {
    setup: Setup,
    state: Mutex<State>,
    poll: tokio::sync::Mutex<()>,
    wake: tokio::sync::Notify,
}

impl Calls {
    pub fn new(setup: Setup) -> Calls {
        let state = State {
            account: AccountView {
                configured: setup.account.account().is_ok(),
                ..AccountView::none()
            },
            stage: Stage::Idle,
            flow: 0,
            contacts: Vec::new(),
            rings: Vec::new(),
            members: Vec::new(),
            knock_seen: HashMap::new(),
            notice: None,
            error: None,
            needs: None,
            failures: 0,
            knock_trouble_since: None,
            requested: HashSet::new(),
            stuck: false,
            sent_to_settings: HashSet::new(),
            rechecking: false,
            rechecked: None,
            owed: None,
            notice_needs: None,
            engine: None,
            gen: 0,
            proc: 0,
            asked_seen: HashMap::new(),
            sight: Sight::Always,
            participants: Vec::new(),
            engine_error: None,
            engine_failed: false,
            early_exit: None,
            snapshot: Snapshot::default(),
            last_home: None,
            arm_timer: false,
            awake: None,
            leftover: None,
        };
        Calls {
            inner: Arc::new(Inner {
                setup,
                state: Mutex::new(state),
                poll: tokio::sync::Mutex::new(()),
                wake: tokio::sync::Notify::new(),
            }),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.inner.state.lock().unwrap()
    }

    fn now(&self) -> Instant {
        self.inner.setup.clock.now()
    }

    fn trace(&self, value: Value) {
        (self.inner.setup.trace)(value);
    }

    // The view.

    fn build(&self, s: &mut State) -> HomeView {
        let now = self.now();
        let stage = match &s.stage {
            Stage::Idle => StageView::Idle,
            Stage::Starting => StageView::Starting,
            Stage::Hosting { call_id, code } => StageView::Hosting {
                call_id: call_id.clone(),
                link: link::make(code).unwrap_or_default(),
            },
            Stage::Knocking { host, .. } => StageView::Knocking { host: host.clone() },
            Stage::Joined { host, .. } => StageView::Joined { host: host.clone() },
            Stage::Ending => StageView::Ending,
        };
        let hosting = matches!(s.stage, Stage::Hosting { .. });
        let mut knocks = Vec::new();
        let mut members = Vec::new();
        if hosting {
            for m in &s.members {
                if m.state == MemberState::Knocking {
                    // Shown now, if not before: the arming delay counts from here.
                    let seen = *s.knock_seen.entry(m.user_id.clone()).or_insert(now);
                    knocks.push(KnockView {
                        user_id: m.user_id.clone(),
                        name: m.name.clone(),
                        handle: m.handle.clone(),
                        avatar_url: m.avatar_url.clone(),
                        armed: armed(now, seen, s.sight),
                    });
                } else {
                    members.push(MemberView {
                        user_id: m.user_id.clone(),
                        name: m.name.clone(),
                        handle: m.handle.clone(),
                        avatar_url: m.avatar_url.clone(),
                        state: m.state,
                    });
                }
            }
        }
        let owed = match s.owed.as_mut() {
            Some(owed) if hosting && s.needs != Some(Permission::Accessibility) => {
                let shown = *owed.shown.get_or_insert(now);
                Some(OwedView {
                    identity: owed.identity.clone(),
                    name: owed.name.clone(),
                    armed: armed(now, shown, s.sight),
                })
            }
            _ => None,
        };
        HomeView {
            account: s.account.clone(),
            stage,
            contacts: s.contacts.clone(),
            rings: s.rings.clone(),
            knocks,
            members,
            notice: s.notice.clone(),
            error: s.error.clone(),
            needs: s.needs,
            stuck: s.needs.is_some() && s.stuck,
            owed,
            server_problem: (s.failures >= FAILURES_BEFORE_BACKOFF).then(|| CANT_REACH.to_owned()),
        }
    }

    /// The view now, for `home_state`. What it shows counts as shown.
    pub fn home(&self) -> HomeView {
        let mut s = self.lock();
        self.build(&mut s)
    }

    /// Sends the view to the window if it changed.
    fn emit(&self) {
        let (view, changed, arm) = {
            let mut s = self.lock();
            let view = self.build(&mut s);
            let changed = s.last_home.as_ref() != Some(&view);
            if changed {
                s.last_home = Some(view.clone());
            }
            let arm = (view.knocks.iter().any(|k| !k.armed)
                || view.owed.as_ref().is_some_and(|o| !o.armed))
                && !s.arm_timer;
            if arm {
                s.arm_timer = true;
            }
            (view, changed, arm)
        };
        if changed {
            (self.inner.setup.on_home)(&view);
        }
        if arm {
            // Knocks (and Give control for someone owed it) turn armed with time alone: say so
            // when they do.
            let me = self.clone();
            self.inner.setup.runtime.spawn(async move {
                tokio::time::sleep(ARMING / 4).await;
                me.lock().arm_timer = false;
                me.emit();
            });
        }
    }

    /// The call bar came on screen, or went. Arming counts from when it came: a knock or a
    /// control request delivered while it was hidden can't be accepted until it has been on
    /// screen for [`ARMING`].
    pub fn set_bar_visible(&self, visible: bool) {
        {
            let mut s = self.lock();
            s.sight = match (visible, s.sight) {
                (false, _) => Sight::Hidden,
                (true, Sight::Hidden) => Sight::Since(self.now()),
                (true, sight) => sight,
            };
        }
        self.emit();
    }

    /// The current call's engine output, for a window opened mid-call (`call_snapshot`).
    pub fn snapshot(&self) -> Vec<Output> {
        self.lock().snapshot.outputs()
    }

    // The account.

    async fn signed_in(&self) -> Result<Arc<Account>, String> {
        let account = self.inner.setup.account.account()?;
        if account.is_signed_in().await {
            Ok(account)
        } else {
            Err(NOT_SIGNED_IN.into())
        }
    }

    /// Starting a call is only for people the server lets host. Refused here when the server
    /// has said so already; the server refuses it anyway (`not_invited`).
    fn may_host(&self) -> Result<(), String> {
        if self.inner.setup.account.can_host() {
            Ok(())
        } else {
            Err(AccountError::NotInvited.to_string())
        }
    }

    /// The server refused a call because this person may not host: show it at once.
    async fn not_invited(&self, e: &AccountError) {
        if *e == AccountError::NotInvited {
            self.inner.setup.account.learned(Some(false), None);
            self.refresh_account().await;
            self.poll_soon();
        }
    }

    async fn refresh_account(&self) {
        let view = self.inner.setup.account.view().await;
        let mut s = self.lock();
        if !view.signed_in {
            s.contacts.clear();
            s.rings.clear();
        }
        s.account = view;
    }

    // Polling.

    /// Polls every [`POLL_EVERY`] (or [`POLL_BACKOFF`]) until the app quits, and sooner when
    /// [`Calls::poll_soon`] is called.
    pub fn start_polling(&self) {
        let me = self.clone();
        self.inner.setup.runtime.spawn(async move {
            loop {
                me.poll_once().await;
                let delay = poll_delay(me.lock().failures);
                tokio::select! {
                    _ = tokio::time::sleep(delay) => {}
                    _ = me.inner.wake.notified() => {}
                }
            }
        });
    }

    /// Asks the polling task to poll now.
    pub fn poll_soon(&self) {
        self.inner.wake.notify_one();
    }

    /// One poll: the inbox (with this app's call as the heartbeat), then the knock's keep-alive
    /// or the guest's check that the call goes on. Never two at once.
    pub async fn poll_once(&self) -> Polled {
        let Ok(_one) = self.inner.poll.try_lock() else {
            return Polled::Busy;
        };
        let Ok(account) = self.inner.setup.account.account() else {
            return Polled::NotConfigured;
        };
        if !account.is_signed_in().await {
            self.refresh_account().await;
            self.emit();
            return Polled::SignedOut;
        }
        let (flow, hosting) = {
            let s = self.lock();
            (s.flow, s.hosting_id())
        };
        let mut outcome = Polled::Done;
        match account.inbox(hosting.as_deref()).await {
            Ok(inbox) => {
                self.inner.setup.account.set_me(Some(&inbox.me));
                self.lock().failures = 0;
                self.apply(&account, inbox, flow);
            }
            Err(AccountError::NotSignedIn) => {
                self.inner.setup.account.set_me(None);
                outcome = Polled::SignedOut;
            }
            Err(e) if is_network(&e) => {
                self.lock().failures += 1;
                outcome = Polled::Failed;
            }
            Err(_) => outcome = Polled::Failed,
        }
        self.keep_alive(&account).await;
        self.refresh_account().await;
        self.emit();
        outcome
    }

    fn apply(&self, account: &Arc<Account>, inbox: Inbox, flow: u64) {
        let mut leftover = None;
        let mut gone = false;
        {
            let mut s = self.lock();
            s.contacts = inbox.contacts.iter().map(person).collect();
            s.rings = inbox
                .rings
                .iter()
                .map(|r| RingView {
                    call_id: r.call_id.clone(),
                    host: person(&r.host),
                })
                .collect();
            if s.flow != flow {
                // The stage changed while the backend answered: the next poll is about it.
                return;
            }
            match (&s.stage, inbox.hosting) {
                (Stage::Hosting { call_id, .. }, Some(h)) if h.call_id == *call_id => {
                    let knocking: HashSet<String> = h
                        .members
                        .iter()
                        .filter(|m| m.state == MemberState::Knocking)
                        .map(|m| m.user_id.clone())
                        .collect();
                    s.knock_seen.retain(|id, _| knocking.contains(id));
                    s.members = h.members;
                }
                // The backend no longer has this app's call open: it is over.
                (Stage::Hosting { .. }, _) => gone = true,
                // A call this app isn't sharing, left open by a crash: end it.
                (Stage::Idle | Stage::Knocking { .. } | Stage::Joined { .. }, Some(h))
                    if s.leftover.as_deref() != Some(h.call_id.as_str()) =>
                {
                    s.leftover = Some(h.call_id.clone());
                    leftover = Some(h.call_id);
                }
                _ => {}
            }
        }
        if gone {
            self.call_over(flow, CALL_ENDED);
        }
        if let Some(call_id) = leftover {
            let account = account.clone();
            let me = self.clone();
            self.inner.setup.runtime.spawn(async move {
                let ended = account.end_call(&call_id).await;
                me.trace(
                    json!({ "kind": "leftover_ended", "call_id": call_id, "ok": ended.is_ok() }),
                );
            });
        }
    }

    async fn keep_alive(&self, account: &Arc<Account>) {
        let (flow, stage) = {
            let s = self.lock();
            (s.flow, s.stage.clone())
        };
        match stage {
            Stage::Knocking {
                call_id,
                code,
                host,
            } => match account.knock(&code).await {
                Ok(knock) => {
                    self.lock().knock_trouble_since = None;
                    match knock.state {
                        MemberState::Knocking => {}
                        MemberState::Admitted => {
                            let _ = self.join_call(flow, &call_id, host).await;
                        }
                        MemberState::TurnedAway => {
                            self.give_up(flow, &call_id, turned_away(&host.name), false)
                        }
                        _ => self.give_up(flow, &call_id, NO_LONGER_WAITING.into(), false),
                    }
                }
                Err(e) if is_network(&e) => {
                    let now = self.now();
                    let since = *self.lock().knock_trouble_since.get_or_insert(now);
                    if now.saturating_duration_since(since) >= KNOCK_GIVE_UP {
                        self.give_up(flow, &call_id, KNOCK_GAVE_UP.into(), true);
                    }
                }
                Err(AccountError::CallNotFound) => self.give_up(
                    flow,
                    &call_id,
                    AccountError::CallNotFound.to_string(),
                    false,
                ),
                Err(e) => self.give_up(flow, &call_id, e.to_string(), true),
            },
            Stage::Joined { call_id, .. } => {
                if let Ok(false) = account.can_join(&call_id).await {
                    self.call_over(flow, CALL_ENDED);
                }
            }
            _ => {}
        }
    }

    /// Stops knocking, with a sentence; tells the backend if `leave`.
    fn give_up(&self, flow: u64, call_id: &str, notice: String, leave: bool) {
        {
            let mut s = self.lock();
            if s.flow != flow {
                return;
            }
            s.idle(Some(notice), None);
        }
        if leave {
            self.leave_later(call_id);
        }
        self.emit();
    }

    fn leave_later(&self, call_id: &str) {
        if let Ok(account) = self.inner.setup.account.account() {
            let call_id = call_id.to_owned();
            self.inner.setup.runtime.spawn(async move {
                let _ = account.leave_call(&call_id).await;
            });
        }
    }

    /// The call this app is in is over on the server: leave it here too.
    fn call_over(&self, flow: u64, notice: &str) {
        {
            let mut s = self.lock();
            if s.flow != flow || !matches!(s.stage, Stage::Hosting { .. } | Stage::Joined { .. }) {
                return;
            }
            if let Some(running) = s.engine.as_mut() {
                // The backend side is done already.
                running.call = None;
                running.handle.leave();
            }
            s.idle(Some(notice.to_owned()), None);
        }
        self.emit();
    }

    // Permissions.

    /// Whether `which` is granted, asking macOS the first time it is missing in this run. Sets
    /// or clears what the view says is needed. Refused again after macOS was asked, it is
    /// [`HomeView::stuck`]. While it is missing it is checked again every [`RECHECK_EVERY`].
    async fn ensure(&self, which: Permission) -> Result<bool, String> {
        let permits = self.inner.setup.permits.clone();
        let asked = self.lock().requested.contains(&which);
        let (granted, requested) = tokio::task::spawn_blocking(move || {
            if permits.granted(which)? {
                return Ok((true, false));
            }
            if asked {
                return Ok((false, false));
            }
            permits.request(which).map(|g| (g, true))
        })
        .await
        .map_err(|e| e.to_string())
        .and_then(|r: Result<(bool, bool), String>| r)
        .inspect_err(|e| {
            crate::applog::line(format!(
                "permission: {} couldn't be checked: {e}",
                which.word()
            ))
        })?;
        crate::applog::line(format!(
            "permission: {} {}{}",
            which.word(),
            if granted { "granted" } else { "not granted" },
            match (requested, asked) {
                (true, _) => ", after asking macOS",
                (false, true) => ", macOS was asked earlier in this run",
                (false, false) => "",
            }
        ));
        {
            let mut s = self.lock();
            if requested {
                s.requested.insert(which);
            }
            if granted {
                if s.needs == Some(which) {
                    Self::no_longer_needed(&mut s, None);
                }
            } else {
                if s.needs != Some(which) {
                    s.stuck = false;
                }
                s.needs = Some(which);
                if asked {
                    s.stuck = true;
                }
            }
        }
        if !granted {
            self.start_rechecking();
        }
        Ok(granted)
    }

    /// `needs` is granted now: the view stops asking, and a notice that said it was missing
    /// gives way to `notice`.
    fn no_longer_needed(s: &mut State, notice: Option<&str>) {
        s.needs = None;
        s.stuck = false;
        s.rechecked = None;
        if s.notice.is_some() && s.notice == s.notice_needs {
            s.notice = notice.map(str::to_owned);
        } else if let Some(notice) = notice {
            s.notice = Some(notice.to_owned());
        }
        s.notice_needs = None;
    }

    /// A notice that says a permission is missing.
    fn needs_notice(s: &mut State, text: String) {
        s.notice = Some(text.clone());
        s.notice_needs = Some(text);
    }

    /// Checks `needs` again every [`RECHECK_EVERY`] until it is granted. One timer at a time.
    fn start_rechecking(&self) {
        {
            let mut s = self.lock();
            if s.rechecking || s.needs.is_none() {
                return;
            }
            s.rechecking = true;
        }
        let me = self.clone();
        self.inner.setup.runtime.spawn(async move {
            loop {
                tokio::time::sleep(RECHECK_EVERY).await;
                me.recheck().await;
                let mut s = me.lock();
                if s.needs.is_none() {
                    s.rechecking = false;
                    break;
                }
            }
        });
    }

    /// Checks the needed permission again, without asking macOS. Granted, the view stops
    /// saying it is needed; still refused after macOS was asked and the person was sent to
    /// System Settings, it is [`HomeView::stuck`]. Never gives anyone control.
    pub async fn recheck(&self) {
        let needs = self.lock().needs;
        let Some(which) = needs else {
            return;
        };
        let permits = self.inner.setup.permits.clone();
        let answer = tokio::task::spawn_blocking(move || permits.granted(which))
            .await
            .map_err(|e| e.to_string())
            .and_then(|r| r);
        {
            let mut s = self.lock();
            match answer {
                Ok(granted) if s.needs == Some(which) => {
                    if s.rechecked != Some((which, granted)) {
                        crate::applog::line(format!(
                            "permission: {} {} at a re-check",
                            which.word(),
                            if granted {
                                "granted"
                            } else {
                                "still not granted"
                            }
                        ));
                        s.rechecked = Some((which, granted));
                    }
                    if granted {
                        let notice = (which == Permission::Accessibility && s.owed.is_none())
                            .then_some(ACCESSIBILITY_ON);
                        Self::no_longer_needed(&mut s, notice);
                    } else if s.requested.contains(&which) && s.sent_to_settings.contains(&which) {
                        s.stuck = true;
                    }
                }
                Ok(_) => {}
                Err(e) => crate::applog::line(format!(
                    "permission: {} couldn't be re-checked: {e}",
                    which.word()
                )),
            }
        }
        self.emit();
    }

    /// The window opened System Settings for `which` (its Open Settings).
    pub fn opened_settings(&self, which: Permission) {
        crate::applog::line(format!(
            "permission: System Settings opened for {}",
            which.word()
        ));
        self.lock().sent_to_settings.insert(which);
        self.start_rechecking();
    }

    /// Reset and ask again: clears TwinCursor's own entry for `which` in macOS's privacy list
    /// (`tccutil reset`) and asks macOS again, which lists this copy of the app afresh. Only
    /// while `which` is needed. Touches no other app's entry, and gives nobody control.
    pub async fn reset_permission(&self, which: Permission) -> Result<HomeView, String> {
        if self.lock().needs != Some(which) {
            return Err(NOTHING_TO_RESET.into());
        }
        let permits = self.inner.setup.permits.clone();
        let (code, granted) = tokio::task::spawn_blocking(move || {
            let code = permits.reset(which)?;
            let granted = if code == 0 {
                permits.request(which)?
            } else {
                permits.granted(which)?
            };
            Ok((code, granted))
        })
        .await
        .map_err(|e| e.to_string())
        .and_then(|r: Result<(i32, bool), String>| r)
        .inspect_err(|e| {
            crate::applog::line(format!("permission: {} reset failed: {e}", which.word()))
        })?;
        crate::applog::line(format!(
            "permission: {} reset (tccutil exit {code}){}; {}",
            which.word(),
            if code == 0 { ", macOS asked again" } else { "" },
            if granted { "granted" } else { "not granted" }
        ));
        {
            let mut s = self.lock();
            s.requested.insert(which);
            s.sent_to_settings.remove(&which);
            s.stuck = false;
            s.rechecked = None;
            if granted && s.needs == Some(which) {
                Self::no_longer_needed(&mut s, None);
            }
        }
        self.start_rechecking();
        self.emit();
        if code != 0 {
            return Err(reset_failed(code));
        }
        Ok(self.home())
    }

    // Engines.

    fn sink_for(&self, gen: u64) -> Sink {
        let weak = Arc::downgrade(&self.inner);
        Arc::new(move |output: Output| {
            if let Some(inner) = weak.upgrade() {
                Calls { inner }.on_output(gen, output);
            }
        })
    }

    /// Starts an engine for an account call, with its pass in the environment. A replacement
    /// engine (the switch to H.264) gets a fresh pass. Returns its generation.
    fn start_engine(&self, role: Role, pass: &CallPass, call: CallRef) -> Result<u64, String> {
        let account = self.inner.setup.account.account()?;
        let gen = {
            let mut s = self.lock();
            if s.engine.as_ref().is_some_and(|r| !r.handle.is_done()) {
                return Err(IN_CALL.into());
            }
            s.engine = None;
            s.gen += 1;
            s.proc = 0;
            s.early_exit = None;
            s.gen
        };
        let plan = Plan {
            role,
            room: String::new(),
            name: String::new(),
            quality: self.inner.setup.options.quality,
            voice: self.inner.setup.options.voice,
            pass: true,
        };
        let refresh: EnvHook = {
            let runtime = self.inner.setup.runtime.clone();
            let call_id = call.call_id.clone();
            let trace = self.inner.setup.trace.clone();
            Arc::new(move || {
                let pass = runtime
                    .block_on(account.call_pass(&call_id))
                    .map_err(|e| format!("The call couldn't go on: {e}"))?;
                trace(json!({ "kind": "pass", "identity": pass.identity, "reason": "restart" }));
                Ok(pass_env(&pass))
            })
        };
        let spec = EngineSpec {
            plan,
            env: pass_env(pass),
            refresh_env: Some(refresh),
        };
        let handle = self.inner.setup.engines.start(spec, self.sink_for(gen))?;
        let mut s = self.lock();
        s.engine = Some(Running {
            handle,
            gen,
            call: Some(call),
        });
        if s.early_exit == Some(gen) || !s.engine_alive(gen) {
            s.engine = None;
            return Err(s
                .engine_error
                .clone()
                .unwrap_or_else(|| ENGINE_STOPPED.into()));
        }
        Ok(gen)
    }

    /// A room-code call from the start screen, as before accounts. The flows leave it alone.
    pub fn start_plan(&self, plan: Plan, env: Vec<(String, String)>) -> Result<(), String> {
        let plan = plan.checked()?;
        let gen = {
            let mut s = self.lock();
            if !s.free() {
                return Err("A call is already running.".into());
            }
            s.engine = None;
            s.gen += 1;
            s.proc = 0;
            s.early_exit = None;
            s.gen
        };
        let spec = EngineSpec {
            plan,
            env,
            refresh_env: None,
        };
        let handle = self.inner.setup.engines.start(spec, self.sink_for(gen))?;
        let mut s = self.lock();
        if s.early_exit != Some(gen) {
            s.engine = Some(Running {
                handle,
                gen,
                call: None,
            });
        }
        Ok(())
    }

    /// Everything an engine says, on the engine's thread.
    fn on_output(&self, gen: u64, output: Output) {
        let mut follow: Option<(String, bool)> = None;
        {
            let mut s = self.lock();
            if gen != s.gen {
                // An engine this app has moved on from.
                return;
            }
            let now = self.now();
            match &output {
                Output::Started { .. } => {
                    if s.proc == 0 {
                        s.snapshot.clear();
                        s.engine_error = None;
                        s.engine_failed = false;
                    }
                    s.proc += 1;
                    s.asked_seen.clear();
                    s.participants.clear();
                }
                Output::Restarting { message, .. } => {
                    s.asked_seen.clear();
                    s.notice = Some(restarted(message));
                }
                Output::Event { event } => self.on_event(&mut s, event, now),
                Output::Log { .. } => {}
                Output::Exited { code } => {
                    if *code != Some(0) {
                        s.engine_failed = true;
                    }
                    follow = self.on_exit(&mut s, gen);
                }
            }
            s.snapshot.push(&output);
        }
        (self.inner.setup.on_engine)(output);
        if let Some((call_id, host)) = follow {
            if let Ok(account) = self.inner.setup.account.account() {
                let trace = self.inner.setup.trace.clone();
                self.inner.setup.runtime.spawn(async move {
                    let ok = if host {
                        account.end_call(&call_id).await.is_ok()
                    } else {
                        account.leave_call(&call_id).await.is_ok()
                    };
                    trace(
                        json!({ "kind": if host { "ended_call" } else { "left_call" }, "ok": ok }),
                    );
                });
            }
        }
        self.emit();
    }

    fn on_event(&self, s: &mut State, event: &Value, now: Instant) {
        match event.get("event").and_then(Value::as_str).unwrap_or("") {
            "control_asked" => {
                if let Some(request) = event.get("request").and_then(Value::as_u64) {
                    let proc = s.proc;
                    let name = event
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("Someone")
                        .to_owned();
                    // First delivery: the window hears it right after this.
                    s.asked_seen.entry(request).or_insert((now, proc, name));
                }
            }
            "participants" => {
                s.participants = event
                    .get("participants")
                    .cloned()
                    .and_then(|p| serde_json::from_value(p).ok())
                    .unwrap_or_default();
            }
            "error" => {
                s.engine_error = event
                    .get("message")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
            }
            "ended" if event.get("reason").and_then(Value::as_str) == Some("failed") => {
                s.engine_failed = true;
            }
            _ => {}
        }
    }

    /// The engine is gone. Returns the backend call to make, if the engine ended on its own.
    fn on_exit(&self, s: &mut State, gen: u64) -> Option<(String, bool)> {
        let running = match s.engine.take() {
            Some(r) if r.gen == gen => Some(r),
            other => {
                s.engine = other;
                s.early_exit = Some(gen);
                None
            }
        };
        s.asked_seen.clear();
        s.participants.clear();
        let call = running.and_then(|r| r.call)?;
        let ours = match &s.stage {
            Stage::Hosting { call_id, .. } | Stage::Joined { call_id, .. } => {
                *call_id == call.call_id
            }
            _ => false,
        };
        if !ours {
            return None;
        }
        let error = s.engine_failed.then(|| {
            s.engine_error
                .clone()
                .unwrap_or_else(|| ENGINE_STOPPED.into())
        });
        s.idle(None, error);
        Some((call.call_id, call.host))
    }

    /// A command from the window for the engine. An allow must name a request the window was
    /// shown at least [`ARMING`] ago, and goes only to the engine that asked. Trusting someone
    /// isn't a window command: `trust_control` and `trust_on_arrival` are refused here.
    pub async fn send_command(&self, command: &ipc::Command) -> Result<(), String> {
        if let ipc::Command::AllowControl { request } = command {
            return self.allow_control(*request).await;
        }
        let s = self.lock();
        let running = s
            .engine
            .as_ref()
            .filter(|r| !r.handle.is_done())
            .ok_or(NO_CALL)?;
        match command {
            // Only Let in with control and Give control trust anyone, after their checks.
            ipc::Command::TrustControl { .. } | ipc::Command::TrustOnArrival { .. } => {
                Err(TRUST_FROM_WINDOW.into())
            }
            _ => running.handle.send(command),
        }
    }

    /// Allow on a control request: armed first, then Accessibility, as Let in with control and
    /// Give control check it. Without Accessibility macOS is asked (once per run), the request
    /// is turned down so the guest isn't left waiting, and the notice says why; they can ask
    /// again once it is granted.
    async fn allow_control(&self, request: u64) -> Result<(), String> {
        let (proc, name) = self.open_request(request)?;
        if !self.ensure(Permission::Accessibility).await? {
            {
                let mut s = self.lock();
                if let Some(running) = s.engine.as_ref().filter(|r| !r.handle.is_done()) {
                    let _ = running
                        .handle
                        .send_for(&ipc::Command::DenyControl { request }, proc);
                }
                s.asked_seen.remove(&request);
                Self::needs_notice(&mut s, not_given_control(&name));
            }
            self.trace(json!({ "kind": "allow_refused", "why": "accessibility" }));
            self.emit();
            return Ok(());
        }
        // Asking macOS can take a while: the request must still be open, from the same engine.
        let s = self.lock();
        let running = s
            .engine
            .as_ref()
            .filter(|r| !r.handle.is_done())
            .ok_or(NO_CALL)?;
        if s.asked_seen.get(&request).map(|a| a.1) != Some(proc) {
            return Err(REQUEST_NOT_OPEN.into());
        }
        running
            .handle
            .send_for(&ipc::Command::AllowControl { request }, proc)
    }

    /// The engine process that showed `request` and who asked, if it is open and armed.
    fn open_request(&self, request: u64) -> Result<(u64, String), String> {
        let s = self.lock();
        s.engine
            .as_ref()
            .filter(|r| !r.handle.is_done())
            .ok_or(NO_CALL)?;
        let (seen, proc, name) = s.asked_seen.get(&request).ok_or(REQUEST_NOT_OPEN)?;
        if !armed(self.now(), *seen, s.sight) {
            return Err(TOO_SOON.into());
        }
        Ok((*proc, name.clone()))
    }

    /// Leaves the engine's call from the call screen. If it was an account call, the backend is
    /// told when the engine has gone.
    pub fn leave_engine(&self) {
        if let Some(running) = self.lock().engine.as_ref() {
            running.handle.leave();
        }
    }

    /// The app is quitting: the engine stops now, and the backend is told, briefly.
    pub fn shutdown(&self) {
        let running = self.lock().engine.take();
        let call = running.and_then(|r| {
            let call = r.call.clone();
            r.handle.shutdown();
            call
        });
        let Some(call) = call else { return };
        let Ok(account) = self.inner.setup.account.account() else {
            return;
        };
        let _ = self.inner.setup.runtime.block_on(async move {
            tokio::time::timeout(Duration::from_secs(2), async {
                if call.host {
                    account.end_call(&call.call_id).await
                } else {
                    account.leave_call(&call.call_id).await
                }
            })
            .await
        });
    }

    async fn wait_engine_gone(&self) {
        let deadline = tokio::time::Instant::now() + ENGINE_GONE_TIMEOUT;
        loop {
            let alive = self
                .lock()
                .engine
                .as_ref()
                .is_some_and(|r| !r.handle.is_done());
            if !alive || tokio::time::Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let lingering = self.lock().engine.take();
        if let Some(running) = lingering {
            let _ = tokio::task::spawn_blocking(move || running.handle.shutdown()).await;
        }
    }

    // The flows.

    fn begin_flow(&self) -> Result<u64, String> {
        let mut s = self.lock();
        if !s.free() {
            return Err(IN_CALL.into());
        }
        s.flow += 1;
        s.stage = Stage::Starting;
        s.notice = None;
        s.error = None;
        Ok(s.flow)
    }

    fn fail(&self, flow: u64, error: String) -> Result<HomeView, String> {
        {
            let mut s = self.lock();
            if s.flow == flow {
                s.idle(None, Some(error.clone()));
            }
        }
        self.emit();
        Err(error)
    }

    fn current(&self, flow: u64) -> bool {
        self.lock().flow == flow
    }

    /// Share my screen: start a call, get its pass, start the engine as sharer, put the link on
    /// the clipboard.
    pub async fn share(&self) -> Result<HomeView, String> {
        if !self.inner.setup.options.can_share {
            return Err(crate::platform::CANT_SHARE.into());
        }
        let account = self.signed_in().await?;
        if !self.lock().free() {
            return Err(IN_CALL.into());
        }
        self.may_host()?;
        match self.ensure(Permission::ScreenRecording).await {
            Ok(true) => {}
            Ok(false) => {
                self.emit();
                return Err(NEEDS_SCREEN_RECORDING.into());
            }
            Err(e) => return Err(e),
        }
        let flow = self.begin_flow()?;
        self.emit();
        let started = match account.start_call().await {
            Ok(started) => started,
            Err(e) => {
                self.not_invited(&e).await;
                return self.fail(flow, e.to_string());
            }
        };
        let call_id = started.call_id.clone();
        let ended = |me: &Calls| {
            let account = account.clone();
            let call_id = call_id.clone();
            me.inner.setup.runtime.spawn(async move {
                let _ = account.end_call(&call_id).await;
            });
        };
        let pass = match account.call_pass(&call_id).await {
            Ok(pass) => pass,
            Err(e) => {
                ended(self);
                return self.fail(flow, e.to_string());
            }
        };
        if !self.current(flow) {
            ended(self);
            return Err(STOPPED.into());
        }
        let call = CallRef {
            call_id: call_id.clone(),
            host: true,
        };
        let gen = match self.start_engine(Role::Share, &pass, call) {
            Ok(gen) => gen,
            Err(e) => {
                ended(self);
                return self.fail(flow, e);
            }
        };
        self.trace(json!({ "kind": "pass", "identity": pass.identity, "reason": "start" }));
        let link = link::make(&started.code).map_err(|e| e.to_string());
        {
            let mut s = self.lock();
            if s.flow != flow || !s.engine_alive(gen) {
                drop(s);
                self.leave_engine();
                ended(self);
                return self.fail(flow, ENGINE_STOPPED.into());
            }
            s.stage = Stage::Hosting {
                call_id,
                code: started.code.clone(),
            };
            s.awake = Some(Awake::hold("Sharing a TwinCursor call"));
        }
        let copied = match link {
            Ok(link) => {
                let clipboard = self.inner.setup.clipboard.clone();
                tokio::task::spawn_blocking(move || clipboard.write(&link))
                    .await
                    .map_err(|e| e.to_string())
                    .and_then(|r| r)
            }
            Err(e) => Err(e),
        };
        self.lock().notice = Some(if copied.is_ok() {
            LINK_COPIED.into()
        } else {
            LINK_NOT_COPIED.into()
        });
        self.poll_soon();
        self.emit();
        Ok(self.home())
    }

    /// Paste link: the clipboard is read here, never by the window, and its text goes nowhere
    /// but the link parser.
    pub async fn join_from_clipboard(&self) -> Result<HomeView, String> {
        let clipboard = self.inner.setup.clipboard.clone();
        let text = tokio::task::spawn_blocking(move || clipboard.read(link::MAX_TEXT))
            .await
            .map_err(|e| e.to_string())??;
        let text = text.ok_or_else(|| link::LinkError::Empty.to_string())?;
        self.join_link(&text).await
    }

    /// Joins with a call link (or a bare code): knocks, and waits to be let in.
    pub async fn join_link(&self, text: &str) -> Result<HomeView, String> {
        let code = link::parse(text).map_err(|e| e.to_string())?;
        let account = self.signed_in().await?;
        let flow = self.begin_flow()?;
        self.emit();
        let knock = match account.knock(&code).await {
            Ok(knock) => knock,
            Err(e) => return self.fail(flow, e.to_string()),
        };
        let host = person(&knock.host);
        match knock.state {
            MemberState::Knocking => {
                {
                    let mut s = self.lock();
                    if s.flow != flow {
                        drop(s);
                        self.leave_later(&knock.call_id);
                        return Err(STOPPED.into());
                    }
                    s.stage = Stage::Knocking {
                        call_id: knock.call_id,
                        code,
                        host,
                    };
                    s.awake = Some(Awake::hold("Waiting to join a TwinCursor call"));
                }
                self.poll_soon();
                self.emit();
                Ok(self.home())
            }
            MemberState::Admitted => self.join_call(flow, &knock.call_id, host).await,
            MemberState::TurnedAway => self.fail(flow, turned_away(&host.name)),
            _ => self.fail(flow, NO_LONGER_WAITING.into()),
        }
    }

    /// Let in (or accepted a ring): get a pass and start the engine as a guest.
    async fn join_call(&self, flow: u64, call_id: &str, host: Person) -> Result<HomeView, String> {
        let account = self.inner.setup.account.account()?;
        let pass = match account.call_pass(call_id).await {
            Ok(pass) => pass,
            Err(e) => {
                self.leave_later(call_id);
                return self.fail(flow, e.to_string());
            }
        };
        if !self.current(flow) {
            self.leave_later(call_id);
            return Err(STOPPED.into());
        }
        let call = CallRef {
            call_id: call_id.to_owned(),
            host: false,
        };
        let gen = match self.start_engine(Role::Join, &pass, call) {
            Ok(gen) => gen,
            Err(e) => {
                self.leave_later(call_id);
                return self.fail(flow, e);
            }
        };
        self.trace(json!({ "kind": "pass", "identity": pass.identity, "reason": "start" }));
        {
            let mut s = self.lock();
            if s.flow != flow || !s.engine_alive(gen) {
                drop(s);
                self.leave_engine();
                self.leave_later(call_id);
                return self.fail(flow, ENGINE_STOPPED.into());
            }
            s.stage = Stage::Joined {
                call_id: call_id.to_owned(),
                host,
            };
            s.knock_trouble_since = None;
            s.notice = None;
            s.awake = Some(Awake::hold("In a TwinCursor call"));
        }
        self.emit();
        Ok(self.home())
    }

    /// Stops waiting to be let in.
    pub async fn cancel_knock(&self) -> Result<HomeView, String> {
        let call_id = {
            let mut s = self.lock();
            let Stage::Knocking { call_id, .. } = s.stage.clone() else {
                return Err(NOT_KNOCKING.into());
            };
            s.idle(None, None);
            call_id
        };
        self.emit();
        if let Ok(account) = self.inner.setup.account.account() {
            let _ = account.leave_call(&call_id).await;
        }
        Ok(self.home())
    }

    /// Lets someone knocking in (maybe with control), or turns them away.
    pub async fn answer_knock(
        &self,
        user_id: &str,
        let_in: bool,
        control: bool,
    ) -> Result<HomeView, String> {
        let account = self.signed_in().await?;
        let (call_id, name) = {
            let s = self.lock();
            let call_id = s.hosting_id().ok_or(NOT_HOSTING)?;
            let member = s
                .members
                .iter()
                .find(|m| m.user_id == user_id && m.state == MemberState::Knocking)
                .ok_or(NOT_WAITING)?;
            if let_in {
                let seen = s.knock_seen.get(user_id).ok_or(TOO_SOON)?;
                if !armed(self.now(), *seen, s.sight) {
                    return Err(TOO_SOON.into());
                }
            }
            (call_id, member.name.clone())
        };
        if !let_in {
            account
                .answer_knock(&call_id, user_id, false)
                .await
                .map_err(|e| e.to_string())?;
            self.set_member_state(user_id, MemberState::TurnedAway);
            self.emit();
            return Ok(self.home());
        }
        let give = control && self.ensure(Permission::Accessibility).await?;
        let state = account
            .answer_knock(&call_id, user_id, true)
            .await
            .map_err(|e| e.to_string())?;
        self.set_member_state(user_id, state);
        if give && state == MemberState::Admitted {
            self.trust_on_arrival(&call_id, &format!("guest-{user_id}"));
        }
        if control && !give {
            let mut s = self.lock();
            Self::needs_notice(&mut s, let_in_without_control(&name));
            if state == MemberState::Admitted {
                s.owed = Some(Owed {
                    identity: format!("guest-{user_id}"),
                    name: name.clone(),
                    shown: None,
                });
            }
        }
        self.trace(json!({ "kind": "let_in", "user_id": user_id, "control": give }));
        self.emit();
        Ok(self.home())
    }

    /// Asks the engine running `call_id` to trust `identity` when they join (now, if they are
    /// in the call already). Once, to that engine process only; the engine keeps the promise
    /// and every revoke cancels it.
    fn trust_on_arrival(&self, call_id: &str, identity: &str) {
        let s = self.lock();
        let ours = s.hosting_id().as_deref() == Some(call_id);
        let Some(running) = s.engine.as_ref().filter(|r| ours && !r.handle.is_done()) else {
            return;
        };
        let command = ipc::Command::TrustOnArrival {
            identity: identity.to_owned(),
        };
        let sent = running.handle.send_for(&command, s.proc).is_ok();
        self.trace(
            json!({ "kind": "trust_sent", "identity": identity, "why": "on_arrival", "ok": sent }),
        );
    }

    fn set_member_state(&self, user_id: &str, state: MemberState) {
        let mut s = self.lock();
        s.knock_seen.remove(user_id);
        for m in s.members.iter_mut().filter(|m| m.user_id == user_id) {
            m.state = state;
        }
    }

    /// Gives control to someone already in this app's call: their requests are granted without
    /// a prompt until a revoke.
    pub async fn give_control(&self, identity: &str) -> Result<HomeView, String> {
        let name = {
            let s = self.lock();
            s.hosting_id().ok_or(NOT_HOSTING)?;
            s.participants
                .iter()
                .find(|p| p.identity == identity && p.role == ipc::Role::Guest)
                .map(|p| p.name.clone())
                .ok_or(NOT_IN_THE_CALL)?
        };
        if !self.ensure(Permission::Accessibility).await? {
            self.lock().owed = Some(Owed {
                identity: identity.to_owned(),
                name,
                shown: None,
            });
            self.emit();
            return Err(NEEDS_ACCESSIBILITY.into());
        }
        {
            let mut s = self.lock();
            let running = s.engine.as_ref().ok_or(NO_CALL)?;
            running.handle.send_for(
                &ipc::Command::TrustControl {
                    identity: identity.to_owned(),
                },
                s.proc,
            )?;
            if s.owed.as_ref().is_some_and(|o| o.identity == identity) {
                s.owed = None;
            }
        }
        self.trace(json!({ "kind": "trust_sent", "identity": identity, "why": "give_control", "ok": true }));
        self.emit();
        Ok(self.home())
    }

    /// Give control to the guest [`HomeView::owed`] names: armed like a knock, then exactly
    /// what Give control does (or, for someone let in who hasn't joined yet, what Let in with
    /// control does). The Accessibility check comes first, as always.
    pub async fn give_owed_control(&self) -> Result<HomeView, String> {
        let (identity, present, call_id) = {
            let s = self.lock();
            let call_id = s.hosting_id().ok_or(NOT_HOSTING)?;
            let owed = s.owed.as_ref().ok_or(NOBODY_OWED)?;
            let shown = owed.shown.ok_or(TOO_SOON)?;
            if !armed(self.now(), shown, s.sight) {
                return Err(TOO_SOON.into());
            }
            let present = s
                .participants
                .iter()
                .any(|p| p.identity == owed.identity && p.role == ipc::Role::Guest);
            (owed.identity.clone(), present, call_id)
        };
        if present {
            return self.give_control(&identity).await;
        }
        if !self.ensure(Permission::Accessibility).await? {
            self.emit();
            return Err(NEEDS_ACCESSIBILITY.into());
        }
        self.trust_on_arrival(&call_id, &identity);
        self.lock().owed = None;
        self.emit();
        Ok(self.home())
    }

    /// Rings a contact into this app's call, sharing first if there is no call yet.
    pub async fn ring(&self, user_id: &str) -> Result<HomeView, String> {
        if !self.inner.setup.options.can_share {
            return Err(crate::platform::CANT_SHARE.into());
        }
        let account = self.signed_in().await?;
        self.may_host()?;
        let hosting = {
            let s = self.lock();
            match &s.stage {
                Stage::Hosting { call_id, .. } => Some(call_id.clone()),
                Stage::Idle if s.free() => None,
                _ => return Err(IN_CALL.into()),
            }
        };
        if hosting.is_none() {
            self.share().await?;
        }
        let rung = account.ring(user_id).await;
        match rung {
            Ok(started) => {
                self.trace(json!({ "kind": "rang", "user_id": user_id, "same_call": Some(&started.call_id) == self.lock().hosting_id().as_ref() }));
                self.poll_soon();
                self.emit();
                Ok(self.home())
            }
            Err(e) => {
                self.not_invited(&e).await;
                self.lock().error = Some(e.to_string());
                self.emit();
                Err(e.to_string())
            }
        }
    }

    /// Ask to be invited: tells whoever runs the server this person would like to start calls.
    pub async fn ask_to_host(&self) -> Result<HomeView, String> {
        let account = self.signed_in().await?;
        account.ask_to_host().await.map_err(|e| e.to_string())?;
        self.inner.setup.account.learned(None, Some(true));
        self.refresh_account().await;
        self.emit();
        Ok(self.home())
    }

    /// Answers a call ringing this person. Accepting ends or leaves any call this app is in.
    pub async fn answer_ring(&self, call_id: &str, accept: bool) -> Result<HomeView, String> {
        let account = self.signed_in().await?;
        let ring = self
            .lock()
            .rings
            .iter()
            .find(|r| r.call_id == call_id)
            .cloned()
            .ok_or(NO_RING)?;
        if !accept {
            account
                .answer_ring(call_id, false)
                .await
                .map_err(|e| e.to_string())?;
            self.lock().rings.retain(|r| r.call_id != call_id);
            self.emit();
            return Ok(self.home());
        }
        if !self.lock().free() {
            self.end().await?;
        }
        let flow = self.begin_flow()?;
        self.lock().rings.retain(|r| r.call_id != call_id);
        self.emit();
        match account.answer_ring(call_id, true).await {
            Ok(MemberState::Admitted) => self.join_call(flow, call_id, ring.host).await,
            Ok(_) => self.fail(flow, NO_RING.into()),
            Err(e) => self.fail(flow, e.to_string()),
        }
    }

    /// Stop sharing, or leave: the engine leaves first, then the backend is told.
    pub async fn end(&self) -> Result<HomeView, String> {
        enum Then {
            Done,
            Leave,
            Cancel,
            Tell(String, bool),
        }
        let then = {
            let mut s = self.lock();
            match s.stage.clone() {
                Stage::Idle => Then::Leave,
                Stage::Ending => Then::Done,
                Stage::Starting => {
                    // The flow under way sees this and undoes what it started.
                    s.idle(None, None);
                    Then::Done
                }
                Stage::Knocking { .. } => Then::Cancel,
                Stage::Hosting { call_id, .. } => Then::Tell(call_id, true),
                Stage::Joined { call_id, .. } => Then::Tell(call_id, false),
            }
        };
        let what = match then {
            Then::Done => {
                self.emit();
                return Ok(self.home());
            }
            Then::Leave => {
                self.leave_engine();
                return Ok(self.home());
            }
            Then::Cancel => return self.cancel_knock().await,
            Then::Tell(call_id, host) => (call_id, host),
        };
        let flow = {
            let mut s = self.lock();
            s.stage = Stage::Ending;
            s.flow += 1;
            if let Some(running) = s.engine.as_mut() {
                running.call = None;
                running.handle.leave();
            }
            s.flow
        };
        self.emit();
        self.wait_engine_gone().await;
        let told = match self.inner.setup.account.account() {
            Ok(account) if what.1 => account.end_call(&what.0).await,
            Ok(account) => account.leave_call(&what.0).await,
            Err(_) => Ok(()),
        };
        {
            let mut s = self.lock();
            if s.flow == flow {
                s.idle(None, None);
            }
            if let Err(e) = told {
                s.error = Some(e.to_string());
            }
        }
        self.emit();
        Ok(self.home())
    }

    /// Signing in or out changed who this is: poll now.
    pub async fn refresh(&self) {
        if self.poll_once().await == Polled::Busy {
            self.poll_soon();
        }
    }

    /// The identity of the guest in this app's call with this name, for scripts.
    pub fn guest_identity(&self, name: &str) -> Option<String> {
        self.lock()
            .participants
            .iter()
            .find(|p| p.name == name && p.role == ipc::Role::Guest)
            .map(|p| p.identity.clone())
    }

    /// Whether this app is in a call of any kind.
    pub fn busy(&self) -> bool {
        !self.lock().free()
    }
}

#[cfg(test)]
mod tests;
