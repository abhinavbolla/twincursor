//! Auto-throttle: picks the resolution tier the call can sustain, text first.
//!
//! libwebrtc already adapts the bitrate to the bandwidth estimate, and for screen share it
//! keeps resolution and drops frame rate when squeezed. That keeps text sharp until the
//! budget is too small for the resolution: then every frame is blurry and the rate collapses.
//! This controller steps the resolution down before that point, and back up once there is
//! clearly room again. It's a pure state machine fed once a second, so it's tested on traces.

use crate::capture::Tier;
use crate::netstats::{Limit, NetSnapshot};

/// A budget just under a tier's floor (e.g. a cap set exactly at it) isn't trouble.
const FLOOR_TOLERANCE: f64 = 0.9;
/// Trouble has to last this long before stepping down.
const DOWN_AFTER_S: f64 = 3.0;
/// Calm with headroom has to last this long before stepping up (doubles after a flap).
const UP_AFTER_S: f64 = 10.0;
const UP_AFTER_MAX_S: f64 = 80.0;
/// Stepping down again within this long of stepping up counts as a flap.
const FLAP_WINDOW_S: f64 = 30.0;
/// Headroom needed to step up: the next tier's floor times this.
const UP_HEADROOM: f64 = 1.5;
/// Below this send rate while the encoder reports a limit, motion is already suffering.
const MIN_FPS: f64 = 15.0;
/// Viewer-side distress: loss or frozen time within one second.
const VIEWER_LOSS_PCT: f64 = 5.0;
const VIEWER_FREEZE_MS: f64 = 300.0;

/// Smallest bitrate (kbit/s) at which a tier still shows sharp text while scrolling. Measured
/// on the synthetic pattern with H.264 (`research/m15-results.md`); VP8 needs about twice.
fn floor_kbps(tier: Tier, codec_factor: f64) -> f64 {
    let base = match tier {
        Tier::P720 => 500.0,
        Tier::P1080 => 1000.0,
        Tier::P1440 => 1600.0,
        Tier::Native => 2500.0,
    };
    base * codec_factor
}

fn lower(tier: Tier) -> Option<Tier> {
    match tier {
        Tier::P720 => None,
        Tier::P1080 => Some(Tier::P720),
        Tier::P1440 => Some(Tier::P1080),
        Tier::Native => Some(Tier::P1440),
    }
}

fn higher(tier: Tier) -> Option<Tier> {
    match tier {
        Tier::P720 => Some(Tier::P1080),
        Tier::P1080 => Some(Tier::P1440),
        Tier::P1440 => Some(Tier::Native),
        Tier::Native => None,
    }
}

/// A tier change and why, for logs and the HUD.
#[derive(Debug, Clone, PartialEq)]
pub struct Change {
    pub from: Tier,
    pub to: Tier,
    pub reason: String,
}

#[derive(Debug, Clone)]
pub struct Adapter {
    max: Tier,
    tier: Tier,
    codec_factor: f64,
    trouble_since: Option<f64>,
    calm_since: Option<f64>,
    last_up: Option<f64>,
    up_after: f64,
}

impl Adapter {
    /// Starts at `max`, the tier the user asked for; never goes above it. `codec_factor`
    /// scales the bitrate floors (1.0 for H.264, about 2.0 for VP8).
    pub fn new(max: Tier, codec_factor: f64) -> Self {
        Self {
            max,
            tier: max,
            codec_factor,
            trouble_since: None,
            calm_since: None,
            last_up: None,
            up_after: UP_AFTER_S,
        }
    }

    pub fn tier(&self) -> Tier {
        self.tier
    }

    /// Feeds one second of stats: ours (`own`, the publishing connection) and the viewer's,
    /// if it sent any. Returns a change when the tier should move.
    pub fn update(
        &mut self,
        t_s: f64,
        own: &NetSnapshot,
        viewer: Option<&NetSnapshot>,
    ) -> Option<Change> {
        let send = own.send.as_ref()?;
        // What the encoder may spend: its target, which follows the bandwidth estimate.
        let budget = match (send.target_kbps > 0.0, own.bwe_kbps) {
            (true, Some(bwe)) => send.target_kbps.min(bwe),
            (true, None) => send.target_kbps,
            (false, bwe) => bwe?,
        };
        let floor = floor_kbps(self.tier, self.codec_factor);
        let recv = viewer.and_then(|v| v.recv.as_ref());

        let trouble = if send.limit == Limit::Cpu {
            Some("encoder is CPU-limited".to_owned())
        } else if budget < floor * FLOOR_TOLERANCE {
            Some(format!(
                "budget {:.1} Mbps < {:.1} Mbps needed",
                budget / 1000.0,
                floor / 1000.0
            ))
        } else if send.limit == Limit::Bandwidth && send.fps < MIN_FPS {
            Some(format!("bandwidth-limited at {:.0} fps", send.fps))
        } else if let Some(r) = recv.filter(|r| r.loss_pct >= VIEWER_LOSS_PCT) {
            Some(format!("viewer loss {:.1}%", r.loss_pct))
        } else {
            recv.filter(|r| r.freeze_ms >= VIEWER_FREEZE_MS)
                .map(|r| format!("viewer froze {:.0} ms", r.freeze_ms))
        };

        if let Some(reason) = trouble {
            self.calm_since = None;
            let since = *self.trouble_since.get_or_insert(t_s);
            if t_s - since >= DOWN_AFTER_S {
                if let Some(to) = lower(self.tier) {
                    if self.last_up.is_some_and(|up| t_s - up < FLAP_WINDOW_S) {
                        self.up_after = (self.up_after * 2.0).min(UP_AFTER_MAX_S);
                    }
                    self.trouble_since = None;
                    return Some(self.change(to, reason));
                }
            }
            return None;
        }

        self.trouble_since = None;
        let viewer_clean = recv.is_none_or(|r| r.loss_pct < 1.0 && r.freezes == 0);
        let to = higher(self.tier).filter(|&t| t <= self.max)?;
        let needed = floor_kbps(to, self.codec_factor) * UP_HEADROOM;
        // The budget only grows to what the encoder asks for; on a mostly static screen it
        // asks for little, so also accept a bandwidth estimate with headroom.
        let room = budget.max(own.bwe_kbps.unwrap_or(0.0)) >= needed;
        if !(room && viewer_clean && send.limit == Limit::None) {
            self.calm_since = None;
            return None;
        }
        let since = *self.calm_since.get_or_insert(t_s);
        if t_s - since >= self.up_after {
            self.calm_since = None;
            self.last_up = Some(t_s);
            return Some(self.change(
                to,
                format!(
                    "{:.1} Mbps available",
                    budget.max(own.bwe_kbps.unwrap_or(0.0)) / 1000.0
                ),
            ));
        }
        None
    }

    fn change(&mut self, to: Tier, reason: String) -> Change {
        let from = self.tier;
        self.tier = to;
        Change { from, to, reason }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::netstats::{RecvStats, SendStats};

    fn own(target_kbps: f64, bwe_kbps: f64, fps: f64, limit: Limit) -> NetSnapshot {
        NetSnapshot {
            bwe_kbps: Some(bwe_kbps),
            send: Some(SendStats {
                target_kbps,
                fps,
                limit,
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn good() -> NetSnapshot {
        own(8000.0, 9000.0, 60.0, Limit::None)
    }

    fn viewer(loss_pct: f64, freeze_ms: f64) -> NetSnapshot {
        NetSnapshot {
            recv: Some(RecvStats {
                loss_pct,
                freeze_ms,
                freezes: u32::from(freeze_ms > 0.0),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    /// Runs one snapshot per second from `t0`, returning the changes with their times.
    fn run(
        a: &mut Adapter,
        t0: u32,
        secs: u32,
        own: &NetSnapshot,
        v: Option<&NetSnapshot>,
    ) -> Vec<(u32, Tier)> {
        (t0..t0 + secs)
            .filter_map(|t| a.update(f64::from(t), own, v).map(|c| (t, c.to)))
            .collect()
    }

    #[test]
    fn steady_good_network_stays_put() {
        let mut a = Adapter::new(Tier::P1440, 1.0);
        assert!(run(&mut a, 0, 120, &good(), None).is_empty());
        assert_eq!(a.tier(), Tier::P1440);
    }

    #[test]
    fn bandwidth_drop_steps_down_after_three_seconds_and_recovers() {
        let mut a = Adapter::new(Tier::P1440, 1.0);
        let squeezed = own(1200.0, 1300.0, 40.0, Limit::Bandwidth);
        // 1.2 Mbps is below 1440's floor but enough for 1080.
        assert_eq!(run(&mut a, 0, 10, &squeezed, None), vec![(3, Tier::P1080)]);
        // Back to plenty: up again after UP_AFTER_S of calm.
        let ups = run(&mut a, 10, 15, &good(), None);
        assert_eq!(ups, vec![(20, Tier::P1440)]);
    }

    #[test]
    fn severe_drop_walks_down_one_tier_at_a_time() {
        let mut a = Adapter::new(Tier::P1440, 1.0);
        let starved = own(300.0, 350.0, 10.0, Limit::Bandwidth);
        let downs = run(&mut a, 0, 20, &starved, None);
        assert_eq!(downs, vec![(3, Tier::P1080), (7, Tier::P720)]);
        assert_eq!(a.tier(), Tier::P720, "never below 720");
    }

    #[test]
    fn short_blips_are_ignored() {
        let mut a = Adapter::new(Tier::P1080, 1.0);
        let squeezed = own(500.0, 600.0, 20.0, Limit::Bandwidth);
        for t in 0..60 {
            let s = if t % 4 == 0 { &squeezed } else { &good() };
            assert_eq!(a.update(f64::from(t), s, None), None, "t={t}");
        }
    }

    #[test]
    fn cpu_limit_steps_down() {
        let mut a = Adapter::new(Tier::Native, 1.0);
        let cpu = own(9000.0, 9000.0, 25.0, Limit::Cpu);
        let changes = run(&mut a, 0, 5, &cpu, None);
        assert_eq!(changes, vec![(3, Tier::P1440)]);
    }

    #[test]
    fn viewer_distress_steps_down_even_with_good_uplink() {
        let mut a = Adapter::new(Tier::P1440, 1.0);
        let lossy = viewer(8.0, 0.0);
        assert_eq!(
            run(&mut a, 0, 5, &good(), Some(&lossy)),
            vec![(3, Tier::P1080)]
        );
        let mut a = Adapter::new(Tier::P1080, 1.0);
        let frozen = viewer(0.0, 600.0);
        assert_eq!(
            run(&mut a, 0, 5, &good(), Some(&frozen)),
            vec![(3, Tier::P720)]
        );
    }

    #[test]
    fn does_not_step_up_while_viewer_struggles() {
        let mut a = Adapter::new(Tier::P1440, 1.0);
        run(
            &mut a,
            0,
            4,
            &own(1200.0, 1300.0, 30.0, Limit::Bandwidth),
            None,
        );
        assert_eq!(a.tier(), Tier::P1080);
        let some_loss = viewer(2.0, 0.0);
        assert!(run(&mut a, 4, 60, &good(), Some(&some_loss)).is_empty());
    }

    #[test]
    fn never_exceeds_requested_tier() {
        let mut a = Adapter::new(Tier::P1080, 1.0);
        assert!(run(&mut a, 0, 120, &good(), None).is_empty());
        assert_eq!(a.tier(), Tier::P1080);
    }

    #[test]
    fn flapping_backs_off() {
        let mut a = Adapter::new(Tier::P1440, 1.0);
        let squeezed = own(1200.0, 1300.0, 30.0, Limit::Bandwidth);
        // Down at 3, up at 14 (10 s of calm from 4).
        assert_eq!(run(&mut a, 0, 4, &squeezed, None), vec![(3, Tier::P1080)]);
        assert_eq!(run(&mut a, 4, 11, &good(), None), vec![(14, Tier::P1440)]);
        // Squeezed again soon after: down, and the next step up waits twice as long.
        assert_eq!(run(&mut a, 15, 4, &squeezed, None), vec![(18, Tier::P1080)]);
        let ups = run(&mut a, 19, 30, &good(), None);
        assert_eq!(ups, vec![(39, Tier::P1440)], "20 s of calm, not 10");
    }

    #[test]
    fn vp8_needs_more_bitrate() {
        let mut h264 = Adapter::new(Tier::P1080, 1.0);
        let mut vp8 = Adapter::new(Tier::P1080, 2.0);
        let budget = own(1500.0, 1600.0, 50.0, Limit::None);
        assert!(run(&mut h264, 0, 10, &budget, None).is_empty());
        assert_eq!(run(&mut vp8, 0, 10, &budget, None), vec![(3, Tier::P720)]);
    }

    #[test]
    fn static_screen_low_fps_is_not_trouble() {
        // SCK sends few frames on a static screen; with no limit reported that's fine.
        let mut a = Adapter::new(Tier::P1440, 1.0);
        let idle = own(8000.0, 9000.0, 4.0, Limit::None);
        assert!(run(&mut a, 0, 60, &idle, None).is_empty());
    }

    #[test]
    fn no_send_stats_no_decision() {
        let mut a = Adapter::new(Tier::P1440, 1.0);
        assert_eq!(a.update(0.0, &NetSnapshot::default(), None), None);
    }
}
