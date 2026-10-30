//! Synthetic capture: streams the test pattern at the configured frame rate, on any OS.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::{wall_clock_us, CaptureConfig, Frame, FrameBuffer};
use crate::pattern::Pattern;

/// Rendering thread; stopped and joined on drop.
pub struct PatternCapture {
    running: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for PatternCapture {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

pub fn start(config: CaptureConfig, sink: impl Fn(Frame) + Send + 'static) -> PatternCapture {
    let running = Arc::new(AtomicBool::new(true));
    let thread = {
        let running = running.clone();
        std::thread::spawn(move || {
            let pattern = Pattern::new(config.width, config.height);
            let interval = Duration::from_secs_f64(1.0 / f64::from(config.max_fps.max(1)));
            let start = Instant::now();
            let mut n: u32 = 0;
            while running.load(Ordering::Relaxed) {
                let captured_at_us = wall_clock_us();
                sink(Frame {
                    buffer: FrameBuffer::I420(pattern.render_i420(n)),
                    captured_at_us,
                });
                n = n.wrapping_add(1);
                // Pace by the clock, not by sleeps, so rendering time doesn't slow the rate.
                let due = start + interval * n;
                if let Some(rest) = due.checked_duration_since(Instant::now()) {
                    std::thread::sleep(rest);
                }
            }
        })
    };
    PatternCapture {
        running,
        thread: Some(thread),
    }
}
