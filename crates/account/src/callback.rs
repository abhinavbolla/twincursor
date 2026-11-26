//! The one-time listener that receives the end of a sign-in: the browser comes back to
//! `http://127.0.0.1:53682/callback?code=…` (or `?error=…`).
//!
//! It listens on the loopback address only, answers nothing but `GET /callback` (everything else
//! gets a 404 and doesn't end the flow), stops after the first request that carries a `code` or an
//! `error`, and gives up after [`SIGN_IN_TIMEOUT`] or when cancelled. Its pages are fixed text:
//! nothing from the request is ever written back.

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, watch};

use crate::{Error, Secret};

/// The port the backend's redirect allow-list names (`supabase/config.toml`).
pub const CALLBACK_PORT: u16 = 53682;
pub const CALLBACK_PATH: &str = "/callback";
/// How long a sign-in may take in the browser.
pub const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// How long one connection may take to send its request line and headers.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// The most a request's head may be; a real callback is a few hundred bytes.
const MAX_HEAD: usize = 8 * 1024;

const SIGNED_IN_PAGE: &str = "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
<title>TwinCursor</title></head><body style=\"font: 16px -apple-system, system-ui, sans-serif; \
margin: 20vh auto; max-width: 28em; text-align: center\"><p>You're signed in. You can close this \
tab.</p></body></html>";
const FAILED_PAGE: &str = "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
<title>TwinCursor</title></head><body style=\"font: 16px -apple-system, system-ui, sans-serif; \
margin: 20vh auto; max-width: 28em; text-align: center\"><p>Sign-in didn't finish. Go back to \
TwinCursor and try again.</p></body></html>";

/// Stops a sign-in that is waiting. Clones share one switch.
#[derive(Debug, Clone)]
pub struct Cancel(Arc<watch::Sender<bool>>);

impl Default for Cancel {
    fn default() -> Cancel {
        Cancel(Arc::new(watch::channel(false).0))
    }
}

impl Cancel {
    pub fn new() -> Cancel {
        Cancel::default()
    }

    pub fn cancel(&self) {
        self.0.send_replace(true);
    }

    pub fn is_cancelled(&self) -> bool {
        *self.0.borrow()
    }

    async fn cancelled(&self) {
        let mut rx = self.0.subscribe();
        let _ = rx.wait_for(|cancelled| *cancelled).await;
    }
}

/// The listener, bound and waiting.
#[derive(Debug)]
pub struct Callback {
    listener: TcpListener,
    port: u16,
}

/// What a callback request said.
enum Said {
    Code(Secret),
    /// The provider's `error` word, if it is a plain lowercase word; never its description.
    Error(Option<String>),
}

/// The browser's request that carried the code, kept open so the page it gets can say whether
/// sign-in actually finished.
#[derive(Debug)]
pub struct Arrival {
    pub code: Secret,
    stream: TcpStream,
}

impl Arrival {
    /// Answers the browser with the fixed "signed in" or "didn't finish" page.
    pub async fn reply(mut self, signed_in: bool) {
        let page = if signed_in {
            SIGNED_IN_PAGE
        } else {
            FAILED_PAGE
        };
        respond(&mut self.stream, 200, "OK", page).await;
    }
}

impl Callback {
    /// Listens on `127.0.0.1:port` (`0` picks a free port, for tests).
    pub async fn bind(port: u16) -> Result<Callback, Error> {
        let listener = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, port)))
            .await
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::AddrInUse => Error::PortTaken(port),
                _ => Error::Listener(e.to_string()),
            })?;
        let port = listener
            .local_addr()
            .map_err(|e| Error::Listener(e.to_string()))?
            .port();
        Ok(Callback { listener, port })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// Where the backend should send the browser back to.
    pub fn redirect_uri(&self) -> String {
        format!("http://127.0.0.1:{}{CALLBACK_PATH}", self.port)
    }

    /// Waits for the browser to come back with a code. An `error` instead ends the flow with a
    /// plain sentence (and the "didn't finish" page); so do `timeout` and `cancel`.
    pub async fn wait(self, cancel: &Cancel, timeout: Duration) -> Result<Arrival, Error> {
        let (tx, mut rx) = mpsc::channel::<(Said, TcpStream)>(4);
        let deadline = tokio::time::sleep(timeout);
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => return Err(Error::SignInCancelled),
                _ = &mut deadline => return Err(Error::SignInTimedOut),
                Some((said, mut stream)) = rx.recv() => {
                    return match said {
                        Said::Code(code) => Ok(Arrival { code, stream }),
                        Said::Error(word) => {
                            respond(&mut stream, 200, "OK", FAILED_PAGE).await;
                            Err(match word.as_deref() {
                                Some("access_denied") => Error::ProviderDenied,
                                _ => Error::ProviderFailed,
                            })
                        }
                    };
                }
                accepted = self.listener.accept() => {
                    let Ok((stream, _)) = accepted else { continue };
                    tokio::spawn(serve(stream, tx.clone()));
                }
            }
        }
    }
}

/// Reads one request; answers it unless it is a callback that ends the flow, which goes to the
/// waiting sign-in with its connection.
async fn serve(mut stream: TcpStream, tx: mpsc::Sender<(Said, TcpStream)>) {
    let Ok(Some(head)) = tokio::time::timeout(REQUEST_TIMEOUT, read_head(&mut stream)).await else {
        return;
    };
    match parse(&head) {
        Parsed::NotFound => respond(&mut stream, 404, "Not Found", "Not found.").await,
        Parsed::Incomplete => respond(&mut stream, 400, "Bad Request", FAILED_PAGE).await,
        Parsed::Callback(said) => {
            // If the sign-in has already finished, the connection is simply dropped.
            let _ = tx.send((said, stream)).await;
        }
    }
}

async fn read_head(stream: &mut TcpStream) -> Option<String> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    loop {
        let n = stream.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            buf.truncate(end);
            return String::from_utf8(buf).ok();
        }
        if buf.len() > MAX_HEAD {
            return None;
        }
    }
}

enum Parsed {
    NotFound,
    /// `GET /callback` with neither a code nor an error.
    Incomplete,
    Callback(Said),
}

fn parse(head: &str) -> Parsed {
    let line = head.lines().next().unwrap_or_default();
    let mut parts = line.split(' ');
    let (Some("GET"), Some(target)) = (parts.next(), parts.next()) else {
        return Parsed::NotFound;
    };
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    if path != CALLBACK_PATH {
        return Parsed::NotFound;
    }
    let mut code = None;
    let mut error = None;
    for (key, value) in reqwest::Url::parse(&format!("http://127.0.0.1/?{query}"))
        .map(|u| u.query_pairs().into_owned().collect::<Vec<_>>())
        .unwrap_or_default()
    {
        match key.as_str() {
            "code" if !value.is_empty() => code = Some(value),
            "error" => error = Some(value),
            _ => {}
        }
    }
    if let Some(word) = error {
        let plain = !word.is_empty()
            && word.len() <= 64
            && word.bytes().all(|b| b.is_ascii_lowercase() || b == b'_');
        return Parsed::Callback(Said::Error(plain.then_some(word)));
    }
    match code {
        Some(code) => Parsed::Callback(Said::Code(Secret::new(code))),
        None => Parsed::Incomplete,
    }
}

async fn respond(stream: &mut TcpStream, status: u16, reason: &str, body: &str) {
    let kind = if body.starts_with('<') {
        "text/html; charset=utf-8"
    } else {
        "text/plain; charset=utf-8"
    };
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\n\
         Cache-Control: no-store\r\nReferrer-Policy: no-referrer\r\n\
         Content-Security-Policy: default-src 'none'; style-src 'unsafe-inline'\r\n\
         X-Content-Type-Options: nosniff\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes()).await;
    let _ = stream.write_all(body.as_bytes()).await;
    let _ = stream.shutdown().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sends a raw request and returns the whole answer.
    async fn request(port: u16, raw: &str) -> String {
        let mut s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        s.write_all(raw.as_bytes()).await.unwrap();
        let mut out = String::new();
        let _ = s.read_to_string(&mut out).await;
        out
    }

    fn get(path: &str) -> String {
        format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
    }

    const SECRET_CODE: &str = "6d1e2f9a-code-that-must-stay-secret";

    #[tokio::test]
    async fn a_code_ends_the_flow_and_the_page_says_so() {
        let cb = Callback::bind(0).await.unwrap();
        let port = cb.port();
        assert_eq!(
            cb.redirect_uri(),
            format!("http://127.0.0.1:{port}/callback")
        );
        let cancel = Cancel::new();
        let waiting = tokio::spawn(async move { cb.wait(&cancel, Duration::from_secs(5)).await });
        let browser = tokio::spawn(async move {
            request(port, &get(&format!("/callback?code={SECRET_CODE}"))).await
        });
        let arrival = waiting.await.unwrap().unwrap();
        assert_eq!(arrival.code.expose(), SECRET_CODE);
        assert!(!format!("{arrival:?}").contains(SECRET_CODE));
        arrival.reply(true).await;
        let page = browser.await.unwrap();
        assert!(page.starts_with("HTTP/1.1 200 OK\r\n"), "{page}");
        assert!(page.contains("You're signed in. You can close this tab."));
        assert!(!page.contains(SECRET_CODE));
        // The listener is gone.
        assert!(TcpStream::connect(("127.0.0.1", port)).await.is_err());
    }

    #[tokio::test]
    async fn a_failed_exchange_gets_the_other_page() {
        let cb = Callback::bind(0).await.unwrap();
        let port = cb.port();
        let browser = tokio::spawn(async move { request(port, &get("/callback?code=abc")).await });
        let arrival = cb
            .wait(&Cancel::new(), Duration::from_secs(5))
            .await
            .unwrap();
        arrival.reply(false).await;
        let page = browser.await.unwrap();
        assert!(page.contains("Sign-in didn't finish. Go back to TwinCursor and try again."));
    }

    #[tokio::test]
    async fn other_paths_and_methods_get_404_and_dont_end_the_flow() {
        let cb = Callback::bind(0).await.unwrap();
        let port = cb.port();
        let waiting =
            tokio::spawn(async move { cb.wait(&Cancel::new(), Duration::from_secs(5)).await });
        for raw in [
            get("/"),
            get("/favicon.ico"),
            get("/callbackx?code=1"),
            get("/callback/?code=1"),
            "POST /callback?code=1 HTTP/1.1\r\nContent-Length: 0\r\n\r\n".to_owned(),
            "HEAD /callback?code=1 HTTP/1.1\r\n\r\n".to_owned(),
            "garbage\r\n\r\n".to_owned(),
        ] {
            let answer = request(port, &raw).await;
            assert!(
                answer.starts_with("HTTP/1.1 404 Not Found\r\n"),
                "{raw}: {answer}"
            );
            assert!(!answer.contains("code=1"));
        }
        // A callback with neither a code nor an error is answered, and the flow goes on.
        let answer = request(port, &get("/callback?state=x")).await;
        assert!(
            answer.starts_with("HTTP/1.1 400 Bad Request\r\n"),
            "{answer}"
        );
        assert!(!waiting.is_finished());
        let browser = tokio::spawn(async move { request(port, &get("/callback?code=real")).await });
        let arrival = waiting.await.unwrap().unwrap();
        assert_eq!(arrival.code.expose(), "real");
        arrival.reply(true).await;
        browser.await.unwrap();
    }

    #[tokio::test]
    async fn a_provider_error_is_a_plain_sentence() {
        for (query, expected) in [
            (
                "error=access_denied&error_description=The+user+has+denied+your+application+access.",
                Error::ProviderDenied,
            ),
            (
                "error=server_error&error_description=%3Cscript%3Ealert(1)%3C%2Fscript%3E",
                Error::ProviderFailed,
            ),
            ("error=%3Cb%3Ebold%3C%2Fb%3E", Error::ProviderFailed),
            ("error=&code=abc", Error::ProviderFailed),
        ] {
            let cb = Callback::bind(0).await.unwrap();
            let port = cb.port();
            let browser =
                tokio::spawn(async move { request(port, &get(&format!("/callback?{query}"))).await });
            let e = cb.wait(&Cancel::new(), Duration::from_secs(5)).await.unwrap_err();
            assert_eq!(e, expected, "{query}");
            let text = format!("{e} {e:?}");
            assert!(!text.contains("script") && !text.contains("denied your"), "{text}");
            let page = browser.await.unwrap();
            assert!(page.contains("Sign-in didn't finish."), "{page}");
            assert!(!page.contains("script") && !page.contains("bold"), "{page}");
        }
    }

    #[tokio::test]
    async fn it_times_out() {
        let cb = Callback::bind(0).await.unwrap();
        let e = cb
            .wait(&Cancel::new(), Duration::from_millis(100))
            .await
            .unwrap_err();
        assert_eq!(e, Error::SignInTimedOut);
        assert_eq!(e.to_string(), "Sign-in took too long. Try again.");
    }

    #[tokio::test]
    async fn it_can_be_cancelled_and_a_silent_connection_doesnt_block_it() {
        let cb = Callback::bind(0).await.unwrap();
        let port = cb.port();
        let cancel = Cancel::new();
        let c = cancel.clone();
        let waiting = tokio::spawn(async move { cb.wait(&c, Duration::from_secs(30)).await });
        // A browser's speculative connection that never sends anything.
        let _idle = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        cancel.cancel();
        let e = tokio::time::timeout(Duration::from_secs(2), waiting)
            .await
            .expect("cancel ends the wait at once")
            .unwrap()
            .unwrap_err();
        assert_eq!(e, Error::SignInCancelled);
        // Cancelled before waiting starts: ends at once too.
        let cb = Callback::bind(0).await.unwrap();
        assert_eq!(
            cb.wait(&cancel, Duration::from_secs(30)).await.unwrap_err(),
            Error::SignInCancelled
        );
    }

    #[tokio::test]
    async fn a_taken_port_is_a_clear_error() {
        let taken = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = taken.local_addr().unwrap().port();
        let e = Callback::bind(port).await.unwrap_err();
        assert_eq!(e, Error::PortTaken(port));
        assert!(e.to_string().starts_with(&format!(
            "Sign-in needs port {port} on this computer, and another app is using it."
        )));
    }

    #[tokio::test]
    async fn it_listens_on_loopback_only() {
        let cb = Callback::bind(0).await.unwrap();
        let addr = cb.listener.local_addr().unwrap();
        assert!(addr.ip().is_loopback(), "{addr}");
    }
}
