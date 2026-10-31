//! Measures the data-packet path remote-control input takes: viewer → SFU → sharer.
//!
//! The viewer sends pings and the sharer echoes them at once, so the round trip needs only the
//! viewer's clock. Where both ends share a clock (one machine, the network lab) it also gives
//! the one-way delay, which is what input feels. There are four streams: both data channels (reliable and lossy), each
//! answered from the sharer's main loop or from a task of its own. The main loop also awaits
//! stats polls and republishing, so comparing the routes shows what that costs.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use livekit::prelude::*;
use serde::Serialize;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;

use crate::capture::wall_clock_us;
use crate::latency::Recorder;
use crate::stats::{summarize, Summary};
use crate::wire::{self, Ping};

/// Each stream pings this often.
pub const PING_EVERY: Duration = Duration::from_millis(100);
/// How long a sent ping is kept waiting for its echo before it counts as lost.
const PING_TTL_SECS: f64 = 5.0;

/// Which of the sharer's event consumers answers a ping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Route {
    /// The loop in `publish::run`, shared with stats polls and republishing.
    Main,
    /// A task of its own.
    Task,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Stream {
    pub reliable: bool,
    pub route: Route,
}

pub const STREAMS: [Stream; 4] = [
    Stream {
        reliable: true,
        route: Route::Task,
    },
    Stream {
        reliable: false,
        route: Route::Task,
    },
    Stream {
        reliable: true,
        route: Route::Main,
    },
    Stream {
        reliable: false,
        route: Route::Main,
    },
];

impl std::fmt::Display for Stream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let channel = if self.reliable { "reliable" } else { "lossy" };
        let route = match self.route {
            Route::Main => "main",
            Route::Task => "task",
        };
        write!(f, "{channel}/{route}")
    }
}

/// Viewer-side bookkeeping for all streams.
#[derive(Debug, Default)]
pub struct PingLog {
    streams: HashMap<Stream, StreamLog>,
}

#[derive(Debug, Default)]
struct StreamLog {
    pending: HashMap<u32, Instant>,
    sent: u64,
    last_id: u32,
    out_of_order: u64,
    rtt: Vec<f64>,
    one_way: Vec<f64>,
    send_await: Vec<f64>,
    echo_await: Vec<f64>,
}

/// One stream's results for a run. Times are in ms.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StreamReport {
    pub stream: String,
    pub sent: u64,
    pub received: usize,
    pub out_of_order: u64,
    /// Ping sent → echo received.
    pub rtt: Option<Summary>,
    /// Ping sent → reached the sharer. Only meaningful when both ends share a clock.
    pub one_way: Option<Summary>,
    /// Time the viewer spent in `publish_data`.
    pub send_await: Option<Summary>,
    /// Time the sharer spent in `publish_data` for its echoes.
    pub echo_await: Option<Summary>,
}

impl PingLog {
    pub fn sent(&mut self, stream: Stream, id: u32, at: Instant) {
        let log = self.streams.entry(stream).or_default();
        log.pending
            .retain(|_, t| at.duration_since(*t).as_secs_f64() < PING_TTL_SECS);
        log.pending.insert(id, at);
        log.sent += 1;
    }

    pub fn send_returned(&mut self, stream: Stream, awaited: Duration) {
        let log = self.streams.entry(stream).or_default();
        log.send_await.push(awaited.as_secs_f64() * 1000.0);
    }

    pub fn echoed(&mut self, ping: Ping, at: Instant) {
        let log = self.streams.entry(ping.stream).or_default();
        let Some(sent) = log.pending.remove(&ping.id) else {
            return;
        };
        if ping.id < log.last_id {
            log.out_of_order += 1;
        }
        log.last_id = log.last_id.max(ping.id);
        log.rtt
            .push(at.saturating_duration_since(sent).as_secs_f64() * 1000.0);
        if ping.arrived_at_us >= ping.sent_at_us {
            log.one_way
                .push((ping.arrived_at_us - ping.sent_at_us) as f64 / 1000.0);
        }
        // The sharer reports its previous echo: it can't know this one's time yet.
        if ping.echo_await_us > 0 {
            log.echo_await.push(f64::from(ping.echo_await_us) / 1000.0);
        }
    }

    /// Per-stream results, in `STREAMS` order. Empty when nothing was measured.
    pub fn report(&self) -> Vec<StreamReport> {
        STREAMS
            .iter()
            .filter_map(|stream| {
                let log = self.streams.get(stream)?;
                Some(StreamReport {
                    stream: stream.to_string(),
                    sent: log.sent,
                    received: log.rtt.len(),
                    out_of_order: log.out_of_order,
                    rtt: summarize(&log.rtt),
                    one_way: summarize(&log.one_way),
                    send_await: summarize(&log.send_await),
                    echo_await: summarize(&log.echo_await),
                })
            })
            .collect()
    }
}

/// Viewer: sends all streams and records the echoes until the tasks are aborted.
pub fn start_probe(
    participant: LocalParticipant,
    events: UnboundedReceiver<RoomEvent>,
    recorder: Arc<Mutex<Recorder>>,
) -> Vec<JoinHandle<()>> {
    let mut tasks = vec![tokio::spawn(receive_echoes(events, recorder.clone()))];
    for (i, stream) in STREAMS.into_iter().enumerate() {
        // Spread evenly, so streams don't queue behind each other.
        let offset = PING_EVERY * i as u32 / STREAMS.len() as u32;
        tasks.push(tokio::spawn(send_pings(
            participant.clone(),
            stream,
            recorder.clone(),
            offset,
        )));
    }
    tasks
}

async fn send_pings(
    participant: LocalParticipant,
    stream: Stream,
    recorder: Arc<Mutex<Recorder>>,
    offset: Duration,
) {
    tokio::time::sleep(offset).await;
    let mut interval = tokio::time::interval(PING_EVERY);
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut id: u32 = 0;
    loop {
        interval.tick().await;
        id += 1;
        let packet = DataPacket {
            payload: wire::encode_ping(Ping {
                id,
                stream,
                echo_await_us: 0,
                sent_at_us: wall_clock_us(),
                arrived_at_us: 0,
            }),
            topic: Some(wire::PING_TOPIC.into()),
            reliable: stream.reliable,
            ..Default::default()
        };
        let sent_at = Instant::now();
        // Recorded first: the echo can beat `publish_data` returning.
        recorder.lock().unwrap().input.sent(stream, id, sent_at);
        if participant.publish_data(packet).await.is_ok() {
            let awaited = sent_at.elapsed();
            recorder
                .lock()
                .unwrap()
                .input
                .send_returned(stream, awaited);
        }
    }
}

async fn receive_echoes(mut events: UnboundedReceiver<RoomEvent>, recorder: Arc<Mutex<Recorder>>) {
    while let Some(event) = events.recv().await {
        let at = Instant::now();
        if let RoomEvent::DataReceived { payload, topic, .. } = event {
            if topic.as_deref() == Some(wire::PONG_TOPIC) {
                if let Some(ping) = wire::decode_ping(&payload) {
                    recorder.lock().unwrap().input.echoed(ping, at);
                }
            }
        }
    }
}

/// Sharer: answers pings meant for `route`, on the channel they came in on.
pub async fn answer(
    local: &LocalParticipant,
    route: Route,
    payload: &[u8],
    from: Option<&RemoteParticipant>,
    last_await_us: &AtomicU32,
) {
    let Some(mut ping) = wire::decode_ping(payload).filter(|p| p.stream.route == route) else {
        return;
    };
    ping.arrived_at_us = wall_clock_us();
    ping.echo_await_us = last_await_us.load(Ordering::Relaxed);
    let packet = DataPacket {
        payload: wire::encode_ping(ping),
        topic: Some(wire::PONG_TOPIC.into()),
        reliable: ping.stream.reliable,
        destination_identities: from.map(|p| p.identity()).into_iter().collect(),
    };
    let started = Instant::now();
    if let Err(e) = local.publish_data(packet).await {
        tracing::debug!("echo not sent: {e}");
    }
    let awaited = started.elapsed().as_micros().min(u128::from(u32::MAX)) as u32;
    last_await_us.store(awaited.max(1), Ordering::Relaxed);
}

/// Sharer: answers the `Route::Task` streams from their own event subscription.
pub async fn serve(local: LocalParticipant, mut events: UnboundedReceiver<RoomEvent>) {
    let last_await_us = AtomicU32::new(0);
    while let Some(event) = events.recv().await {
        if let RoomEvent::DataReceived {
            payload,
            topic,
            participant,
            ..
        } = event
        {
            if topic.as_deref() == Some(wire::PING_TOPIC) {
                answer(
                    &local,
                    Route::Task,
                    &payload,
                    participant.as_ref(),
                    &last_await_us,
                )
                .await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RELIABLE: Stream = STREAMS[0];
    const LOSSY: Stream = STREAMS[1];

    fn echo(id: u32, stream: Stream, echo_await_us: u32) -> Ping {
        Ping {
            id,
            stream,
            echo_await_us,
            sent_at_us: 1_000_000,
            arrived_at_us: 1_004_000,
        }
    }

    #[test]
    fn measures_each_stream_separately() {
        let t0 = Instant::now();
        let mut log = PingLog::default();
        log.sent(RELIABLE, 1, t0);
        log.sent(LOSSY, 1, t0);
        log.send_returned(RELIABLE, Duration::from_micros(500));
        log.echoed(echo(1, RELIABLE, 0), t0 + Duration::from_millis(20));
        log.echoed(echo(1, LOSSY, 2000), t0 + Duration::from_millis(10));

        let report = log.report();
        assert_eq!(report[0].stream, "reliable/task");
        assert_eq!(report[0].rtt.unwrap().p50, 20.0);
        assert_eq!(report[0].send_await.unwrap().p50, 0.5);
        assert_eq!(report[0].echo_await, None);
        assert_eq!(report[1].stream, "lossy/task");
        assert_eq!(report[1].rtt.unwrap().p50, 10.0);
        assert_eq!(report[1].echo_await.unwrap().p50, 2.0);
        assert_eq!(report[1].one_way.unwrap().p50, 4.0);
    }

    #[test]
    fn skips_one_way_when_clocks_disagree() {
        let t0 = Instant::now();
        let mut log = PingLog::default();
        log.sent(LOSSY, 1, t0);
        let mut skewed = echo(1, LOSSY, 0);
        skewed.arrived_at_us = skewed.sent_at_us - 1;
        log.echoed(skewed, t0 + Duration::from_millis(5));
        let report = log.report();
        assert_eq!(report[0].one_way, None);
        assert_eq!(report[0].received, 1);
    }

    #[test]
    fn unanswered_pings_count_as_sent_only() {
        let t0 = Instant::now();
        let mut log = PingLog::default();
        log.sent(LOSSY, 1, t0);
        log.sent(LOSSY, 2, t0);
        log.echoed(echo(2, LOSSY, 0), t0 + Duration::from_millis(5));
        let report = log.report();
        assert_eq!((report[0].sent, report[0].received), (2, 1));
    }

    #[test]
    fn counts_out_of_order_and_ignores_duplicates() {
        let t0 = Instant::now();
        let mut log = PingLog::default();
        for id in 1..=3 {
            log.sent(LOSSY, id, t0);
        }
        let later = t0 + Duration::from_millis(5);
        log.echoed(echo(1, LOSSY, 0), later);
        log.echoed(echo(3, LOSSY, 0), later);
        log.echoed(echo(2, LOSSY, 0), later);
        log.echoed(echo(2, LOSSY, 0), later);
        let report = log.report();
        assert_eq!((report[0].received, report[0].out_of_order), (3, 1));
    }

    #[test]
    fn forgets_old_pings() {
        let t0 = Instant::now();
        let mut log = PingLog::default();
        log.sent(LOSSY, 1, t0);
        log.sent(LOSSY, 2, t0 + Duration::from_secs(10));
        log.echoed(echo(1, LOSSY, 0), t0 + Duration::from_secs(11));
        assert_eq!(log.report()[0].received, 0);
    }

    #[test]
    fn nothing_measured_reports_nothing() {
        assert!(PingLog::default().report().is_empty());
    }
}
