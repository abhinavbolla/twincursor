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
                let _ = ready_tx.send(Err(CaptureError::Backend(
                    "DesktopCapturer::new failed".into(),
                )));
                return;
            };
            // The share's display, by id (on macOS a screen source's id is its
            // CGDirectDisplayID), never just the first source listed. libwebrtc's ScreenCaptureKit
            // capturer lists no sources at all (seen on macOS 26) and then captures its own
            // default: that is taken only for the main display, the one it is assumed to pick.
            let sources = capturer.get_source_list();
            let listed: Vec<String> = sources
                .iter()
                .map(|s| format!("{} ({})", s.id(), s.title()))
                .collect();
            let empty = sources.is_empty();
            let source = config
                .display
                .and_then(|id| sources.into_iter().find(|s| s.id() == u64::from(id)));
            let default_is_ok = empty && config.display.is_some() && config.display == main_id();
            if source.is_none() && !default_is_ok {
                tracing::warn!("screen sources: [{}]", listed.join(", "));
                let _ = ready_tx.send(Err(match config.display {
                    Some(id) => CaptureError::DisplayNotListed(id),
                    None => CaptureError::NoDisplayChosen,
                }));
                return;
            }
            if source.is_none() {
                tracing::warn!(
                    "libwebrtc lists no screens; capturing its default, taken to be the main display"
                );
            }
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
    Ok(LkCapture {
        running,
        thread: Some(thread),
    })
}

/// The main display's id, where the platform has one.
fn main_id() -> Option<u32> {
    #[cfg(target_os = "macos")]
    {
        Some(super::sck::main_display_id())
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

fn to_frame(frame: &DesktopFrame, width: u32, height: u32) -> Frame {
    let captured_at_us = wall_clock_us();
    let (src_w, src_h) = (frame.width(), frame.height());
    let mut full = I420Buffer::new(src_w as u32, src_h as u32);
    let (sy, su, sv) = full.strides();
    let (y, u, v) = full.data_mut();
    yuv_helper::argb_to_i420(
        frame.data(),
        frame.stride(),
        y,
        sy,
        u,
        su,
        v,
        sv,
        src_w,
        src_h,
    );
    let buffer = if (src_w as u32, src_h as u32) == (width, height) {
        full
    } else {
        full.scale(width as i32, height as i32)
    };
    Frame {
        buffer: FrameBuffer::I420(buffer),
        captured_at_us,
    }
}
