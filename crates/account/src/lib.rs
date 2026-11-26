//! TwinCursor accounts: signing in with GitHub through Supabase Auth, staying signed in, and the
//! backend's calls (`supabase/README.md`).
//!
//! - [`Backend`]: where the backend is (its URL and public key).
//! - [`Account`]: the signed-in person. Signs in ([`Account::sign_in`], the PKCE flow through the
//!   system browser and a one-time listener on `127.0.0.1`), refreshes the session before it
//!   expires, signs out, and calls the backend ([`Account::inbox`], [`Account::start_call`], …).
//! - [`SessionStore`]: where the session is kept. [`FileStore`] writes an owner-only file; it holds
//!   a bearer credential (see `docs/accounts.md`).
//!
//! Tokens, the authorization code and the PKCE verifier are [`Secret`]s: they have no `Display`
//! and no `Serialize`, their `Debug` hides them, and no error carries one.

mod callback;
mod client;
mod config;
mod error;
mod pkce;
mod secret;
mod session;
mod types;

#[cfg(test)]
mod fake;

pub use callback::{Arrival, Callback, Cancel, CALLBACK_PATH, CALLBACK_PORT, SIGN_IN_TIMEOUT};
pub use client::{Account, SignInOptions, REFRESH_MARGIN_SECS};
pub use config::Backend;
pub use error::Error;
pub use pkce::Pkce;
pub use secret::Secret;
pub use session::{FileStore, MemoryStore, Session, SessionStore};
pub use types::{
    CallPass, Hosting, Inbox, Knock, Member, MemberState, PassRole, Profile, Ring, StartedCall,
    Waiting,
};
