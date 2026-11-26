//! The engine's side of the desktop app: who is who in a call, and the events the app is sent.
//!
//! The app and the engine speak `twincursor-ipc` over the engine's stdin and stdout. The parts
//! of the engine that see something the app shows (the control thread, the guest thread, the
//! viewer) report it through an [`Events`] sink; `tc-engine` writes each event as a line.
//!
//! **Roles travel in the identity.** A participant's LiveKit identity starts with their role:
//! `sharer-…` or `guest-…` (`tc-spike` uses the plain `sharer` and `viewer`). The token signs
//! the identity, and every data packet carries the sender's identity, so the role of whoever sent
//! a control message is known without a lookup. **Names travel in the token's name field.**
//!
//! Where both come from depends on how the engine joined. With a **call pass**, the account
//! server signed the token: the identity is `sharer-<account id>` or `guest-<account id>` (the id
//! is a UUID, dashes and all), and the name is that account's display name. Without one, the
//! engine mints its own token: the identity is [`new_identity`]'s and the name is whatever the
//! person typed, so it is shown, never trusted. Either way the name in a guest's control `Hello`
//! is the guest's own claim, not the token's, so the sharer shows the token's name instead
//! (`control::Controller::joined`).

use std::collections::BTreeMap;
use std::hash::{BuildHasher, Hasher};
use std::sync::{Arc, OnceLock};

use ipc::{Event, Participant, Role};

/// Where the engine's events go. Called from any thread.
pub type Events = Arc<dyn Fn(Event) + Send + Sync>;

/// Reads stdin line by line on a thread of its own, handing each line to `on_line`. When stdin
/// reaches end of file (the app that started us closed its end, or died), runs `on_close` and
/// exits the process, so a screen is never shared with no app showing it. Nothing else may read
/// stdin while this runs.
pub fn watch_stdin(
    mut on_line: impl FnMut(&str) + Send + 'static,
    on_close: impl FnOnce() + Send + 'static,
) {
    use std::io::BufRead;
    std::thread::Builder::new()
        .name("stdin-watch".into())
        .spawn(move || {
            let mut stdin = std::io::stdin().lock();
            let mut line = Vec::new();
            loop {
                line.clear();
                match stdin.read_until(b'\n', &mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => on_line(&String::from_utf8_lossy(&line)),
                }
            }
            on_close();
            // No log line: if the app died, stdout and stderr are closed pipes too, and a
            // failed write to stderr panics this thread instead of exiting.
            std::process::exit(0);
        })
        .expect("starting the stdin watcher");
}

/// The role an identity carries in its prefix.
pub fn role_of(identity: &str) -> Role {
    match identity.split('-').next().unwrap_or_default() {
        "sharer" => Role::Sharer,
        "guest" | "viewer" => Role::Guest,
        _ => Role::Unknown,
    }
}

/// A fresh identity for `role`, unique in practice: two people may type the same name.
pub fn new_identity(role: Role) -> String {
    let prefix = match role {
        Role::Sharer => "sharer",
        Role::Guest | Role::Unknown => "guest",
    };
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u32(std::process::id());
    hasher.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos()),
    );
    format!("{prefix}-{:016x}", hasher.finish())
}

static ANNOUNCED: OnceLock<Vec<protocol::Codec>> = OnceLock::new();

/// For tests: announce only these codecs (`h264`, `vp8`, ...), as a viewer that can't decode
/// the others would. Call before the call starts; later calls change nothing.
pub fn announce_only(names: &[String]) -> Result<(), String> {
    let codecs = names
        .iter()
        .map(
            |name| match serde_json::from_value(serde_json::Value::String(name.clone())) {
                Ok(protocol::Codec::Unknown) | Err(_) => Err(format!("unknown codec `{name}`")),
                Ok(codec) => Ok(codec),
            },
        )
        .collect::<Result<Vec<_>, _>>()?;
    let _ = ANNOUNCED.set(codecs);
    Ok(())
}

/// The video codecs this build can decode on this platform, as a viewer announces them.
/// libwebrtc decodes VP8, VP9 and AV1 in software everywhere; on macOS VideoToolbox decodes
/// H.264 and H.265 in hardware (docs/adr/0002, docs/adr/0008).
pub fn decoders() -> Vec<protocol::Codec> {
    use protocol::Codec;
    if let Some(only) = ANNOUNCED.get() {
        return only.clone();
    }
    if cfg!(target_os = "macos") {
        vec![Codec::H264, Codec::H265, Codec::Vp8, Codec::Vp9, Codec::Av1]
    } else {
        vec![Codec::H264, Codec::Vp8, Codec::Vp9, Codec::Av1]
    }
}

/// A codec as WebRTC's stats name it (`video/H264`), as the app's events name it (`h264`).
pub fn codec_name(stats: &str) -> Option<String> {
    let name = stats.rsplit('/').next().unwrap_or_default().to_lowercase();
    match name.as_str() {
        "" => None,
        "hevc" => Some("h265".into()),
        _ => Some(name),
    }
}

/// Everyone else in the call, kept from link events and hellos.
#[derive(Debug, Default)]
pub struct People {
    by_identity: BTreeMap<String, Participant>,
}

impl People {
    /// Someone joined. Returns the event to send.
    pub fn joined(&mut self, identity: &str, name: &str) -> Event {
        let entry = self
            .by_identity
            .entry(identity.to_owned())
            .or_insert_with(|| Participant {
                identity: identity.to_owned(),
                name: String::new(),
                role: role_of(identity),
                decoders: Vec::new(),
            });
        if !name.is_empty() {
            entry.name = name.to_owned();
        }
        if entry.name.is_empty() {
            entry.name = identity.to_owned();
        }
        self.event()
    }

    /// A guest said hello. Older guests send no codecs, which leaves the list as it was.
    pub fn hello(&mut self, identity: &str, decoders: &[protocol::Codec]) -> Option<Event> {
        let person = self.by_identity.get_mut(identity)?;
        let decoders: Vec<String> = decoders.iter().map(|c| c.name().to_owned()).collect();
        if decoders.is_empty() || person.decoders == decoders {
            return None;
        }
        person.decoders = decoders;
        Some(self.event())
    }

    /// Someone left. Returns the event to send, if they were known.
    pub fn left(&mut self, identity: &str) -> Option<Event> {
        self.by_identity.remove(identity)?;
        Some(self.event())
    }

    /// The name to show for `identity`, if known.
    pub fn name(&self, identity: &str) -> Option<&str> {
        self.by_identity.get(identity).map(|p| p.name.as_str())
    }

    fn event(&self) -> Event {
        Event::Participants {
            participants: self.by_identity.values().cloned().collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::Codec;

    #[test]
    fn roles_come_from_the_identity_prefix() {
        assert_eq!(role_of("sharer"), Role::Sharer);
        assert_eq!(role_of("viewer"), Role::Guest);
        assert_eq!(role_of("sharer-00ff"), Role::Sharer);
        assert_eq!(role_of("guest-00ff"), Role::Guest);
        assert_eq!(role_of("Sam"), Role::Unknown);
        assert_eq!(role_of(""), Role::Unknown);
        assert_eq!(role_of(&new_identity(Role::Sharer)), Role::Sharer);
        assert_eq!(role_of(&new_identity(Role::Guest)), Role::Guest);
        // A call pass's identity carries an account's UUID: only the first dash counts.
        let uuid = "3f2c8a8e-5b7d-4e8f-9a51-0d6c1e2b7f40";
        assert_eq!(role_of(&format!("sharer-{uuid}")), Role::Sharer);
        assert_eq!(role_of(&format!("guest-{uuid}")), Role::Guest);
        assert_eq!(role_of(&format!("{uuid}-sharer")), Role::Unknown);
    }

    #[test]
    fn codec_names() {
        assert_eq!(codec_name("video/H264").as_deref(), Some("h264"));
        assert_eq!(codec_name("video/H265").as_deref(), Some("h265"));
        assert_eq!(codec_name("video/HEVC").as_deref(), Some("h265"));
        assert_eq!(codec_name(""), None);
    }

    #[test]
    fn announcing_an_unknown_codec_is_refused() {
        // Errors set nothing, so the other tests still see this build's own decoders.
        let err = announce_only(&["h264".into(), "h266".into()]).unwrap_err();
        assert!(err.contains("h266"), "{err}");
        assert!(announce_only(&["unknown".into()]).is_err());
    }

    #[test]
    fn identities_differ() {
        assert_ne!(new_identity(Role::Guest), new_identity(Role::Guest));
    }

    #[test]
    fn people_join_say_hello_and_leave() {
        let mut people = People::default();
        let Event::Participants { participants } = people.joined("guest-1", "Sam") else {
            panic!()
        };
        assert_eq!(participants.len(), 1);
        assert_eq!(participants[0].name, "Sam");
        assert_eq!(participants[0].role, Role::Guest);
        assert!(participants[0].decoders.is_empty());

        let Some(Event::Participants { participants }) =
            people.hello("guest-1", &[Codec::H264, Codec::H265])
        else {
            panic!()
        };
        assert_eq!(participants[0].decoders, ["h264", "h265"]);
        // Nothing new, nothing sent; a hello from someone unknown is ignored.
        assert!(people
            .hello("guest-1", &[Codec::H264, Codec::H265])
            .is_none());
        assert!(people.hello("guest-2", &[Codec::H264]).is_none());

        // Identities from call passes, with dashes in them, are kept apart like any other.
        let uuid = "3f2c8a8e-5b7d-4e8f-9a51-0d6c1e2b7f40";
        people.joined(&format!("guest-{uuid}"), "Kim");
        assert_eq!(people.name(&format!("guest-{uuid}")), Some("Kim"));
        assert!(people.left(&format!("guest-{uuid}")).is_some());

        // A participant without a name in their token shows as their identity.
        people.joined("viewer", "");
        assert_eq!(people.name("viewer"), Some("viewer"));

        let Some(Event::Participants { participants }) = people.left("guest-1") else {
            panic!()
        };
        assert_eq!(participants.len(), 1);
        assert!(people.left("guest-1").is_none());
    }
}
