//! libwebrtc `DesktopCapturer` backend (shipped with the LiveKit SDK). It captures at full
//! display resolution in BGRA and has to be polled, so conversion and scaling run on the CPU.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use livekit::webrtc::desktop_capturer::{
    DesktopCaptureSourceType, DesktopCapturer, DesktopCapturerOptions, DesktopFrame,
};
use livekit::webrtc::native::yuv_helper;
use livekit::webrtc::video_frame::I420Buffer;

use super::{wall_clock_us, CaptureConfig, CaptureError, Frame, FrameBuffer};

/// Polling thread; stopped and joined on drop.
pub struct LkCapture {
    running: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for LkCapture {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

pub fn start(
    config: CaptureConfig,
    sink: impl Fn(Frame) + Send + Sync + 'static,
) -> Result<LkCapture, CaptureError> {
    let running = Arc::new(AtomicBool::new(true));
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let thread = {
        let running = running.clone();
        std::thread::spawn(move || {
            let mut options = DesktopCapturerOptions::new(DesktopCaptureSourceType::Screen);
            options.set_include_cursor(config.show_cursor);
            #[cfg(target_os = "macos")]
            options.set_sck_system_picker(false);
            let Some(mut capturer) = DesktopCapturer::new(options) else {
                let _ = ready_tx.send(Err(CaptureError::Backend("DesktopCapturer::new failed".into())));
                return;
            };
            let source = capturer.get_source_list().into_iter().next();
            capturer.start_capture(source, move |result| {
                if let Ok(frame) = result {
                    sink(to_frame(&frame, config.width, config.height));
                }
            });
            let _ = ready_tx.send(Ok(()));
            let interval = Duration::from_secs_f64(1.0 / f64::from(config.max_fps));
            while running.load(Ordering::Relaxed) {
                let started = Instant::now();
                capturer.capture_frame();
                if let Some(rest) = interval.checked_sub(started.elapsed()) {
                    std::thread::sleep(rest);
                }
            }
        })
    };
    ready_rx
        .recv()
        .map_err(|_| CaptureError::Backend("capture thread exited".into()))??;
    Ok(LkCapture { running, thread: Some(thread) })
}

fn to_frame(frame: &DesktopFrame, width: u32, height: u32) -> Frame {
    let captured_at_us = wall_clock_us();
    let (src_w, src_h) = (frame.width(), frame.height());
    let mut full = I420Buffer::new(src_w as u32, src_h as u32);
    let (sy, su, sv) = full.strides();
    let (y, u, v) = full.data_mut();
    yuv_helper::argb_to_i420(frame.data(), frame.stride(), y, sy, u, su, v, sv, src_w, src_h);
    let buffer = if (src_w as u32, src_h as u32) == (width, height) {
        full
    } else {
        full.scale(width as i32, height as i32)
    };
    Frame { buffer: FrameBuffer::I420(buffer), captured_at_us }
}
