//! Call passes: LiveKit access tokens the account server signs for one person in one room.
//!
//! The engine reads who the pass is for (its `sub`, `name` and `video.room` claims) so it can
//! check the pass fits the call before connecting, and show the right name. It does **not**
//! verify the signature: it has no secret to verify it with, by design. That is safe because the
//! pass grants nothing here; the LiveKit server checks the signature when the engine connects, and
//! turns away a pass it didn't sign. A forged pass can only fail later.
//!
//! The pass itself is a bearer credential: nothing here keeps it, prints it or puts it in an
//! error. [`Pass`] holds only the claims read from it.

use base64::engine::general_purpose::{GeneralPurpose, GeneralPurposeConfig};
use base64::engine::DecodePaddingMode;
use base64::{alphabet, Engine as _};
use ipc::Role;
use serde_json::Value;

/// Shown for a pass whose account has no display name.
pub const PLACEHOLDER_NAME: &str = "Unnamed";

/// Base64url as JWTs use it: no padding, but padding is no reason to refuse.
const BASE64URL: GeneralPurpose = GeneralPurpose::new(
    &alphabet::URL_SAFE,
    GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent),
);

/// Who a call pass is for, as its claims say. Unverified: see the module docs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pass {
    /// `sharer-<id>` or `guest-<id>`.
    pub identity: String,
    pub role: Role,
    /// The account's display name, or [`PLACEHOLDER_NAME`].
    pub name: String,
    pub room: String,
}

impl Pass {
    /// Reads a pass's claims without checking its signature, and checks they make a usable pass
    /// at `now` (seconds since the Unix epoch). Errors never contain the pass.
    pub fn decode(jwt: &str, now: u64) -> anyhow::Result<Pass> {
        let parts: Vec<&str> = jwt.trim().split('.').collect();
        anyhow::ensure!(
            parts.len() == 3 && parts.iter().all(|p| !p.is_empty()),
            "This call pass isn't a valid token: it needs three parts separated by dots"
        );
        let payload = BASE64URL.decode(parts[1]).map_err(|_| {
            anyhow::anyhow!("This call pass isn't a valid token: its payload isn't base64url")
        })?;
        let claims: Value = serde_json::from_slice(&payload).map_err(|_| {
            anyhow::anyhow!("This call pass isn't a valid token: its payload isn't JSON")
        })?;
        let Value::Object(claims) = claims else {
            anyhow::bail!("This call pass isn't a valid token: its payload isn't a JSON object");
        };

        if let Some(exp) = claims.get("exp") {
            let exp = exp.as_f64().ok_or_else(|| {
                anyhow::anyhow!("This call pass isn't a valid token: its expiry isn't a number")
            })?;
            anyhow::ensure!(exp > now as f64, "This call pass has expired");
        }

        let identity = text(claims.get("sub"));
        anyhow::ensure!(
            !identity.is_empty(),
            "This call pass doesn't say who it is for"
        );
        let role = match identity.split_once('-') {
            Some(("sharer", id)) if !id.is_empty() => Role::Sharer,
            Some(("guest", id)) if !id.is_empty() => Role::Guest,
            _ => anyhow::bail!("This call pass is for neither a sharer nor a guest"),
        };

        let video = claims.get("video");
        let room = text(video.and_then(|v| v.get("room")));
        anyhow::ensure!(!room.is_empty(), "This call pass doesn't name a room");
        let joins = video
            .and_then(|v| v.get("roomJoin"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        anyhow::ensure!(joins, "This call pass doesn't let anyone join its room");

        let name = text(claims.get("name"));
        let name = if name.trim().is_empty() {
            PLACEHOLDER_NAME.to_owned()
        } else {
            check_name(&name)?;
            name
        };
        Ok(Pass {
            identity,
            role,
            name,
            room,
        })
    }

    /// Fails unless this pass is for `role`, the role of the subcommand it was given to.
    pub fn check_role(&self, role: Role) -> anyhow::Result<()> {
        match (self.role, role) {
            (Role::Guest, Role::Sharer) => {
                anyhow::bail!("This pass is for a guest; it can't be used to share")
            }
            (Role::Sharer, Role::Guest) => {
                anyhow::bail!("This pass is for the sharer; it can't be used to view")
            }
            _ => Ok(()),
        }
    }
}

/// The rules the control protocol applies to a guest's name (`protocol::Message::validate`), so
/// a name the server signed is one the sharer accepts.
pub fn check_name(name: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        name.chars().count() <= protocol::MAX_NAME_CHARS,
        "This call pass's name is longer than {} characters",
        protocol::MAX_NAME_CHARS
    );
    anyhow::ensure!(
        !name.chars().any(char::is_control),
        "This call pass's name has control characters in it"
    );
    Ok(())
}

fn text(value: Option<&Value>) -> String {
    value.and_then(Value::as_str).unwrap_or_default().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const NOW: u64 = 1_800_000_000;

    /// A token with these claims and a signature nobody checks here.
    fn jwt(claims: &Value) -> String {
        let header = BASE64URL.encode(br#"{"alg":"HS256","typ":"JWT"}"#);
        let payload = BASE64URL.encode(claims.to_string());
        format!("{header}.{payload}.c2lnbmF0dXJl")
    }

    fn claims(sub: &str, name: &str, room: &str) -> Value {
        json!({
            "sub": sub,
            "name": name,
            "exp": NOW + 600,
            "video": {"room": room, "roomJoin": true},
        })
    }

    const UUID: &str = "3f2c8a8e-5b7d-4e8f-9a51-0d6c1e2b7f40";

    fn err(jwt: &str) -> String {
        format!("{:#}", Pass::decode(jwt, NOW).unwrap_err())
    }

    #[test]
    fn a_sharer_pass() {
        let pass =
            Pass::decode(&jwt(&claims(&format!("sharer-{UUID}"), "Alex", "r1")), NOW).unwrap();
        assert_eq!(pass.identity, format!("sharer-{UUID}"));
        assert_eq!(pass.role, Role::Sharer);
        assert_eq!(pass.name, "Alex");
        assert_eq!(pass.room, "r1");
        pass.check_role(Role::Sharer).unwrap();
    }

    #[test]
    fn a_guest_pass() {
        let pass = Pass::decode(&jwt(&claims(&format!("guest-{UUID}"), "Sam", "r1")), NOW).unwrap();
        assert_eq!(pass.role, Role::Guest);
        assert_eq!(pass.name, "Sam");
        pass.check_role(Role::Guest).unwrap();
    }

    #[test]
    fn a_pass_made_by_livekit_reads_back() {
        let jwt = media::token::mint(
            "key",
            "a-secret-long-enough",
            "r1",
            &format!("guest-{UUID}"),
            "Sam",
        )
        .unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let pass = Pass::decode(&jwt, now).unwrap();
        assert_eq!((pass.role, pass.room.as_str()), (Role::Guest, "r1"));
    }

    #[test]
    fn the_role_must_match_the_subcommand() {
        let guest = Pass::decode(&jwt(&claims("guest-1", "Sam", "r1")), NOW).unwrap();
        let e = guest.check_role(Role::Sharer).unwrap_err().to_string();
        assert_eq!(e, "This pass is for a guest; it can't be used to share");
        let sharer = Pass::decode(&jwt(&claims("sharer-1", "Alex", "r1")), NOW).unwrap();
        let e = sharer.check_role(Role::Guest).unwrap_err().to_string();
        assert_eq!(e, "This pass is for the sharer; it can't be used to view");
    }

    #[test]
    fn a_pass_needs_three_parts() {
        let good = jwt(&claims("guest-1", "Sam", "r1"));
        let parts: Vec<&str> = good.split('.').collect();
        for bad in [
            String::new(),
            parts[1].to_owned(),
            format!("{}.{}", parts[0], parts[1]),
            format!("{}.{}.", parts[0], parts[1]),
            format!("{good}.more"),
        ] {
            assert!(err(&bad).contains("three parts"), "{bad}");
        }
    }

    #[test]
    fn a_payload_must_be_base64url_json_object() {
        assert!(err("aGVhZA.not*base64!.c2ln").contains("isn't base64url"));
        let not_json = format!("aGVhZA.{}.c2ln", BASE64URL.encode("not json"));
        assert!(err(&not_json).contains("isn't JSON"));
        for value in [json!([1, 2]), json!("sub"), json!(7), json!(null)] {
            assert!(err(&jwt(&value)).contains("isn't a JSON object"), "{value}");
        }
    }

    #[test]
    fn a_pass_names_someone() {
        let mut c = claims("", "Sam", "r1");
        assert_eq!(err(&jwt(&c)), "This call pass doesn't say who it is for");
        c.as_object_mut().unwrap().remove("sub");
        assert_eq!(err(&jwt(&c)), "This call pass doesn't say who it is for");
        c["sub"] = json!(42);
        assert_eq!(err(&jwt(&c)), "This call pass doesn't say who it is for");
        for sub in ["Sam", "sharer", "guest-", "viewer-1", "admin-1"] {
            assert_eq!(
                err(&jwt(&claims(sub, "Sam", "r1"))),
                "This call pass is for neither a sharer nor a guest",
                "{sub}"
            );
        }
    }

    #[test]
    fn a_pass_names_a_room_it_may_join() {
        assert_eq!(
            err(&jwt(&claims("guest-1", "Sam", ""))),
            "This call pass doesn't name a room"
        );
        let mut c = claims("guest-1", "Sam", "r1");
        c.as_object_mut().unwrap().remove("video");
        assert_eq!(err(&jwt(&c)), "This call pass doesn't name a room");
        let mut c = claims("guest-1", "Sam", "r1");
        c["video"]["roomJoin"] = json!(false);
        assert_eq!(
            err(&jwt(&c)),
            "This call pass doesn't let anyone join its room"
        );
        c["video"].as_object_mut().unwrap().remove("roomJoin");
        assert_eq!(
            err(&jwt(&c)),
            "This call pass doesn't let anyone join its room"
        );
    }

    #[test]
    fn an_expired_pass_is_refused() {
        let mut c = claims("guest-1", "Sam", "r1");
        c["exp"] = json!(NOW - 1);
        assert_eq!(err(&jwt(&c)), "This call pass has expired");
        c["exp"] = json!(NOW);
        assert_eq!(err(&jwt(&c)), "This call pass has expired");
        c["exp"] = json!("tomorrow");
        assert!(err(&jwt(&c)).contains("expiry isn't a number"));
        // No expiry is the server's choice to make.
        c.as_object_mut().unwrap().remove("exp");
        assert!(Pass::decode(&jwt(&c), NOW).is_ok());
    }

    #[test]
    fn names_follow_the_protocol_rules() {
        let longest = "n".repeat(protocol::MAX_NAME_CHARS);
        let pass = Pass::decode(&jwt(&claims("guest-1", &longest, "r1")), NOW).unwrap();
        assert_eq!(pass.name, longest);
        // Characters, not bytes.
        let accents = "é".repeat(protocol::MAX_NAME_CHARS);
        assert!(Pass::decode(&jwt(&claims("guest-1", &accents, "r1")), NOW).is_ok());
        let too_long = "n".repeat(protocol::MAX_NAME_CHARS + 1);
        assert!(err(&jwt(&claims("guest-1", &too_long, "r1"))).contains("longer than 64"));
        for bad in ["Sam\n", "Sa\u{7}m", "\u{1b}[31mSam"] {
            assert!(
                err(&jwt(&claims("guest-1", bad, "r1"))).contains("control characters"),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn a_missing_name_gets_a_placeholder() {
        for name in ["", "   "] {
            let pass = Pass::decode(&jwt(&claims("guest-1", name, "r1")), NOW).unwrap();
            assert_eq!(pass.name, PLACEHOLDER_NAME);
        }
        let mut c = claims("guest-1", "", "r1");
        c.as_object_mut().unwrap().remove("name");
        assert_eq!(Pass::decode(&jwt(&c), NOW).unwrap().name, PLACEHOLDER_NAME);
    }

    #[test]
    fn errors_never_contain_the_pass() {
        let mut bad = Vec::new();
        let mut expired = claims("guest-1", "Sam", "r1");
        expired["exp"] = json!(1);
        bad.push(jwt(&expired));
        bad.push(jwt(&claims("Sam", "Sam", "r1")));
        bad.push(jwt(&claims("guest-1", "Sam", "")));
        bad.push(jwt(&claims("guest-1", &"n".repeat(65), "r1")));
        bad.push(jwt(&json!([1])));
        bad.push("aGVhZA.not*base64!.c2ln".into());
        bad.push("only-one-part".into());
        for pass in &bad {
            let e = Pass::decode(pass, NOW).unwrap_err();
            for text in [format!("{e:#}"), format!("{e:?}")] {
                assert!(!text.contains(pass.as_str()), "{text}");
                for part in pass.split('.').filter(|p| p.len() > 4) {
                    assert!(!text.contains(part), "{text}");
                }
            }
        }
    }
}
