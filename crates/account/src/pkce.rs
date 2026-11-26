//! Proof Key for Code Exchange (RFC 7636), S256: the app keeps a random verifier and sends only
//! its SHA-256 (the challenge) with the sign-in request. Whoever exchanges the code must present
//! the verifier, so a code caught on its way back to the app is useless to anyone else.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use sha2::{Digest, Sha256};

use crate::{Error, Secret};

/// Random bytes in a verifier: 48 make 64 base64url characters, within RFC 7636's 43 to 128.
const VERIFIER_BYTES: usize = 48;

/// One sign-in's verifier and challenge.
#[derive(Debug, Clone)]
pub struct Pkce {
    verifier: Secret,
    challenge: String,
}

impl Pkce {
    /// A fresh pair from the operating system's random source.
    pub fn new() -> Result<Pkce, Error> {
        let mut bytes = [0u8; VERIFIER_BYTES];
        getrandom::fill(&mut bytes)
            .map_err(|_| Error::Listener("no random numbers from the system".into()))?;
        Ok(Pkce::from_verifier(Secret::new(
            URL_SAFE_NO_PAD.encode(bytes),
        )))
    }

    /// The pair for a given verifier.
    pub fn from_verifier(verifier: Secret) -> Pkce {
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.expose().as_bytes()));
        Pkce {
            verifier,
            challenge,
        }
    }

    pub fn verifier(&self) -> &Secret {
        &self.verifier
    }

    /// `BASE64URL(SHA256(verifier))`, sent as `code_challenge`.
    pub fn challenge(&self) -> &str {
        &self.challenge
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rfc_7636_example() {
        // Appendix B of RFC 7636.
        let pkce = Pkce::from_verifier(Secret::new("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"));
        assert_eq!(
            pkce.challenge(),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn fresh_pairs_are_long_random_and_url_safe() {
        let a = Pkce::new().unwrap();
        let b = Pkce::new().unwrap();
        assert_ne!(a.verifier(), b.verifier());
        let v = a.verifier().expose();
        assert_eq!(v.len(), 64);
        assert!(v
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
        assert_eq!(a.challenge().len(), 43);
        assert_eq!(
            Pkce::from_verifier(a.verifier().clone()).challenge(),
            a.challenge()
        );
        assert!(!format!("{a:?}").contains(v));
    }
}
