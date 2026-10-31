//! Data messages exchanged between viewer and sharer during a call.

use crate::netstats::NetSnapshot;
use crate::ping::{Route, Stream};

/// Data-packet topic for latency ticks (viewer → sharer).
pub const TICK_TOPIC: &str = "tc.tick";
/// Data-packet topic for network stats, once a second, both ways: the viewer shows the
/// sharer's, and the sharer's auto-throttle uses the viewer's.
pub const STATS_TOPIC: &str = "tc.stats";
/// Data-packet topics for input-path measurements (viewer → sharer, and the echo).
pub const PING_TOPIC: &str = "tc.ping";
pub const PONG_TOPIC: &str = "tc.pong";

/// Pings are padded to about the size of a future input event.
const PING_LEN: usize = 64;

/// A measurement packet. The echo uses the same format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ping {
    pub id: u32,
    pub stream: Stream,
    /// On echoes: how long the sharer's previous echo took to send, in µs (0 = unknown).
    pub echo_await_us: u32,
    /// Viewer's wall clock when it sent the ping, in µs.
    pub sent_at_us: u64,
    /// On echoes: the sharer's wall clock when the ping reached it, in µs.
    pub arrived_at_us: u64,
}

pub fn encode_tick(id: u32) -> Vec<u8> {
    id.to_le_bytes().to_vec()
}

pub fn decode_tick(payload: &[u8]) -> Option<u32> {
    Some(u32::from_le_bytes(payload.try_into().ok()?))
}

pub fn encode_ping(ping: Ping) -> Vec<u8> {
    let mut out = vec![0; PING_LEN];
    out[..4].copy_from_slice(&ping.id.to_le_bytes());
    out[4] = u8::from(ping.stream.reliable);
    out[5] = u8::from(ping.stream.route == Route::Task);
    out[6..10].copy_from_slice(&ping.echo_await_us.to_le_bytes());
    out[10..18].copy_from_slice(&ping.sent_at_us.to_le_bytes());
    out[18..26].copy_from_slice(&ping.arrived_at_us.to_le_bytes());
    out
}

pub fn decode_ping(payload: &[u8]) -> Option<Ping> {
    if payload.len() != PING_LEN {
        return None;
    }
    Some(Ping {
        id: u32::from_le_bytes(payload[..4].try_into().ok()?),
        stream: Stream {
            reliable: payload[4] != 0,
            route: if payload[5] != 0 {
                Route::Task
            } else {
                Route::Main
            },
        },
        echo_await_us: u32::from_le_bytes(payload[6..10].try_into().ok()?),
        sent_at_us: u64::from_le_bytes(payload[10..18].try_into().ok()?),
        arrived_at_us: u64::from_le_bytes(payload[18..26].try_into().ok()?),
    })
}

pub fn encode_stats(stats: &NetSnapshot) -> Vec<u8> {
    serde_json::to_vec(stats).expect("NetSnapshot serializes")
}

pub fn decode_stats(payload: &[u8]) -> Option<NetSnapshot> {
    serde_json::from_slice(payload).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tick_round_trip() {
        assert_eq!(decode_tick(&encode_tick(0xdead_beef)), Some(0xdead_beef));
    }

    #[test]
    fn rejects_wrong_length() {
        assert_eq!(decode_tick(&[1, 2, 3]), None);
        assert_eq!(decode_tick(&[]), None);
    }

    #[test]
    fn ping_round_trip() {
        for stream in crate::ping::STREAMS {
            let ping = Ping {
                id: 0xdead_beef,
                stream,
                echo_await_us: 1234,
                sent_at_us: 1_793_000_000_000_000,
                arrived_at_us: 1_793_000_000_001_500,
            };
            assert_eq!(decode_ping(&encode_ping(ping)), Some(ping));
        }
        assert_eq!(decode_ping(&[0; 4]), None);
    }

    #[test]
    fn stats_round_trip() {
        let stats = NetSnapshot {
            rtt_ms: Some(21.5),
            path: Some("host udp".into()),
            ..Default::default()
        };
        assert_eq!(decode_stats(&encode_stats(&stats)), Some(stats));
        assert_eq!(decode_stats(b"not json"), None);
    }
}
