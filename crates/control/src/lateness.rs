//! How late a guest's message is, without a shared clock.

use std::time::{Duration, Instant};

/// Arrival time minus the guest's timestamp is the network delay plus a fixed clock offset. The
/// smallest recent value stands in for "on time", and anything above it is lateness (ADR 0003).
///
/// The baseline is the minimum over the last 10 to 20 s rather than all time, so drift between
/// the two clocks (tens of ppm, or about 100 ms an hour) can't build up into false lateness. The
/// window only moves when messages arrive, so after a stall of any length the pre-stall baseline
/// is still there to measure the backlog against.
#[derive(Debug)]
pub(crate) struct Lateness {
    epoch: Instant,
    window_start: Option<Instant>,
    current: i128,
    previous: i128,
}

const WINDOW: Duration = Duration::from_secs(10);

impl Lateness {
    pub fn new(epoch: Instant) -> Self {
        Self {
            epoch,
            window_start: None,
            current: i128::MAX,
            previous: i128::MAX,
        }
    }

    /// Records a message that arrived `now` and returns how late it is.
    pub fn observe(&mut self, sent_at_us: u64, now: Instant) -> Duration {
        let offset =
            now.saturating_duration_since(self.epoch).as_micros() as i128 - i128::from(sent_at_us);
        if self
            .window_start
            .is_none_or(|start| now.saturating_duration_since(start) >= WINDOW)
        {
            self.previous = self.current;
            self.current = i128::MAX;
            self.window_start = Some(now);
        }
        self.current = self.current.min(offset);
        let late = offset - self.current.min(self.previous);
        Duration::from_micros(u64::try_from(late).unwrap_or(u64::MAX))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: u64 = 1_000;

    #[test]
    fn measures_against_the_fastest_message() {
        let t0 = Instant::now();
        let mut l = Lateness::new(t0);
        // The guest's clock started 5 s before ours; the first message took 20 ms.
        assert_eq!(
            l.observe(5_000 * MS, t0 + Duration::from_millis(20)),
            Duration::ZERO
        );
        // This one took 70 ms: 50 ms later than the best so far.
        let late = l.observe(5_100 * MS, t0 + Duration::from_millis(170));
        assert_eq!(late, Duration::from_millis(50));
        // A faster one becomes the new baseline.
        assert_eq!(
            l.observe(5_200 * MS, t0 + Duration::from_millis(210)),
            Duration::ZERO
        );
    }

    #[test]
    fn a_backlog_after_a_long_stall_still_counts_as_late() {
        let t0 = Instant::now();
        let mut l = Lateness::new(t0);
        l.observe(0, t0 + Duration::from_millis(10));
        // Sent at 1 s, but held up by a 30 s stall.
        let late = l.observe(1_000 * MS, t0 + Duration::from_millis(31_010));
        assert_eq!(late, Duration::from_secs(30));
    }
}
