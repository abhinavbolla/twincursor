//! Viewer-side audio latency bookkeeping: pairs the markers heard with the ticks sent, the
//! sharer's emit times, and the video frames carrying the same tick.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::stats::{summarize, Summary};

/// How long an unmatched entry waits for its other half.
const TTL_SECS: f64 = 5.0;
/// Sound already on its way when the sharer muted may still be heard for this long.
const MUTE_GRACE: Duration = Duration::from_millis(500);

#[derive(Debug, Default)]
pub struct AudioLog {
    sent: HashMap<u32, Instant>,
    /// When the frame carrying a tick's video marker was first shown.
    video: HashMap<u32, Instant>,
    heard: HashMap<u32, (Instant, u64)>,
    /// The sharer's wall clock when it started each burst, in µs, and when we learned it.
    emitted: HashMap<u32, (u64, Instant)>,
    /// Tick sent → its burst heard, in ms.
    pub rtt: Vec<f64>,
    /// Sharer emitted → heard here, in ms. Needs a shared wall clock.
    pub one_way: Vec<f64>,
    /// Burst heard − the same tick's video shown, in ms: positive when audio is later.
    pub av_offset: Vec<f64>,
    pub rejected: u64,
    /// Since when the sharer's voice has been muted.
    muted_since: Option<Instant>,
    /// How often the sharer muted, and for how long in all, in ms.
    pub mutes: u32,
    pub muted_ms: f64,
    /// Bursts heard while the sharer was muted: there should be none.
    pub heard_while_muted: u32,
    /// Bursts heard after the sharer unmuted again.
    pub heard_after_unmute: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct AudioReport {
    pub rtt: Option<Summary>,
    pub one_way: Option<Summary>,
    pub av_offset: Option<Summary>,
    pub bursts_heard: usize,
    pub bursts_rejected: u64,
    /// Share of received audio the jitter buffer had to make up, over the whole run.
    pub concealed_pct: Option<f64>,
    /// How often the sharer muted its voice, and for how long in all.
    pub mutes: u32,
    pub muted_s: f64,
    /// Bursts heard while it was muted (none should be), and after it unmuted.
    pub heard_while_muted: u32,
    pub heard_after_unmute: u32,
}

impl AudioLog {
    pub fn tick_sent(&mut self, id: u32, at: Instant) {
        self.prune(at);
        self.sent.insert(id, at);
    }

    /// The first time a frame with this tick's video marker was shown.
    pub fn video_seen(&mut self, id: u32, at: Instant) {
        match self.heard.get(&id) {
            Some(&(heard, _)) => self.av_offset.push(signed_ms(at, heard)),
            None => {
                self.video.insert(id, at);
            }
        }
    }

    /// The sharer started the burst for `id` at `wall_us` on its clock.
    pub fn emitted(&mut self, id: u32, wall_us: u64) {
        match self.heard.get(&id) {
            Some(&(_, heard_us)) => self.push_one_way(wall_us, heard_us),
            None => {
                self.emitted.insert(id, (wall_us, Instant::now()));
            }
        }
    }

    /// The burst for `id` started playing at `at` (`wall_us` on our clock).
    pub fn heard(&mut self, id: u32, at: Instant, wall_us: u64) {
        if self.heard.contains_key(&id) {
            return;
        }
        match self.muted_since {
            Some(since) if at.saturating_duration_since(since) > MUTE_GRACE => {
                self.heard_while_muted += 1;
            }
            Some(_) => {}
            None if self.mutes > 0 => self.heard_after_unmute += 1,
            None => {}
        }
        if let Some(sent) = self.sent.remove(&id) {
            self.rtt
                .push(at.saturating_duration_since(sent).as_secs_f64() * 1000.0);
        }
        if let Some(video) = self.video.remove(&id) {
            self.av_offset.push(signed_ms(video, at));
        }
        if let Some((emitted_us, _)) = self.emitted.remove(&id) {
            self.push_one_way(emitted_us, wall_us);
        }
        self.heard.insert(id, (at, wall_us));
    }

    /// The sharer muted or unmuted its voice.
    pub fn remote_muted(&mut self, muted: bool, at: Instant) {
        match (muted, self.muted_since.take()) {
            (true, None) => {
                self.mutes += 1;
                self.muted_since = Some(at);
            }
            (true, since) => self.muted_since = since,
            (false, Some(since)) => {
                self.muted_ms += at.saturating_duration_since(since).as_secs_f64() * 1000.0;
            }
            (false, None) => {}
        }
    }

    fn push_one_way(&mut self, emitted_us: u64, heard_us: u64) {
        if emitted_us > 0 && emitted_us <= heard_us {
            self.one_way.push((heard_us - emitted_us) as f64 / 1000.0);
        }
    }

    fn prune(&mut self, now: Instant) {
        let fresh = |t: &Instant| now.saturating_duration_since(*t).as_secs_f64() < TTL_SECS;
        self.sent.retain(|_, t| fresh(t));
        self.video.retain(|_, t| fresh(t));
        self.heard.retain(|_, (t, _)| fresh(t));
        self.emitted.retain(|_, (_, t)| fresh(t));
    }

    /// `None` until a burst has been heard or rejected, so runs without audio report nothing.
    pub fn report(&self) -> Option<AudioReport> {
        let heard = self.rtt.len().max(self.one_way.len());
        (heard > 0 || self.rejected > 0).then(|| AudioReport {
            rtt: summarize(&self.rtt),
            one_way: summarize(&self.one_way),
            av_offset: summarize(&self.av_offset),
            bursts_heard: heard,
            bursts_rejected: self.rejected,
            concealed_pct: None,
            mutes: self.mutes,
            muted_s: (self.muted_ms
                + self
                    .muted_since
                    .map_or(0.0, |s| s.elapsed().as_secs_f64() * 1000.0))
                / 1000.0,
            heard_while_muted: self.heard_while_muted,
            heard_after_unmute: self.heard_after_unmute,
        })
    }
}

/// `to − from` in ms, negative when `to` came first.
fn signed_ms(from: Instant, to: Instant) -> f64 {
    match to.checked_duration_since(from) {
        Some(d) => d.as_secs_f64() * 1000.0,
        None => -from.duration_since(to).as_secs_f64() * 1000.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn round_trip_one_way_and_av_offset() {
        let t0 = Instant::now();
        let mut log = AudioLog::default();
        log.tick_sent(5, t0);
        log.emitted(5, 1_000_000);
        log.video_seen(5, t0 + ms(40));
        log.heard(5, t0 + ms(70), 1_055_000);
        assert_eq!(log.rtt, vec![70.0]);
        assert_eq!(log.one_way, vec![55.0]);
        assert!((log.av_offset[0] - 30.0).abs() < 1e-9);
    }

    #[test]
    fn halves_can_arrive_in_any_order() {
        let t0 = Instant::now();
        let mut log = AudioLog::default();
        log.tick_sent(9, t0);
        log.heard(9, t0 + ms(50), 2_050_000);
        // Video after audio, and the sharer's emit time after the burst was heard.
        log.video_seen(9, t0 + ms(60));
        log.emitted(9, 2_000_000);
        assert_eq!(log.one_way, vec![50.0]);
        assert!((log.av_offset[0] + 10.0).abs() < 1e-9, "audio 10 ms early");
        // Hearing the same burst again doesn't count twice.
        log.heard(9, t0 + ms(90), 2_090_000);
        assert_eq!(log.rtt.len(), 1);
    }

    #[test]
    fn nothing_to_report_without_audio() {
        let mut log = AudioLog::default();
        log.tick_sent(1, Instant::now());
        assert!(log.report().is_none());
        log.rejected = 1;
        assert_eq!(log.report().unwrap().bursts_heard, 0);
    }

    #[test]
    fn bursts_are_sorted_by_whether_the_sharer_was_muted() {
        let t0 = Instant::now();
        let mut log = AudioLog::default();
        log.heard(1, t0, 0);
        log.remote_muted(true, t0 + ms(1000));
        // Already on its way when the sharer muted.
        log.heard(2, t0 + ms(1200), 0);
        // Muted again while muted changes nothing.
        log.remote_muted(true, t0 + ms(2000));
        log.heard(3, t0 + ms(3000), 0);
        log.remote_muted(false, t0 + ms(5000));
        log.heard(4, t0 + ms(6000), 0);
        log.heard(5, t0 + ms(7000), 0);
        assert_eq!((log.mutes, log.muted_ms), (1, 4000.0));
        assert_eq!((log.heard_while_muted, log.heard_after_unmute), (1, 2));
        log.rejected = 1;
        let report = log.report().unwrap();
        assert_eq!((report.mutes, report.muted_s), (1, 4.0));
    }

    #[test]
    fn old_entries_expire() {
        let t0 = Instant::now();
        let mut log = AudioLog::default();
        log.tick_sent(1, t0);
        log.tick_sent(2, t0 + Duration::from_secs(10));
        log.heard(1, t0 + Duration::from_secs(10), 0);
        assert!(log.rtt.is_empty());
    }
}
