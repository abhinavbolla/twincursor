//! Data messages exchanged between viewer and sharer during a latency run.

/// Data-packet topic for latency ticks (viewer → sharer).
pub const TICK_TOPIC: &str = "tc.tick";

pub fn encode_tick(id: u32) -> Vec<u8> {
    id.to_le_bytes().to_vec()
}

pub fn decode_tick(payload: &[u8]) -> Option<u32> {
    Some(u32::from_le_bytes(payload.try_into().ok()?))
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
}
