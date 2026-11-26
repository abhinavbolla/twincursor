//! The account, as the window sees it: whether one is set up, who is signed in, and signing in
//! and out. The session lives in this process and in an owner-only file
//! (`<app config dir>/session.json`); **the webview never receives a token**. Everything it gets
//! is an [`AccountView`], built only from strings and booleans; the crate's tokens are
//! `account::Secret`s, which can't be serialized, so a token can't reach the window without
//! someone writing `.expose()` here (the tests check the view's exact shape).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use account::{Account, Backend, Cancel, Error, FileStore, Profile, SignInOptions};
use serde::Serialize;

/// The session file's name in the app's config directory.
pub const SESSION_FILE: &str = "session.json";

/// Who is signed in, as the window shows them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Me {
    pub id: String,
    pub name: String,
    pub handle: Option<String>,
    pub avatar_url: Option<String>,
}

impl From<Profile> for Me {
    fn from(p: Profile) -> Me {
        Me {
            id: p.id,
            name: p.name,
            handle: p.handle,
            avatar_url: p.avatar_url,
        }
    }
}

/// Everything the window learns about the account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AccountView {
    /// This build or run names an account backend. Without one, the window shows nothing of
    /// accounts.
    pub configured: bool,
    pub signed_in: bool,
    /// `null` until the profile has been read (or while the server can't be reached).
    pub me: Option<Me>,
    /// A sign-in is waiting for the browser.
    pub signing_in: bool,
}

/// The app's account state, managed by Tauri.
pub struct AccountShell {
    account: Option<Arc<Account>>,
    signing_in: Mutex<Option<Cancel>>,
    me: Mutex<Option<Me>>,
}

impl AccountShell {
    /// No backend set up: the window shows nothing of accounts.
    pub fn none() -> AccountShell {
        AccountShell {
            account: None,
            signing_in: Mutex::new(None),
            me: Mutex::new(None),
        }
    }

    pub fn new(backend: Backend, session_file: PathBuf) -> Result<AccountShell, String> {
        let account =
            Account::new(backend, FileStore::new(session_file)).map_err(|e| e.to_string())?;
        Ok(AccountShell {
            account: Some(Arc::new(account)),
            ..AccountShell::none()
        })
    }

    /// The account, for the call flows (`calls.rs`), which call the backend as this person.
    pub fn account(&self) -> Result<Arc<Account>, String> {
        self.account
            .clone()
            .ok_or_else(|| "This build isn't set up for accounts.".to_owned())
    }

    /// The profile, as the call flows' poll last read it.
    pub fn set_me(&self, me: Option<Me>) {
        *self.me.lock().unwrap() = me;
    }

    pub async fn view(&self) -> AccountView {
        let Some(account) = self.account.clone() else {
            return AccountView {
                configured: false,
                signed_in: false,
                me: None,
                signing_in: false,
            };
        };
        let signing_in = self.signing_in.lock().unwrap().is_some();
        let mut signed_in = account.is_signed_in().await;
        let mut me = self.me.lock().unwrap().clone();
        if signed_in && me.is_none() {
            match account.inbox(None).await {
                Ok(inbox) => {
                    let fetched = Me::from(inbox.me);
                    *self.me.lock().unwrap() = Some(fetched.clone());
                    me = Some(fetched);
                }
                Err(Error::NotSignedIn) => signed_in = false,
                // Can't reach the server: still signed in, the profile comes later.
                Err(_) => {}
            }
        }
        if !signed_in {
            me = None;
            *self.me.lock().unwrap() = None;
        }
        AccountView {
            configured: true,
            signed_in,
            me,
            signing_in,
        }
    }

    /// Runs the whole sign-in: the browser opens (through `open`), and this resolves once the
    /// person is signed in, or with a plain sentence.
    pub async fn sign_in(
        &self,
        open: impl FnOnce(&str) -> Result<(), String>,
    ) -> Result<AccountView, String> {
        let account = self.account()?;
        let cancel = Cancel::new();
        {
            let mut slot = self.signing_in.lock().unwrap();
            if slot.is_some() {
                return Err(Error::SignInInProgress.to_string());
            }
            *slot = Some(cancel.clone());
        }
        let result = account
            .sign_in(SignInOptions::default(), open, &cancel)
            .await;
        *self.signing_in.lock().unwrap() = None;
        *self.me.lock().unwrap() = None;
        result.map_err(|e| e.to_string())?;
        Ok(self.view().await)
    }

    /// Stops a sign-in that is waiting for the browser. Nothing waiting is fine.
    pub fn cancel_sign_in(&self) {
        if let Some(cancel) = self.signing_in.lock().unwrap().as_ref() {
            cancel.cancel();
        }
    }

    pub async fn sign_out(&self) -> Result<AccountView, String> {
        let account = self.account()?;
        *self.me.lock().unwrap() = None;
        account.sign_out().await.map_err(|e| e.to_string())?;
        Ok(self.view().await)
    }
}

/// Opens a page in the default browser: the sign-in page, or the self-hosting guide.
pub fn open_browser(url: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("/usr/bin/open")
            .arg(url)
            .status()
            .map_err(|e| e.to_string())
            .and_then(|s| {
                s.success()
                    .then_some(())
                    .ok_or_else(|| "open failed".to_owned())
            })
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = url;
        Err("Opening the browser is only built for macOS so far.".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use account::{Secret, Session, SessionStore};
    use serde_json::{json, Value};

    const ACCESS: &str = "access.token-payload.signature";
    const REFRESH: &str = "refresh-token-value";

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "twincursor-shell-account-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join(SESSION_FILE)
    }

    fn keys(v: &Value) -> Vec<String> {
        v.as_object().unwrap().keys().cloned().collect()
    }

    #[test]
    fn the_view_has_exactly_this_shape() {
        let view = AccountView {
            configured: true,
            signed_in: true,
            me: Some(Me {
                id: "u1".into(),
                name: "Alex".into(),
                handle: Some("alex".into()),
                avatar_url: None,
            }),
            signing_in: false,
        };
        let v = serde_json::to_value(&view).unwrap();
        assert_eq!(keys(&v), ["configured", "me", "signed_in", "signing_in"]);
        assert_eq!(keys(&v["me"]), ["avatar_url", "handle", "id", "name"]);
        assert_eq!(
            v["me"],
            json!({ "id": "u1", "name": "Alex", "handle": "alex", "avatar_url": null })
        );
    }

    #[tokio::test]
    async fn not_set_up_shows_nothing() {
        let shell = AccountShell::none();
        assert_eq!(
            serde_json::to_value(shell.view().await).unwrap(),
            json!({ "configured": false, "signed_in": false, "me": null, "signing_in": false })
        );
        assert!(shell.sign_in(|_| Ok(())).await.is_err());
        assert!(shell.sign_out().await.is_err());
    }

    /// A signed-in session on disk, a backend that can't be reached: the view says signed in,
    /// and nothing in it is a token.
    #[tokio::test]
    async fn a_saved_session_never_reaches_the_view() {
        let path = temp("view");
        FileStore::new(&path)
            .save(&Session {
                access_token: Secret::new(ACCESS),
                refresh_token: Secret::new(REFRESH),
                expires_at: u64::MAX / 2,
                user_id: "u1".into(),
            })
            .unwrap();
        let backend = Backend::new("http://127.0.0.1:1", "public-key").unwrap();
        let shell = AccountShell::new(backend, path.clone()).unwrap();
        let view = shell.view().await;
        assert!(view.configured && view.signed_in && view.me.is_none());
        let text = serde_json::to_string(&view).unwrap();
        assert!(!text.contains(ACCESS) && !text.contains(REFRESH), "{text}");

        // Sign-out forgets the session even with the server gone.
        let view = shell.sign_out().await.unwrap();
        assert!(!view.signed_in);
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[tokio::test]
    async fn cancel_ends_a_waiting_sign_in() {
        let path = temp("cancel");
        let backend = Backend::new("http://127.0.0.1:1", "public-key").unwrap();
        let shell = Arc::new(AccountShell::new(backend, path.clone()).unwrap());
        // Another process may hold the real callback port; then the error is about that.
        let waiting = {
            let shell = shell.clone();
            tokio::spawn(async move { shell.sign_in(|_| Ok(())).await })
        };
        for _ in 0..100 {
            if shell.view().await.signing_in || waiting.is_finished() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        shell.cancel_sign_in();
        let e = waiting.await.unwrap().unwrap_err();
        assert!(
            e == "Sign-in was cancelled." || e.starts_with("Sign-in needs port 53682"),
            "{e}"
        );
        assert!(!shell.view().await.signing_in);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
