//! Viewer-side latency bookkeeping: remembers when ticks were sent and turns marker
//! sightings into samples.

use std::collections::HashMap;
use std::time::Instant;

use serde::Serialize;

use crate::ping::{PingLog, StreamReport};
use crate::quality::{summarize_low, LowSummary, Scores};
use crate::stats::{summarize, Summary};

/// How long a sent tick is kept waiting for its marker before it counts as lost.
const TICK_TTL_SECS: f64 = 5.0;

#[derive(Debug, Default)]
pub struct Recorder {
    sent: HashMap<u32, Instant>,
    last_seen: Option<u32>,
    /// Tick sent → frame carrying its marker decoded, in ms.
    pub rtt_decoded: Vec<f64>,
    /// Tick sent → that frame presented in the window, in ms.
    pub rtt_presented: Vec<f64>,
    /// Sharer capture → presented on the viewer, in ms. Needs a shared wall clock.
    pub one_way_presented: Vec<f64>,
    pub ticks_sent: u64,
    pub frames: u64,
    /// Decoded vs expected picture, for the synthetic pattern.
    pub quality: Vec<Scores>,
    /// The subset taken while the pattern was scrolling fast.
    pub quality_fast: Vec<Scores>,
    /// Input-path measurements, when `--measure-input` is on.
    pub input: PingLog,
}

/// Summaries for one reporting window or a whole run.
#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub rtt_decoded: Option<Summary>,
    pub rtt_presented: Option<Summary>,
    pub one_way_presented: Option<Summary>,
    pub ticks_sent: u64,
    pub ticks_seen: usize,
    pub frames: u64,
    pub ssim: Option<LowSummary>,
    pub psnr_db: Option<LowSummary>,
    pub ssim_fast_scroll: Option<LowSummary>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub input: Vec<StreamReport>,
}

impl Recorder {
    pub fn tick_sent(&mut self, id: u32, at: Instant) {
        self.sent
            .retain(|_, t| at.duration_since(*t).as_secs_f64() < TICK_TTL_SECS);
        self.sent.insert(id, at);
        self.ticks_sent += 1;
    }

    /// Records a presented frame. `marker` is the id read from the frame, if any;
    /// `captured_at_us` / `presented_at_us` are wall-clock times for the one-way sample.
    pub fn frame_presented(
        &mut self,
        marker: Option<u32>,
        decoded_at: Instant,
        presented_at: Instant,
        captured_at_us: Option<u64>,
        presented_at_us: u64,
    ) {
        self.frames += 1;
        if let Some(captured) = captured_at_us.filter(|&c| c > 0 && c <= presented_at_us) {
            self.one_way_presented
                .push((presented_at_us - captured) as f64 / 1000.0);
        }
        let Some(id) = marker else { return };
        if self.last_seen == Some(id) {
            return;
        }
        self.last_seen = Some(id);
        if let Some(sent) = self.sent.remove(&id) {
            let ms = |t: Instant| t.saturating_duration_since(sent).as_secs_f64() * 1000.0;
            self.rtt_decoded.push(ms(decoded_at));
            self.rtt_presented.push(ms(presented_at));
        }
    }

    pub fn report(&self) -> Report {
        Report {
            rtt_decoded: summarize(&self.rtt_decoded),
            rtt_presented: summarize(&self.rtt_presented),
            one_way_presented: summarize(&self.one_way_presented),
            ticks_sent: self.ticks_sent,
            ticks_seen: self.rtt_presented.len(),
            frames: self.frames,
            ssim: summarize_low(&self.quality.iter().map(|q| q.ssim).collect::<Vec<_>>()),
            psnr_db: summarize_low(&self.quality.iter().map(|q| q.psnr_db).collect::<Vec<_>>()),
            ssim_fast_scroll: summarize_low(
                &self.quality_fast.iter().map(|q| q.ssim).collect::<Vec<_>>(),
            ),
            input: self.input.report(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn measures_first_sighting_only() {
        let t0 = Instant::now();
        let mut r = Recorder::default();
        r.tick_sent(7, t0);
        let decoded = t0 + Duration::from_millis(40);
        let presented = t0 + Duration::from_millis(45);
        r.frame_presented(Some(7), decoded, presented, None, 0);
        r.frame_presented(Some(7), decoded, t0 + Duration::from_millis(80), None, 0);
        assert_eq!(r.rtt_decoded, vec![40.0]);
        assert_eq!(r.rtt_presented, vec![45.0]);
        assert_eq!(r.frames, 2);
    }

    #[test]
    fn ignores_unknown_and_missing_markers() {
        let t0 = Instant::now();
        let mut r = Recorder::default();
        r.frame_presented(None, t0, t0, None, 0);
        r.frame_presented(Some(99), t0, t0, None, 0);
        assert!(r.rtt_presented.is_empty());
    }

    #[test]
    fn records_one_way_latency() {
        let t0 = Instant::now();
        let mut r = Recorder::default();
        r.frame_presented(None, t0, t0, Some(1_000_000), 1_030_500);
        r.frame_presented(None, t0, t0, Some(2_000_000), 1_000_000); // clock skew: dropped
        assert_eq!(r.one_way_presented, vec![30.5]);
    }

    #[test]
    fn expires_old_ticks() {
        let t0 = Instant::now();
        let mut r = Recorder::default();
        r.tick_sent(1, t0);
        r.tick_sent(2, t0 + Duration::from_secs(10));
        let late = t0 + Duration::from_secs(11);
        r.frame_presented(Some(1), late, late, None, 0);
        assert!(r.rtt_presented.is_empty());
    }
}
