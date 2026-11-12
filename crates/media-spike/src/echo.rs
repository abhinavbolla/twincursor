//! The arithmetic behind `tc-spike listen --echo` (`bench/audio-echo.sh`).
//!
//! Three sounds on one clock, as 10 ms levels: the far end's voice as it was sent, the raw mic
//! in the room where it plays, and that mic after echo cancellation, as the far end hears it
//! back. Each 10 ms is sorted by who should be making sound then:
//!
//! - the far end only: what comes back is echo, so the drop from the raw mic to the returned
//!   track is what cancellation removed;
//! - nobody (pauses in the far end's speech): the raw mic shows whether the room was quiet.
//!   Someone talking nearby is passed on, as it should be, and holds the canceller back, so a
//!   run in a room that wasn't quiet can't be judged;
//! - a local voice only (played out loud by the script, at times it tells us): it has to come
//!   through;
//! - both: reported, not judged.
//!
//! Levels are compared at the 95th percentile, the loud moments, so stray silent frames and
//! clocks that disagree by a tenth of a second don't matter.

use media::audio::marker::FRAME;
use serde_json::{json, Value};

const SLOT_US: u64 = 10_000;
const SLOTS_PER_S: f64 = 100.0;
/// An hour: a time stamp further out than this is a broken clock.
const MAX_SLOTS: usize = 360_000;

/// The far end counts as talking above this level, as sent.
const FAR_TALKING_DB: f32 = -45.0;
/// After the far end stops, its sound can still be on its way to the speakers, ringing in the
/// room, or on its way back for this long...
const TAIL_SLOTS: usize = 100;
/// ...and the three clocks can disagree by this much the other way.
const LEAD_SLOTS: usize = 20;
/// The canceller gets this long after the far end's first word before it's judged.
const SETTLE_S: f64 = 5.0;
/// The local voice is measured this far inside the times we were told, and nothing else is
/// measured from `LOCAL_BEFORE_S` before to `LOCAL_AFTER_S` after: a player takes a moment to
/// start.
const LOCAL_INSIDE_S: f64 = 0.7;
const LOCAL_BEFORE_S: f64 = 0.5;
const LOCAL_AFTER_S: f64 = 1.0;

/// The mic has to hear the speakers at least this loud for the run to mean anything.
const MIN_HEARD_DB: f64 = -40.0;
/// The room wasn't quiet if the raw mic is louder than this in the far end's pauses. Quiet
/// rooms measured −65 to −51 dBFS on a MacBook's mic; a voice across the desk is about −35.
const ROOM_QUIET_DB: f64 = -45.0;
/// The echo must come back at least this much quieter than the mic heard it. Measured on a
/// MacBook's own speakers and mic at half and full volume: 42–60 dB.
pub const MIN_REMOVED_DB: f64 = 30.0;
/// The local voice may come back at most this much quieter than the mic heard it. Gain control
/// usually makes it louder.
const MAX_VOICE_LOSS_DB: f64 = 10.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stream {
    /// The far end's voice, as sent.
    Far,
    /// The raw mic in the room.
    Mic,
    /// The mic after echo cancellation, as the far end hears it.
    Returned,
}

/// 10 ms levels of the three sounds, on the wall clock.
pub struct Levels {
    origin_us: u64,
    /// dBFS for each slot; NaN where nothing arrived.
    tracks: [Vec<f32>; 3],
}

fn rms_db(samples: &[i16]) -> f32 {
    let sum: f64 = samples.iter().map(|&s| f64::from(s).powi(2)).sum();
    let rms = (sum / samples.len().max(1) as f64).sqrt() / 32768.0;
    // Digital silence would be −∞.
    (20.0 * rms.log10()).max(-100.0) as f32
}

impl Levels {
    /// Slot 0 starts at `origin_us` on the wall clock; earlier sound is ignored.
    pub fn new(origin_us: u64) -> Self {
        Self {
            origin_us,
            tracks: [Vec::new(), Vec::new(), Vec::new()],
        }
    }

    /// Adds samples whose first was captured, or arrived, at `wall_us`.
    pub fn push(&mut self, stream: Stream, wall_us: u64, samples: &[i16]) {
        let track = &mut self.tracks[stream as usize];
        for (k, frame) in samples.chunks(FRAME).enumerate() {
            let at = wall_us + k as u64 * SLOT_US;
            let Some(slot) = at
                .checked_sub(self.origin_us)
                .map(|d| (d / SLOT_US) as usize)
            else {
                continue;
            };
            if slot >= MAX_SLOTS {
                continue;
            }
            if track.len() <= slot {
                track.resize(slot + 1, f32::NAN);
            }
            // Two frames can land in one slot when they arrive in a bunch; keep the louder.
            track[slot] = rms_db(frame).max(track[slot]);
        }
    }

    fn track(&self, stream: Stream) -> &[f32] {
        &self.tracks[stream as usize]
    }

    fn at(&self, stream: Stream, slot: usize) -> Option<f32> {
        self.track(stream)
            .get(slot)
            .copied()
            .filter(|db| !db.is_nan())
    }
}

/// The raw mic and the returned track over the slots of one kind.
#[derive(Default)]
struct Stretch {
    mic: Vec<f32>,
    returned: Vec<f32>,
}

fn percentile(levels: &[f32], q: f64) -> Option<f64> {
    let mut sorted = levels.to_vec();
    sorted.sort_by(f32::total_cmp);
    sorted
        .get(((sorted.len() as f64 - 1.0) * q).round() as usize)
        .map(|&db| f64::from(db))
}

impl Stretch {
    fn seconds(&self) -> f64 {
        self.mic.len() as f64 / SLOTS_PER_S
    }

    fn mic_p95(&self) -> Option<f64> {
        percentile(&self.mic, 0.95)
    }

    fn returned_p95(&self) -> Option<f64> {
        percentile(&self.returned, 0.95)
    }

    /// How much quieter the loud moments came back than the mic heard them.
    fn removed(&self) -> Option<f64> {
        Some(self.mic_p95()? - self.returned_p95()?)
    }

    fn json(&self) -> Value {
        json!({
            "seconds": self.seconds(),
            "mic_p50_dbfs": percentile(&self.mic, 0.5),
            "mic_p95_dbfs": self.mic_p95(),
            "returned_p50_dbfs": percentile(&self.returned, 0.5),
            "returned_p95_dbfs": self.returned_p95(),
            "returned_loudest_dbfs": percentile(&self.returned, 1.0),
        })
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    Pass,
    Fail,
    /// A check failed, but the room wasn't quiet, which can cause exactly that.
    Inconclusive,
}

pub struct Report {
    /// When the far end first talked, in seconds after `Levels`' origin.
    far_started_s: Option<f64>,
    /// The far end alone, in its first [`SETTLE_S`] and after.
    settling: Stretch,
    settled: Stretch,
    /// Nobody.
    quiet: Stretch,
    /// The local voice alone, and with the far end.
    local: Stretch,
    both: Stretch,
    returned_any: bool,
    expects_local: bool,
}

/// Sorts the levels by who was making sound. `local` holds the times the local voice played,
/// in seconds after the far end's first word.
pub fn analyse(levels: &Levels, local: &[(f64, f64)]) -> Report {
    let far = levels.track(Stream::Far);
    let slots = levels
        .track(Stream::Mic)
        .len()
        .max(levels.track(Stream::Returned).len());
    let mut report = Report {
        far_started_s: None,
        settling: Stretch::default(),
        settled: Stretch::default(),
        quiet: Stretch::default(),
        local: Stretch::default(),
        both: Stretch::default(),
        returned_any: levels.track(Stream::Returned).iter().any(|db| !db.is_nan()),
        expects_local: !local.is_empty(),
    };
    let Some(start) = far.iter().position(|&db| db > FAR_TALKING_DB) else {
        return report;
    };
    report.far_started_s = Some(start as f64 / SLOTS_PER_S);

    // Where the far end's sound may be in the room or on its way back.
    let mut far_around = vec![false; slots];
    for (slot, _) in far
        .iter()
        .enumerate()
        .filter(|(_, &db)| db > FAR_TALKING_DB)
    {
        let from = slot.saturating_sub(LEAD_SLOTS).min(slots);
        let to = (slot + TAIL_SLOTS + 1).min(slots);
        far_around[from..to].fill(true);
    }

    for (slot, &far_around) in far_around.iter().enumerate() {
        let t = (slot as f64 - start as f64) / SLOTS_PER_S;
        let within = |before: f64, after: f64| {
            local
                .iter()
                .any(|&(from, to)| t >= from - before && t < to + after)
        };
        let stretch = if within(-LOCAL_INSIDE_S, -LOCAL_INSIDE_S) {
            if far_around {
                &mut report.both
            } else {
                &mut report.local
            }
        } else if within(LOCAL_BEFORE_S, LOCAL_AFTER_S) {
            continue;
        } else if !far_around {
            &mut report.quiet
        } else if t < SETTLE_S {
            &mut report.settling
        } else {
            &mut report.settled
        };
        stretch.mic.extend(levels.at(Stream::Mic, slot));
        stretch.returned.extend(levels.at(Stream::Returned, slot));
    }
    report
}

impl Report {
    /// Whether the room was quiet in the far end's pauses; `None` with under a second of them.
    fn room_quiet(&self) -> Option<bool> {
        (self.quiet.seconds() >= 1.0)
            .then(|| self.quiet.mic_p95())
            .flatten()
            .map(|db| db <= ROOM_QUIET_DB)
    }

    /// The verdict, and what's wrong in words.
    pub fn verdict(&self) -> (Verdict, Vec<String>) {
        // Without these there's nothing to judge, whatever the room was like.
        if self.far_started_s.is_none() {
            return (
                Verdict::Fail,
                vec!["never received the far end's voice".into()],
            );
        }
        if !self.returned_any {
            let problem = "never received the mic track that goes back to the far end";
            return (Verdict::Fail, vec![problem.into()]);
        }
        let mut problems = Vec::new();
        match (self.settled.mic_p95(), self.settled.removed()) {
            (Some(heard), Some(removed)) if self.settled.seconds() >= 3.0 => {
                if heard < MIN_HEARD_DB {
                    problems.push(format!(
                        "the mic barely heard the far end ({heard:.1} dBFS): turn the volume up"
                    ));
                } else if removed < MIN_REMOVED_DB {
                    problems.push(format!(
                        "the echo came back only {removed:.1} dB quieter (at least \
                         {MIN_REMOVED_DB} dB expected)"
                    ));
                }
            }
            _ => problems.push(format!(
                "the far end talked for under 3 s past its first {SETTLE_S} s"
            )),
        }
        if self.expects_local {
            match (self.local.mic_p95(), self.local.returned_p95()) {
                (Some(heard), Some(returned)) if self.local.seconds() >= 1.0 => {
                    if heard < MIN_HEARD_DB {
                        problems.push(format!(
                            "the mic barely heard the local voice ({heard:.1} dBFS): turn the \
                             volume up"
                        ));
                    } else if returned < heard - MAX_VOICE_LOSS_DB {
                        problems.push(format!(
                            "the local voice came back {:.1} dB quieter than the mic heard it",
                            heard - returned
                        ));
                    }
                }
                _ => problems.push("the local voice never played on its own".into()),
            }
        }
        let noisy = self.room_quiet() == Some(false);
        if noisy {
            problems.push(format!(
                "the room wasn't quiet: {:.1} dBFS in the far end's pauses (a quiet room stays \
                 under {ROOM_QUIET_DB} dBFS)",
                self.quiet.mic_p95().unwrap_or(f64::NAN)
            ));
        }
        let verdict = match (problems.len(), noisy) {
            (0, _) | (1, true) => Verdict::Pass,
            (_, true) => Verdict::Inconclusive,
            (_, false) => Verdict::Fail,
        };
        (verdict, problems)
    }

    pub fn json(&self) -> Value {
        let (verdict, problems) = self.verdict();
        json!({
            "verdict": match verdict {
                Verdict::Pass => "pass",
                Verdict::Fail => "fail",
                Verdict::Inconclusive => "inconclusive",
            },
            "problems": problems,
            "room_quiet": self.room_quiet(),
            "far_started_s": self.far_started_s,
            "removed_settling_db": self.settling.removed(),
            "removed_db": self.settled.removed(),
            // Above zero: the local voice came back louder than the mic heard it.
            "local_voice_db": self.local.removed().map(|db| -db),
            // The returned track while both talk, against the local voice alone.
            "double_talk_db": self
                .both
                .returned_p95()
                .zip(self.local.returned_p95())
                .map(|(both, alone)| both - alone),
            "settling": self.settling.json(),
            "settled": self.settled.json(),
            "quiet": self.quiet.json(),
            "local_voice": self.local.json(),
            "double_talk": self.both.json(),
        })
    }
}

/// `FROM:TO` in seconds, for `--local-voice` and `--mute-for`.
pub fn parse_window(text: &str) -> Result<(f64, f64), String> {
    let parsed = text
        .split_once(':')
        .and_then(|(from, to)| Some((from.parse::<f64>().ok()?, to.parse::<f64>().ok()?)));
    match parsed {
        Some((from, to)) if from >= 0.0 && to > from => Ok((from, to)),
        _ => Err("expected FROM:TO in seconds, like 17.5:24".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: u64 = 1_700_000_000_000_000;

    /// A scripted run: who makes sound when, in seconds after the levels' origin.
    struct Scene {
        levels: Levels,
    }

    /// 10 ms of a tone at `db` dBFS (rms).
    fn tone(db: f64) -> Vec<i16> {
        let peak = 32768.0 * 10f64.powf(db / 20.0) * std::f64::consts::SQRT_2;
        (0..FRAME)
            .map(|i| {
                (peak * (std::f64::consts::TAU * 1000.0 * i as f64 / 48_000.0).sin()).round() as i16
            })
            .collect()
    }

    impl Scene {
        fn new() -> Self {
            Self {
                levels: Levels::new(T0),
            }
        }

        fn play(&mut self, stream: Stream, from_s: f64, to_s: f64, db: f64) -> &mut Self {
            let frame = tone(db);
            for slot in (from_s * 100.0) as u64..(to_s * 100.0) as u64 {
                self.levels.push(stream, T0 + slot * SLOT_US, &frame);
            }
            self
        }

        /// The usual run, up to what the canceller and the room do: the far end talks in
        /// sentences with pauses from 2 s on, stops for the local voice (18–24 s), and talks
        /// again from 26 s while the local voice plays a second time (28–34 s). The mic is 200 ms
        /// behind the far end; the returned track is silent unless a test adds to it.
        fn usual() -> Self {
            let mut scene = Self::new();
            scene
                .play(Stream::Mic, 0.0, 40.0, -68.0)
                .play(Stream::Returned, 1.0, 40.0, -70.0);
            for (from, to) in [(2.0, 6.0), (8.0, 12.0), (14.0, 16.0), (26.0, 38.0)] {
                scene.play(Stream::Far, from, to, -20.0).play(
                    Stream::Mic,
                    from + 0.2,
                    to + 0.2,
                    -18.0,
                );
            }
            // The local voice, as the mic hears it; the far end starts at 2 s.
            scene
                .play(Stream::Mic, 18.2, 24.2, -22.0)
                .play(Stream::Mic, 28.2, 34.2, -17.0);
            scene
        }

        fn report(&self) -> Report {
            analyse(&self.levels, &[(16.0, 22.0), (26.0, 32.0)])
        }
    }

    fn close(value: Option<f64>, want: f64) -> bool {
        value.is_some_and(|v| (v - want).abs() < 0.5)
    }

    #[test]
    fn a_clean_run_passes_with_its_numbers() {
        let mut scene = Scene::usual();
        // The local voice comes back a little louder, alone and with the far end.
        scene
            .play(Stream::Returned, 18.3, 24.3, -16.0)
            .play(Stream::Returned, 28.3, 34.3, -22.0);
        let report = scene.report();
        assert_eq!(report.verdict(), (Verdict::Pass, vec![]));
        assert_eq!(report.far_started_s, Some(2.0));
        // −18 dBFS at the mic, −70 back: 52 dB, before and after settling.
        assert!(close(report.settled.removed(), 52.0));
        assert!(close(report.settling.removed(), 52.0));
        assert_eq!(report.room_quiet(), Some(true));
        let json = report.json();
        assert_eq!(json["verdict"], "pass");
        assert!(close(json["local_voice_db"].as_f64(), 6.0), "{json}");
        assert!(close(json["double_talk_db"].as_f64(), -6.0), "{json}");
        // The local voice alone is measured inside its times only: 6 s less 0.7 s at each end.
        assert!((report.local.seconds() - 4.6).abs() < 0.05);
    }

    #[test]
    fn echo_that_comes_back_fails() {
        let mut scene = Scene::usual();
        scene
            .play(Stream::Returned, 18.3, 24.3, -16.0)
            // From 8 s on the far end hears itself 20 dB down.
            .play(Stream::Returned, 8.3, 12.3, -38.0);
        let (verdict, problems) = scene.report().verdict();
        assert_eq!(verdict, Verdict::Fail);
        assert!(problems[0].contains("only 20.0 dB quieter"), "{problems:?}");
    }

    #[test]
    fn echo_while_settling_is_not_judged() {
        let mut scene = Scene::usual();
        scene
            .play(Stream::Returned, 18.3, 24.3, -16.0)
            // The first sentence comes back; the canceller hasn't heard the room yet.
            .play(Stream::Returned, 2.3, 6.3, -30.0);
        let report = scene.report();
        assert_eq!(report.verdict().0, Verdict::Pass);
        assert!(close(report.settling.removed(), 12.0));
    }

    #[test]
    fn a_voice_in_the_room_makes_a_failure_inconclusive() {
        let mut scene = Scene::usual();
        scene
            .play(Stream::Returned, 18.3, 24.3, -16.0)
            // Someone talks from 5 to 15 s, and is passed on.
            .play(Stream::Mic, 5.0, 15.0, -36.0)
            .play(Stream::Returned, 5.1, 15.1, -24.0);
        let report = scene.report();
        assert_eq!(report.room_quiet(), Some(false));
        let (verdict, problems) = report.verdict();
        assert_eq!(verdict, Verdict::Inconclusive);
        assert!(problems.iter().any(|p| p.contains("the room wasn't quiet")));
        assert_eq!(report.json()["verdict"], "inconclusive");
    }

    #[test]
    fn a_noisy_room_that_passes_anyway_still_passes() {
        let mut scene = Scene::usual();
        scene
            .play(Stream::Returned, 18.3, 24.3, -16.0)
            // A noise the canceller's noise suppression keeps out of the returned track.
            .play(Stream::Mic, 5.0, 15.0, -45.0);
        let (verdict, problems) = scene.report().verdict();
        assert_eq!(verdict, Verdict::Pass);
        assert_eq!(problems.len(), 1, "{problems:?}");
    }

    #[test]
    fn a_local_voice_that_is_cut_fails() {
        // Nothing of the local voice comes back.
        let (verdict, problems) = Scene::usual().report().verdict();
        assert_eq!(verdict, Verdict::Fail);
        assert!(
            problems[0].contains("the local voice came back"),
            "{problems:?}"
        );
    }

    #[test]
    fn speakers_too_quiet_is_its_own_failure() {
        let mut scene = Scene::new();
        scene
            .play(Stream::Mic, 0.0, 20.0, -68.0)
            .play(Stream::Returned, 0.0, 20.0, -85.0)
            .play(Stream::Far, 2.0, 18.0, -20.0)
            .play(Stream::Mic, 2.2, 18.2, -48.0);
        let (verdict, problems) = analyse(&scene.levels, &[]).verdict();
        assert_eq!(verdict, Verdict::Fail);
        assert!(problems[0].contains("turn the volume up"), "{problems:?}");
    }

    #[test]
    fn missing_tracks_fail_plainly() {
        let mut scene = Scene::new();
        scene.play(Stream::Mic, 0.0, 10.0, -60.0);
        let (verdict, problems) = analyse(&scene.levels, &[]).verdict();
        assert_eq!(verdict, Verdict::Fail);
        assert!(problems[0].contains("far end's voice"), "{problems:?}");
        scene.play(Stream::Far, 1.0, 9.0, -20.0);
        let (verdict, problems) = analyse(&scene.levels, &[]).verdict();
        assert_eq!(verdict, Verdict::Fail);
        assert!(problems[0].contains("goes back"), "{problems:?}");
    }

    #[test]
    fn levels_land_on_the_wall_clock() {
        let mut levels = Levels::new(T0);
        // Before the origin: ignored. A chunk of two and a half frames, 30 ms in.
        levels.push(Stream::Mic, T0 - 50_000, &tone(-10.0));
        let chunk: Vec<i16> =
            [tone(-20.0), tone(-30.0), tone(-40.0)[..FRAME / 2].to_vec()].concat();
        levels.push(Stream::Mic, T0 + 30_000, &chunk);
        assert_eq!(levels.at(Stream::Mic, 0), None);
        assert_eq!(levels.at(Stream::Mic, 2), None);
        assert!((levels.at(Stream::Mic, 3).unwrap() + 20.0).abs() < 0.2);
        assert!((levels.at(Stream::Mic, 4).unwrap() + 30.0).abs() < 0.2);
        assert!(levels.at(Stream::Mic, 5).is_some());
        // A second frame in the same slot: the louder one counts.
        levels.push(Stream::Mic, T0 + 30_500, &tone(-12.0));
        assert!((levels.at(Stream::Mic, 3).unwrap() + 12.0).abs() < 0.2);
        // Silence has a level too, and other streams are separate.
        levels.push(Stream::Far, T0, &[0; FRAME]);
        assert_eq!(levels.at(Stream::Far, 0), Some(-100.0));
        assert_eq!(levels.at(Stream::Returned, 0), None);
    }

    #[test]
    fn windows_parse() {
        assert_eq!(parse_window("17.5:24"), Ok((17.5, 24.0)));
        for bad in ["17.5", "24:17", "a:b", "-1:3", "3:3"] {
            assert!(parse_window(bad).is_err(), "{bad}");
        }
    }
}
