//! Screen capture backends. Each one delivers frames to a sink callback on its own thread.

mod lk;
#[cfg(target_os = "macos")]
mod sck;
mod synthetic;

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
    /// The synthetic code-editor pattern (`crate::pattern`), for quality measurements.
    Pattern,
}

/// Output size presets, by number of lines. Widths follow the display's aspect ratio.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    P720,
    P1080,
    P1440,
    /// The display's own pixel size.
    Native,
}

impl Tier {
    fn lines(self) -> Option<u32> {
        match self {
            Self::P720 => Some(720),
            Self::P1080 => Some(1080),
            Self::P1440 => Some(1440),
            Self::Native => None,
        }
    }

    /// Default maximum bitrate. Screen content is mostly static, so this is a ceiling for
    /// scrolling and typing bursts, not a steady rate.
    pub fn default_bitrate_kbps(self) -> u64 {
        match self {
            Self::P720 => 2_500,
            Self::P1080 => 5_000,
            Self::P1440 => 8_000,
            Self::Native => 12_000,
        }
    }

    /// Output size for this tier on a display of `native` pixels. Keeps the aspect ratio,
    /// rounds the width to a multiple of 16 (encoders crop other widths: 1662 became 1660),
    /// keeps the height even (NV12) and never upscales.
    pub fn size(self, native: (u32, u32)) -> (u32, u32) {
        let (nw, nh) = (native.0.max(16), native.1.max(2));
        let max_w = nw / 16 * 16;
        let lines = self.lines().unwrap_or(nh).min(nh) & !1;
        let width = (u64::from(lines) * u64::from(nw) + u64::from(nh) / 2) / u64::from(nh);
        let width = ((width as u32 + 8) / 16 * 16).clamp(16, max_w);
        (width, lines)
    }
}

/// Assumed when the display size can't be read (non-macOS, or a locked or sleeping screen).
pub const FALLBACK_DISPLAY: (u32, u32) = (1920, 1080);

/// The "display" the synthetic pattern pretends to have: 16:9, so its tiers are 1920x1080
/// and 2560x1440 on every machine and results compare across machines.
pub const PATTERN_DISPLAY: (u32, u32) = (2560, 1440);

/// Pixel size of the display we capture (the first one), if the platform can tell us.
pub fn display_pixels() -> Option<(u32, u32)> {
    #[cfg(target_os = "macos")]
    {
        sck::display_pixels()
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
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
        Backend::Pattern => Box::new(synthetic::start(config, sink)),
    };
    Ok(CaptureHandle { _inner: inner })
}

/// Current wall-clock time in microseconds since the Unix epoch.
pub fn wall_clock_us() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_micros() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RETINA_14: (u32, u32) = (3024, 1964);

    #[test]
    fn tiers_on_a_retina_macbook() {
        assert_eq!(Tier::P720.size(RETINA_14), (1104, 720));
        assert_eq!(Tier::P1080.size(RETINA_14), (1664, 1080));
        assert_eq!(Tier::P1440.size(RETINA_14), (2224, 1440));
        assert_eq!(Tier::Native.size(RETINA_14), (3024, 1964));
    }

    #[test]
    fn tiers_on_16_by_9() {
        assert_eq!(Tier::P1080.size((1920, 1080)), (1920, 1080));
        assert_eq!(Tier::P1440.size((2560, 1440)), (2560, 1440));
        assert_eq!(Tier::P720.size((3840, 2160)), (1280, 720));
    }

    #[test]
    fn never_upscales() {
        assert_eq!(Tier::P1440.size((1920, 1080)), (1920, 1080));
        assert_eq!(Tier::Native.size((1366, 768)), (1360, 768));
    }

    #[test]
    fn widths_are_multiples_of_16_and_heights_even() {
        for native in [
            (3024, 1964),
            (2560, 1600),
            (1440, 900),
            (1366, 768),
            (1001, 777),
        ] {
            for tier in [Tier::P720, Tier::P1080, Tier::P1440, Tier::Native] {
                let (w, h) = tier.size(native);
                assert_eq!(w % 16, 0, "{tier:?} on {native:?}");
                assert_eq!(h % 2, 0, "{tier:?} on {native:?}");
                assert!(w <= native.0 && h <= native.1, "{tier:?} on {native:?}");
            }
        }
    }

    #[test]
    fn bitrate_grows_with_tier() {
        let rates: Vec<_> = [Tier::P720, Tier::P1080, Tier::P1440, Tier::Native]
            .map(Tier::default_bitrate_kbps)
            .to_vec();
        assert!(rates.windows(2).all(|w| w[0] < w[1]));
    }
}
