//! The call flows against a fake backend (`account::fake`) and a fake engine.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use account::fake::{Fake, Reply, Request};
use account::{Backend, FileStore, Secret, Session, SessionStore};
use serde_json::{json, Value};

use super::*;
use crate::engine::Codec;
use crate::seams::{ManualClock, MemoryClipboard};

const KEY: &str = "public-anon-key";
const ME: &str = "00000000-0000-4000-8000-00000000000a";
const SAM: &str = "00000000-0000-4000-8000-00000000000b";
const KIM: &str = "00000000-0000-4000-8000-00000000000c";
const CALL: &str = "11111111-1111-4111-8111-111111111111";
const OTHER_CALL: &str = "22222222-2222-4222-8222-222222222222";
const CODE: &str = "0123456789abcdef0123456789abcdef";
const ACCESS: &str = "access.token-payload-that-must-not-leak.sig";
const REFRESH: &str = "refresh-token-that-must-not-leak";
const PASS: &str = "eyJhbGciOiJIUzI1NiJ9.pass-payload-that-must-not-leak";

fn profile(id: &str, name: &str) -> Value {
    json!({ "id": id, "name": name, "handle": null, "avatar_url": null })
}

/// What the fake backend answers; tests change it as they go.
struct Knobs {
    down: bool,
    signed_in: bool,
    start_call: Result<(), u16>,
    pass: Result<(), u16>,
    passes: usize,
    knock: Result<&'static str, &'static str>,
    answer_knock: &'static str,
    can_join: bool,
    hosting: Option<Value>,
    rings: Value,
    contacts: Value,
    inbox_delay: Duration,
}

impl Default for Knobs {
    fn default() -> Knobs {
        Knobs {
            down: false,
            signed_in: true,
            start_call: Ok(()),
            pass: Ok(()),
            passes: 0,
            knock: Ok("knocking"),
            answer_knock: "admitted",
            can_join: true,
            hosting: None,
            rings: json!([]),
            contacts: json!([]),
            inbox_delay: Duration::ZERO,
        }
    }
}

fn rest_error(status: u16, word: &str) -> Reply {
    Reply::json(status, json!({ "code": "P0001", "message": word }))
}

fn answer(knobs: &Mutex<Knobs>, r: &Request) -> Reply {
    let mut k = knobs.lock().unwrap();
    if k.down {
        return Reply::json(503, json!({}));
    }
    if !k.signed_in {
        return Reply::json(401, json!({}));
    }
    let path = r.target.as_str();
    match path {
        "/rest/v1/rpc/inbox" => Reply::json(
            200,
            json!({
                "me": profile(ME, "Alex"),
                "contacts": k.contacts,
                "hosting": k.hosting,
                "rings": k.rings,
                "waiting": [],
            }),
        )
        .after(k.inbox_delay),
        "/rest/v1/rpc/start_call" | "/rest/v1/rpc/ring" => match k.start_call {
            Ok(()) => Reply::json(200, json!([{ "call_id": CALL, "code": CODE }])),
            Err(status) => Reply::json(status, json!({})),
        },
        "/functions/v1/call-pass" => match k.pass {
            Ok(()) => {
                k.passes += 1;
                Reply::json(
                    200,
                    json!({
                        "url": "ws://127.0.0.1:7880", "token": format!("{PASS}-{}", k.passes),
                        "room": "room-1", "identity": format!("sharer-{ME}"), "name": "Alex",
                        "role": "sharer"
                    }),
                )
            }
            Err(status) => Reply::json(status, json!({ "error": "not_allowed" })),
        },
        "/rest/v1/rpc/knock" => match k.knock {
            Ok(state) => Reply::json(
                200,
                json!([{ "call_id": OTHER_CALL, "state": state, "host_id": SAM,
                         "host_name": "Sam", "host_handle": null, "host_avatar_url": null }]),
            ),
            Err(word) => rest_error(400, word),
        },
        "/rest/v1/rpc/answer_knock" => {
            let let_in = r.json()["let_in"] == true;
            Reply::json(
                200,
                json!(if let_in {
                    k.answer_knock
                } else {
                    "turned_away"
                }),
            )
        }
        "/rest/v1/rpc/answer_ring" => {
            let accept = r.json()["accept"] == true;
            Reply::json(200, json!(if accept { "admitted" } else { "declined" }))
        }
        "/rest/v1/rpc/pass_claims" => {
            if k.can_join {
                Reply::json(
                    200,
                    json!([{ "room": "r", "identity": "guest-x", "name": "A", "role": "guest" }]),
                )
            } else {
                Reply::json(403, json!({ "code": "42501", "message": "not_allowed" }))
            }
        }
        "/rest/v1/rpc/leave_call" | "/rest/v1/rpc/end_call" => Reply::empty(204),
        _ => Reply::empty(404),
    }
}

/// A fake engine: records what it was started with and sent, and says what the test makes it.
#[derive(Default)]
struct EngineLog {
    commands: Mutex<Vec<(ipc::Command, Option<u64>)>>,
    leaves: AtomicUsize,
    done: AtomicBool,
}

struct FakeHandle {
    log: Arc<EngineLog>,
    sink: Sink,
}

impl EngineHandle for FakeHandle {
    fn send(&self, command: &ipc::Command) -> Result<(), String> {
        self.log
            .commands
            .lock()
            .unwrap()
            .push((command.clone(), None));
        Ok(())
    }
    fn send_for(&self, command: &ipc::Command, engine: u64) -> Result<(), String> {
        self.log
            .commands
            .lock()
            .unwrap()
            .push((command.clone(), Some(engine)));
        Ok(())
    }
    fn leave(&self) {
        if self.log.leaves.fetch_add(1, Ordering::SeqCst) == 0 {
            let sink = self.sink.clone();
            let log = self.log.clone();
            std::thread::spawn(move || {
                sink(Output::Event {
                    event: json!({ "event": "ended", "reason": "left" }),
                });
                sink(Output::Exited { code: Some(0) });
                log.done.store(true, Ordering::SeqCst);
            });
        }
    }
    fn is_done(&self) -> bool {
        self.log.done.load(Ordering::SeqCst)
    }
    fn shutdown(self: Box<Self>) {
        self.log.done.store(true, Ordering::SeqCst);
    }
}

/// What a test looks at after the lock is gone.
struct Seen {
    plan: Plan,
    env: Vec<(String, String)>,
}

struct Started {
    plan: Plan,
    env: Vec<(String, String)>,
    refresh: Option<EnvHook>,
    sink: Sink,
    log: Arc<EngineLog>,
}

#[derive(Default)]
struct FakeEngines {
    started: Mutex<Vec<Started>>,
    fail: AtomicBool,
}

impl Engines for FakeEngines {
    fn start(&self, spec: EngineSpec, sink: Sink) -> Result<Box<dyn EngineHandle>, String> {
        if self.fail.load(Ordering::SeqCst) {
            return Err("The engine couldn't start (/x/tc-engine): gone.".into());
        }
        let log = Arc::new(EngineLog::default());
        let codec = if spec.plan.role == Role::Share {
            Codec::H265
        } else {
            Codec::H264
        };
        sink(Output::Started {
            role: spec.plan.role,
            codec,
            pid: 4242,
            args: crate::engine::engine_args(&spec.plan, codec, 7, &Default::default()),
        });
        self.started.lock().unwrap().push(Started {
            plan: spec.plan,
            env: spec.env,
            refresh: spec.refresh_env,
            sink: sink.clone(),
            log: log.clone(),
        });
        Ok(Box::new(FakeHandle { log, sink }))
    }
}

#[derive(Default)]
struct FakePermits {
    missing: Mutex<HashSet<Permission>>,
    requests: Mutex<Vec<Permission>>,
}

impl Permits for FakePermits {
    fn granted(&self, which: Permission) -> Result<bool, String> {
        Ok(!self.missing.lock().unwrap().contains(&which))
    }
    fn request(&self, which: Permission) -> Result<bool, String> {
        self.requests.lock().unwrap().push(which);
        self.granted(which)
    }
}

struct World {
    calls: Calls,
    fake: Fake,
    knobs: Arc<Mutex<Knobs>>,
    engines: Arc<FakeEngines>,
    clipboard: Arc<MemoryClipboard>,
    permits: Arc<FakePermits>,
    clock: Arc<ManualClock>,
    homes: Arc<Mutex<Vec<HomeView>>>,
    outputs: Arc<Mutex<Vec<Output>>>,
    traces: Arc<Mutex<Vec<Value>>>,
    dir: std::path::PathBuf,
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

static WORLDS: AtomicUsize = AtomicUsize::new(0);

async fn world(signed_in: bool) -> World {
    world_on(signed_in, true).await
}

/// A world on a platform that can share (macOS) or can't (Windows, a guest only).
async fn world_on(signed_in: bool, can_share: bool) -> World {
    let knobs = Arc::new(Mutex::new(Knobs::default()));
    let fake = {
        let knobs = knobs.clone();
        Fake::start(move |r| answer(&knobs, r)).await
    };
    let dir = std::env::temp_dir().join(format!(
        "twincursor-calls-{}-{}",
        std::process::id(),
        WORLDS.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let file = dir.join("session.json");
    if signed_in {
        FileStore::new(&file)
            .save(&Session {
                access_token: Secret::new(ACCESS),
                refresh_token: Secret::new(REFRESH),
                expires_at: u64::MAX / 2,
                user_id: ME.into(),
            })
            .unwrap();
    }
    let shell = AccountShell::new(Backend::new(&fake.url, KEY).unwrap(), file).unwrap();
    let engines = Arc::new(FakeEngines::default());
    let clipboard = Arc::new(MemoryClipboard::default());
    let permits = Arc::new(FakePermits::default());
    let clock = Arc::new(ManualClock::default());
    let homes = Arc::new(Mutex::new(Vec::new()));
    let outputs = Arc::new(Mutex::new(Vec::new()));
    let traces = Arc::new(Mutex::new(Vec::new()));
    let setup = Setup {
        account: Arc::new(shell),
        engines: engines.clone(),
        clipboard: clipboard.clone(),
        permits: permits.clone(),
        clock: clock.clone(),
        options: Options {
            quality: Quality::K4,
            voice: false,
            can_share,
        },
        on_home: {
            let homes = homes.clone();
            Arc::new(move |h: &HomeView| homes.lock().unwrap().push(h.clone()))
        },
        on_engine: {
            let outputs = outputs.clone();
            Arc::new(move |o: Output| outputs.lock().unwrap().push(o))
        },
        trace: {
            let traces = traces.clone();
            Arc::new(move |v: Value| traces.lock().unwrap().push(v))
        },
        runtime: tokio::runtime::Handle::current(),
    };
    World {
        calls: Calls::new(setup),
        fake,
        knobs,
        engines,
        clipboard,
        permits,
        clock,
        homes,
        outputs,
        traces,
        dir,
    }
}

impl World {
    fn knobs(&self) -> std::sync::MutexGuard<'_, Knobs> {
        self.knobs.lock().unwrap()
    }

    fn rpc(&self, name: &str) -> Vec<Request> {
        self.fake.to(&format!("/rest/v1/rpc/{name}"))
    }

    fn passes(&self) -> usize {
        self.fake.to("/functions/v1/call-pass").len()
    }

    /// The `n`th engine started (0-based): its sink and what it was sent.
    fn engine(&self, n: usize) -> (Sink, Arc<EngineLog>) {
        let started = self.engines.started.lock().unwrap();
        (started[n].sink.clone(), started[n].log.clone())
    }

    fn engines_started(&self) -> usize {
        self.engines.started.lock().unwrap().len()
    }

    fn event(&self, n: usize, event: Value) {
        (self.engine(n).0)(Output::Event { event });
    }

    fn sent(&self, n: usize) -> Vec<(ipc::Command, Option<u64>)> {
        self.engine(n).1.commands.lock().unwrap().clone()
    }

    fn trusts(&self, n: usize) -> Vec<(String, Option<u64>)> {
        self.sent(n)
            .into_iter()
            .filter_map(|(c, e)| match c {
                ipc::Command::TrustControl { identity } => Some((identity, e)),
                _ => None,
            })
            .collect()
    }

    /// The `trust_on_arrival` commands engine `n` was sent.
    fn arrivals(&self, n: usize) -> Vec<(String, Option<u64>)> {
        self.sent(n)
            .into_iter()
            .filter_map(|(c, e)| match c {
                ipc::Command::TrustOnArrival { identity } => Some((identity, e)),
                _ => None,
            })
            .collect()
    }

    fn hosting(&self, members: Value) {
        self.knobs().hosting = Some(json!({ "call_id": CALL, "code": CODE, "members": members }));
    }

    async fn host(&self) {
        self.calls.share().await.unwrap();
        self.hosting(json!([]));
    }

    /// Hosting, with Sam knocking and shown to the window.
    async fn knocked(&self) {
        self.host().await;
        self.hosting(json!([{ "user_id": SAM, "name": "Sam", "handle": null, "avatar_url": null, "state": "knocking" }]));
        assert_eq!(self.calls.poll_once().await, Polled::Done);
    }

    fn participants(&self, n: usize, who: &[(&str, &str)]) {
        let list: Vec<Value> = who
            .iter()
            .map(|(identity, role)| {
                json!({ "identity": identity, "name": "Sam", "role": role, "decoders": [] })
            })
            .collect();
        self.event(n, json!({ "event": "participants", "participants": list }));
    }

    /// Everything the window or a test log was given, as text.
    fn everything(&self) -> String {
        format!(
            "{} {} {} {:?}",
            serde_json::to_string(&*self.homes.lock().unwrap()).unwrap(),
            serde_json::to_string(&*self.outputs.lock().unwrap()).unwrap(),
            serde_json::to_string(&*self.traces.lock().unwrap()).unwrap(),
            self.engines
                .started
                .lock()
                .unwrap()
                .iter()
                .map(|s| crate::engine::engine_args(&s.plan, Codec::H264, 7, &Default::default()))
                .collect::<Vec<_>>()
        )
    }
}

async fn eventually(what: &str, check: impl Fn() -> bool) {
    for _ in 0..200 {
        if check() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("{what} never happened");
}

fn keys(v: &Value) -> Vec<&str> {
    v.as_object().unwrap().keys().map(String::as_str).collect()
}

// Links and the home view.

#[tokio::test(flavor = "multi_thread")]
async fn the_home_view_has_exactly_this_shape() {
    let w = world(true).await;
    w.knocked().await;
    let v = serde_json::to_value(w.calls.home()).unwrap();
    assert_eq!(
        keys(&v),
        [
            "account",
            "contacts",
            "error",
            "knocks",
            "members",
            "needs",
            "notice",
            "rings",
            "server_problem",
            "stage"
        ]
    );
    assert_eq!(keys(&v["stage"]), ["call_id", "kind", "link"]);
    assert_eq!(v["stage"]["kind"], "hosting");
    assert_eq!(v["stage"]["link"], format!("twincursor://call/{CODE}"));
    assert_eq!(
        keys(&v["knocks"][0]),
        ["armed", "avatar_url", "handle", "name", "user_id"]
    );
    assert_eq!(v["account"]["signed_in"], true);
    assert_eq!(v["account"]["me"]["name"], "Alex");
    assert_eq!(
        serde_json::to_value(StageView::Knocking {
            host: Me {
                id: "u".into(),
                name: "Sam".into(),
                handle: None,
                avatar_url: None
            }
        })
        .unwrap(),
        json!({ "kind": "knocking", "host": { "id": "u", "name": "Sam", "handle": null, "avatar_url": null } })
    );
    assert_eq!(
        serde_json::to_value(StageView::Idle).unwrap(),
        json!({ "kind": "idle" })
    );
    assert!(
        !w.everything().contains("must-not-leak"),
        "a token reached the window"
    );
}

// Share my screen.

#[tokio::test(flavor = "multi_thread")]
async fn share_starts_a_call_with_a_pass_and_copies_the_link() {
    let w = world(true).await;
    let view = w.calls.share().await.unwrap();
    assert_eq!(
        view.stage,
        StageView::Hosting {
            call_id: CALL.into(),
            link: format!("twincursor://call/{CODE}")
        }
    );
    assert_eq!(view.notice.as_deref(), Some(LINK_COPIED));
    assert_eq!(
        w.clipboard.text().as_deref(),
        Some(format!("twincursor://call/{CODE}").as_str())
    );
    let (plan, env) = {
        let started = w.engines.started.lock().unwrap();
        (started[0].plan.clone(), started[0].env.clone())
    };
    let s = Seen { plan, env };
    assert!(s.plan.pass && s.plan.role == Role::Share && s.plan.quality == Quality::K4);
    let names: Vec<&str> = s.env.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(names, ["TWINCURSOR_SERVER_URL", "TWINCURSOR_PASS"]);
    assert_eq!(s.env[1].1, format!("{PASS}-1"));
    let args = crate::engine::engine_args(&s.plan, Codec::H265, 7, &Default::default());
    assert!(args.iter().any(|a| a == "--allow-control"));
    assert!(args.iter().any(|a| a == "--exclude-app-pid=7"));
    assert!(!args
        .iter()
        .any(|a| a.starts_with("--room") || a.contains("must-not-leak")));
    // The poll is the heartbeat for this call.
    w.hosting(json!([]));
    w.calls.poll_once().await;
    assert_eq!(
        w.rpc("inbox").last().unwrap().json(),
        json!({ "hosting": CALL })
    );
    assert!(!w.everything().contains("must-not-leak"));
}

#[tokio::test(flavor = "multi_thread")]
async fn share_needs_a_session_and_no_other_call() {
    let w = world(false).await;
    assert_eq!(w.calls.share().await.unwrap_err(), NOT_SIGNED_IN);
    assert!(w.rpc("start_call").is_empty());

    let w = world(true).await;
    w.calls.share().await.unwrap();
    assert_eq!(w.calls.share().await.unwrap_err(), IN_CALL);
    assert_eq!(w.calls.join_link(CODE).await.unwrap_err(), IN_CALL);
    assert_eq!(w.rpc("start_call").len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn where_sharing_isnt_built_share_and_ring_are_refused_and_joining_works() {
    let w = world_on(true, false).await;
    assert_eq!(
        w.calls.share().await.unwrap_err(),
        crate::platform::CANT_SHARE
    );
    assert_eq!(
        w.calls.ring(SAM).await.unwrap_err(),
        crate::platform::CANT_SHARE
    );
    // Nothing started: no call on the backend, no engine, no permission asked for.
    assert!(w.rpc("start_call").is_empty());
    assert!(w.rpc("ring").is_empty());
    assert_eq!(w.engines_started(), 0);
    assert!(w.permits.requests.lock().unwrap().is_empty());
    assert!(matches!(w.calls.home().stage, StageView::Idle));
    // A guest joins as on a Mac.
    w.calls.join_link(CODE).await.unwrap();
    w.knobs().knock = Ok("admitted");
    w.calls.poll_once().await;
    assert!(matches!(&w.calls.home().stage, StageView::Joined { host } if host.name == "Sam"));
    assert_eq!(w.engines.started.lock().unwrap()[0].plan.role, Role::Join);
}

#[tokio::test(flavor = "multi_thread")]
async fn share_undoes_itself_when_a_step_fails() {
    // The backend refuses to start a call.
    let w = world(true).await;
    w.knobs().start_call = Err(500);
    let e = w.calls.share().await.unwrap_err();
    assert_eq!(
        e,
        "The server had a problem (HTTP 500). Try again in a moment."
    );
    let home = w.calls.home();
    assert_eq!(home.stage, StageView::Idle);
    assert_eq!(home.error.as_deref(), Some(e.as_str()));
    assert_eq!(w.engines_started(), 0);

    // The pass is refused: the call is ended again.
    let w = world(true).await;
    w.knobs().pass = Err(403);
    let e = w.calls.share().await.unwrap_err();
    assert_eq!(e, AccountError::NotAllowed.to_string());
    assert_eq!(w.calls.home().stage, StageView::Idle);
    eventually("end_call", || w.rpc("end_call").len() == 1).await;
    assert_eq!(w.engines_started(), 0);

    // The engine doesn't start.
    let w = world(true).await;
    w.engines.fail.store(true, Ordering::SeqCst);
    let e = w.calls.share().await.unwrap_err();
    assert!(e.starts_with("The engine couldn't start"), "{e}");
    assert_eq!(w.calls.home().stage, StageView::Idle);
    eventually("end_call", || w.rpc("end_call").len() == 1).await;
    assert!(
        w.clipboard.text().is_none(),
        "no link for a call that didn't start"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn share_asks_for_screen_recording_once_and_waits_for_it() {
    let w = world(true).await;
    w.permits
        .missing
        .lock()
        .unwrap()
        .insert(Permission::ScreenRecording);
    assert_eq!(w.calls.share().await.unwrap_err(), NEEDS_SCREEN_RECORDING);
    assert_eq!(w.calls.home().needs, Some(Permission::ScreenRecording));
    assert_eq!(w.calls.share().await.unwrap_err(), NEEDS_SCREEN_RECORDING);
    assert_eq!(
        *w.permits.requests.lock().unwrap(),
        [Permission::ScreenRecording],
        "macOS is asked once"
    );
    assert!(w.rpc("start_call").is_empty() && w.engines_started() == 0);
    // Granted in System Settings: the next share goes ahead.
    w.permits.missing.lock().unwrap().clear();
    w.calls.share().await.unwrap();
    assert_eq!(w.calls.home().needs, None);
}

// Knocks and arming.

#[tokio::test(flavor = "multi_thread")]
async fn a_knock_can_only_be_let_in_a_second_after_it_was_shown() {
    let w = world(true).await;
    w.knocked().await;
    let home = w.calls.home();
    assert_eq!(home.knocks.len(), 1);
    assert!(!home.knocks[0].armed);
    assert_eq!(
        w.calls.answer_knock(SAM, true, false).await.unwrap_err(),
        TOO_SOON
    );
    w.clock.advance(Duration::from_millis(999));
    assert_eq!(
        w.calls.answer_knock(SAM, true, false).await.unwrap_err(),
        TOO_SOON
    );
    assert!(
        w.rpc("answer_knock").is_empty(),
        "nothing reached the backend"
    );
    w.clock.advance(Duration::from_millis(1));
    assert!(w.calls.home().knocks[0].armed);
    w.calls.answer_knock(SAM, true, false).await.unwrap();
    let r = w.rpc("answer_knock");
    assert_eq!(
        r[0].json(),
        json!({ "call_id": CALL, "user_id": SAM, "let_in": true })
    );
    assert_eq!(w.calls.home().members[0].state, MemberState::Admitted);
}

#[tokio::test(flavor = "multi_thread")]
async fn turning_away_is_never_delayed_and_unknown_knocks_are_refused() {
    let w = world(true).await;
    w.knocked().await;
    assert_eq!(
        w.calls.answer_knock(KIM, true, false).await.unwrap_err(),
        NOT_WAITING
    );
    assert_eq!(
        w.calls.answer_knock(KIM, false, false).await.unwrap_err(),
        NOT_WAITING
    );
    w.calls.answer_knock(SAM, false, false).await.unwrap();
    assert_eq!(w.rpc("answer_knock")[0].json()["let_in"], false);
    assert_eq!(w.engines_started(), 1, "nothing else started");
    assert!(w.trusts(0).is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_knock_that_comes_back_is_armed_again() {
    let w = world(true).await;
    w.knocked().await;
    w.clock.advance(ARMING);
    assert!(w.calls.home().knocks[0].armed);
    // Gone quiet, then knocking again: shown anew, so a second again.
    w.hosting(json!([]));
    w.calls.poll_once().await;
    w.hosting(json!([{ "user_id": SAM, "name": "Sam", "handle": null, "avatar_url": null, "state": "knocking" }]));
    w.calls.poll_once().await;
    assert!(!w.calls.home().knocks[0].armed);
    assert_eq!(
        w.calls.answer_knock(SAM, true, true).await.unwrap_err(),
        TOO_SOON
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_allow_needs_a_second_after_the_request_was_shown() {
    let w = world(true).await;
    w.host().await;
    let asked = json!({ "event": "control_asked", "request": 1, "identity": format!("guest-{SAM}"), "name": "Sam", "platform": "macos" });
    w.event(0, asked.clone());
    let allow = ipc::Command::AllowControl { request: 1 };
    assert_eq!(w.calls.send_command(&allow).await.unwrap_err(), TOO_SOON);
    // A deny is never delayed.
    w.calls
        .send_command(&ipc::Command::DenyControl { request: 1 })
        .await
        .unwrap();
    w.clock.advance(Duration::from_millis(999));
    assert_eq!(w.calls.send_command(&allow).await.unwrap_err(), TOO_SOON);
    w.clock.advance(Duration::from_millis(1));
    w.calls.send_command(&allow).await.unwrap();
    // A request never shown can't be allowed.
    assert_eq!(
        w.calls
            .send_command(&ipc::Command::AllowControl { request: 7 })
            .await
            .unwrap_err(),
        REQUEST_NOT_OPEN
    );
    let sent = w.sent(0);
    assert_eq!(
        sent,
        [
            (ipc::Command::DenyControl { request: 1 }, None),
            (allow.clone(), Some(1)),
        ],
        "the allow goes only to the engine that asked"
    );
    // After a restart, request numbers start again: the old one can't be allowed.
    (w.engine(0).0)(Output::Restarting {
        codec: Codec::H264,
        message: "Sam's computer can't decode HEVC; switched to H.264.".into(),
    });
    (w.engine(0).0)(Output::Started {
        role: Role::Share,
        codec: Codec::H264,
        pid: 2,
        args: vec![],
    });
    assert_eq!(
        w.calls.send_command(&allow).await.unwrap_err(),
        REQUEST_NOT_OPEN
    );
    w.event(0, asked);
    w.clock.advance(ARMING);
    w.calls.send_command(&allow).await.unwrap();
    assert_eq!(w.sent(0).last().unwrap(), &(allow, Some(2)));
}

#[test]
fn arming_counts_from_when_it_could_be_seen() {
    let t0 = std::time::Instant::now();
    let later = t0 + Duration::from_millis(400);
    assert_eq!(arming_from(t0, Sight::Always), Some(t0));
    assert_eq!(arming_from(t0, Sight::Hidden), None);
    // Delivered before the bar came on screen: from when it came.
    assert_eq!(arming_from(t0, Sight::Since(later)), Some(later));
    // Delivered while the bar was on screen: from delivery.
    assert_eq!(arming_from(later, Sight::Since(t0)), Some(later));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_knock_arms_a_second_after_the_bar_came_on_screen() {
    let w = world(true).await;
    w.calls.set_bar_visible(false);
    w.knocked().await;
    w.clock.advance(ARMING * 3);
    assert!(!w.calls.home().knocks[0].armed, "never seen, never armed");
    assert_eq!(
        w.calls.answer_knock(SAM, true, false).await.unwrap_err(),
        TOO_SOON
    );
    w.calls.set_bar_visible(true);
    w.clock.advance(Duration::from_millis(999));
    assert!(!w.calls.home().knocks[0].armed);
    assert_eq!(
        w.calls.answer_knock(SAM, true, false).await.unwrap_err(),
        TOO_SOON
    );
    // Shown again while on screen: still counts from when it came.
    w.calls.set_bar_visible(true);
    w.clock.advance(Duration::from_millis(1));
    assert!(w.calls.home().knocks[0].armed);
    w.calls.answer_knock(SAM, true, false).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn an_allow_waits_for_the_bar_to_have_been_on_screen_a_second() {
    let w = world(true).await;
    w.host().await;
    w.calls.set_bar_visible(false);
    let asked = json!({ "event": "control_asked", "request": 1, "identity": format!("guest-{SAM}"), "name": "Sam", "platform": "macos" });
    w.event(0, asked);
    w.clock.advance(ARMING * 2);
    let allow = ipc::Command::AllowControl { request: 1 };
    assert_eq!(w.calls.send_command(&allow).await.unwrap_err(), TOO_SOON);
    w.calls.set_bar_visible(true);
    w.clock.advance(Duration::from_millis(999));
    assert_eq!(w.calls.send_command(&allow).await.unwrap_err(), TOO_SOON);
    w.clock.advance(Duration::from_millis(1));
    w.calls.send_command(&allow).await.unwrap();
    // Hidden again: refused until it has been back on screen for a second.
    w.calls.set_bar_visible(false);
    assert_eq!(w.calls.send_command(&allow).await.unwrap_err(), TOO_SOON);
    // Denying is never delayed, seen or not.
    w.calls
        .send_command(&ipc::Command::DenyControl { request: 1 })
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn an_allow_without_accessibility_asks_once_and_turns_the_request_down() {
    let w = world(true).await;
    w.permits
        .missing
        .lock()
        .unwrap()
        .insert(Permission::Accessibility);
    w.host().await;
    let asked = |request: u64| json!({ "event": "control_asked", "request": request, "identity": format!("guest-{SAM}"), "name": "Sam", "platform": "macos" });
    w.event(0, asked(1));
    let allow = |request: u64| ipc::Command::AllowControl { request };

    // The arming rule still comes first: too soon is refused without asking macOS.
    assert_eq!(w.calls.send_command(&allow(1)).await.unwrap_err(), TOO_SOON);
    assert!(w.permits.requests.lock().unwrap().is_empty());
    assert!(w.sent(0).is_empty());

    // Armed, but no Accessibility: macOS is asked, the request is turned down, the bar says why.
    w.clock.advance(ARMING);
    w.calls.send_command(&allow(1)).await.unwrap();
    assert_eq!(
        *w.permits.requests.lock().unwrap(),
        [Permission::Accessibility]
    );
    assert_eq!(
        w.sent(0),
        [(ipc::Command::DenyControl { request: 1 }, Some(1))],
        "no allow, and the guest isn't left waiting"
    );
    let view = w.calls.home();
    assert_eq!(
        view.notice.as_deref(),
        Some("Sam wasn't given control: TwinCursor needs Accessibility.")
    );
    assert_eq!(view.needs, Some(Permission::Accessibility));
    // That request is closed now.
    assert_eq!(
        w.calls.send_command(&allow(1)).await.unwrap_err(),
        REQUEST_NOT_OPEN
    );

    // They ask again while it's still missing: macOS isn't asked a second time.
    w.event(0, asked(2));
    w.clock.advance(ARMING);
    w.calls.send_command(&allow(2)).await.unwrap();
    assert_eq!(w.permits.requests.lock().unwrap().len(), 1);
    assert_eq!(
        w.sent(0).last().unwrap(),
        &(ipc::Command::DenyControl { request: 2 }, Some(1))
    );

    // Granted in System Settings: the next request is allowed and the bar stops asking.
    w.permits.missing.lock().unwrap().clear();
    w.event(0, asked(3));
    w.clock.advance(ARMING);
    w.calls.send_command(&allow(3)).await.unwrap();
    assert_eq!(w.sent(0).last().unwrap(), &(allow(3), Some(1)));
    assert_eq!(w.calls.home().needs, None);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_allow_with_accessibility_doesnt_ask_or_say_anything() {
    let w = world(true).await;
    w.host().await;
    w.event(
        0,
        json!({ "event": "control_asked", "request": 1, "identity": format!("guest-{SAM}"), "name": "Sam", "platform": "macos" }),
    );
    w.clock.advance(ARMING);
    let allow = ipc::Command::AllowControl { request: 1 };
    w.calls.send_command(&allow).await.unwrap();
    assert!(w.permits.requests.lock().unwrap().is_empty());
    assert_eq!(w.sent(0), [(allow, Some(1))]);
    let view = w.calls.home();
    assert_eq!(view.needs, None);
    assert_eq!(
        view.notice.as_deref(),
        Some(LINK_COPIED),
        "only sharing's own notice"
    );
}

// Control on arrival.

#[tokio::test(flavor = "multi_thread")]
async fn let_in_with_control_asks_the_engine_once_after_the_backend_lets_them_in() {
    let w = world(true).await;
    w.knocked().await;
    w.clock.advance(ARMING);
    w.calls.answer_knock(SAM, true, true).await.unwrap();
    let sam = format!("guest-{SAM}");
    // Once, to the engine process running the call; it keeps the promise from here.
    assert_eq!(w.arrivals(0), [(sam.clone(), Some(1))]);
    assert!(w.trusts(0).is_empty(), "no trust_control from the shell");
    // Their arrival changes nothing here: the engine does it, and nothing is sent twice.
    w.participants(0, &[(&sam, "guest")]);
    w.participants(0, &[(&sam, "guest"), (&format!("guest-{KIM}"), "guest")]);
    assert_eq!(w.arrivals(0).len(), 1);
    assert!(w.trusts(0).is_empty());
    let traces = w.traces.lock().unwrap().clone();
    assert_eq!(
        traces
            .iter()
            .filter(|t| t["kind"] == "trust_sent" && t["why"] == "on_arrival")
            .count(),
        1
    );
    // A revoke goes to the engine as it is: there is nothing left in the shell to clear.
    w.calls
        .send_command(&ipc::Command::RevokeControl)
        .await
        .unwrap();
    assert_eq!(
        w.sent(0).last().unwrap(),
        &(ipc::Command::RevokeControl, None)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn no_trust_on_arrival_without_control_or_when_not_let_in() {
    // Let in without control.
    let w = world(true).await;
    w.knocked().await;
    w.clock.advance(ARMING);
    w.calls.answer_knock(SAM, true, false).await.unwrap();
    assert!(
        w.arrivals(0).is_empty() && w.trusts(0).is_empty(),
        "without control"
    );

    // The backend didn't let them in after all.
    let w = world(true).await;
    w.knocked().await;
    w.clock.advance(ARMING);
    w.knobs().answer_knock = "turned_away";
    w.calls.answer_knock(SAM, true, true).await.unwrap();
    assert!(w.arrivals(0).is_empty(), "not admitted");

    // Turned away.
    let w = world(true).await;
    w.knocked().await;
    w.calls.answer_knock(SAM, false, true).await.unwrap();
    assert!(w.arrivals(0).is_empty(), "turned away");

    // Too soon: refused before anything is sent.
    let w = world(true).await;
    w.knocked().await;
    assert_eq!(
        w.calls.answer_knock(SAM, true, true).await.unwrap_err(),
        TOO_SOON
    );
    assert!(w.arrivals(0).is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn after_a_restart_control_has_to_be_given_again() {
    let w = world(true).await;
    w.knocked().await;
    w.clock.advance(ARMING);
    w.calls.answer_knock(SAM, true, true).await.unwrap();
    (w.engine(0).0)(Output::Restarting {
        codec: Codec::H264,
        message: "Sharing with HEVC failed; switched to H.264.".into(),
    });
    (w.engine(0).0)(Output::Started {
        role: Role::Share,
        codec: Codec::H264,
        pid: 2,
        args: vec![],
    });
    w.participants(0, &[(&format!("guest-{SAM}"), "guest")]);
    // Only the first process was told: engine.rs drops it for the replacement.
    assert_eq!(w.arrivals(0), [(format!("guest-{SAM}"), Some(1))]);
    assert!(w.trusts(0).is_empty());
    assert_eq!(
        w.calls.home().notice.as_deref(),
        Some("Sharing with HEVC failed; switched to H.264. Control was reset: anyone you gave it to has to be given it again.")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_window_cant_trust_anyone_directly() {
    let w = world(true).await;
    w.host().await;
    let sam = format!("guest-{SAM}");
    w.participants(0, &[(&sam, "guest")]);
    for command in [
        ipc::Command::TrustControl {
            identity: sam.clone(),
        },
        ipc::Command::TrustOnArrival {
            identity: sam.clone(),
        },
    ] {
        assert_eq!(
            w.calls.send_command(&command).await.unwrap_err(),
            TRUST_FROM_WINDOW
        );
    }
    assert!(w.sent(0).is_empty(), "nothing reached the engine");
    // Give control still does, after its checks.
    w.calls.give_control(&sam).await.unwrap();
    assert_eq!(w.trusts(0), [(sam, Some(1))]);
}

#[tokio::test(flavor = "multi_thread")]
async fn without_accessibility_they_are_let_in_without_control() {
    let w = world(true).await;
    w.permits
        .missing
        .lock()
        .unwrap()
        .insert(Permission::Accessibility);
    w.knocked().await;
    w.clock.advance(ARMING);
    let view = w.calls.answer_knock(SAM, true, true).await.unwrap();
    assert_eq!(
        view.notice.as_deref(),
        Some("Sam was let in without control: TwinCursor needs Accessibility to give control.")
    );
    assert_eq!(view.needs, Some(Permission::Accessibility));
    assert_eq!(w.rpc("answer_knock")[0].json()["let_in"], true);
    w.participants(0, &[(&format!("guest-{SAM}"), "guest")]);
    assert!(w.trusts(0).is_empty() && w.arrivals(0).is_empty());
    assert_eq!(
        w.calls
            .give_control(&format!("guest-{SAM}"))
            .await
            .unwrap_err(),
        NEEDS_ACCESSIBILITY
    );
    assert!(w.trusts(0).is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn give_control_only_to_a_guest_in_the_call() {
    let w = world(true).await;
    assert_eq!(
        w.calls.give_control("guest-x").await.unwrap_err(),
        NOT_HOSTING
    );
    w.host().await;
    let sam = format!("guest-{SAM}");
    assert_eq!(
        w.calls.give_control(&sam).await.unwrap_err(),
        NOT_IN_THE_CALL
    );
    w.participants(0, &[(&sam, "guest"), (&format!("sharer-{ME}"), "sharer")]);
    assert_eq!(
        w.calls
            .give_control(&format!("sharer-{ME}"))
            .await
            .unwrap_err(),
        NOT_IN_THE_CALL
    );
    w.calls.give_control(&sam).await.unwrap();
    assert_eq!(w.trusts(0), [(sam, Some(1))]);
}

// Joining by link.

#[tokio::test(flavor = "multi_thread")]
async fn join_link_knocks_then_joins_when_let_in() {
    let w = world(true).await;
    assert_eq!(
        w.calls.join_link("not a link").await.unwrap_err(),
        "That call link is incomplete or mistyped."
    );
    assert!(w.rpc("knock").is_empty());
    let view = w
        .calls
        .join_link(&format!(" twincursor://call/{CODE}\n"))
        .await
        .unwrap();
    assert!(matches!(&view.stage, StageView::Knocking { host } if host.name == "Sam"));
    assert_eq!(w.rpc("knock")[0].json(), json!({ "code": CODE }));
    // The keep-alive: still knocking.
    w.calls.poll_once().await;
    assert_eq!(w.rpc("knock").len(), 2);
    assert_eq!(w.engines_started(), 0);
    // Let in.
    w.knobs().knock = Ok("admitted");
    w.calls.poll_once().await;
    let home = w.calls.home();
    assert!(matches!(&home.stage, StageView::Joined { host } if host.name == "Sam"));
    let plan = w.engines.started.lock().unwrap()[0].plan.clone();
    assert_eq!(plan.role, Role::Join);
    assert!(plan.pass);
    assert_eq!(w.passes(), 1);
    // In the call: the poll checks it goes on.
    w.calls.poll_once().await;
    assert_eq!(
        w.rpc("pass_claims")[0].json(),
        json!({ "call_id": OTHER_CALL })
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_knock_turned_away_or_on_a_dead_link() {
    let w = world(true).await;
    w.calls.join_link(CODE).await.unwrap();
    w.knobs().knock = Ok("turned_away");
    w.calls.poll_once().await;
    let home = w.calls.home();
    assert_eq!(home.stage, StageView::Idle);
    assert_eq!(home.notice.as_deref(), Some("Sam didn't let you in."));
    assert_eq!(w.engines_started(), 0);
    assert_eq!(w.passes(), 0);

    let w = world(true).await;
    w.knobs().knock = Err("call_not_found");
    assert_eq!(
        w.calls.join_link(CODE).await.unwrap_err(),
        "That call has ended or the link is wrong."
    );
    assert_eq!(w.calls.home().stage, StageView::Idle);

    let w = world(true).await;
    w.calls.join_link(CODE).await.unwrap();
    w.knobs().knock = Err("call_not_found");
    w.calls.poll_once().await;
    assert_eq!(
        w.calls.home().notice.as_deref(),
        Some("That call has ended or the link is wrong.")
    );

    let w = world(true).await;
    w.knobs().knock = Err("own_call");
    assert_eq!(
        w.calls.join_link(CODE).await.unwrap_err(),
        "That's your own call."
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_knock_rides_out_a_short_outage_and_gives_up_on_a_long_one() {
    let w = world(true).await;
    w.calls.join_link(CODE).await.unwrap();
    w.knobs().down = true;
    w.calls.poll_once().await;
    w.clock.advance(Duration::from_secs(20));
    w.calls.poll_once().await;
    assert!(matches!(w.calls.home().stage, StageView::Knocking { .. }));
    // Back: still knocking, and the outage is forgotten.
    w.knobs().down = false;
    w.calls.poll_once().await;
    w.knobs().down = true;
    w.clock.advance(Duration::from_secs(40));
    w.calls.poll_once().await;
    assert!(matches!(w.calls.home().stage, StageView::Knocking { .. }));
    w.clock.advance(KNOCK_GIVE_UP);
    w.calls.poll_once().await;
    let home = w.calls.home();
    assert_eq!(home.stage, StageView::Idle);
    assert_eq!(home.notice.as_deref(), Some(KNOCK_GAVE_UP));
}

#[tokio::test(flavor = "multi_thread")]
async fn cancel_knock_leaves() {
    let w = world(true).await;
    assert_eq!(w.calls.cancel_knock().await.unwrap_err(), NOT_KNOCKING);
    w.calls.join_link(CODE).await.unwrap();
    let view = w.calls.cancel_knock().await.unwrap();
    assert_eq!(view.stage, StageView::Idle);
    assert_eq!(
        w.rpc("leave_call")[0].json(),
        json!({ "call_id": OTHER_CALL })
    );
    // An admission that comes after is ignored.
    w.knobs().knock = Ok("admitted");
    w.calls.poll_once().await;
    assert_eq!(w.engines_started(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn join_from_the_clipboard_never_shows_what_was_there() {
    let w = world(true).await;
    assert_eq!(
        w.calls.join_from_clipboard().await.unwrap_err(),
        "There's no call link to join. Copy the link first."
    );
    w.clipboard.set(Some("my secret note https://x.test/a"));
    let e = w.calls.join_from_clipboard().await.unwrap_err();
    assert_eq!(e, "That isn't a TwinCursor call link.");
    w.clipboard.set(Some(&"x".repeat(5000)));
    assert!(w.calls.join_from_clipboard().await.is_err());
    assert!(!w.everything().contains("secret note"));
    w.clipboard.set(Some(&format!("twincursor://call/{CODE}")));
    let view = w.calls.join_from_clipboard().await.unwrap();
    assert!(matches!(view.stage, StageView::Knocking { .. }));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_joined_guest_leaves_by_itself_when_the_call_ends_on_the_server() {
    let w = world(true).await;
    w.knobs().knock = Ok("admitted");
    w.calls.join_link(CODE).await.unwrap();
    assert!(matches!(w.calls.home().stage, StageView::Joined { .. }));
    w.knobs().can_join = false;
    w.calls.poll_once().await;
    let home = w.calls.home();
    assert_eq!(home.stage, StageView::Idle);
    assert_eq!(home.notice.as_deref(), Some(CALL_ENDED));
    assert_eq!(w.engine(0).1.leaves.load(Ordering::SeqCst), 1);
    eventually("the engine exits", || {
        w.engine(0).1.done.load(Ordering::SeqCst)
    })
    .await;
    assert!(w.rpc("leave_call").is_empty(), "the call is over already");
}

// Ending.

#[tokio::test(flavor = "multi_thread")]
async fn end_leaves_the_engine_then_tells_the_backend() {
    let w = world(true).await;
    w.host().await;
    let view = w.calls.end().await.unwrap();
    assert_eq!(view.stage, StageView::Idle);
    assert_eq!(w.engine(0).1.leaves.load(Ordering::SeqCst), 1);
    assert_eq!(w.rpc("end_call")[0].json(), json!({ "call_id": CALL }));
    // The engine's own exit didn't tell the backend a second time.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(w.rpc("end_call").len(), 1);

    let w = world(true).await;
    w.knobs().knock = Ok("admitted");
    w.calls.join_link(CODE).await.unwrap();
    w.calls.end().await.unwrap();
    assert_eq!(
        w.rpc("leave_call")[0].json(),
        json!({ "call_id": OTHER_CALL })
    );
    assert!(w.rpc("end_call").is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn an_engine_that_ends_on_its_own_ends_the_call() {
    let w = world(true).await;
    w.host().await;
    w.event(
        0,
        json!({ "event": "error", "message": "Sharing needs Accessibility." }),
    );
    w.event(0, json!({ "event": "ended", "reason": "failed" }));
    (w.engine(0).0)(Output::Exited { code: Some(1) });
    eventually("end_call", || w.rpc("end_call").len() == 1).await;
    let home = w.calls.home();
    assert_eq!(home.stage, StageView::Idle);
    assert_eq!(home.error.as_deref(), Some("Sharing needs Accessibility."));

    let w = world(true).await;
    w.knobs().knock = Ok("admitted");
    w.calls.join_link(CODE).await.unwrap();
    w.engine(0).1.done.store(true, Ordering::SeqCst);
    (w.engine(0).0)(Output::Exited { code: Some(0) });
    eventually("leave_call", || w.rpc("leave_call").len() == 1).await;
    assert_eq!(w.calls.home().stage, StageView::Idle);
    assert_eq!(w.calls.home().error, None);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_hosted_call_gone_from_the_server_stops_here() {
    let w = world(true).await;
    w.host().await;
    w.knobs().hosting = None;
    w.calls.poll_once().await;
    let home = w.calls.home();
    assert_eq!(home.stage, StageView::Idle);
    assert_eq!(home.notice.as_deref(), Some(CALL_ENDED));
    assert_eq!(w.engine(0).1.leaves.load(Ordering::SeqCst), 1);
}

// Polling.

#[test]
fn the_poll_backs_off_after_repeated_failures() {
    assert_eq!(poll_delay(0), POLL_EVERY);
    assert_eq!(poll_delay(FAILURES_BEFORE_BACKOFF - 1), POLL_EVERY);
    assert_eq!(poll_delay(FAILURES_BEFORE_BACKOFF), POLL_BACKOFF);
    assert_eq!(poll_delay(100), POLL_BACKOFF);
    assert_eq!(POLL_EVERY, Duration::from_secs(2));
    assert_eq!(POLL_BACKOFF, Duration::from_secs(10));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_poll_says_when_the_server_cant_be_reached() {
    let w = world(true).await;
    w.calls.poll_once().await;
    w.knobs().down = true;
    for _ in 0..FAILURES_BEFORE_BACKOFF - 1 {
        assert_eq!(w.calls.poll_once().await, Polled::Failed);
        assert_eq!(w.calls.home().server_problem, None);
    }
    w.calls.poll_once().await;
    assert_eq!(w.calls.home().server_problem.as_deref(), Some(CANT_REACH));
    assert!(w.calls.home().account.signed_in, "still signed in");
    w.knobs().down = false;
    assert_eq!(w.calls.poll_once().await, Polled::Done);
    assert_eq!(w.calls.home().server_problem, None);
}

#[tokio::test(flavor = "multi_thread")]
async fn polls_never_overlap() {
    let w = world(true).await;
    w.knobs().inbox_delay = Duration::from_millis(300);
    let (a, b) = tokio::join!(w.calls.poll_once(), w.calls.poll_once());
    let mut outcomes = [a, b];
    outcomes.sort_by_key(|o| *o == Polled::Busy);
    assert_eq!(outcomes, [Polled::Done, Polled::Busy]);
    assert_eq!(w.rpc("inbox").len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_poll_pauses_while_signed_out() {
    let w = world(false).await;
    assert_eq!(w.calls.poll_once().await, Polled::SignedOut);
    assert!(w.fake.requests().is_empty());
    assert!(!w.calls.home().account.signed_in);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_leftover_call_is_ended() {
    let w = world(true).await;
    w.hosting(json!([]));
    w.calls.poll_once().await;
    w.calls.poll_once().await;
    eventually("end_call", || !w.rpc("end_call").is_empty()).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(w.rpc("end_call").len(), 1, "once");
    assert_eq!(w.rpc("end_call")[0].json(), json!({ "call_id": CALL }));
    assert_eq!(w.engines_started(), 0);
}

// Rings.

#[tokio::test(flavor = "multi_thread")]
async fn ringing_shares_first_and_rings() {
    let w = world(true).await;
    let view = w.calls.ring(SAM).await.unwrap();
    assert!(matches!(view.stage, StageView::Hosting { .. }));
    assert_eq!(w.rpc("start_call").len(), 1);
    assert_eq!(w.rpc("ring")[0].json(), json!({ "contact": SAM }));
    // While sharing, it only rings.
    w.calls.ring(KIM).await.unwrap();
    assert_eq!(w.rpc("start_call").len(), 1);
    assert_eq!(w.engines_started(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_ring_is_answered_and_accepting_leaves_the_call_first() {
    let w = world(true).await;
    assert_eq!(
        w.calls.answer_ring(OTHER_CALL, true).await.unwrap_err(),
        NO_RING
    );
    w.host().await;
    w.knobs().rings = json!([{ "call_id": OTHER_CALL, "host": profile(SAM, "Sam") }]);
    w.calls.poll_once().await;
    assert_eq!(w.calls.home().rings[0].host.name, "Sam");
    let view = w.calls.answer_ring(OTHER_CALL, true).await.unwrap();
    assert!(matches!(&view.stage, StageView::Joined { host } if host.name == "Sam"));
    assert_eq!(
        w.engine(0).1.leaves.load(Ordering::SeqCst),
        1,
        "the share ended"
    );
    assert_eq!(w.rpc("end_call")[0].json(), json!({ "call_id": CALL }));
    assert_eq!(
        w.rpc("answer_ring")[0].json(),
        json!({ "call_id": OTHER_CALL, "accept": true })
    );
    assert_eq!(w.engines_started(), 2);
    assert_eq!(w.engines.started.lock().unwrap()[1].plan.role, Role::Join);

    let w = world(true).await;
    w.knobs().rings = json!([{ "call_id": OTHER_CALL, "host": profile(SAM, "Sam") }]);
    w.calls.poll_once().await;
    w.calls.answer_ring(OTHER_CALL, false).await.unwrap();
    assert_eq!(w.rpc("answer_ring")[0].json()["accept"], false);
    assert_eq!(w.engines_started(), 0);
}

// Restarts.

#[tokio::test(flavor = "multi_thread")]
async fn a_replacement_engine_gets_a_fresh_pass() {
    let w = world(true).await;
    w.host().await;
    assert_eq!(w.passes(), 1);
    let refresh = w.engines.started.lock().unwrap()[0]
        .refresh
        .clone()
        .unwrap();
    // engine.rs calls it on the call's own thread.
    let env = std::thread::spawn(move || refresh())
        .join()
        .unwrap()
        .unwrap();
    assert_eq!(w.passes(), 2);
    assert_eq!(env[1], ("TWINCURSOR_PASS".to_owned(), format!("{PASS}-2")));
    assert!(!w.everything().contains("must-not-leak"));
    // No pass for it: the engine is told why.
    w.knobs().pass = Err(403);
    let refresh = w.engines.started.lock().unwrap()[0]
        .refresh
        .clone()
        .unwrap();
    let e = std::thread::spawn(move || refresh())
        .join()
        .unwrap()
        .unwrap_err();
    assert!(e.starts_with("The call couldn't go on:"), "{e}");
}

// Snapshots.

#[tokio::test(flavor = "multi_thread")]
async fn the_snapshot_is_the_current_calls() {
    let w = world(true).await;
    w.host().await;
    w.event(0, json!({ "event": "connection", "state": "connecting" }));
    w.event(0, json!({ "event": "connection", "state": "connected" }));
    w.event(
        0,
        json!({ "event": "voice", "sending": false, "muted": false, "mic": null, "speaker": null }),
    );
    let kinds: Vec<String> = w
        .calls
        .snapshot()
        .iter()
        .map(|o| serde_json::to_value(o).unwrap())
        .map(|v| {
            format!(
                "{}{}",
                v["kind"].as_str().unwrap(),
                v["event"]["event"]
                    .as_str()
                    .map(|e| format!(":{e}"))
                    .unwrap_or_default()
            )
        })
        .collect();
    assert_eq!(kinds, ["started", "event:connection", "event:voice"]);
    w.calls.end().await.unwrap();
    // The next call starts a new one.
    w.calls.share().await.unwrap();
    assert_eq!(w.calls.snapshot().len(), 1);
}

/// One of everything, as `home.test.ts` reads it (`fixtures/home-view.json`; rewrite it with
/// `UPDATE_FIXTURES=1 cargo test`).
#[test]
fn the_home_view_fixture_is_what_the_window_gets() {
    let sam = Me {
        id: SAM.into(),
        name: "Sam".into(),
        handle: Some("sam".into()),
        avatar_url: Some("https://avatars.githubusercontent.com/u/2".into()),
    };
    let view = HomeView {
        account: AccountView {
            configured: true,
            signed_in: true,
            me: Some(Me {
                id: ME.into(),
                name: "Alex".into(),
                handle: Some("alex".into()),
                avatar_url: None,
            }),
            signing_in: false,
        },
        stage: StageView::Hosting {
            call_id: CALL.into(),
            link: format!("twincursor://call/{CODE}"),
        },
        contacts: vec![sam.clone()],
        rings: vec![RingView {
            call_id: OTHER_CALL.into(),
            host: sam,
        }],
        knocks: vec![KnockView {
            user_id: KIM.into(),
            name: "Kim".into(),
            handle: None,
            avatar_url: None,
            armed: false,
        }],
        members: vec![MemberView {
            user_id: SAM.into(),
            name: "Sam".into(),
            handle: Some("sam".into()),
            avatar_url: None,
            state: MemberState::Admitted,
        }],
        notice: Some(LINK_COPIED.into()),
        error: None,
        needs: Some(Permission::Accessibility),
        server_problem: Some(CANT_REACH.into()),
    };
    let text = serde_json::to_string_pretty(&view).unwrap() + "\n";
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/home-view.json");
    if std::env::var_os("UPDATE_FIXTURES").is_some() {
        std::fs::write(&path, &text).unwrap();
    }
    assert_eq!(std::fs::read_to_string(&path).unwrap_or_default(), text);
}
