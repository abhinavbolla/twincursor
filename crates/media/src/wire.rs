//! Data messages exchanged between viewer and sharer during a call.

use crate::netstats::NetSnapshot;

/// Data-packet topic for latency ticks (viewer → sharer).
pub const TICK_TOPIC: &str = "tc.tick";
/// Data-packet topic for the sharer's network stats (sharer → viewer), once a second.
pub const STATS_TOPIC: &str = "tc.stats";

pub fn encode_tick(id: u32) -> Vec<u8> {
    id.to_le_bytes().to_vec()
}

pub fn decode_tick(payload: &[u8]) -> Option<u32> {
    Some(u32::from_le_bytes(payload.try_into().ok()?))
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
