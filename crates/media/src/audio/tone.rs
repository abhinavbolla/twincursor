//! The sharer's synthetic voice: marker tones or a recording, fed to the track every 10 ms like
//! a mic.

use std::borrow::Cow;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use livekit::webrtc::audio_frame::AudioFrame;
use livekit::webrtc::audio_source::native::NativeAudioSource;

use super::marker::{Generator, FRAME, RATE};
use crate::capture::wall_clock_us;

const PERIOD: Duration = Duration::from_millis(10);
/// A tick that waited longer than this for a burst to finish is skipped: sending it late would
/// count the wait as network delay.
const MAX_WAIT: Duration = Duration::from_millis(15);

/// The newest tick from the viewer, waiting for the next burst.
#[derive(Debug, Default)]
pub struct TickSlot(Mutex<Option<(u32, Instant)>>);

impl TickSlot {
    pub fn offer(&self, id: u32) {
        *self.0.lock().unwrap() = Some((id, Instant::now()));
    }

    fn take(&self) -> Option<(u32, Instant)> {
        self.0.lock().unwrap().take()
    }
}

/// Feeding thread; stopped and joined on drop.
pub struct ToneSource {
    running: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for ToneSource {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// What the synthetic voice says.
pub(super) enum Sound {
    /// A marker burst for each tick; `on_burst(id, wall_us)` runs as each one starts.
    Markers {
        ticks: Arc<TickSlot>,
        on_burst: Box<dyn Fn(u32, u64) + Send>,
    },
    /// A recording (never empty), over and over.
    Recording(Vec<i16>),
}

/// Fills `out` from `recording`, starting at `*at` and wrapping around at its end.
fn fill_looped(recording: &[i16], at: &mut usize, out: &mut [i16]) {
    for sample in out {
        *sample = recording[*at];
        *at = (*at + 1) % recording.len();
    }
}

/// Starts feeding `source` with `sound`.
pub(super) fn start(source: NativeAudioSource, sound: Sound) -> ToneSource {
    let running = Arc::new(AtomicBool::new(true));
    let thread = {
        let running = running.clone();
        std::thread::spawn(move || {
            raise_priority();
            let mut generator = Generator::default();
            let mut samples = vec![0i16; FRAME];
            let mut at = 0;
            let mut due = Instant::now();
            // How late frames left, over the last 10 s: a source that misses its slot looks
            // like network jitter to the receiver.
            let (mut worst, mut late, mut since) = (Duration::ZERO, 0u32, Instant::now());
            while running.load(Ordering::Relaxed) {
                let lateness = Instant::now().saturating_duration_since(due);
                worst = worst.max(lateness);
                late += u32::from(lateness > Duration::from_millis(5));
                if since.elapsed() >= Duration::from_secs(10) {
                    if late > 0 {
                        tracing::warn!(late, ?worst, "audio frames left more than 5 ms late");
                    }
                    (worst, late, since) = (Duration::ZERO, 0, Instant::now());
                }
                match &sound {
                    Sound::Markers { ticks, on_burst } => {
                        if !generator.busy() {
                            if let Some((id, arrived)) = ticks.take() {
                                if arrived.elapsed() <= MAX_WAIT && generator.start(id) {
                                    on_burst(id, wall_clock_us());
                                }
                            }
                        }
                        generator.fill(&mut samples);
                    }
                    Sound::Recording(recording) => fill_looped(recording, &mut at, &mut samples),
                }
                let frame = AudioFrame {
                    data: Cow::Borrowed(&samples),
                    sample_rate: RATE,
                    num_channels: 1,
                    samples_per_channel: FRAME as u32,
                };
                // Without a queue this hands the frame on and returns at once.
                if let Err(e) = futures::executor::block_on(source.capture_frame(&frame)) {
                    tracing::warn!("audio frame not taken: {e}");
                }
                // Keep to the clock like a sound card; after a long stall, start over rather
                // than catch up in a burst.
                due += PERIOD;
                let now = Instant::now();
                match due.checked_duration_since(now) {
                    Some(rest) => std::thread::sleep(rest),
                    None if now - due > PERIOD * 10 => due = now,
                    None => {}
                }
            }
        })
    };
    ToneSource {
        running,
        thread: Some(thread),
    }
}

/// Makes this a real-time thread, as Core Audio does for a sound card's I/O thread: every
/// 10 ms it gets up to 1 ms of CPU within 5 ms. At a raised QoS class alone, macOS still woke the
/// thread up to 87 ms late under ordinary desktop load, which the receiver took for network
/// jitter and answered by growing its buffer.
#[cfg(target_os = "macos")]
// libc marks the timebase call deprecated in favour of the `mach2` crate, which isn't worth a
// dependency for one call.
#[allow(deprecated)]
fn raise_priority() {
    let mut timebase = libc::mach_timebase_info { numer: 0, denom: 0 };
    // SAFETY: fills in the struct we pass.
    unsafe { libc::mach_timebase_info(&mut timebase) };
    let ticks = |d: Duration| {
        (d.as_nanos() as u64 * u64::from(timebase.denom) / u64::from(timebase.numer.max(1))) as u32
    };
    let mut policy = libc::thread_time_constraint_policy {
        period: ticks(PERIOD),
        computation: ticks(Duration::from_millis(1)),
        constraint: ticks(Duration::from_millis(5)),
        preemptible: 1,
    };
    // SAFETY: changes only the calling thread's scheduling; the policy struct outlives the call
    // and its size is what the count says.
    let result = unsafe {
        libc::thread_policy_set(
            libc::pthread_mach_thread_np(libc::pthread_self()),
            libc::THREAD_TIME_CONSTRAINT_POLICY as libc::thread_policy_flavor_t,
            (&mut policy as *mut libc::thread_time_constraint_policy).cast(),
            libc::THREAD_TIME_CONSTRAINT_POLICY_COUNT,
        )
    };
    if result != 0 {
        tracing::warn!(
            result,
            "audio thread not made real-time; expect jitter under load"
        );
    }
}

/// The same on Linux, where the network lab runs: a low real-time FIFO priority. Needs
/// CAP_SYS_NICE, which the lab's containers have; without it the thread stays as it was.
#[cfg(target_os = "linux")]
fn raise_priority() {
    let param = libc::sched_param { sched_priority: 10 };
    // SAFETY: changes only the calling thread's scheduling; `param` outlives the call.
    let result =
        unsafe { libc::pthread_setschedparam(libc::pthread_self(), libc::SCHED_FIFO, &param) };
    if result != 0 {
        tracing::warn!(
            result,
            "audio thread not made real-time; expect jitter under load"
        );
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn raise_priority() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_recording_starts_over_at_its_end() {
        let recording = [1, 2, 3, 4, 5];
        let (mut at, mut out) = (0, [0i16; 4]);
        fill_looped(&recording, &mut at, &mut out);
        assert_eq!(out, [1, 2, 3, 4]);
        fill_looped(&recording, &mut at, &mut out);
        assert_eq!(out, [5, 1, 2, 3]);
        // Shorter than one frame: it repeats inside the frame.
        let mut at = 0;
        fill_looped(&[7, 8], &mut at, &mut out);
        assert_eq!(out, [7, 8, 7, 8]);
    }
}
