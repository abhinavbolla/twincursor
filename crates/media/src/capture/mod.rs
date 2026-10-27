//! Screen capture backends. Each one delivers frames to a sink callback on its own thread.

mod lk;
#[cfg(target_os = "macos")]
mod sck;

use std::time::{SystemTime, UNIX_EPOCH};

use livekit::webrtc::video_frame::{I420Buffer, NV12Buffer, VideoBuffer};

/// Which capture implementation to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    /// Apple ScreenCaptureKit via the `screencapturekit` crate: GPU-scaled NV12.
    #[cfg(target_os = "macos")]
    ScreenCaptureKit,
    /// libwebrtc's cross-platform `DesktopCapturer`, shipped with the LiveKit SDK.
    LiveKit,
}

#[derive(Debug, Clone, Copy)]
pub struct CaptureConfig {
    /// Output size. Backends that can't scale on capture scale on the CPU instead.
    pub width: u32,
    pub height: u32,
    pub max_fps: u32,
    pub show_cursor: bool,
}

/// Pixel data of a captured frame.
pub enum FrameBuffer {
    Nv12(NV12Buffer),
    I420(I420Buffer),
}

impl FrameBuffer {
    pub fn width(&self) -> u32 {
        self.as_video_buffer().width()
    }

    pub fn height(&self) -> u32 {
        self.as_video_buffer().height()
    }

    pub fn as_video_buffer(&self) -> &dyn VideoBuffer {
        match self {
            Self::Nv12(b) => b,
            Self::I420(b) => b,
        }
    }
}

/// A captured frame and the wall-clock time its content was captured.
pub struct Frame {
    pub buffer: FrameBuffer,
    pub captured_at_us: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    #[error("no display available to capture (is Screen Recording permission granted?)")]
    NoDisplay,
    #[error("capture backend failed: {0}")]
    Backend(String),
}

/// Keeps a capture running; capture stops when this is dropped.
pub struct CaptureHandle {
    _inner: Box<dyn Send>,
}

/// Starts capturing the main display and calls `sink` for every new frame.
pub fn start(
    backend: Backend,
    config: CaptureConfig,
    sink: impl Fn(Frame) + Send + Sync + 'static,
) -> Result<CaptureHandle, CaptureError> {
    let inner: Box<dyn Send> = match backend {
        #[cfg(target_os = "macos")]
        Backend::ScreenCaptureKit => Box::new(sck::start(config, sink)?),
        Backend::LiveKit => Box::new(lk::start(config, sink)?),
    };
    Ok(CaptureHandle { _inner: inner })
}

/// Current wall-clock time in microseconds since the Unix epoch.
pub fn wall_clock_us() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_micros() as u64)
}
