//! Access tokens: for the local LiveKit dev server (`livekit-server --dev`), and for any server
//! whose key and secret the engine is given.

use livekit_api::access_token::{AccessToken, AccessTokenError, VideoGrants};

pub const DEV_URL: &str = "ws://localhost:7880";
pub const DEV_API_KEY: &str = "devkey";
pub const DEV_API_SECRET: &str = "secret";

/// Mints a token that lets `identity` join `room` with publish and subscribe rights.
pub fn dev_token(room: &str, identity: &str) -> Result<String, AccessTokenError> {
    AccessToken::with_api_key(DEV_API_KEY, DEV_API_SECRET)
        .with_identity(identity)
        .with_grants(VideoGrants {
            room_join: true,
            room: room.to_owned(),
            ..Default::default()
        })
        .to_jwt()
}

/// Mints a token that lets `identity` join `room` with publish and subscribe rights, showing
/// `name` to the others. The name is whatever the person typed: the server signs it, but nothing
/// checks it, so it never proves who someone is.
pub fn mint(
    api_key: &str,
    api_secret: &str,
    room: &str,
    identity: &str,
    name: &str,
) -> Result<String, AccessTokenError> {
    AccessToken::with_api_key(api_key, api_secret)
        .with_identity(identity)
        .with_name(name)
        .with_grants(VideoGrants {
            room_join: true,
            room: room.to_owned(),
            ..Default::default()
        })
        .to_jwt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mints_a_jwt() {
        let jwt = dev_token("room", "alice").unwrap();
        assert_eq!(jwt.split('.').count(), 3);
    }

    #[test]
    fn mints_a_named_token_for_any_server() {
        let jwt = mint("key", "a-secret-long-enough", "room", "guest-1", "Sam").unwrap();
        let claims = livekit_api::access_token::Claims::from_unverified(&jwt).unwrap();
        assert_eq!(claims.sub, "guest-1");
        assert_eq!(claims.name, "Sam");
        assert_eq!(claims.video.room, "room");
        assert!(claims.video.room_join);
    }
}
