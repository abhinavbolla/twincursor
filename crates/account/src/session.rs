//! The signed-in session and where it is kept.
//!
//! The session is a bearer credential: whoever has the refresh token can act as this person until
//! they sign out. [`FileStore`] keeps it in a file only this user can read (mode 0600, written
//! atomically). Once builds are signed, a Keychain store replaces it; an unsigned build would make
//! macOS ask for the Keychain on every update.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::{Error, Secret};

/// A Supabase Auth session.
#[derive(Clone, PartialEq, Eq)]
pub struct Session {
    pub access_token: Secret,
    pub refresh_token: Secret,
    /// When the access token expires, in seconds since the Unix epoch.
    pub expires_at: u64,
    /// The account's id (a UUID).
    pub user_id: String,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("user_id", &self.user_id)
            .field("expires_at", &self.expires_at)
            .field("access_token", &"(hidden)")
            .field("refresh_token", &"(hidden)")
            .finish()
    }
}

impl Session {
    /// The access token expires within `margin` seconds of `now` (or already has).
    pub fn expires_within(&self, now: u64, margin: u64) -> bool {
        self.expires_at <= now.saturating_add(margin)
    }
}

/// The file's layout. Private, so nothing else can serialize a session.
#[derive(Serialize, Deserialize)]
struct Stored {
    version: u32,
    user_id: String,
    expires_at: u64,
    access_token: String,
    refresh_token: String,
}

const STORED_VERSION: u32 = 1;

impl From<&Session> for Stored {
    fn from(s: &Session) -> Stored {
        Stored {
            version: STORED_VERSION,
            user_id: s.user_id.clone(),
            expires_at: s.expires_at,
            access_token: s.access_token.expose().to_owned(),
            refresh_token: s.refresh_token.expose().to_owned(),
        }
    }
}

impl Stored {
    fn into_session(self) -> Option<Session> {
        (self.version == STORED_VERSION
            && !self.access_token.is_empty()
            && !self.refresh_token.is_empty())
        .then(|| Session {
            access_token: Secret::new(self.access_token),
            refresh_token: Secret::new(self.refresh_token),
            expires_at: self.expires_at,
            user_id: self.user_id,
        })
    }
}

/// Keeps the session between runs.
pub trait SessionStore: Send + Sync {
    /// The saved session, or `None` when signed out. A file that can't be understood counts as
    /// signed out.
    fn load(&self) -> Result<Option<Session>, Error>;
    fn save(&self, session: &Session) -> Result<(), Error>;
    /// Forgets the session. Forgetting nothing is fine.
    fn clear(&self) -> Result<(), Error>;
}

/// The session as a JSON file, readable and writable by this user only, replaced atomically
/// (written beside it, flushed, then renamed over it).
#[derive(Debug, Clone)]
pub struct FileStore {
    path: PathBuf,
}

impl FileStore {
    pub fn new(path: impl Into<PathBuf>) -> FileStore {
        FileStore { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn tmp(&self) -> PathBuf {
        let mut name = self.path.file_name().unwrap_or_default().to_owned();
        name.push(".tmp");
        self.path.with_file_name(name)
    }
}

fn storage(path: &Path, e: std::io::Error) -> Error {
    Error::Storage(format!("{} ({e})", path.display()))
}

impl SessionStore for FileStore {
    fn load(&self) -> Result<Option<Session>, Error> {
        match std::fs::read(&self.path) {
            Ok(bytes) => Ok(serde_json::from_slice::<Stored>(&bytes)
                .ok()
                .and_then(Stored::into_session)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(storage(&self.path, e)),
        }
    }

    fn save(&self, session: &Session) -> Result<(), Error> {
        let fail = |e| storage(&self.path, e);
        if let Some(dir) = self.path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir).map_err(fail)?;
        }
        let text = serde_json::to_vec_pretty(&Stored::from(session)).expect("a session serializes");
        let tmp = self.tmp();
        let _ = std::fs::remove_file(&tmp);
        {
            use std::io::Write;
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&tmp).map_err(fail)?;
            file.write_all(&text).map_err(fail)?;
            file.sync_all().map_err(fail)?;
        }
        std::fs::rename(&tmp, &self.path).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            fail(e)
        })
    }

    fn clear(&self) -> Result<(), Error> {
        let _ = std::fs::remove_file(self.tmp());
        match std::fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(storage(&self.path, e)),
        }
    }
}

/// A session kept in memory only, for tests and for runs that shouldn't stay signed in.
#[derive(Debug, Default)]
pub struct MemoryStore {
    session: Mutex<Option<Session>>,
}

impl MemoryStore {
    pub fn new(session: Option<Session>) -> MemoryStore {
        MemoryStore {
            session: Mutex::new(session),
        }
    }
}

impl SessionStore for MemoryStore {
    fn load(&self) -> Result<Option<Session>, Error> {
        Ok(self.session.lock().unwrap().clone())
    }

    fn save(&self, session: &Session) -> Result<(), Error> {
        *self.session.lock().unwrap() = Some(session.clone());
        Ok(())
    }

    fn clear(&self) -> Result<(), Error> {
        *self.session.lock().unwrap() = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ACCESS: &str = "eyJhbGciOiJIUzI1NiJ9.access-payload.access-signature";
    const REFRESH: &str = "refresh-token-value";

    pub(crate) fn session() -> Session {
        Session {
            access_token: Secret::new(ACCESS),
            refresh_token: Secret::new(REFRESH),
            expires_at: 1_800_000_000,
            user_id: "3f2c8a8e-5b7d-4e8f-9a51-0d6c1e2b7f40".into(),
        }
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("twincursor-account-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn debug_never_shows_the_tokens() {
        let text = format!("{:?} {:#?}", session(), session());
        assert!(!text.contains(ACCESS) && !text.contains(REFRESH), "{text}");
        assert!(text.contains("3f2c8a8e"));
    }

    #[test]
    fn expiry_margin() {
        let s = session();
        assert!(!s.expires_within(1_800_000_000 - 61, 60));
        assert!(s.expires_within(1_800_000_000 - 60, 60));
        assert!(s.expires_within(1_800_000_001, 0));
    }

    #[test]
    fn a_file_round_trip() {
        let dir = temp_dir("round-trip");
        let store = FileStore::new(dir.join("nested").join("session.json"));
        assert_eq!(store.load().unwrap(), None);
        store.save(&session()).unwrap();
        assert_eq!(store.load().unwrap(), Some(session()));
        store.clear().unwrap();
        assert_eq!(store.load().unwrap(), None);
        store.clear().unwrap();
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[test]
    fn only_this_user_can_read_the_file_and_it_is_replaced_whole() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let dir = temp_dir("mode");
        let path = dir.join("session.json");
        let store = FileStore::new(&path);
        store.save(&session()).unwrap();
        let first = std::fs::metadata(&path).unwrap();
        assert_eq!(first.permissions().mode() & 0o777, 0o600);

        // A leftover temporary file from a crash doesn't get in the way, and a save puts a new
        // file in place (a new inode) rather than rewriting the old one in place.
        std::fs::write(store.tmp(), b"half-written").unwrap();
        let mut next = session();
        next.expires_at += 3600;
        store.save(&next).unwrap();
        let second = std::fs::metadata(&path).unwrap();
        assert_ne!(first.ino(), second.ino());
        assert_eq!(second.permissions().mode() & 0o777, 0o600);
        assert_eq!(store.load().unwrap(), Some(next));
        assert!(!store.tmp().exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_file_that_cant_be_understood_is_signed_out() {
        let dir = temp_dir("garbage");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("session.json");
        let store = FileStore::new(&path);
        for text in [
            "not json",
            "{}",
            r#"{"version":2,"user_id":"u","expires_at":1,"access_token":"a","refresh_token":"r"}"#,
            r#"{"version":1,"user_id":"u","expires_at":1,"access_token":"","refresh_token":"r"}"#,
        ] {
            std::fs::write(&path, text).unwrap();
            assert_eq!(store.load().unwrap(), None, "{text}");
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn storage_errors_name_the_file_not_the_tokens() {
        let dir = temp_dir("not-a-dir");
        std::fs::create_dir_all(&dir).unwrap();
        // A file where the directory should be.
        std::fs::write(dir.join("blocker"), b"").unwrap();
        let store = FileStore::new(dir.join("blocker").join("session.json"));
        let e = store.save(&session()).unwrap_err();
        let text = format!("{e} {e:?}");
        assert!(text.contains("blocker"), "{text}");
        assert!(!text.contains(ACCESS) && !text.contains(REFRESH));
        let _ = std::fs::remove_dir_all(dir);
    }
}
