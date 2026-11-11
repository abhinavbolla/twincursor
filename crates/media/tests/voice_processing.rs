//! WebRTC's echo cancellation (AEC3) and noise suppression, driven offline with synthetic
//! speech through a simulated room. A device mic track runs the same modules (both come from
//! WebRTC's built-in audio processing), so these show what a call gets, without a mic.

use livekit::webrtc::native::apm::AudioProcessingModule;
use media::audio::marker::Generator;

const RATE: usize = 48_000;
const FRAME: usize = RATE / 100;

/// A small deterministic random generator (xorshift64*), so runs are repeatable.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    /// Uniform in [0, 1).
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Roughly normal, mean 0, standard deviation 1.
    fn gauss(&mut self) -> f64 {
        (0..12).map(|_| self.unit()).sum::<f64>() - 6.0
    }
}

/// Speech-like sound, as floats near ±1: voiced syllables (a drifting pitch with its
/// harmonics) of 120–300 ms with 50–200 ms pauses, active only inside `talking` (seconds).
fn voice(seed: u64, pitch: f64, seconds: f64, talking: &[(f64, f64)]) -> Vec<f64> {
    let mut rng = Rng(seed);
    let n = (seconds * RATE as f64) as usize;
    let mut out = vec![0.0; n];
    let mut phase = 0.0f64;
    let mut i = 0;
    while i < n {
        let syllable = ((0.12 + 0.18 * rng.unit()) * RATE as f64) as usize;
        let pause = ((0.05 + 0.15 * rng.unit()) * RATE as f64) as usize;
        let f0 = pitch * (0.8 + 0.4 * rng.unit());
        for k in 0..syllable.min(n - i) {
            let t = (i + k) as f64 / RATE as f64;
            if !talking.iter().any(|&(a, b)| (a..b).contains(&t)) {
                continue;
            }
            let f = f0 * (1.0 + 0.1 * (k as f64 / syllable as f64 - 0.5));
            phase += std::f64::consts::TAU * f / RATE as f64;
            let envelope = (std::f64::consts::PI * k as f64 / syllable as f64).sin();
            let sample: f64 = (1..=20).map(|h| (h as f64 * phase).sin() / h as f64).sum();
            out[i + k] = 0.25 * envelope * sample;
        }
        i += syllable + pause;
    }
    out
}

/// A room between the speaker and the mic: the direct sound after `delay_ms`, and a few
/// fading reflections.
fn room(far: &[f64], delay_ms: f64, gain: f64) -> Vec<f64> {
    let taps = [
        (0.0, 1.0),
        (3.0, 0.5),
        (7.0, -0.3),
        (12.0, 0.2),
        (21.0, -0.1),
    ];
    let mut out = vec![0.0; far.len()];
    for (extra_ms, g) in taps {
        let d = ((delay_ms + extra_ms) * RATE as f64 / 1000.0) as usize;
        for i in d..far.len() {
            out[i] += gain * g * far[i - d];
        }
    }
    out
}

fn to_i16(x: &[f64]) -> Vec<i16> {
    x.iter()
        .map(|&v| (v * i16::MAX as f64).clamp(i16::MIN as f64, i16::MAX as f64) as i16)
        .collect()
}

/// Mean power of `x` between two times, in dB relative to full scale.
fn level_db(x: &[i16], from_s: f64, to_s: f64) -> f64 {
    let (a, b) = (
        (from_s * RATE as f64) as usize,
        (to_s * RATE as f64) as usize,
    );
    let power: f64 = x[a..b]
        .iter()
        .map(|&s| (s as f64 / 32768.0).powi(2))
        .sum::<f64>()
        / (b - a) as f64;
    10.0 * power.max(1e-12).log10()
}

/// Feeds the far end (what the speakers play) and the mic through the APM, 10 ms at a time,
/// and returns the processed mic.
fn process(apm: &mut AudioProcessingModule, far: &[i16], mic: &[i16]) -> Vec<i16> {
    let mut out = mic.to_vec();
    for (f, m) in far.chunks(FRAME).zip(out.chunks_mut(FRAME)) {
        let mut f = f.to_vec();
        apm.process_reverse_stream(&mut f, RATE as i32, 1).unwrap();
        apm.process_stream(m, RATE as i32, 1).unwrap();
    }
    out
}

/// Which modules to run; the high-pass filter is always on, as on a mic track.
fn apm(echo: bool, gain: bool, noise: bool) -> AudioProcessingModule {
    AudioProcessingModule::new(echo, gain, true, noise)
}

/// The mic: the far end through a room, plus a local talker and a faint noise floor.
fn mic(echo: &[f64], near: &[f64], noise: f64, seed: u64) -> Vec<i16> {
    let mut rng = Rng(seed);
    let mixed: Vec<f64> = (0..echo.len())
        .map(|i| echo[i] + near[i] + noise * rng.gauss())
        .collect();
    to_i16(&mixed)
}

#[test]
fn cancels_echo_and_keeps_the_local_voice() {
    // The far end talks for 12 s, pauses, then talks again over the local talker, who starts at
    // 13 s. Laptop echo paths are 40–120 ms (speakers, air, mic and their buffers); the echo is
    // a third as loud as the local voice, or as loud.
    let far = voice(1, 120.0, 20.0, &[(0.0, 12.0), (16.0, 20.0)]);
    let near = voice(2, 210.0, 20.0, &[(13.0, 20.0)]);
    let (far_i, near_i) = (to_i16(&far), to_i16(&near));
    for delay_ms in [40.0, 80.0, 120.0] {
        for gain in [0.3, 1.0] {
            let mic = mic(&room(&far, delay_ms, gain), &near, 0.001, 3);
            let out = process(&mut apm(true, false, false), &far_i, &mic);
            let removed = |a, b| level_db(&mic, a, b) - level_db(&out, a, b);
            let kept = |a, b| level_db(&out, a, b) - level_db(&near_i, a, b);
            let case = format!("echo after {delay_ms} ms at {gain}");
            // Measured: 31–39 dB in the first 2 s, 37–47 dB after, the local voice within
            // 0.2 dB alone and 0.5–4.6 dB lower while both talk.
            assert!(
                removed(0.5, 2.0) >= 20.0,
                "{case}: {:.1} dB at first",
                removed(0.5, 2.0)
            );
            assert!(
                removed(4.0, 12.0) >= 30.0,
                "{case}: {:.1} dB",
                removed(4.0, 12.0)
            );
            assert!(
                kept(13.0, 16.0).abs() <= 1.0,
                "{case}: voice {:+.1} dB",
                kept(13.0, 16.0)
            );
            assert!(
                kept(16.0, 20.0) >= -6.0,
                "{case}: double talk {:+.1} dB",
                kept(16.0, 20.0)
            );
        }
    }
}

#[test]
fn recovers_when_the_echo_path_changes() {
    // The delay jumps from 60 to 250 ms at 10 s, as when switching to other speakers mid-call,
    // with clean and with overdriven (distorting) laptop speakers. The canceller has to find
    // the new delay on its own: for a second or two the far end hears itself.
    let far = voice(1, 120.0, 20.0, &[(0.0, 20.0)]);
    let far_i = to_i16(&far);
    let (before, after) = (room(&far, 60.0, 1.0), room(&far, 250.0, 1.0));
    let silent = vec![0.0; far.len()];
    for distort in [false, true] {
        let echo: Vec<f64> = (0..far.len())
            .map(|i| {
                let e = if i < 10 * RATE { before[i] } else { after[i] };
                if distort {
                    (2.0 * e).tanh() / 2.0
                } else {
                    e
                }
            })
            .collect();
        let mic = mic(&echo, &silent, 0.001, 3);
        let out = process(&mut apm(true, false, false), &far_i, &mic);
        let removed = |a, b| level_db(&mic, a, b) - level_db(&out, a, b);
        // Measured: 41–45 dB before, 1–9 dB for 2 s after the jump, then 39–49 dB again.
        assert!(
            removed(5.0, 10.0) >= 30.0,
            "distort {distort}: {:.1} dB before",
            removed(5.0, 10.0)
        );
        assert!(
            removed(12.5, 20.0) >= 30.0,
            "distort {distort}: {:.1} dB after",
            removed(12.5, 20.0)
        );
    }
}

#[test]
fn suppresses_steady_noise_and_keeps_speech() {
    // A fan-like noise floor, 3 s alone and then under speech.
    let clean = voice(4, 160.0, 12.0, &[(3.0, 12.0)]);
    let clean_i = to_i16(&clean);
    let silent = vec![0.0; clean.len()];
    let no_far = vec![0i16; clean.len()];
    for noise in [0.01, 0.03] {
        let noisy = mic(&silent, &clean, noise, 5);
        let out = process(&mut apm(false, false, true), &no_far, &noisy);
        // Measured: 15 dB less noise, speech within 0.5 dB.
        let removed = level_db(&noisy, 1.0, 3.0) - level_db(&out, 1.0, 3.0);
        let speech = level_db(&out, 4.0, 12.0) - level_db(&clean_i, 4.0, 12.0);
        assert!(removed >= 10.0, "noise {noise}: {removed:.1} dB removed");
        assert!(speech.abs() <= 1.0, "noise {noise}: speech {speech:+.1} dB");
    }
}

#[test]
fn everything_on_as_on_a_mic_track() {
    // Echo, noise and the local voice together, with gain control on too: what a mic track runs.
    let far = voice(1, 120.0, 16.0, &[(0.0, 16.0)]);
    let near = voice(2, 210.0, 16.0, &[(10.0, 16.0)]);
    let near_i = to_i16(&near);
    let mic = mic(&room(&far, 80.0, 1.0), &near, 0.01, 9);
    let out = process(&mut apm(true, true, true), &to_i16(&far), &mic);
    // Measured: the echo 25 dB down (gain control lifts what's left), the voice 0.4 dB off.
    let removed = level_db(&mic, 4.0, 10.0) - level_db(&out, 4.0, 10.0);
    let voice = level_db(&out, 10.0, 16.0) - level_db(&near_i, 10.0, 16.0);
    assert!(removed >= 20.0, "echo {removed:.1} dB down");
    assert!(voice.abs() <= 3.0, "voice {voice:+.1} dB");
}

#[test]
fn cancels_sudden_sounds_after_silence() {
    // Marker bursts once a second, as the acoustic check plays them: a sudden tone after a
    // quiet second is where a canceller that's slow to react lets echo through.
    let mut far = vec![0i16; 14 * RATE];
    let mut generator = Generator::default();
    for (k, frame) in far.chunks_mut(FRAME).enumerate() {
        if k % 100 == 50 {
            generator.start(k as u32);
        }
        generator.fill(frame);
    }
    let far_f: Vec<f64> = far.iter().map(|&s| s as f64 / i16::MAX as f64).collect();
    let mic = mic(&room(&far_f, 60.0, 1.0), &vec![0.0; far.len()], 0.0, 3);
    let out = process(&mut apm(true, false, false), &far, &mic);
    // The loudest 10 ms of each burst, before and after, once the canceller has settled (6 s).
    let peak = |x: &[i16], frame: usize| {
        (frame..frame + 30)
            .map(|f| level_db(x, f as f64 / 100.0, (f + 1) as f64 / 100.0))
            .fold(f64::NEG_INFINITY, f64::max)
    };
    for second in 6..13 {
        let frame = second * 100 + 50;
        let down = peak(&mic, frame) - peak(&out, frame);
        // Measured: 28–66 dB. With the echo less than about 5 ms behind the sound, closer
        // than a real speaker and mic get, this drops to 1–2 dB.
        assert!(down >= 15.0, "burst at {second}.5 s only {down:.1} dB down");
    }
}
