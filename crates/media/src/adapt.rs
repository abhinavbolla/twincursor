//! Auto-throttle: picks the resolution and frame rate the call can sustain, text first.
//!
//! libwebrtc already adapts the bitrate to the bandwidth estimate, and for screen share it
//! keeps resolution and drops frame rate when squeezed. That isn't enough on its own:
//! - When the budget is too small for the resolution, every frame is blurry.
//! - When the *viewer's* downlink is the bottleneck, the sharer's bandwidth estimate doesn't
//!   see it. Without simulcast the SFU can only drop packets.
//!
//! This controller walks a ladder that halves the frame rate before giving up resolution,
//! because text needs pixels more than it needs 60 fps:
//! 1440p 60 → 1440p 30 → 1080p 30 → 720p 30 → 720p 15.
//! It steps down after 3 s of trouble and back up after a stretch of calm with headroom. It's a
//! pure state machine fed once a second, so it's tested on traces.

use crate::capture::Tier;
use crate::netstats::{Limit, NetSnapshot};

/// A budget just under a level's floor (e.g. a cap set exactly at it) isn't trouble.
const FLOOR_TOLERANCE: f64 = 0.9;
/// Trouble has to last this long before stepping down.
const DOWN_AFTER_S: f64 = 3.0;
/// A budget under this share of the floor is a severe drop: act after `SEVERE_AFTER_S`.
const SEVERE_SHARE: f64 = 0.5;
const SEVERE_AFTER_S: f64 = 1.0;
/// Calm with headroom has to last this long before stepping up (doubles after a flap).
const UP_AFTER_S: f64 = 10.0;
const UP_AFTER_MAX_S: f64 = 80.0;
/// Stepping down again within this long of stepping up counts as a flap.
const FLAP_WINDOW_S: f64 = 30.0;
/// Headroom needed to step up: the next level's floor times this.
const UP_HEADROOM: f64 = 1.5;
/// Bandwidth-limited and sending under this share of the level's frame rate means motion is
/// already suffering.
const MIN_FPS_SHARE: f64 = 0.5;
/// Frame rate of the reduced levels.
const REDUCED_FPS: u32 = 30;
const LOWEST_FPS: u32 = 15;
/// Viewer-side distress: loss or frozen time within one second.
const VIEWER_LOSS_PCT: f64 = 5.0;
const VIEWER_FREEZE_MS: f64 = 300.0;
/// Freezes per second that count as distress. libwebrtc's `totalFreezesDuration` stays 0 on
/// the builds we use (seen on Windows), so the count is the signal we can rely on.
const VIEWER_FREEZES: u32 = 2;

/// A resolution tier and frame-rate cap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Level {
    pub tier: Tier,
    pub fps: u32,
}

impl Level {
    /// Smallest bitrate (kbit/s) at which this level still shows sharp text while scrolling.
    /// The tier bases are from the synthetic pattern with H.264 (`research/m15-results.md`);
    /// VP8 needs about twice. Bitrate grows less than linearly with frame rate.
    fn floor_kbps(self, codec_factor: f64) -> f64 {
        let base = match self.tier {
            Tier::P720 => 500.0,
            Tier::P1080 => 1000.0,
            Tier::P1440 => 1600.0,
            Tier::Native => 2500.0,
        };
        base * codec_factor * (f64::from(self.fps) / 60.0).powf(0.75)
    }
}

impl std::fmt::Display for Level {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let lines = match self.tier {
            Tier::P720 => "720p",
            Tier::P1080 => "1080p",
            Tier::P1440 => "1440p",
            Tier::Native => "native",
        };
        write!(f, "{lines} {} fps", self.fps)
    }
}

/// The levels from best to worst, starting at what the user asked for.
pub fn ladder(max: Tier, max_fps: u32) -> Vec<Level> {
    let mut levels = vec![Level {
        tier: max,
        fps: max_fps,
    }];
    let reduced = max_fps.min(REDUCED_FPS);
    if reduced < max_fps {
        levels.push(Level {
            tier: max,
            fps: reduced,
        });
    }
    let mut tier = max;
    while let Some(lower) = lower(tier) {
        tier = lower;
        levels.push(Level { tier, fps: reduced });
    }
    if LOWEST_FPS < reduced {
        levels.push(Level {
            tier,
            fps: LOWEST_FPS,
        });
    }
    levels
}

fn lower(tier: Tier) -> Option<Tier> {
    match tier {
        Tier::P720 => None,
        Tier::P1080 => Some(Tier::P720),
        Tier::P1440 => Some(Tier::P1080),
        Tier::Native => Some(Tier::P1440),
    }
}

/// A level change and why, for logs and the HUD.
#[derive(Debug, Clone, PartialEq)]
pub struct Change {
    pub from: Level,
    pub to: Level,
    pub reason: String,
}

#[derive(Debug, Clone)]
pub struct Adapter {
    ladder: Vec<Level>,
    /// Index into `ladder`; 0 is the best.
    at: usize,
    codec_factor: f64,
    trouble_since: Option<f64>,
    calm_since: Option<f64>,
    last_up: Option<f64>,
    up_after: f64,
}

impl Adapter {
    /// Starts at `max` tier and `max_fps`, what the user asked for, and never goes above them.
    /// `codec_factor` scales the bitrate floors (1.0 for H.264, about 2.0 for VP8).
    pub fn new(max: Tier, max_fps: u32, codec_factor: f64) -> Self {
        Self {
            ladder: ladder(max, max_fps),
            at: 0,
            codec_factor,
            trouble_since: None,
            calm_since: None,
            last_up: None,
            up_after: UP_AFTER_S,
        }
    }

    pub fn level(&self) -> Level {
        self.ladder[self.at]
    }

    /// Feeds one second of stats: ours (`own`, the publishing connection) and the viewer's,
    /// if it sent any. Returns a change when the level should move.
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
        let level = self.level();
        let floor = level.floor_kbps(self.codec_factor);
        let recv = viewer.and_then(|v| v.recv.as_ref());

        // On a big drop, go straight to the first level the budget can carry rather than
        // walking down one level every few seconds while the picture stalls.
        let fits =
            |i: usize| self.ladder[i].floor_kbps(self.codec_factor) * FLOOR_TOLERANCE <= budget;
        let mut step_to = self.at + 1;
        let severe = budget < floor * SEVERE_SHARE;
        let trouble = if send.limit == Limit::Cpu {
            Some("encoder is CPU-limited".to_owned())
        } else if budget < floor * FLOOR_TOLERANCE {
            step_to = (self.at + 1..self.ladder.len())
                .find(|&i| fits(i))
                .unwrap_or(self.ladder.len() - 1);
            Some(format!(
                "budget {:.1} Mbps < {:.1} Mbps needed",
                budget / 1000.0,
                floor / 1000.0
            ))
        } else if send.limit == Limit::Bandwidth && send.fps < f64::from(level.fps) * MIN_FPS_SHARE
        {
            Some(format!("bandwidth-limited at {:.0} fps", send.fps))
        } else if let Some(r) = recv.filter(|r| r.loss_pct >= VIEWER_LOSS_PCT) {
            Some(format!("viewer loss {:.1}%", r.loss_pct))
        } else {
            recv.filter(|r| r.freeze_ms >= VIEWER_FREEZE_MS || r.freezes >= VIEWER_FREEZES)
                .map(|r| format!("viewer froze {}x ({:.0} ms)", r.freezes, r.freeze_ms))
        };

        if let Some(reason) = trouble {
            self.calm_since = None;
            let since = *self.trouble_since.get_or_insert(t_s);
            let wait = if severe { SEVERE_AFTER_S } else { DOWN_AFTER_S };
            if t_s - since >= wait && self.at + 1 < self.ladder.len() {
                if self.last_up.is_some_and(|up| t_s - up < FLAP_WINDOW_S) {
                    self.up_after = (self.up_after * 2.0).min(UP_AFTER_MAX_S);
                }
                self.trouble_since = None;
                return Some(self.move_to(step_to.min(self.ladder.len() - 1), reason));
            }
            return None;
        }

        self.trouble_since = None;
        if self.at == 0 {
            return None;
        }
        let viewer_clean = recv.is_none_or(|r| r.loss_pct < 1.0 && r.freezes == 0);
        let needed = self.ladder[self.at - 1].floor_kbps(self.codec_factor) * UP_HEADROOM;
        // The budget only grows to what the encoder asks for; on a mostly static screen it
        // asks for little, so also accept a bandwidth estimate with headroom.
        let available = budget.max(own.bwe_kbps.unwrap_or(0.0));
        if !(available >= needed && viewer_clean && send.limit == Limit::None) {
            self.calm_since = None;
            return None;
        }
        let since = *self.calm_since.get_or_insert(t_s);
        if t_s - since >= self.up_after {
            self.calm_since = None;
            self.last_up = Some(t_s);
            return Some(self.move_to(
                self.at - 1,
                format!("{:.1} Mbps available", available / 1000.0),
            ));
        }
        None
    }

    fn move_to(&mut self, at: usize, reason: String) -> Change {
        let from = self.level();
        self.at = at;
        Change {
            from,
            to: self.level(),
            reason,
        }
    }
}

/// Caps the sender's bitrate to what actually reaches the viewer.
///
/// The ladder alone can't help when the viewer's downlink is the bottleneck: the encoder spends
/// whatever target the sharer's own uplink estimate allows, at any frame rate. In the lab, VP8
/// kept sending 4–5 Mbps into a 3 Mbit downlink at 720p 15 fps, and the viewer decoded nothing.
/// A cap at 1.5 Mbps fixed it (`research/m15-results.md`). This controller sets the cap from
/// the viewer's goodput when it's losing packets, and lifts it slowly when things are calm.
#[derive(Debug, Clone)]
pub struct BitrateCap {
    max_kbps: f64,
    cap_kbps: Option<f64>,
    trouble_since: Option<f64>,
    calm_since: Option<f64>,
    /// No decisions until this time: a change republishes the track, which takes a moment.
    settle_until: f64,
}

/// Viewer loss that means its downlink is overflowing.
const CAP_LOSS_PCT: f64 = 5.0;
/// Overflowing has to last this long before capping.
const CAP_AFTER_S: f64 = 4.0;
/// The new cap, as a share of what the viewer receives. Halving, like TCP's multiplicative
/// decrease: while the link overflows, goodput includes bursts draining from queues, so a gentle
/// cut leaves keyframes still overflowing (in the lab 2.1 Mbps into 3 Mbit kept losing 20%).
const CAP_SHARE: f64 = 0.5;
const CAP_MIN_KBPS: f64 = 300.0;
/// Calm this long before raising the cap by `CAP_RAISE`.
const CAP_RAISE_AFTER_S: f64 = 30.0;
const CAP_RAISE: f64 = 1.25;
const CAP_SETTLE_S: f64 = 8.0;

impl BitrateCap {
    pub fn new(max_kbps: f64) -> Self {
        Self {
            max_kbps,
            cap_kbps: None,
            trouble_since: None,
            calm_since: None,
            settle_until: 0.0,
        }
    }

    pub fn cap_kbps(&self) -> Option<f64> {
        self.cap_kbps
    }

    /// Returns the new max bitrate (kbit/s) when it should change: a cap, or `max_kbps` when
    /// the cap is lifted.
    pub fn update(
        &mut self,
        t_s: f64,
        own: &NetSnapshot,
        viewer: Option<&NetSnapshot>,
    ) -> Option<f64> {
        let (send, recv) = (own.send.as_ref()?, viewer?.recv.as_ref()?);
        if t_s < self.settle_until {
            return None;
        }
        // We send more than arrives, and the rest is lost: the viewer's downlink overflows.
        let overflowing = recv.loss_pct >= CAP_LOSS_PCT && send.kbps > recv.kbps * 1.1;
        if overflowing {
            self.calm_since = None;
            let since = *self.trouble_since.get_or_insert(t_s);
            let current = self.cap_kbps.unwrap_or(self.max_kbps);
            let new = (recv.kbps * CAP_SHARE).max(CAP_MIN_KBPS);
            if t_s - since >= CAP_AFTER_S && new < current * 0.95 {
                return Some(self.set(t_s, Some(new)));
            }
            return None;
        }
        self.trouble_since = None;
        let cap = self.cap_kbps?;
        if recv.loss_pct >= 1.0 {
            self.calm_since = None;
            return None;
        }
        let since = *self.calm_since.get_or_insert(t_s);
        if t_s - since < CAP_RAISE_AFTER_S {
            return None;
        }
        let raised = cap * CAP_RAISE;
        Some(self.set(t_s, (raised < self.max_kbps).then_some(raised)))
    }

    fn set(&mut self, t_s: f64, cap: Option<f64>) -> f64 {
        self.cap_kbps = cap;
        self.trouble_since = None;
        self.calm_since = None;
        self.settle_until = t_s + CAP_SETTLE_S;
        cap.unwrap_or(self.max_kbps)
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

    fn viewer(loss_pct: f64, freezes: u32) -> NetSnapshot {
        NetSnapshot {
            recv: Some(RecvStats {
                loss_pct,
                freezes,
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn l(tier: Tier, fps: u32) -> Level {
        Level { tier, fps }
    }

    /// Runs one snapshot per second from `t0`, returning the changes with their times.
    fn run(
        a: &mut Adapter,
        t0: u32,
        secs: u32,
        own: &NetSnapshot,
        v: Option<&NetSnapshot>,
    ) -> Vec<(u32, Level)> {
        (t0..t0 + secs)
            .filter_map(|t| a.update(f64::from(t), own, v).map(|c| (t, c.to)))
            .collect()
    }

    #[test]
    fn ladder_drops_frame_rate_before_resolution() {
        assert_eq!(
            ladder(Tier::P1440, 60),
            vec![
                l(Tier::P1440, 60),
                l(Tier::P1440, 30),
                l(Tier::P1080, 30),
                l(Tier::P720, 30),
                l(Tier::P720, 15),
            ]
        );
        assert_eq!(
            ladder(Tier::P720, 30),
            vec![l(Tier::P720, 30), l(Tier::P720, 15)]
        );
        assert_eq!(ladder(Tier::P720, 15), vec![l(Tier::P720, 15)]);
    }

    #[test]
    fn steady_good_network_stays_put() {
        let mut a = Adapter::new(Tier::P1440, 60, 1.0);
        assert!(run(&mut a, 0, 120, &good(), None).is_empty());
        assert_eq!(a.level(), l(Tier::P1440, 60));
    }

    #[test]
    fn bandwidth_drop_halves_frame_rate_first_and_recovers() {
        let mut a = Adapter::new(Tier::P1440, 60, 1.0);
        // 1.2 Mbps is below 1440p 60's floor but enough for 1440p 30: text stays sharp.
        let squeezed = own(1200.0, 1300.0, 40.0, Limit::Bandwidth);
        assert_eq!(
            run(&mut a, 0, 10, &squeezed, None),
            vec![(3, l(Tier::P1440, 30))]
        );
        let ups = run(&mut a, 10, 15, &good(), None);
        assert_eq!(ups, vec![(20, l(Tier::P1440, 60))]);
    }

    #[test]
    fn severe_drop_jumps_to_a_level_that_fits() {
        let mut a = Adapter::new(Tier::P1440, 60, 1.0);
        // 0.35 Mbps: only 720p fits, at 30 fps (floor ~0.3).
        let starved = own(350.0, 400.0, 10.0, Limit::Bandwidth);
        let downs = run(&mut a, 0, 12, &starved, None);
        assert_eq!(
            downs[0],
            (1, l(Tier::P720, 30)),
            "severe: one jump after 1 s, not three steps"
        );
        // Still bandwidth-limited at a third of the frame rate: one more step.
        assert_eq!(downs[1..], [(5, l(Tier::P720, 15))]);
    }

    #[test]
    fn nothing_fits_goes_to_the_bottom_fast() {
        let mut a = Adapter::new(Tier::P1440, 60, 1.0);
        let starved = own(50.0, 60.0, 5.0, Limit::Bandwidth);
        // Under half the floor is severe: one second, not three.
        assert_eq!(
            run(&mut a, 0, 5, &starved, None),
            vec![(1, l(Tier::P720, 15))]
        );
    }

    #[test]
    fn other_trouble_steps_one_level() {
        // Viewer loss says nothing about how far to go: one level at a time.
        let mut a = Adapter::new(Tier::P1440, 60, 1.0);
        let changes = run(&mut a, 0, 4, &good(), Some(&viewer(9.0, 0)));
        assert_eq!(changes, vec![(3, l(Tier::P1440, 30))]);
    }

    #[test]
    fn short_blips_are_ignored() {
        let mut a = Adapter::new(Tier::P1080, 60, 1.0);
        let squeezed = own(500.0, 600.0, 20.0, Limit::Bandwidth);
        for t in 0..60 {
            let s = if t % 4 == 0 { &squeezed } else { &good() };
            assert_eq!(a.update(f64::from(t), s, None), None, "t={t}");
        }
    }

    #[test]
    fn cpu_limit_steps_down() {
        let mut a = Adapter::new(Tier::Native, 60, 1.0);
        let cpu = own(9000.0, 9000.0, 25.0, Limit::Cpu);
        assert_eq!(
            run(&mut a, 0, 5, &cpu, None),
            vec![(3, l(Tier::Native, 30))]
        );
    }

    #[test]
    fn viewer_distress_steps_down_even_with_good_uplink() {
        // The viewer's downlink is the bottleneck: our own estimate looks fine.
        let mut a = Adapter::new(Tier::P1440, 60, 1.0);
        let lossy = viewer(8.0, 0);
        assert_eq!(
            run(&mut a, 0, 8, &good(), Some(&lossy)),
            vec![(3, l(Tier::P1440, 30)), (7, l(Tier::P1080, 30))]
        );
    }

    #[test]
    fn repeated_freezes_count_even_without_duration() {
        let mut a = Adapter::new(Tier::P1080, 60, 1.0);
        let freezing = viewer(0.0, 2);
        assert_eq!(
            run(&mut a, 0, 5, &good(), Some(&freezing)),
            vec![(3, l(Tier::P1080, 30))]
        );
        // One freeze now and then (scrolling after a pause) is not distress.
        let mut a = Adapter::new(Tier::P1080, 60, 1.0);
        assert!(run(&mut a, 0, 30, &good(), Some(&viewer(0.0, 1))).is_empty());
    }

    #[test]
    fn does_not_step_up_while_viewer_struggles() {
        let mut a = Adapter::new(Tier::P1440, 60, 1.0);
        run(
            &mut a,
            0,
            4,
            &own(1200.0, 1300.0, 30.0, Limit::Bandwidth),
            None,
        );
        assert_eq!(a.level(), l(Tier::P1440, 30));
        assert!(run(&mut a, 4, 60, &good(), Some(&viewer(2.0, 0))).is_empty());
    }

    #[test]
    fn never_exceeds_requested_level() {
        let mut a = Adapter::new(Tier::P1080, 30, 1.0);
        assert!(run(&mut a, 0, 120, &good(), None).is_empty());
        assert_eq!(a.level(), l(Tier::P1080, 30));
    }

    #[test]
    fn flapping_backs_off() {
        let mut a = Adapter::new(Tier::P1440, 60, 1.0);
        let squeezed = own(1200.0, 1300.0, 30.0, Limit::Bandwidth);
        let (down, up) = (l(Tier::P1440, 30), l(Tier::P1440, 60));
        assert_eq!(run(&mut a, 0, 4, &squeezed, None), vec![(3, down)]);
        assert_eq!(run(&mut a, 4, 11, &good(), None), vec![(14, up)]);
        // Squeezed again soon after: down, and the next step up waits twice as long.
        assert_eq!(run(&mut a, 15, 4, &squeezed, None), vec![(18, down)]);
        assert_eq!(
            run(&mut a, 19, 30, &good(), None),
            vec![(39, up)],
            "20 s, not 10"
        );
    }

    #[test]
    fn vp8_needs_more_bitrate() {
        let mut h264 = Adapter::new(Tier::P1080, 60, 1.0);
        let mut vp8 = Adapter::new(Tier::P1080, 60, 2.0);
        let budget = own(1500.0, 1600.0, 50.0, Limit::None);
        assert!(run(&mut h264, 0, 10, &budget, None).is_empty());
        assert_eq!(
            run(&mut vp8, 0, 10, &budget, None),
            vec![(3, l(Tier::P1080, 30))]
        );
    }

    #[test]
    fn static_screen_low_fps_is_not_trouble() {
        // SCK sends few frames on a static screen; with no limit reported that's fine.
        let mut a = Adapter::new(Tier::P1440, 60, 1.0);
        let idle = own(8000.0, 9000.0, 4.0, Limit::None);
        assert!(run(&mut a, 0, 60, &idle, None).is_empty());
    }

    #[test]
    fn no_send_stats_no_decision() {
        let mut a = Adapter::new(Tier::P1440, 60, 1.0);
        assert_eq!(a.update(0.0, &NetSnapshot::default(), None), None);
    }

    #[test]
    fn level_floors_follow_tier_and_frame_rate() {
        let f = |t, fps| l(t, fps).floor_kbps(1.0);
        assert!(f(Tier::P1440, 30) < f(Tier::P1440, 60));
        assert!(f(Tier::P1080, 30) < f(Tier::P1440, 30));
        assert!(f(Tier::P720, 15) < f(Tier::P720, 30));
        assert_eq!(l(Tier::P1080, 30).to_string(), "1080p 30 fps");
    }

    fn flows(send_kbps: f64, recv_kbps: f64, loss_pct: f64) -> (NetSnapshot, NetSnapshot) {
        let own = NetSnapshot {
            send: Some(SendStats {
                kbps: send_kbps,
                ..Default::default()
            }),
            ..Default::default()
        };
        let viewer = NetSnapshot {
            recv: Some(RecvStats {
                kbps: recv_kbps,
                loss_pct,
                ..Default::default()
            }),
            ..Default::default()
        };
        (own, viewer)
    }

    fn cap_run(
        c: &mut BitrateCap,
        t0: u32,
        secs: u32,
        f: &(NetSnapshot, NetSnapshot),
    ) -> Vec<(u32, f64)> {
        (t0..t0 + secs)
            .filter_map(|t| c.update(f64::from(t), &f.0, Some(&f.1)).map(|k| (t, k)))
            .collect()
    }

    #[test]
    fn caps_to_viewer_goodput_when_its_downlink_overflows() {
        let mut c = BitrateCap::new(5000.0);
        // Sending 4.5 Mbps, 2.8 Mbps arrives, 40% lost: like the lab's hotel profile.
        let changes = cap_run(&mut c, 0, 6, &flows(4500.0, 2800.0, 40.0));
        assert_eq!(changes, vec![(4, 1400.0)]);
        assert_eq!(c.cap_kbps(), Some(1400.0));
    }

    #[test]
    fn loss_without_overflow_is_not_a_bandwidth_problem() {
        // Random loss on a link with room: everything we send arrives (minus the loss).
        let mut c = BitrateCap::new(5000.0);
        assert!(cap_run(&mut c, 0, 60, &flows(1000.0, 980.0, 6.0)).is_empty());
    }

    #[test]
    fn waits_for_the_republish_to_settle_then_caps_further() {
        let mut c = BitrateCap::new(5000.0);
        let bad = flows(4500.0, 2800.0, 40.0);
        assert_eq!(cap_run(&mut c, 0, 6, &bad), vec![(4, 1400.0)]);
        // Still overflowing at the new cap: nothing until settled (t=12), then 4 s more.
        let still = flows(1400.0, 1000.0, 30.0);
        assert_eq!(cap_run(&mut c, 6, 12, &still), vec![(16, 500.0)]);
    }

    #[test]
    fn lifts_the_cap_slowly_when_calm() {
        let mut c = BitrateCap::new(5000.0);
        cap_run(&mut c, 0, 6, &flows(4500.0, 2800.0, 40.0));
        let calm = flows(2000.0, 2000.0, 0.0);
        let ups: Vec<f64> = cap_run(&mut c, 6, 400, &calm)
            .into_iter()
            .map(|u| u.1)
            .collect();
        // ×1.25 per 30 calm seconds until a raise passes the max, which lifts the cap.
        assert_eq!(
            ups,
            vec![1750.0, 2187.5, 2734.375, 3417.96875, 4272.4609375, 5000.0]
        );
        assert_eq!(c.cap_kbps(), None);
    }

    #[test]
    fn never_caps_below_the_minimum() {
        let mut c = BitrateCap::new(5000.0);
        let changes = cap_run(&mut c, 0, 6, &flows(1000.0, 100.0, 80.0));
        assert_eq!(changes, vec![(4, CAP_MIN_KBPS)]);
    }

    #[test]
    fn no_viewer_stats_no_cap() {
        let mut c = BitrateCap::new(5000.0);
        let own = flows(4500.0, 0.0, 0.0).0;
        assert_eq!(c.update(10.0, &own, None), None);
    }
}
