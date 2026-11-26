//! The signed-in person and the backend's calls.
//!
//! Supabase Auth (GoTrue) routes used, as the local stack answers them:
//!
//! - `GET  /auth/v1/authorize?provider=github&redirect_to=…&code_challenge=…&code_challenge_method=s256`
//!   redirects to GitHub, and in the end to `redirect_to?code=…` (or `?error=…&error_description=…`).
//! - `POST /auth/v1/token?grant_type=pkce` `{auth_code, code_verifier}`,
//!   `POST /auth/v1/token?grant_type=refresh_token` `{refresh_token}` and
//!   `POST /auth/v1/token?grant_type=password` `{email, password}` answer
//!   `{access_token, token_type, expires_in, expires_at, refresh_token, user: {id, …}}`, or an error
//!   `{code, error_code, msg}`.
//! - `POST /auth/v1/logout?scope=local` ends this session only (204).

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::de::{DeserializeOwned, IgnoredAny};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::callback::{Callback, Cancel, CALLBACK_PORT, SIGN_IN_TIMEOUT};
use crate::types::{CallPassBody, KnockRow};
use crate::{
    Backend, CallPass, Error, Inbox, Knock, MemberState, Pkce, Secret, Session, SessionStore,
    StartedCall,
};

/// A session is refreshed when its access token has less than this left.
pub const REFRESH_MARGIN_SECS: u64 = 60;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

/// How [`Account::sign_in`] listens.
#[derive(Debug, Clone, Copy)]
pub struct SignInOptions {
    /// The callback port; the backend's redirect allow-list must name it.
    pub port: u16,
    pub timeout: Duration,
}

impl Default for SignInOptions {
    fn default() -> SignInOptions {
        SignInOptions {
            port: CALLBACK_PORT,
            timeout: SIGN_IN_TIMEOUT,
        }
    }
}

/// One person's account on one backend: their session, kept in a [`SessionStore`], and the
/// backend's functions called as them.
pub struct Account {
    backend: Backend,
    http: reqwest::Client,
    store: Box<dyn SessionStore>,
    /// Held across a refresh, so concurrent calls wait for one refresh instead of each spending
    /// the refresh token.
    session: tokio::sync::Mutex<Option<Session>>,
    /// One sign-in at a time.
    flow: tokio::sync::Mutex<()>,
}

impl std::fmt::Debug for Account {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Account")
            .field("backend", &self.backend)
            .finish_non_exhaustive()
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

#[derive(Deserialize)]
struct TokenBody {
    access_token: String,
    refresh_token: String,
    #[serde(default)]
    expires_in: Option<u64>,
    #[serde(default)]
    expires_at: Option<u64>,
    user: UserBody,
}

#[derive(Deserialize)]
struct UserBody {
    id: String,
}

/// An error body from the auth server (`error_code`) or an older one (`error`).
#[derive(Deserialize, Default)]
struct AuthErrorBody {
    #[serde(default)]
    error_code: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

/// An error body from PostgREST.
#[derive(Deserialize, Default)]
struct RestErrorBody {
    #[serde(default)]
    message: Option<String>,
}

enum RefreshFailed {
    /// The server says the refresh token is no good: signed out.
    Rejected,
    /// Couldn't tell: keep the session.
    Other(Error),
}

struct Answer {
    status: u16,
    body: Vec<u8>,
}

impl Answer {
    fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }

    fn json<T: DeserializeOwned>(&self) -> Result<T, Error> {
        let body: &[u8] = if self.body.iter().all(u8::is_ascii_whitespace) {
            b"null"
        } else {
            &self.body
        };
        serde_json::from_slice(body).map_err(|_| Error::BadAnswer)
    }

    fn auth_error_word(&self) -> String {
        let body: AuthErrorBody = serde_json::from_slice(&self.body).unwrap_or_default();
        body.error_code.or(body.error).unwrap_or_default()
    }

    /// The error a backend function answered with.
    fn rest_error(&self) -> Error {
        let body: RestErrorBody = serde_json::from_slice(&self.body).unwrap_or_default();
        body.message
            .as_deref()
            .and_then(Error::from_backend_word)
            .unwrap_or_else(|| Error::from_status(self.status))
    }
}

impl Account {
    /// The account for `backend`, signed in if `store` has a session.
    pub fn new(backend: Backend, store: impl SessionStore + 'static) -> Result<Account, Error> {
        let session = store.load()?;
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|_| Error::Unreachable)?;
        Ok(Account {
            backend,
            http,
            store: Box::new(store),
            session: tokio::sync::Mutex::new(session),
            flow: tokio::sync::Mutex::new(()),
        })
    }

    pub fn backend(&self) -> &Backend {
        &self.backend
    }

    /// There is a session. It may still turn out to be expired or revoked on the next call.
    pub async fn is_signed_in(&self) -> bool {
        self.session.lock().await.is_some()
    }

    pub async fn user_id(&self) -> Option<String> {
        self.session
            .lock()
            .await
            .as_ref()
            .map(|s| s.user_id.clone())
    }

    /// The page that starts GitHub sign-in, sending the browser back to `redirect_uri`.
    pub fn authorize_url(&self, redirect_uri: &str, pkce: &Pkce) -> String {
        let mut url = reqwest::Url::parse(&self.backend.endpoint("/auth/v1/authorize"))
            .expect("a checked backend URL");
        url.query_pairs_mut()
            .append_pair("provider", "github")
            .append_pair("redirect_to", redirect_uri)
            .append_pair("code_challenge", pkce.challenge())
            .append_pair("code_challenge_method", "s256");
        url.into()
    }

    /// Signs in with GitHub: listens for the callback, has `open` show the sign-in page (the
    /// system browser; tests pass their own), exchanges the code with the verifier and keeps the
    /// session. Ends with an error after `options.timeout` or when `cancel` fires.
    pub async fn sign_in<F>(
        &self,
        options: SignInOptions,
        open: F,
        cancel: &Cancel,
    ) -> Result<(), Error>
    where
        F: FnOnce(&str) -> Result<(), String>,
    {
        let _flow = self.flow.try_lock().map_err(|_| Error::SignInInProgress)?;
        let pkce = Pkce::new()?;
        let callback = Callback::bind(options.port).await?;
        let url = self.authorize_url(&callback.redirect_uri(), &pkce);
        open(&url).map_err(|_| Error::BrowserDidntOpen)?;
        let arrival = callback.wait(cancel, options.timeout).await?;
        let result = match self.exchange_code(&arrival.code, pkce.verifier()).await {
            Ok(session) => self.adopt(session).await,
            Err(e) => Err(e),
        };
        arrival.reply(result.is_ok()).await;
        result
    }

    /// Trades an authorization code and its verifier for a session (`grant_type=pkce`).
    pub async fn exchange_code(&self, code: &Secret, verifier: &Secret) -> Result<Session, Error> {
        let answer = self
            .token_request(
                "pkce",
                &json!({ "auth_code": code.expose(), "code_verifier": verifier.expose() }),
            )
            .await?;
        match answer.status {
            200..=299 => session_from(&answer),
            400 | 401 | 403 | 404 => Err(Error::CodeRejected),
            status => Err(Error::from_status(status)),
        }
    }

    /// Email and password sign-in. Only for local stacks and tests, where it is turned on; a
    /// hosted backend offers GitHub alone.
    pub async fn password_sign_in(&self, email: &str, password: &Secret) -> Result<(), Error> {
        let answer = self
            .token_request(
                "password",
                &json!({ "email": email, "password": password.expose() }),
            )
            .await?;
        let session = match answer.status {
            200..=299 => session_from(&answer)?,
            400 if answer.auth_error_word() == "invalid_credentials" => {
                return Err(Error::WrongPassword)
            }
            status => return Err(Error::from_status(status)),
        };
        self.adopt(session).await
    }

    async fn adopt(&self, session: Session) -> Result<(), Error> {
        let mut slot = self.session.lock().await;
        self.store.save(&session)?;
        *slot = Some(session);
        Ok(())
    }

    async fn token_request(&self, grant: &str, body: &Value) -> Result<Answer, Error> {
        let url = self
            .backend
            .endpoint(&format!("/auth/v1/token?grant_type={grant}"));
        let response = self
            .http
            .post(url)
            .header("apikey", self.backend.key())
            .json(body)
            .send()
            .await
            .map_err(|e| Error::from_transport(&e))?;
        read(response).await
    }

    async fn refresh(&self, refresh_token: &Secret) -> Result<Session, RefreshFailed> {
        let answer = self
            .token_request(
                "refresh_token",
                &json!({ "refresh_token": refresh_token.expose() }),
            )
            .await
            .map_err(RefreshFailed::Other)?;
        match answer.status {
            200..=299 => session_from(&answer).map_err(RefreshFailed::Other),
            400 | 401 | 403 | 404 => Err(RefreshFailed::Rejected),
            status => Err(RefreshFailed::Other(Error::from_status(status))),
        }
    }

    /// A usable access token: the current one, or a fresh one if it has under
    /// [`REFRESH_MARGIN_SECS`] left or is `stale` (the server just turned it down). Concurrent
    /// callers share one refresh.
    async fn access_token(&self, stale: Option<&Secret>) -> Result<Secret, Error> {
        let mut slot = self.session.lock().await;
        let Some(current) = slot.as_ref() else {
            return Err(Error::NotSignedIn);
        };
        let turned_down = stale.is_some_and(|s| *s == current.access_token);
        if !turned_down && !current.expires_within(now_secs(), REFRESH_MARGIN_SECS) {
            return Ok(current.access_token.clone());
        }
        match self.refresh(&current.refresh_token).await {
            Ok(fresh) => {
                let token = fresh.access_token.clone();
                let saved = self.store.save(&fresh);
                *slot = Some(fresh);
                saved.map(|()| token)
            }
            Err(RefreshFailed::Rejected) => {
                *slot = None;
                self.store.clear()?;
                Err(Error::NotSignedIn)
            }
            Err(RefreshFailed::Other(e)) => Err(e),
        }
    }

    /// Signs out: tells the server to end this session, then forgets it here whatever the server
    /// said (or if it couldn't be reached).
    pub async fn sign_out(&self) -> Result<(), Error> {
        let mut slot = self.session.lock().await;
        if let Some(mut session) = slot.take() {
            // The server only ends a session for a token that is still good.
            if session.expires_within(now_secs(), 0) {
                if let Ok(fresh) = self.refresh(&session.refresh_token).await {
                    session = fresh;
                }
            }
            let _ = self
                .http
                .post(self.backend.endpoint("/auth/v1/logout?scope=local"))
                .header("apikey", self.backend.key())
                .bearer_auth(session.access_token.expose())
                .send()
                .await;
        }
        self.store.clear()
    }

    async fn send(&self, url: &str, body: &Value, token: &Secret) -> Result<Answer, Error> {
        let response = self
            .http
            .post(url)
            .header("apikey", self.backend.key())
            .bearer_auth(token.expose())
            .json(body)
            .send()
            .await
            .map_err(|e| Error::from_transport(&e))?;
        read(response).await
    }

    /// A POST as the signed-in person. A 401 gets one fresh token and one more try.
    async fn post(&self, path: &str, body: &Value) -> Result<Answer, Error> {
        let url = self.backend.endpoint(path);
        let token = self.access_token(None).await?;
        let answer = self.send(&url, body, &token).await?;
        if answer.status != 401 {
            return Ok(answer);
        }
        let token = self.access_token(Some(&token)).await?;
        let answer = self.send(&url, body, &token).await?;
        if answer.status == 401 {
            return Err(Error::NotSignedIn);
        }
        Ok(answer)
    }

    async fn rpc<T: DeserializeOwned>(&self, name: &str, body: Value) -> Result<T, Error> {
        let answer = self.post(&format!("/rest/v1/rpc/{name}"), &body).await?;
        if answer.ok() {
            answer.json()
        } else {
            Err(answer.rest_error())
        }
    }

    async fn rpc_row<T: DeserializeOwned>(&self, name: &str, body: Value) -> Result<T, Error> {
        let rows: Vec<T> = self.rpc(name, body).await?;
        rows.into_iter().next().ok_or(Error::BadAnswer)
    }

    /// Everything the app shows. `hosting` is the call this app is sharing right now: naming it
    /// is the host's heartbeat, which keeps the call live.
    pub async fn inbox(&self, hosting: Option<&str>) -> Result<Inbox, Error> {
        let body = match hosting {
            Some(call) => json!({ "hosting": call }),
            None => json!({}),
        };
        self.rpc("inbox", body).await
    }

    /// Starts a call hosted by you, ending your earlier one.
    pub async fn start_call(&self) -> Result<StartedCall, Error> {
        self.rpc_row("start_call", json!({})).await
    }

    /// Asks to join the call with this code. Repeat it every few seconds while waiting.
    pub async fn knock(&self, code: &str) -> Result<Knock, Error> {
        self.rpc_row::<KnockRow>("knock", json!({ "code": code }))
            .await
            .map(Knock::from)
    }

    /// Lets someone knocking on your call in, or turns them away.
    pub async fn answer_knock(
        &self,
        call_id: &str,
        user_id: &str,
        let_in: bool,
    ) -> Result<MemberState, Error> {
        self.rpc(
            "answer_knock",
            json!({ "call_id": call_id, "user_id": user_id, "let_in": let_in }),
        )
        .await
    }

    /// Rings a contact: starts a call (ending your earlier one) and rings them for 60 seconds.
    pub async fn ring(&self, contact: &str) -> Result<StartedCall, Error> {
        self.rpc_row("ring", json!({ "contact": contact })).await
    }

    pub async fn answer_ring(&self, call_id: &str, accept: bool) -> Result<MemberState, Error> {
        self.rpc(
            "answer_ring",
            json!({ "call_id": call_id, "accept": accept }),
        )
        .await
    }

    pub async fn leave_call(&self, call_id: &str) -> Result<(), Error> {
        self.rpc::<IgnoredAny>("leave_call", json!({ "call_id": call_id }))
            .await
            .map(drop)
    }

    pub async fn end_call(&self, call_id: &str) -> Result<(), Error> {
        self.rpc::<IgnoredAny>("end_call", json!({ "call_id": call_id }))
            .await
            .map(drop)
    }

    pub async fn remove_contact(&self, contact: &str) -> Result<(), Error> {
        self.rpc::<IgnoredAny>("remove_contact", json!({ "contact": contact }))
            .await
            .map(drop)
    }

    /// A LiveKit token for this call, if you may join it.
    pub async fn call_pass(&self, call_id: &str) -> Result<CallPass, Error> {
        let answer = self
            .post("/functions/v1/call-pass", &json!({ "call_id": call_id }))
            .await?;
        match answer.status {
            200..=299 => answer.json::<CallPassBody>().map(CallPass::from),
            403 => Err(Error::NotAllowed),
            400 => Err(Error::BadRequest),
            status => Err(Error::from_status(status)),
        }
    }
}

async fn read(response: reqwest::Response) -> Result<Answer, Error> {
    let status = response.status().as_u16();
    let body = response
        .bytes()
        .await
        .map_err(|_| Error::Unreachable)?
        .to_vec();
    Ok(Answer { status, body })
}

fn session_from(answer: &Answer) -> Result<Session, Error> {
    let body: TokenBody = answer.json()?;
    let expires_at = body
        .expires_at
        .or_else(|| body.expires_in.map(|s| now_secs() + s))
        .ok_or(Error::BadAnswer)?;
    if body.access_token.is_empty() || body.refresh_token.is_empty() || body.user.id.is_empty() {
        return Err(Error::BadAnswer);
    }
    Ok(Session {
        access_token: Secret::new(body.access_token),
        refresh_token: Secret::new(body.refresh_token),
        expires_at,
        user_id: body.user.id,
    })
}

#[cfg(test)]
mod tests;
