use std::sync::Arc;
use std::sync::Mutex as StdMutex;

use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::*;
use crate::fake::{dead_url, Fake, Reply, Request};
use crate::{MemoryStore, PassRole};

const KEY: &str = "public-anon-key";
const OLD_ACCESS: &str = "old-access.payload.signature";
const OLD_REFRESH: &str = "old-refresh-token";
const NEW_ACCESS: &str = "new-access.payload.signature";
const NEW_REFRESH: &str = "new-refresh-token";
const CODE: &str = "c0de-2f9a-must-stay-secret";
const USER: &str = "3f2c8a8e-5b7d-4e8f-9a51-0d6c1e2b7f40";

fn session(expires_in: i64) -> Session {
    Session {
        access_token: Secret::new(OLD_ACCESS),
        refresh_token: Secret::new(OLD_REFRESH),
        expires_at: (now_secs() as i64 + expires_in) as u64,
        user_id: USER.into(),
    }
}

fn token_body(access: &str, refresh: &str) -> Value {
    json!({
        "access_token": access, "token_type": "bearer", "expires_in": 3600,
        "expires_at": now_secs() + 3600, "refresh_token": refresh,
        "user": { "id": USER, "email": "a@example.test" }, "weak_password": null
    })
}

/// A store tests can look into after the account owns it.
#[derive(Clone, Default)]
struct Shared(Arc<MemoryStore>);

impl SessionStore for Shared {
    fn load(&self) -> Result<Option<Session>, Error> {
        self.0.load()
    }
    fn save(&self, s: &Session) -> Result<(), Error> {
        self.0.save(s)
    }
    fn clear(&self) -> Result<(), Error> {
        self.0.clear()
    }
}

fn make(url: &str, signed_in: Option<Session>) -> (Account, Shared) {
    let store = Shared(Arc::new(MemoryStore::new(signed_in)));
    let account = Account::new(Backend::new(url, KEY).unwrap(), store.clone()).unwrap();
    (account, store)
}

/// A backend that refreshes (with `refresh`), and answers every RPC with `{}` for the new token.
fn backend(refresh: impl Fn() -> Reply + Send + Sync + 'static) -> impl Fn(&Request) -> Reply {
    move |r: &Request| {
        if r.target
            .starts_with("/auth/v1/token?grant_type=refresh_token")
        {
            refresh()
        } else if r.target.starts_with("/rest/v1/rpc/") {
            Reply::json(200, json!({ "me": { "id": USER, "name": "Alex" } }))
        } else {
            Reply::empty(404)
        }
    }
}

/// Every text an error or a Debug could show, checked for every secret this file uses.
fn assert_no_secrets(text: &str) {
    for secret in [
        OLD_ACCESS,
        OLD_REFRESH,
        NEW_ACCESS,
        NEW_REFRESH,
        CODE,
        "payload",
    ] {
        assert!(!text.contains(secret), "{secret} in {text}");
    }
}

fn query(url: &str) -> Vec<(String, String)> {
    reqwest::Url::parse(url)
        .unwrap()
        .query_pairs()
        .into_owned()
        .collect()
}

fn param(url: &str, name: &str) -> String {
    query(url)
        .into_iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v)
        .unwrap_or_default()
}

#[tokio::test]
async fn the_authorize_url() {
    let (account, _) = make("https://abc.supabase.co/", None);
    let pkce = Pkce::from_verifier(Secret::new("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"));
    let url = account.authorize_url("http://127.0.0.1:53682/callback", &pkce);
    assert!(
        url.starts_with("https://abc.supabase.co/auth/v1/authorize?"),
        "{url}"
    );
    assert_eq!(
        query(&url),
        [
            ("provider".into(), "github".into()),
            (
                "redirect_to".into(),
                "http://127.0.0.1:53682/callback".into()
            ),
            (
                "code_challenge".into(),
                "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM".into()
            ),
            ("code_challenge_method".into(), "s256".into()),
        ]
    );
    assert!(url.contains("redirect_to=http%3A%2F%2F127.0.0.1%3A53682%2Fcallback"));
    assert!(!url.contains("dBjftJeZ4CVP"), "the verifier never leaves");
}

/// Plays the browser: follows the authorize URL's `redirect_to` back with `query`, and returns
/// the page it got.
type Page = Arc<StdMutex<Option<tokio::task::JoinHandle<String>>>>;

fn browser(query_string: String) -> (impl FnOnce(&str) -> Result<(), String>, Page) {
    let page = Arc::new(StdMutex::new(None));
    let slot = page.clone();
    let open = move |url: &str| {
        let redirect = param(url, "redirect_to");
        let target = reqwest::Url::parse(&redirect).unwrap();
        let port = target.port().unwrap();
        let path = target.path().to_owned();
        *slot.lock().unwrap() = Some(tokio::spawn(async move {
            let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port))
                .await
                .unwrap();
            s.write_all(
                format!("GET {path}?{query_string} HTTP/1.1\r\nHost: x\r\n\r\n").as_bytes(),
            )
            .await
            .unwrap();
            let mut out = String::new();
            let _ = s.read_to_string(&mut out).await;
            out
        }));
        Ok(())
    };
    (open, page)
}

fn options() -> SignInOptions {
    SignInOptions {
        port: 0,
        timeout: Duration::from_secs(10),
    }
}

#[tokio::test]
async fn sign_in_end_to_end_with_a_fake_browser() {
    let challenge = Arc::new(StdMutex::new(String::new()));
    let seen = challenge.clone();
    let fake = Fake::start(move |r| {
        if r.target == "/auth/v1/token?grant_type=pkce" {
            let body = r.json();
            let verifier = body["code_verifier"].as_str().unwrap_or_default();
            let expected = Pkce::from_verifier(Secret::new(verifier));
            if body["auth_code"] == CODE && expected.challenge() == *seen.lock().unwrap() {
                return Reply::json(200, token_body(NEW_ACCESS, NEW_REFRESH));
            }
            return Reply::json(404, json!({"code": 404, "error_code": "flow_state_not_found", "msg": "invalid flow state"}));
        }
        Reply::empty(404)
    })
    .await;
    let (account, store) = make(&fake.url, None);
    let (open, page) = browser(format!("code={CODE}"));
    let keep = challenge.clone();
    let open = move |url: &str| {
        *keep.lock().unwrap() = param(url, "code_challenge");
        assert_eq!(param(url, "code_challenge_method"), "s256");
        open(url)
    };
    account
        .sign_in(options(), open, &Cancel::new())
        .await
        .unwrap();
    assert!(account.is_signed_in().await);
    assert_eq!(account.user_id().await.as_deref(), Some(USER));
    let saved = store.load().unwrap().unwrap();
    assert_eq!(saved.access_token.expose(), NEW_ACCESS);
    assert_eq!(saved.refresh_token.expose(), NEW_REFRESH);
    let page = page.lock().unwrap().take().unwrap();
    let page = page.await.unwrap();
    assert!(
        page.contains("You're signed in. You can close this tab."),
        "{page}"
    );

    let exchange = &fake.to("/auth/v1/token")[0];
    assert_eq!(exchange.method, "POST");
    assert_eq!(exchange.header("apikey"), Some(KEY));
    assert_eq!(exchange.bearer(), None);
    assert_eq!(exchange.json()["auth_code"], CODE);
}

#[tokio::test]
async fn a_code_the_server_rejects() {
    let fake = Fake::start(|_| {
        Reply::json(404, json!({"code": 404, "error_code": "flow_state_not_found", "msg": "invalid flow state, no valid flow state found"}))
    })
    .await;
    let (account, store) = make(&fake.url, None);
    let (open, page) = browser(format!("code={CODE}"));
    let e = account
        .sign_in(options(), open, &Cancel::new())
        .await
        .unwrap_err();
    assert_eq!(e, Error::CodeRejected);
    assert_eq!(
        e.to_string(),
        "Sign-in didn't finish: the server didn't accept it. Try again."
    );
    assert_no_secrets(&format!("{e} {e:?}"));
    assert!(!account.is_signed_in().await);
    assert_eq!(store.load().unwrap(), None);
    let page = page.lock().unwrap().take().unwrap();
    let page = page.await.unwrap();
    assert!(page.contains("Sign-in didn't finish."), "{page}");
}

#[tokio::test]
async fn a_provider_error_signs_nobody_in() {
    let fake = Fake::start(|_| Reply::empty(500)).await;
    let (account, _) = make(&fake.url, None);
    let (open, _page) = browser("error=access_denied&error_description=nope".into());
    let e = account
        .sign_in(options(), open, &Cancel::new())
        .await
        .unwrap_err();
    assert_eq!(e.to_string(), "Sign-in was turned down on GitHub.");
    assert!(fake.requests().is_empty(), "no exchange without a code");
}

#[tokio::test]
async fn a_browser_that_doesnt_open() {
    let (account, _) = make(&dead_url().await, None);
    let e = account
        .sign_in(options(), |_| Err("no browser".into()), &Cancel::new())
        .await
        .unwrap_err();
    assert_eq!(e, Error::BrowserDidntOpen);
    // The flow is over: another can start.
    let e = account
        .sign_in(
            SignInOptions {
                port: 0,
                timeout: Duration::from_millis(50),
            },
            |_| Ok(()),
            &Cancel::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(e, Error::SignInTimedOut);
}

#[tokio::test]
async fn one_sign_in_at_a_time_and_cancel_ends_it() {
    let (account, _) = make(&dead_url().await, None);
    let account = Arc::new(account);
    let cancel = Cancel::new();
    let first = {
        let (account, cancel) = (account.clone(), cancel.clone());
        tokio::spawn(async move { account.sign_in(options(), |_| Ok(()), &cancel).await })
    };
    tokio::time::sleep(Duration::from_millis(100)).await;
    let e = account
        .sign_in(options(), |_| Ok(()), &Cancel::new())
        .await
        .unwrap_err();
    assert_eq!(e, Error::SignInInProgress);
    cancel.cancel();
    assert_eq!(first.await.unwrap().unwrap_err(), Error::SignInCancelled);
}

#[tokio::test]
async fn a_taken_port() {
    let taken = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = taken.local_addr().unwrap().port();
    let (account, _) = make(&dead_url().await, None);
    let opened = Arc::new(StdMutex::new(false));
    let o = opened.clone();
    let e = account
        .sign_in(
            SignInOptions {
                port,
                timeout: Duration::from_secs(1),
            },
            move |_| {
                *o.lock().unwrap() = true;
                Ok(())
            },
            &Cancel::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(e, Error::PortTaken(port));
    assert!(
        !*opened.lock().unwrap(),
        "no browser for a flow that can't finish"
    );
}

#[tokio::test]
async fn password_sign_in() {
    let fake = Fake::start(|r| {
        if r.json()["password"] == "right-password" {
            Reply::json(200, token_body(NEW_ACCESS, NEW_REFRESH))
        } else {
            Reply::json(400, json!({"code": 400, "error_code": "invalid_credentials", "msg": "Invalid login credentials"}))
        }
    })
    .await;
    let (account, store) = make(&fake.url, None);
    let e = account
        .password_sign_in("a@example.test", &Secret::new("wrong-password"))
        .await
        .unwrap_err();
    assert_eq!(
        e.to_string(),
        "That email and password don't match an account."
    );
    assert!(!format!("{e:?}").contains("wrong-password"));
    account
        .password_sign_in("a@example.test", &Secret::new("right-password"))
        .await
        .unwrap();
    assert_eq!(
        store.load().unwrap().unwrap().access_token.expose(),
        NEW_ACCESS
    );
    let r = &fake.requests()[1];
    assert_eq!(r.target, "/auth/v1/token?grant_type=password");
    assert_eq!(
        r.json(),
        json!({"email": "a@example.test", "password": "right-password"})
    );
}

#[tokio::test]
async fn a_fresh_session_is_used_as_it_is() {
    let fake = Fake::start(backend(|| Reply::empty(500))).await;
    let (account, _) = make(&fake.url, Some(session(3600)));
    account.inbox(None).await.unwrap();
    assert!(fake.to("/auth/v1/token").is_empty());
    assert_eq!(fake.to("/rest/v1/rpc/inbox")[0].bearer(), Some(OLD_ACCESS));
}

#[tokio::test]
async fn a_session_about_to_expire_is_refreshed_first() {
    let fake = Fake::start(backend(|| {
        Reply::json(200, token_body(NEW_ACCESS, NEW_REFRESH))
    }))
    .await;
    let (account, store) = make(&fake.url, Some(session(30)));
    account.inbox(None).await.unwrap();
    let refresh = &fake.to("/auth/v1/token")[0];
    assert_eq!(refresh.target, "/auth/v1/token?grant_type=refresh_token");
    assert_eq!(refresh.json(), json!({ "refresh_token": OLD_REFRESH }));
    assert_eq!(refresh.header("apikey"), Some(KEY));
    assert_eq!(fake.to("/rest/v1/rpc/inbox")[0].bearer(), Some(NEW_ACCESS));
    let saved = store.load().unwrap().unwrap();
    assert_eq!(saved.refresh_token.expose(), NEW_REFRESH);
    assert!(!saved.expires_within(now_secs(), 3000));
}

#[tokio::test]
async fn a_refresh_the_server_rejects_signs_out() {
    let fake = Fake::start(backend(|| {
        Reply::json(400, json!({"code": 400, "error_code": "refresh_token_not_found", "msg": "Invalid Refresh Token: Refresh Token Not Found"}))
    }))
    .await;
    let (account, store) = make(&fake.url, Some(session(10)));
    let e = account.inbox(None).await.unwrap_err();
    assert_eq!(e, Error::NotSignedIn);
    assert_eq!(e.to_string(), "You're not signed in.");
    assert!(!account.is_signed_in().await);
    assert_eq!(store.load().unwrap(), None);
    assert!(fake.to("/rest/v1/rpc").is_empty());
}

#[tokio::test]
async fn a_network_failure_keeps_the_session() {
    let (account, store) = make(&dead_url().await, Some(session(10)));
    let e = account.inbox(None).await.unwrap_err();
    assert_eq!(e, Error::Unreachable);
    assert_eq!(
        e.to_string(),
        "Can't reach the server. Check your connection and try again."
    );
    assert!(account.is_signed_in().await);
    let kept = store.load().unwrap().unwrap();
    assert_eq!(kept.refresh_token.expose(), OLD_REFRESH);
    assert_no_secrets(&format!("{e} {e:?}"));
}

#[tokio::test]
async fn a_server_error_while_refreshing_keeps_the_session() {
    let fake = Fake::start(backend(|| Reply::empty(503))).await;
    let (account, store) = make(&fake.url, Some(session(10)));
    assert_eq!(account.inbox(None).await.unwrap_err(), Error::Server(503));
    assert!(store.load().unwrap().is_some());
}

#[tokio::test]
async fn concurrent_calls_share_one_refresh() {
    let fake = Fake::start(backend(|| {
        Reply::json(200, token_body(NEW_ACCESS, NEW_REFRESH)).after(Duration::from_millis(200))
    }))
    .await;
    let (account, _) = make(&fake.url, Some(session(10)));
    let account = Arc::new(account);
    let calls: Vec<_> = (0..4)
        .map(|_| {
            let a = account.clone();
            tokio::spawn(async move { a.inbox(None).await })
        })
        .collect();
    for call in calls {
        call.await.unwrap().unwrap();
    }
    assert_eq!(
        fake.to("/auth/v1/token").len(),
        1,
        "one refresh for all four"
    );
    let rpcs = fake.to("/rest/v1/rpc/inbox");
    assert_eq!(rpcs.len(), 4);
    assert!(rpcs.iter().all(|r| r.bearer() == Some(NEW_ACCESS)));
}

#[tokio::test]
async fn a_token_the_server_turns_down_is_refreshed_once_and_retried() {
    let fake = Fake::start(|r| {
        if r.target.starts_with("/auth/v1/token") {
            Reply::json(200, token_body(NEW_ACCESS, NEW_REFRESH))
        } else if r.bearer() == Some(OLD_ACCESS) {
            Reply::json(401, json!({"code": "PGRST303", "message": "JWT expired"}))
        } else {
            Reply::json(200, json!([{ "call_id": "c1", "code": "k1" }]))
        }
    })
    .await;
    let (account, _) = make(&fake.url, Some(session(3600)));
    let call = account.start_call().await.unwrap();
    assert_eq!((call.call_id.as_str(), call.code.as_str()), ("c1", "k1"));
    assert_eq!(fake.to("/auth/v1/token").len(), 1);
    assert_eq!(fake.to("/rest/v1/rpc/start_call").len(), 2);
}

#[tokio::test]
async fn not_signed_in_without_a_session() {
    let fake = Fake::start(|_| Reply::empty(200)).await;
    let (account, _) = make(&fake.url, None);
    assert_eq!(account.start_call().await.unwrap_err(), Error::NotSignedIn);
    assert_eq!(
        account.call_pass("c1").await.unwrap_err(),
        Error::NotSignedIn
    );
    assert!(fake.requests().is_empty());
}

#[tokio::test]
async fn sign_out_tells_the_server_and_forgets() {
    let fake = Fake::start(|_| Reply::empty(204)).await;
    let (account, store) = make(&fake.url, Some(session(3600)));
    account.sign_out().await.unwrap();
    assert!(!account.is_signed_in().await);
    assert_eq!(store.load().unwrap(), None);
    let logout = &fake.requests()[0];
    assert_eq!(logout.method, "POST");
    assert_eq!(logout.target, "/auth/v1/logout?scope=local");
    assert_eq!(logout.bearer(), Some(OLD_ACCESS));
    assert_eq!(logout.header("apikey"), Some(KEY));
}

#[tokio::test]
async fn sign_out_forgets_even_when_the_server_fails_or_is_gone() {
    let fake = Fake::start(|_| Reply::empty(500)).await;
    let (account, store) = make(&fake.url, Some(session(3600)));
    account.sign_out().await.unwrap();
    assert_eq!(store.load().unwrap(), None);

    let (account, store) = make(&dead_url().await, Some(session(3600)));
    account.sign_out().await.unwrap();
    assert_eq!(store.load().unwrap(), None);
    assert!(!account.is_signed_in().await);
    // Signing out twice is fine.
    account.sign_out().await.unwrap();
}

#[tokio::test]
async fn sign_out_with_an_expired_token_refreshes_it_to_end_the_session() {
    let fake = Fake::start(|r| {
        if r.target.starts_with("/auth/v1/token") {
            Reply::json(200, token_body(NEW_ACCESS, NEW_REFRESH))
        } else {
            Reply::empty(204)
        }
    })
    .await;
    let (account, store) = make(&fake.url, Some(session(-10)));
    account.sign_out().await.unwrap();
    assert_eq!(fake.to("/auth/v1/logout")[0].bearer(), Some(NEW_ACCESS));
    assert_eq!(store.load().unwrap(), None);
}

/// Each function's request, as PostgREST expects it, and its answer read back.
#[tokio::test]
async fn every_rpc_sends_the_right_request() {
    let fake = Fake::start(|r| {
        let name = r.target.trim_start_matches("/rest/v1/rpc/");
        let profile = json!({ "id": "u2", "name": "Sam", "handle": "sam", "avatar_url": null });
        match name {
            "inbox" => Reply::json(200, json!({ "me": profile, "contacts": [], "hosting": null, "rings": [], "waiting": [] })),
            "start_call" | "ring" => Reply::json(200, json!([{ "call_id": "c1", "code": "k1" }])),
            "knock" => Reply::json(200, json!([{ "call_id": "c1", "state": "knocking", "host_id": "u1", "host_name": "Alex", "host_handle": "alex", "host_avatar_url": null }])),
            "answer_knock" => Reply::json(200, json!("admitted")),
            "answer_ring" => Reply::json(200, json!("declined")),
            "leave_call" | "end_call" | "remove_contact" => Reply::empty(204),
            _ => Reply::empty(404),
        }
    })
    .await;
    let (account, _) = make(&fake.url, Some(session(3600)));

    assert_eq!(
        account.inbox(None).await.unwrap().me.handle.as_deref(),
        Some("sam")
    );
    account.inbox(Some("c9")).await.unwrap();
    assert_eq!(account.start_call().await.unwrap().code, "k1");
    let knock = account.knock("k1").await.unwrap();
    assert_eq!(
        (knock.state, knock.host.name.as_str()),
        (MemberState::Knocking, "Alex")
    );
    assert_eq!(knock.host.handle.as_deref(), Some("alex"));
    assert_eq!(
        account.answer_knock("c1", "u2", true).await.unwrap(),
        MemberState::Admitted
    );
    account.answer_knock("c1", "u2", false).await.unwrap();
    assert_eq!(account.ring("u2").await.unwrap().call_id, "c1");
    assert_eq!(
        account.answer_ring("c1", false).await.unwrap(),
        MemberState::Declined
    );
    account.answer_ring("c1", true).await.unwrap();
    account.leave_call("c1").await.unwrap();
    account.end_call("c1").await.unwrap();
    account.remove_contact("u2").await.unwrap();

    let sent: Vec<(String, Value)> = fake
        .requests()
        .iter()
        .map(|r| {
            assert_eq!(r.method, "POST");
            assert_eq!(r.header("apikey"), Some(KEY));
            assert_eq!(r.bearer(), Some(OLD_ACCESS));
            assert_eq!(r.header("content-type"), Some("application/json"));
            (r.target.clone(), r.json())
        })
        .collect();
    let rpc = |name: &str| format!("/rest/v1/rpc/{name}");
    assert_eq!(
        sent,
        [
            (rpc("inbox"), json!({})),
            (rpc("inbox"), json!({ "hosting": "c9" })),
            (rpc("start_call"), json!({})),
            (rpc("knock"), json!({ "code": "k1" })),
            (
                rpc("answer_knock"),
                json!({ "call_id": "c1", "user_id": "u2", "let_in": true })
            ),
            (
                rpc("answer_knock"),
                json!({ "call_id": "c1", "user_id": "u2", "let_in": false })
            ),
            (rpc("ring"), json!({ "contact": "u2" })),
            (
                rpc("answer_ring"),
                json!({ "call_id": "c1", "accept": false })
            ),
            (
                rpc("answer_ring"),
                json!({ "call_id": "c1", "accept": true })
            ),
            (rpc("leave_call"), json!({ "call_id": "c1" })),
            (rpc("end_call"), json!({ "call_id": "c1" })),
            (rpc("remove_contact"), json!({ "contact": "u2" })),
        ]
    );
}

#[tokio::test]
async fn void_functions_accept_an_empty_or_null_answer() {
    for reply in [
        || Reply::empty(204),
        || Reply::empty(200),
        || Reply::json(200, json!(null)),
    ] {
        let fake = Fake::start(move |_| reply()).await;
        let (account, _) = make(&fake.url, Some(session(3600)));
        account.end_call("c1").await.unwrap();
    }
}

#[tokio::test]
async fn backend_errors_become_sentences() {
    let cases: [(u16, &str, &str, Error); 10] = [
        (400, "P0001", "call_not_found", Error::CallNotFound),
        (
            400,
            "P0001",
            "not_knocking: nobody is waiting with that id",
            Error::NotKnocking,
        ),
        (400, "P0001", "no_pending_ring", Error::NoPendingRing),
        (400, "P0001", "not_in_call", Error::NotInCall),
        (400, "22023", "own_call: you host this call", Error::OwnCall),
        (
            400,
            "22023",
            "bad_argument: let_in is required",
            Error::BadRequest,
        ),
        (403, "42501", "not_a_contact", Error::NotAContact),
        (403, "42501", "not_allowed", Error::NotAllowed),
        (400, "P0001", "a_word_from_the_future", Error::Refused(400)),
        (500, "XX000", "internal", Error::Server(500)),
    ];
    for (status, code, message, expected) in cases {
        let fake = Fake::start(move |_| {
            Reply::json(
                status,
                json!({ "code": code, "details": null, "hint": null, "message": message }),
            )
        })
        .await;
        let (account, _) = make(&fake.url, Some(session(3600)));
        let e = account.knock("k1").await.unwrap_err();
        assert_eq!(e, expected, "{message}");
        assert_no_secrets(&format!("{e} {e:?}"));
    }
    // A body that isn't PostgREST's.
    let fake = Fake::start(|_| Reply::empty(404)).await;
    let (account, _) = make(&fake.url, Some(session(3600)));
    assert_eq!(account.knock("k1").await.unwrap_err(), Error::Refused(404));
    // A successful answer that can't be read.
    let fake = Fake::start(|_| Reply::json(200, json!({ "unexpected": true }))).await;
    let (account, _) = make(&fake.url, Some(session(3600)));
    assert_eq!(account.knock("k1").await.unwrap_err(), Error::BadAnswer);
    assert_eq!(account.start_call().await.unwrap_err(), Error::BadAnswer);
}

#[tokio::test]
async fn the_call_pass() {
    let fake = Fake::start(|r| match r.json()["call_id"].as_str() {
        Some("ok") => Reply::json(
            200,
            json!({
                "url": "ws://127.0.0.1:7880", "token": "eyJ.pass-payload.sig", "room": "r1",
                "identity": "guest-u2", "name": "Sam", "role": "guest"
            }),
        ),
        Some("no") => Reply::json(403, json!({ "error": "not_allowed" })),
        Some("bad") => Reply::json(400, json!({ "error": "bad_request" })),
        _ => Reply::json(500, json!({ "error": "server_error" })),
    })
    .await;
    let (account, _) = make(&fake.url, Some(session(3600)));
    let pass = account.call_pass("ok").await.unwrap();
    assert_eq!(pass.url, "ws://127.0.0.1:7880");
    assert_eq!(pass.token.expose(), "eyJ.pass-payload.sig");
    assert_eq!(
        (pass.role, pass.identity.as_str(), pass.name.as_str()),
        (PassRole::Guest, "guest-u2", "Sam")
    );
    assert!(!format!("{pass:?}").contains("pass-payload"));
    let r = &fake.requests()[0];
    assert_eq!(r.target, "/functions/v1/call-pass");
    assert_eq!(r.json(), json!({ "call_id": "ok" }));
    assert_eq!(r.bearer(), Some(OLD_ACCESS));
    assert_eq!(r.header("apikey"), Some(KEY));

    assert_eq!(
        account.call_pass("no").await.unwrap_err(),
        Error::NotAllowed
    );
    assert_eq!(
        account.call_pass("bad").await.unwrap_err(),
        Error::BadRequest
    );
    assert_eq!(
        account.call_pass("other").await.unwrap_err(),
        Error::Server(500)
    );
}

#[tokio::test]
async fn an_account_on_disk_starts_signed_in() {
    let dir =
        std::env::temp_dir().join(format!("twincursor-account-client-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let store = crate::FileStore::new(dir.join("session.json"));
    store.save(&session(3600)).unwrap();
    let account = Account::new(Backend::new("http://127.0.0.1:1", KEY).unwrap(), store).unwrap();
    assert!(account.is_signed_in().await);
    assert_no_secrets(&format!("{account:?}"));
    let _ = std::fs::remove_dir_all(dir);
}

/// Against a running local stack (`supabase start`), with TWINCURSOR_SUPABASE_URL and
/// TWINCURSOR_SUPABASE_KEY set: `cargo test -p twincursor-account -- --ignored`.
#[tokio::test]
#[ignore = "needs a local Supabase stack"]
async fn a_local_stack_accepts_the_authorize_url_and_rejects_a_wrong_code() {
    let url = std::env::var("TWINCURSOR_SUPABASE_URL").expect("TWINCURSOR_SUPABASE_URL");
    let key = std::env::var("TWINCURSOR_SUPABASE_KEY").expect("TWINCURSOR_SUPABASE_KEY");
    let account = Account::new(Backend::new(&url, &key).unwrap(), MemoryStore::new(None)).unwrap();
    let pkce = Pkce::new().unwrap();
    let authorize = account.authorize_url("http://127.0.0.1:53682/callback", &pkce);
    let answer = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
        .get(&authorize)
        .send()
        .await
        .unwrap();
    assert_eq!(answer.status().as_u16(), 302);
    let location = answer.headers()["location"].to_str().unwrap();
    assert!(
        location.starts_with("https://github.com/login/oauth/authorize?"),
        "{location}"
    );
    let e = account
        .exchange_code(
            &Secret::new("00000000-0000-0000-0000-000000000000"),
            pkce.verifier(),
        )
        .await
        .unwrap_err();
    assert_eq!(e, Error::CodeRejected);
}
