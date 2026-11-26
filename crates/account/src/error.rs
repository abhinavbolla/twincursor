//! What can go wrong, in sentences a person can read. No error carries a token, the authorization
//! code, the PKCE verifier, or text taken from a request or a provider's answer.

/// Everything this crate can fail with. `Display` is a plain sentence for the person using the
/// app; [`Error::kind`] is a stable word for scripts.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    BadBackend(&'static str),
    #[error("You're not signed in.")]
    NotSignedIn,
    #[error("Can't reach the server. Check your connection and try again.")]
    Unreachable,
    #[error("The server had a problem (HTTP {0}). Try again in a moment.")]
    Server(u16),
    #[error("The server turned the request down (HTTP {0}).")]
    Refused(u16),
    #[error("The server's answer couldn't be read.")]
    BadAnswer,

    #[error("That call has ended or the link is wrong.")]
    CallNotFound,
    #[error("That person isn't waiting to join any more.")]
    NotKnocking,
    #[error("That call isn't ringing you any more.")]
    NoPendingRing,
    #[error("You aren't in that call.")]
    NotInCall,
    #[error("That's your own call.")]
    OwnCall,
    #[error("You can't join that call: it has ended, or the host hasn't let you in.")]
    NotAllowed,
    #[error("You can only ring someone who has you as a contact too.")]
    NotAContact,
    #[error("The server couldn't read the request.")]
    BadRequest,

    #[error("Sign-in needs port {0} on this computer, and another app is using it. Close that app and try again.")]
    PortTaken(u16),
    #[error("Sign-in is already under way.")]
    SignInInProgress,
    #[error("The browser didn't open, so sign-in couldn't start.")]
    BrowserDidntOpen,
    #[error("Sign-in took too long. Try again.")]
    SignInTimedOut,
    #[error("Sign-in was cancelled.")]
    SignInCancelled,
    #[error("Sign-in was turned down on GitHub.")]
    ProviderDenied,
    #[error("GitHub sign-in didn't finish. Try again.")]
    ProviderFailed,
    #[error("Sign-in didn't finish: the server didn't accept it. Try again.")]
    CodeRejected,
    #[error("That email and password don't match an account.")]
    WrongPassword,
    #[error("The sign-in listener stopped: {0}.")]
    Listener(String),
    #[error("Your sign-in couldn't be saved or read: {0}.")]
    Storage(String),
}

impl Error {
    /// A stable word for this error, for scripts (`tc-account` prints it as `kind`).
    pub fn kind(&self) -> &'static str {
        match self {
            Error::BadBackend(_) => "bad_backend",
            Error::NotSignedIn => "not_signed_in",
            Error::Unreachable => "unreachable",
            Error::Server(_) => "server_error",
            Error::Refused(_) => "refused",
            Error::BadAnswer => "bad_answer",
            Error::CallNotFound => "call_not_found",
            Error::NotKnocking => "not_knocking",
            Error::NoPendingRing => "no_pending_ring",
            Error::NotInCall => "not_in_call",
            Error::OwnCall => "own_call",
            Error::NotAllowed => "not_allowed",
            Error::NotAContact => "not_a_contact",
            Error::BadRequest => "bad_argument",
            Error::PortTaken(_) => "port_taken",
            Error::SignInInProgress => "sign_in_in_progress",
            Error::BrowserDidntOpen => "browser_didnt_open",
            Error::SignInTimedOut => "sign_in_timed_out",
            Error::SignInCancelled => "sign_in_cancelled",
            Error::ProviderDenied => "provider_denied",
            Error::ProviderFailed => "provider_failed",
            Error::CodeRejected => "code_rejected",
            Error::WrongPassword => "wrong_password",
            Error::Listener(_) => "listener",
            Error::Storage(_) => "storage",
        }
    }

    /// The error a backend function raised, from the first word of its message (the stable part;
    /// see "Errors" in `supabase/README.md`). `None` for words this app doesn't know.
    pub(crate) fn from_backend_word(message: &str) -> Option<Error> {
        let word = message
            .split(|c: char| c == ':' || c.is_whitespace())
            .next()
            .unwrap_or_default();
        Some(match word {
            "call_not_found" => Error::CallNotFound,
            "not_knocking" => Error::NotKnocking,
            "no_pending_ring" => Error::NoPendingRing,
            "not_in_call" => Error::NotInCall,
            "own_call" => Error::OwnCall,
            "bad_argument" => Error::BadRequest,
            "not_allowed" => Error::NotAllowed,
            "not_a_contact" => Error::NotAContact,
            "not_signed_in" | "no_profile" => Error::NotSignedIn,
            _ => return None,
        })
    }

    /// An HTTP failure nothing more specific explains.
    pub(crate) fn from_status(status: u16) -> Error {
        match status {
            401 => Error::NotSignedIn,
            500.. => Error::Server(status),
            _ => Error::Refused(status),
        }
    }

    /// A request that never got an answer. Its text (which names the URL) is dropped.
    pub(crate) fn from_transport(e: &reqwest::Error) -> Error {
        if e.is_decode() {
            Error::BadAnswer
        } else {
            Error::Unreachable
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_words_map_to_sentences() {
        let cases = [
            (
                "call_not_found",
                "That call has ended or the link is wrong.",
            ),
            (
                "not_knocking: nobody is waiting with that id",
                "That person isn't waiting to join any more.",
            ),
            ("no_pending_ring", "That call isn't ringing you any more."),
            ("not_in_call", "You aren't in that call."),
            ("own_call: you host this call", "That's your own call."),
            (
                "bad_argument: let_in is required",
                "The server couldn't read the request.",
            ),
            (
                "not_allowed",
                "You can't join that call: it has ended, or the host hasn't let you in.",
            ),
            (
                "not_a_contact",
                "You can only ring someone who has you as a contact too.",
            ),
            ("not_signed_in", "You're not signed in."),
            ("no_profile", "You're not signed in."),
        ];
        for (message, sentence) in cases {
            assert_eq!(
                Error::from_backend_word(message).unwrap().to_string(),
                sentence,
                "{message}"
            );
        }
        assert_eq!(Error::from_backend_word("something_new"), None);
        assert_eq!(Error::from_backend_word(""), None);
    }

    #[test]
    fn statuses_without_a_word() {
        assert_eq!(Error::from_status(401), Error::NotSignedIn);
        assert_eq!(Error::from_status(503), Error::Server(503));
        assert_eq!(Error::from_status(418), Error::Refused(418));
        assert_eq!(
            Error::Server(502).to_string(),
            "The server had a problem (HTTP 502). Try again in a moment."
        );
    }

    #[test]
    fn every_sentence_ends_with_a_full_stop() {
        let all = [
            Error::BadBackend("x."),
            Error::NotSignedIn,
            Error::Unreachable,
            Error::Server(500),
            Error::Refused(400),
            Error::BadAnswer,
            Error::CallNotFound,
            Error::NotKnocking,
            Error::NoPendingRing,
            Error::NotInCall,
            Error::OwnCall,
            Error::NotAllowed,
            Error::NotAContact,
            Error::BadRequest,
            Error::PortTaken(53682),
            Error::SignInInProgress,
            Error::BrowserDidntOpen,
            Error::SignInTimedOut,
            Error::SignInCancelled,
            Error::ProviderDenied,
            Error::ProviderFailed,
            Error::CodeRejected,
            Error::WrongPassword,
            Error::Listener("it broke".into()),
            Error::Storage("disk full".into()),
        ];
        let mut kinds = std::collections::HashSet::new();
        for e in all {
            assert!(e.to_string().ends_with('.'), "{e}");
            assert!(kinds.insert(e.kind()), "{} twice", e.kind());
        }
    }
}
