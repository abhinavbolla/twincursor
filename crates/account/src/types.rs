//! What the backend's functions return (`supabase/README.md`, "Functions").
//!
//! Unknown fields are ignored and unknown member states read as [`MemberState::Unknown`], so a
//! newer backend doesn't break an older app.

use serde::{Deserialize, Serialize};

use crate::Secret;

/// A person, as the backend shows them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    pub id: String,
    pub name: String,
    /// Their GitHub login.
    #[serde(default)]
    pub handle: Option<String>,
    #[serde(default)]
    pub avatar_url: Option<String>,
}

/// Where someone stands in a call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemberState {
    Ringing,
    Knocking,
    Admitted,
    TurnedAway,
    Declined,
    Left,
    /// A ring nobody answered within 60 seconds.
    Missed,
    /// A state this app doesn't know yet.
    #[serde(other)]
    Unknown,
}

/// Someone in the call you host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Member {
    pub user_id: String,
    pub name: String,
    #[serde(default)]
    pub handle: Option<String>,
    #[serde(default)]
    pub avatar_url: Option<String>,
    pub state: MemberState,
}

/// The call you host, while it is live.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hosting {
    pub call_id: String,
    pub code: String,
    #[serde(default)]
    pub members: Vec<Member>,
}

/// A call ringing you.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ring {
    pub call_id: String,
    pub host: Profile,
}

/// A call you knocked on, or a knock or ring answered in the last minute.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Waiting {
    pub call_id: String,
    pub state: MemberState,
    pub host: Profile,
}

/// `inbox()`: everything the app shows, in one answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Inbox {
    pub me: Profile,
    #[serde(default)]
    pub contacts: Vec<Profile>,
    #[serde(default)]
    pub hosting: Option<Hosting>,
    #[serde(default)]
    pub rings: Vec<Ring>,
    #[serde(default)]
    pub waiting: Vec<Waiting>,
}

/// A call just started (`start_call`) or rung (`ring`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartedCall {
    pub call_id: String,
    /// What a call link carries.
    pub code: String,
}

/// Where a knock stands, and whose call it is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Knock {
    pub call_id: String,
    pub state: MemberState,
    pub host: Profile,
}

/// `knock()`'s row, flat as the database returns it.
#[derive(Deserialize)]
pub(crate) struct KnockRow {
    call_id: String,
    state: MemberState,
    host_id: String,
    host_name: String,
    #[serde(default)]
    host_handle: Option<String>,
    #[serde(default)]
    host_avatar_url: Option<String>,
}

impl From<KnockRow> for Knock {
    fn from(r: KnockRow) -> Knock {
        Knock {
            call_id: r.call_id,
            state: r.state,
            host: Profile {
                id: r.host_id,
                name: r.host_name,
                handle: r.host_handle,
                avatar_url: r.host_avatar_url,
            },
        }
    }
}

/// Which side of the call a pass is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PassRole {
    Sharer,
    Guest,
    #[serde(other)]
    Unknown,
}

/// A LiveKit token for one person in one call, from the `call-pass` function. The token is a
/// bearer credential: it is a [`Secret`], and this type is not `Serialize`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallPass {
    /// The LiveKit server, for `TWINCURSOR_SERVER_URL`.
    pub url: String,
    /// For `TWINCURSOR_PASS`.
    pub token: Secret,
    pub room: String,
    pub identity: String,
    pub name: String,
    pub role: PassRole,
}

#[derive(Deserialize)]
pub(crate) struct CallPassBody {
    url: String,
    token: String,
    room: String,
    identity: String,
    name: String,
    role: PassRole,
}

impl From<CallPassBody> for CallPass {
    fn from(b: CallPassBody) -> CallPass {
        CallPass {
            url: b.url,
            token: Secret::new(b.token),
            room: b.room,
            identity: b.identity,
            name: b.name,
            role: b.role,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_readme_inbox_reads() {
        let inbox: Inbox = serde_json::from_value(json!({
            "me": { "id": "u1", "name": "Alex", "handle": "alex", "avatar_url": "https://avatars.githubusercontent.com/u/1" },
            "contacts": [{ "id": "u2", "name": "Sam", "handle": null, "avatar_url": null }],
            "hosting": { "call_id": "c1", "code": "k1", "members": [
                { "user_id": "u2", "name": "Sam", "handle": null, "avatar_url": null, "state": "knocking" },
                { "user_id": "u3", "name": "Kim", "handle": "kim", "avatar_url": null, "state": "missed" },
                { "user_id": "u4", "name": "Lee", "handle": "lee", "avatar_url": null, "state": "beamed_in" }
            ] },
            "rings": [{ "call_id": "c2", "host": { "id": "u5", "name": "Pat", "handle": "pat", "avatar_url": null } }],
            "waiting": [{ "call_id": "c3", "state": "turned_away", "host": { "id": "u6", "name": "Jo", "handle": null, "avatar_url": null } }],
            "something_new": 1
        }))
        .unwrap();
        assert_eq!(inbox.me.handle.as_deref(), Some("alex"));
        assert_eq!(inbox.contacts[0].handle, None);
        let members = &inbox.hosting.as_ref().unwrap().members;
        assert_eq!(
            members.iter().map(|m| m.state).collect::<Vec<_>>(),
            [
                MemberState::Knocking,
                MemberState::Missed,
                MemberState::Unknown
            ]
        );
        assert_eq!(inbox.rings[0].host.name, "Pat");
        assert_eq!(inbox.waiting[0].state, MemberState::TurnedAway);
    }

    #[test]
    fn every_state_reads() {
        for (word, state) in [
            ("ringing", MemberState::Ringing),
            ("knocking", MemberState::Knocking),
            ("admitted", MemberState::Admitted),
            ("turned_away", MemberState::TurnedAway),
            ("declined", MemberState::Declined),
            ("left", MemberState::Left),
            ("missed", MemberState::Missed),
            ("later", MemberState::Unknown),
        ] {
            assert_eq!(
                serde_json::from_value::<MemberState>(json!(word)).unwrap(),
                state
            );
        }
    }

    #[test]
    fn a_pass_hides_its_token() {
        let pass: CallPass = serde_json::from_value::<CallPassBody>(json!({
            "url": "ws://127.0.0.1:7880", "token": "eyJ.pass-payload.sig", "room": "r",
            "identity": "guest-u2", "name": "Sam", "role": "guest"
        }))
        .unwrap()
        .into();
        assert_eq!(pass.role, PassRole::Guest);
        assert!(!format!("{pass:?}").contains("pass-payload"));
    }
}
