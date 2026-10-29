//! Access tokens for the local LiveKit dev server (`livekit-server --dev`).

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mints_a_jwt() {
        let jwt = dev_token("room", "alice").unwrap();
        assert_eq!(jwt.split('.').count(), 3);
    }
}
