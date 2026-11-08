//! In-band latency markers for audio, the counterpart of the luma blocks in [`crate::marker`].
//!
//! Audio frames carry no metadata the viewer can read, so the tick id goes into the sound
//! itself. A burst is a 20 ms sync tone and then six 15 ms tones: five carry the 20-bit tick
//! id, four bits each, and one is a check. Tones are between 1 and 4.2 kHz, which Opus passes at
//! speech bitrates. Between bursts a quiet 200 Hz tone keeps the stream from being silence, so
//! the codec and the receiver's concealment behave as they do with a voice.
//!
//! The receiver's jitter buffer stretches and squeezes audio to change its delay, so tones
//! don't arrive on a fixed grid. The detector therefore doesn't time the symbols: it follows
//! which tone is playing, 1 ms at a time, and reads each change of tone as the next symbol. Each
//! symbol is coded relative to the one before (17 tones, never the same twice in a row), so
//! every symbol is a change, even when two nibbles are equal.

use std::f64::consts::TAU;

/// Samples per second. Mono throughout.
pub const RATE: u32 = 48_000;
/// One audio frame: 10 ms.
pub const FRAME: usize = RATE as usize / 100;

const SYNC_HZ: f64 = 600.0;
const SYNC_LEN: usize = 960;
const SYMBOL_LEN: usize = 720;
const SYMBOLS: usize = 6;
const TONES: u8 = 17;
/// Sync plus symbols: 110 ms.
pub const BURST_LEN: usize = SYNC_LEN + SYMBOLS * SYMBOL_LEN;
/// Fade in and out over 2 ms, so a burst doesn't click.
const RAMP: usize = 96;
const LEVEL: f64 = 0.3 * i16::MAX as f64;
const BED_HZ: f64 = 200.0;
const BED_LEVEL: f64 = 0.02 * i16::MAX as f64;

/// The detector's window: 5 ms, in which every tone here is a whole number of cycles, so the
/// filters don't see each other's tones. It moves 1 ms at a time.
const WINDOW: usize = 240;
const HOP: usize = 48;
/// A tone must hold more than this share of a window's energy to count as playing.
const SHARE: f64 = 0.5;
/// Ignore windows quieter than this (RMS, in i16 units): silence and noise floors.
const MIN_RMS: f64 = 300.0;
/// Shortest sync and symbol the detector accepts, in hops. Unstretched they last about 18
/// and 13; this allows squeezing to under half.
const MIN_SYNC_HOPS: u32 = 8;
const MIN_SYMBOL_HOPS: u32 = 3;
/// A tone that stops for longer than this has ended (brief dropouts don't split it).
const GAP_HOPS: u64 = 4;
/// Give up on a burst that hasn't finished this long after its sync: over twice its length.
const MAX_BURST: u64 = 250 * HOP as u64;
/// Label for the sync tone; symbol tones are 0..17.
const SYNC: u8 = TONES;

fn tone_hz(label: u8) -> f64 {
    if label == SYNC {
        SYNC_HZ
    } else {
        1000.0 + 200.0 * f64::from(label)
    }
}

/// The tick id's five nibbles, most significant first, and a check nibble. Odd weights make
/// any single wrong nibble change the check.
fn nibbles(id: u32) -> [u8; SYMBOLS] {
    let mut out = [0; SYMBOLS];
    for (k, n) in out.iter_mut().take(5).enumerate() {
        *n = ((id >> (16 - 4 * k)) & 0xf) as u8;
    }
    out[5] = check(&out[..5]);
    out
}

fn check(nibbles: &[u8]) -> u8 {
    let sum: u32 = nibbles
        .iter()
        .zip([1u32, 3, 5, 7, 9])
        .map(|(&n, w)| u32::from(n) * w)
        .sum();
    (sum % 16) as u8
}

/// Tones for an id: each is 1 + nibble steps past the one before, out of 17.
fn encode(id: u32) -> [u8; SYMBOLS] {
    let n = nibbles(id);
    let mut tones = [n[0]; SYMBOLS];
    for k in 1..SYMBOLS {
        tones[k] = (tones[k - 1] + 1 + n[k]) % TONES;
    }
    tones
}

fn decode(tones: &[u8]) -> Option<u32> {
    let mut n = [0u8; SYMBOLS];
    n[0] = tones[0];
    for k in 1..SYMBOLS {
        n[k] = (tones[k] + 2 * TONES - tones[k - 1] - 1) % TONES;
    }
    if n.iter().any(|&x| x > 15) || check(&n[..5]) != n[5] {
        return None;
    }
    Some(n[..5].iter().fold(0, |id, &x| (id << 4) | u32::from(x)))
}

/// Makes the marker audio: the quiet bed, and a burst whenever [`Generator::start`] asks.
#[derive(Debug, Default)]
pub struct Generator {
    t: u64,
    phase: f64,
    burst: Option<([u8; SYMBOLS], usize)>,
}

impl Generator {
    /// Whether a burst is still playing.
    pub fn busy(&self) -> bool {
        self.burst.is_some()
    }

    /// Starts a burst for `id` at the next sample. Returns `false` while one is playing.
    pub fn start(&mut self, id: u32) -> bool {
        if self.busy() {
            return false;
        }
        self.burst = Some((encode(id), 0));
        self.phase = 0.0;
        true
    }

    pub fn fill(&mut self, out: &mut [i16]) {
        for sample in out {
            let bed = BED_LEVEL * (TAU * BED_HZ * self.t as f64 / f64::from(RATE)).sin();
            self.t += 1;
            let mut tone = 0.0;
            if let Some((tones, pos)) = self.burst.as_mut() {
                let label = if *pos < SYNC_LEN {
                    SYNC
                } else {
                    tones[(*pos - SYNC_LEN) / SYMBOL_LEN]
                };
                // Continuous phase: changing frequency mid-wave doesn't splatter.
                self.phase = (self.phase + TAU * tone_hz(label) / f64::from(RATE)) % TAU;
                let edge = (*pos).min(BURST_LEN - 1 - *pos);
                let fade = (edge as f64 / RAMP as f64).min(1.0);
                tone = LEVEL * fade * self.phase.sin();
                *pos += 1;
                if *pos == BURST_LEN {
                    self.burst = None;
                }
            }
            *sample = (bed + tone).round() as i16;
        }
    }
}

/// A marker the detector heard: the tick id and the sample where its burst begins, counted
/// from the first sample given to the detector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Heard {
    pub id: u32,
    pub at: u64,
}

/// One tone playing without a break: its label, how many hops, and the window ends of its
/// first and last hop.
#[derive(Debug, Clone, Copy)]
struct Run {
    label: u8,
    hops: u32,
    first: u64,
    last: u64,
}

/// A sync heard, waiting for its symbols.
#[derive(Debug)]
struct Pending {
    start: u64,
    tones: Vec<u8>,
}

/// Finds markers in received audio. Feed it samples in any chunk size.
#[derive(Debug)]
pub struct Detector {
    /// The samples of the next window, and maybe a few after it.
    buf: Vec<f64>,
    /// Sample number of `buf[0]`.
    base: u64,
    /// End of the next window.
    end: u64,
    run: Option<Run>,
    pending: Option<Pending>,
    /// Syncs whose symbols didn't decode: a damaged burst, or something that only sounded
    /// like a sync.
    pub rejected: u64,
}

impl Default for Detector {
    fn default() -> Self {
        Self {
            buf: Vec::new(),
            base: 0,
            end: WINDOW as u64,
            run: None,
            pending: None,
            rejected: 0,
        }
    }
}

impl Detector {
    pub fn push(&mut self, samples: &[i16]) -> Vec<Heard> {
        self.buf.extend(samples.iter().map(|&s| f64::from(s)));
        let mut heard = Vec::new();
        while self.end <= self.base + self.buf.len() as u64 {
            let from = (self.end - WINDOW as u64 - self.base) as usize;
            let label = label(&self.buf[from..from + WINDOW]);
            self.step(label, &mut heard);
            self.end += HOP as u64;
        }
        let keep_from = self.end - WINDOW as u64;
        let drop = ((keep_from - self.base) as usize).min(self.buf.len());
        self.buf.drain(..drop);
        self.base += drop as u64;
        heard
    }

    fn step(&mut self, label: Option<u8>, heard: &mut Vec<Heard>) {
        let end = self.end;
        match (label, self.run.as_mut()) {
            (Some(l), Some(run)) if run.label == l => {
                run.hops += 1;
                run.last = end;
            }
            (Some(l), _) => {
                if let Some(run) = self.run.take() {
                    self.finish(run, heard);
                }
                self.run = Some(Run {
                    label: l,
                    hops: 1,
                    first: end,
                    last: end,
                });
            }
            (None, Some(run)) if end - run.last > GAP_HOPS * HOP as u64 => {
                let run = *run;
                self.run = None;
                self.finish(run, heard);
            }
            (None, _) => {}
        }
        if self
            .pending
            .as_ref()
            .is_some_and(|p| end > p.start + MAX_BURST)
        {
            self.pending = None;
            self.rejected += 1;
        }
    }

    fn finish(&mut self, run: Run, heard: &mut Vec<Heard>) {
        if run.label == SYNC {
            if run.hops < MIN_SYNC_HOPS {
                return;
            }
            match &self.pending {
                // A sync broken by a dropout: keep the start of its first part.
                Some(p) if p.tones.is_empty() => {}
                Some(_) => {
                    self.rejected += 1;
                    self.pending = Some(Pending::new(run.first));
                }
                None => self.pending = Some(Pending::new(run.first)),
            }
            return;
        }
        if run.hops < MIN_SYMBOL_HOPS {
            return;
        }
        let Some(pending) = self.pending.as_mut() else {
            return;
        };
        // Tones never repeat, so the same one again is a symbol a glitch split in two.
        if pending.tones.last() == Some(&run.label) {
            return;
        }
        pending.tones.push(run.label);
        if pending.tones.len() == SYMBOLS {
            let pending = self.pending.take().expect("pending");
            match decode(&pending.tones) {
                Some(id) => heard.push(Heard {
                    id,
                    at: pending.start,
                }),
                None => self.rejected += 1,
            }
        }
    }
}

impl Pending {
    /// `first` is the end of the first window in which the sync tone held the majority. With
    /// little else playing, that's when about half the window has the tone in it.
    fn new(first: u64) -> Self {
        Self {
            start: first.saturating_sub((WINDOW / 2 + HOP / 2) as u64),
            tones: Vec::with_capacity(SYMBOLS),
        }
    }
}

/// Which tone a window holds, if any holds most of it.
fn label(window: &[f64]) -> Option<u8> {
    let energy: f64 = window.iter().map(|x| x * x).sum();
    if (energy / window.len() as f64).sqrt() <= MIN_RMS {
        return None;
    }
    (0..=SYNC)
        .map(|l| (l, power(window, tone_hz(l))))
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .filter(|&(_, p)| 2.0 * p / (window.len() as f64 * energy) > SHARE)
        .map(|(l, _)| l)
}

/// Goertzel power of one frequency over the window: for a pure tone of amplitude A filling it,
/// (A·N/2)².
fn power(window: &[f64], hz: f64) -> f64 {
    let coeff = 2.0 * (TAU * hz / f64::from(RATE)).cos();
    let (mut s1, mut s2) = (0.0, 0.0);
    for &x in window {
        let s = x + coeff * s1 - s2;
        s2 = s1;
        s1 = s;
    }
    s1 * s1 + s2 * s2 - coeff * s1 * s2
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `lead` samples of bed, then a burst for each id, `gap` samples apart, then a frame of
    /// bed so the last burst is followed by something.
    fn render(ids: &[u32], lead: usize, gap: usize) -> (Vec<i16>, Vec<u64>) {
        let mut g = Generator::default();
        let mut out = vec![0; lead];
        g.fill(&mut out);
        let mut starts = Vec::new();
        for &id in ids {
            starts.push(out.len() as u64);
            assert!(g.start(id));
            let mut burst = vec![0; BURST_LEN + gap];
            g.fill(&mut burst);
            out.extend(burst);
        }
        let mut tail = vec![0; FRAME];
        g.fill(&mut tail);
        out.extend(tail);
        (out, starts)
    }

    fn detect(samples: &[i16], chunk: usize) -> (Vec<Heard>, u64) {
        let mut d = Detector::default();
        let heard = samples.chunks(chunk).flat_map(|c| d.push(c)).collect();
        (heard, d.rejected)
    }

    fn ids(heard: &[Heard]) -> Vec<u32> {
        heard.iter().map(|h| h.id).collect()
    }

    fn assert_starts(heard: &[Heard], starts: &[u64], within: u64) {
        for (h, &start) in heard.iter().zip(starts) {
            let error = h.at.abs_diff(start);
            assert!(
                error <= within,
                "{}: off by {error} samples (1 ms = 48)",
                h.id
            );
        }
    }

    /// Deterministic noise, in i16 units.
    fn noise(len: usize, level: f64, mut seed: u32) -> Vec<f64> {
        (0..len)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                (f64::from(seed) / f64::from(u32::MAX) * 2.0 - 1.0) * level
            })
            .collect()
    }

    #[test]
    fn codes_round_trip_and_never_repeat_a_tone() {
        for id in [0, 1, 2, 0xabcde, 0xfffff, 0x11111, 12345] {
            let tones = encode(id);
            assert_eq!(decode(&tones), Some(id));
            assert!(tones.windows(2).all(|w| w[0] != w[1]), "{id:#x}: {tones:?}");
            assert!(tones.iter().all(|&t| t < TONES));
        }
        // Any single wrong nibble fails the check.
        let n = nibbles(0x12345);
        for k in 0..5 {
            for wrong in (0..16u8).filter(|&w| w != n[k]) {
                let mut bad = n;
                bad[k] = wrong;
                assert_ne!(check(&bad[..5]), n[5], "nibble {k} → {wrong}");
            }
        }
        // So does a wrong tone.
        let mut tones = encode(0x12345);
        tones[2] = (tones[2] + 5) % TONES;
        assert_eq!(decode(&tones), None);
    }

    #[test]
    fn hears_every_burst_where_it_starts() {
        let ids_sent = [1, 2, 0xfffff, 0x80001, 77, 0];
        let (audio, starts) = render(&ids_sent, 1234, 6000);
        for chunk in [FRAME, 441, 4096] {
            let (heard, rejected) = detect(&audio, chunk);
            assert_eq!(rejected, 0, "chunk {chunk}");
            assert_eq!(ids(&heard), ids_sent, "chunk {chunk}");
            assert_starts(&heard, &starts, 48);
        }
    }

    #[test]
    fn bursts_back_to_back() {
        let (audio, starts) = render(&[10, 11, 12], 480, 0);
        let (heard, rejected) = detect(&audio, FRAME);
        assert_eq!((ids(&heard), rejected), (vec![10, 11, 12], 0));
        assert_starts(&heard, &starts, 48);
    }

    /// What the receiver's jitter buffer does when it changes its delay: cut a few ms out of
    /// the audio, or play a stretch twice.
    #[test]
    fn survives_time_stretching() {
        let (audio, starts) = render(&[0x00005, 0x12345], 2000, 6000);
        let s = starts[0] as usize;
        let mut warped = audio[..s + 300].to_vec();
        // The sync loses 7 ms, symbol 1 loses 8 ms, and symbol 4 is 8 ms longer.
        warped.extend_from_slice(&audio[s + 640..s + SYNC_LEN + SYMBOL_LEN + 100]);
        warped.extend_from_slice(
            &audio[s + SYNC_LEN + SYMBOL_LEN + 500..s + SYNC_LEN + 4 * SYMBOL_LEN + 300],
        );
        warped.extend_from_slice(&audio[s + SYNC_LEN + 4 * SYMBOL_LEN..]);
        let (heard, rejected) = detect(&warped, FRAME);
        assert_eq!((ids(&heard), rejected), (vec![0x00005, 0x12345], 0));
        assert_starts(&heard[..1], &starts[..1], 48);
    }

    #[test]
    fn survives_gain_and_noise() {
        let (audio, starts) = render(&[0x5a5a5, 0x00f00], 2000, 5000);
        let hiss = noise(audio.len(), 1500.0, 0x1234_5678);
        // A third of the level (tones near -20 dBFS), plus hiss about 8 dB under them.
        let quiet: Vec<i16> = audio
            .iter()
            .zip(&hiss)
            .map(|(&s, n)| (f64::from(s) * 0.3 + n).round() as i16)
            .collect();
        let (heard, _) = detect(&quiet, FRAME);
        assert_eq!(ids(&heard), [0x5a5a5, 0x00f00]);
        assert_starts(&heard, &starts, 96);
    }

    #[test]
    fn ignores_bed_silence_and_noise() {
        let mut g = Generator::default();
        let mut bed = vec![0; 48_000];
        g.fill(&mut bed);
        assert_eq!(detect(&bed, FRAME), (vec![], 0));
        assert_eq!(detect(&vec![0; 48_000], FRAME), (vec![], 0));
        let hiss: Vec<i16> = noise(48_000, 8000.0, 99)
            .into_iter()
            .map(|n| n as i16)
            .collect();
        assert_eq!(detect(&hiss, FRAME), (vec![], 0));
    }

    #[test]
    fn rejects_a_burst_missing_a_symbol() {
        let (mut audio, starts) = render(&[0x12345, 0x54321], 1000, 4000);
        // A whole symbol lost, as if a packet never came and the gap was filled with silence.
        let hole = starts[0] as usize + SYNC_LEN + 2 * SYMBOL_LEN;
        audio[hole..hole + SYMBOL_LEN].fill(0);
        let (heard, rejected) = detect(&audio, FRAME);
        assert_eq!(ids(&heard), [0x54321]);
        assert_eq!(rejected, 1);
    }

    #[test]
    fn a_short_dropout_inside_a_symbol_is_harmless() {
        let (mut audio, starts) = render(&[0x11111], 1000, 2000);
        let at = starts[0] as usize + SYNC_LEN + 3 * SYMBOL_LEN + 200;
        audio[at..at + 96].fill(0);
        let (heard, rejected) = detect(&audio, FRAME);
        assert_eq!((ids(&heard), rejected), (vec![0x11111], 0));
        assert_starts(&heard, &starts, 48);
    }

    #[test]
    fn generator_waits_for_a_burst_to_end() {
        let mut g = Generator::default();
        assert!(g.start(1));
        assert!(!g.start(2), "busy");
        let mut out = vec![0; BURST_LEN];
        g.fill(&mut out);
        assert!(!g.busy());
        assert!(g.start(2));
        assert!(out
            .iter()
            .all(|&s| f64::from(s.unsigned_abs()) <= LEVEL + BED_LEVEL + 1.0));
    }
}
