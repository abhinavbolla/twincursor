//! A byte channel for remote control between the sharer and viewers.
//!
//! Media doesn't know what the bytes mean: `crates/protocol` and `crates/control` do. The link
//! runs on tasks of its own, with its own copy of the room's events, so input never waits behind
//! stats polls or republishing in the main loop (see `ping.rs` for the measurement).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use livekit::prelude::*;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::task::JoinHandle;

/// What the link reports to the app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkEvent {
    /// A participant is in the room: already there when we joined, or just arrived.
    Joined(String),
    /// This participant's screen is now being received (viewer side only).
    Sharing(String),
    Data {
        from: String,
        payload: Vec<u8>,
    },
    Left(String),
}

/// Bytes for the link to send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outgoing {
    /// One participant, or everyone if `None`.
    pub to: Option<String>,
    pub payload: Vec<u8>,
    /// Ordered with retransmits, or unordered without (ADR 0003).
    pub reliable: bool,
}

pub type OnEvent = Arc<dyn Fn(LinkEvent) + Send + Sync>;

/// The app's end for sending. It counts what hasn't gone out yet, so the app can wait for the
/// last messages before it leaves the room.
#[derive(Clone)]
pub struct LinkSender {
    tx: UnboundedSender<Outgoing>,
    pending: Arc<AtomicUsize>,
}

impl LinkSender {
    pub fn send(&self, out: Outgoing) {
        self.pending.fetch_add(1, Ordering::AcqRel);
        if self.tx.send(out).is_err() {
            self.pending.fetch_sub(1, Ordering::AcqRel);
        }
    }

    /// Messages handed over but not sent yet.
    pub fn pending(&self) -> usize {
        self.pending.load(Ordering::Acquire)
    }

    /// Waits until everything handed over has been sent, or `limit` passes. Returns whether it
    /// all went out.
    pub fn flush(&self, limit: Duration) -> bool {
        let start = Instant::now();
        while self.pending() > 0 {
            if start.elapsed() >= limit {
                return false;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        true
    }
}

/// The media side of a link, handed to `publish::run` or `subscribe::run`.
pub struct ControlLink {
    pub topic: String,
    pub on_event: OnEvent,
    pub outgoing: UnboundedReceiver<Outgoing>,
    pending: Arc<AtomicUsize>,
}

impl ControlLink {
    /// A link on `topic`, and the sender the app uses to send on it.
    pub fn new(
        topic: impl Into<String>,
        on_event: impl Fn(LinkEvent) + Send + Sync + 'static,
    ) -> (Self, LinkSender) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let pending = Arc::new(AtomicUsize::new(0));
        let link = Self {
            topic: topic.into(),
            on_event: Arc::new(on_event),
            outgoing: rx,
            pending: pending.clone(),
        };
        (link, LinkSender { tx, pending })
    }
}

/// Starts the link's two tasks: one receives, one sends, so a slow send never holds up input
/// arriving.
pub(crate) fn start(room: &Room, link: ControlLink) -> [JoinHandle<()>; 2] {
    let ControlLink {
        topic,
        on_event,
        outgoing,
        pending,
    } = link;
    let events = room.subscribe();
    // Participants already in the room produce no connect event.
    for identity in room.remote_participants().keys() {
        on_event(LinkEvent::Joined(identity.to_string()));
    }
    [
        tokio::spawn(receive(events, topic.clone(), on_event)),
        tokio::spawn(send(room.local_participant(), outgoing, topic, pending)),
    ]
}

async fn receive(mut events: UnboundedReceiver<RoomEvent>, topic: String, on_event: OnEvent) {
    while let Some(event) = events.recv().await {
        if let RoomEvent::DataReceived {
            topic: t,
            participant,
            ..
        } = &event
        {
            tracing::trace!(topic = ?t, from = ?participant.as_ref().map(|p| p.identity()), "data");
        }
        let report = match event {
            RoomEvent::DataReceived {
                payload,
                topic: Some(t),
                participant: Some(from),
                ..
            } if t == topic => LinkEvent::Data {
                from: from.identity().to_string(),
                payload: payload.to_vec(),
            },
            RoomEvent::ParticipantConnected(p) => LinkEvent::Joined(p.identity().to_string()),
            RoomEvent::ParticipantDisconnected(p) => LinkEvent::Left(p.identity().to_string()),
            RoomEvent::TrackSubscribed {
                track: RemoteTrack::Video(_),
                participant,
                ..
            } => LinkEvent::Sharing(participant.identity().to_string()),
            _ => continue,
        };
        on_event(report);
    }
}

async fn send(
    local: LocalParticipant,
    mut outgoing: UnboundedReceiver<Outgoing>,
    topic: String,
    pending: Arc<AtomicUsize>,
) {
    while let Some(out) = outgoing.recv().await {
        let packet = DataPacket {
            payload: out.payload,
            topic: Some(topic.clone()),
            reliable: out.reliable,
            destination_identities: out.to.map(|to| to.into()).into_iter().collect(),
        };
        if let Err(e) = local.publish_data(packet).await {
            tracing::warn!("control message not sent: {e}");
        }
        pending.fetch_sub(1, Ordering::AcqRel);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn out() -> Outgoing {
        Outgoing {
            to: None,
            payload: vec![1],
            reliable: true,
        }
    }

    #[test]
    fn counts_what_has_not_gone_out() {
        let (link, sender) = ControlLink::new("t", |_| {});
        sender.send(out());
        sender.send(out());
        assert_eq!(sender.pending(), 2);
        assert!(
            !sender.flush(Duration::from_millis(10)),
            "nothing sends them"
        );
        // The send task subtracts one per message it has sent.
        link.pending.fetch_sub(2, Ordering::AcqRel);
        assert!(sender.flush(Duration::from_millis(10)));
        // Once the link is gone, sends are dropped and not counted as pending forever.
        drop(link);
        sender.send(out());
        assert_eq!(sender.pending(), 0);
    }
}
