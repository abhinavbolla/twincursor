//! A value that must never be printed: an access or refresh token, an authorization code, a PKCE
//! verifier, a call pass, a password.

/// Holds a credential. It has no `Display` and no `Serialize`, so it can't end up in a message,
/// a log line or a JSON answer by accident, and its `Debug` shows only that it is there. Read it
/// with [`Secret::expose`], only where it is sent on.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Secret {
        Secret(value.into())
    }

    /// The value itself, to send to the server it belongs to. Never print it.
    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(hidden)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_hides_the_value() {
        let s = Secret::new("eyJhbGciOi.payload.signature");
        assert_eq!(format!("{s:?}"), "Secret(hidden)");
        assert_eq!(format!("{s:#?}"), "Secret(hidden)");
        assert_eq!(s.expose(), "eyJhbGciOi.payload.signature");
    }
}
