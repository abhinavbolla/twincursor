//! Screen capture backends. Each one delivers frames to a sink callback on its own thread.

mod lk;
#[cfg(target_os = "macos")]
mod sck;
pub mod select;
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
    /// 4K: 3840x2160 on a 16:9 display, the display's own size on a shorter one.
    P2160,
    /// The display's own pixel size.
    Native,
}

impl Tier {
    fn lines(self) -> Option<u32> {
        match self {
            Self::P720 => Some(720),
            Self::P1080 => Some(1080),
            Self::P1440 => Some(1440),
            Self::P2160 => Some(2160),
            Self::Native => None,
        }
    }

    /// Default maximum bitrate. Screen content is mostly static, so this is a ceiling for
    /// scrolling and typing bursts, not a steady rate. `Native` is a fixed ceiling, whatever
    /// the display's size.
    pub fn default_bitrate_kbps(self) -> u64 {
        match self {
            Self::P720 => 2_500,
            Self::P1080 => 5_000,
            Self::P1440 => 8_000,
            Self::P2160 => 15_000,
            Self::Native => 12_000,
        }
    }

    /// Output size for this tier on a display (or window) of `native` pixels. Keeps the
    /// aspect ratio, rounds the width to a multiple of 16 and the height down to a multiple
    /// of 4 (the encoder crops other sizes: a width of 1662 became 1660, a height of 1046
    /// became 1044) and never upscales.
    pub fn size(self, native: (u32, u32)) -> (u32, u32) {
        let nh = native.1.max(4);
        sized(native, self.lines().unwrap_or(nh), u32::MAX)
    }
}

/// `native` scaled to `lines` (at most its own), rounded as [`Tier::size`] rounds, and no
/// wider than `max_w`.
fn sized(native: (u32, u32), lines: u32, max_w: u32) -> (u32, u32) {
    let (nw, nh) = (native.0.max(16), native.1.max(4));
    let max_w = (nw.min(max_w) / 16 * 16).max(16);
    let lines = (lines.min(nh) & !3).max(4);
    let width = (u64::from(lines) * u64::from(nw) + u64::from(nh) / 2) / u64::from(nh);
    let width = ((width as u32 + 8) / 16 * 16).clamp(16, max_w);
    (width, lines)
}

/// The largest picture of a `native`-sized source that fits inside `viewport`: the aspect
/// ratio kept, rounded as [`Tier::size`] rounds (so never larger than the viewport), and never
/// larger than the source.
pub fn fit_within(native: (u32, u32), viewport: (u32, u32)) -> (u32, u32) {
    let (nw, nh) = (u64::from(native.0.max(16)), u64::from(native.1.max(4)));
    let max_w = (viewport.0.min(native.0.max(16)) / 16 * 16).max(16);
    // A width rounds to `max_w` or less up to about this many lines: start there and step
    // down (two or three steps at most) until it does.
    let most = (u64::from(max_w) + 8) * nh / nw + 4;
    let mut lines = most.min(u64::from(viewport.1)).min(nh) as u32 & !3;
    loop {
        let size = sized(native, lines, u32::MAX);
        if size.0 <= max_w || lines <= 4 {
            return (size.0.min(max_w), size.1);
        }
        lines -= 4;
    }
}

/// `size` limited to `limit` on each side (a picture fitted to a viewer, with the same aspect
/// ratio up to rounding).
pub fn capped(size: (u32, u32), limit: Option<(u32, u32)>) -> (u32, u32) {
    match limit {
        Some(l) => (size.0.min(l.0), size.1.min(l.1)),
        None => size,
    }
}

/// Assumed when the display size can't be read (non-macOS, or a locked or sleeping screen).
pub const FALLBACK_DISPLAY: (u32, u32) = (1920, 1080);

/// The "display" the synthetic pattern pretends to have: 16:9 4K, so its tiers are 1280x720,
/// 1920x1080, 2560x1440 and 3840x2160 on every machine and results compare across machines.
pub const PATTERN_DISPLAY: (u32, u32) = (3840, 2160);

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

/// What to capture. Only ScreenCaptureKit captures a window or an app.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Source {
    /// The main display, leaving out every TwinCursor window (see
    /// [`select::is_twincursor`]), so a viewer on the same Mac doesn't show itself.
    #[default]
    Display,
    /// One window, by id ([`select::WindowInfo::id`]), wherever it is and whatever covers it.
    Window(u32),
    /// The main display, showing only the windows of the app with this process id.
    App(i32),
}

#[derive(Debug, Clone, Copy)]
pub struct CaptureConfig {
    pub source: Source,
    /// Output size. Backends that can't scale on capture scale on the CPU instead.
    pub width: u32,
    pub height: u32,
    pub max_fps: u32,
    pub show_cursor: bool,
    /// Another app whose windows stay out of the capture, by process id: the desktop app that
    /// started this process, so its windows never reach the guest. Only ScreenCaptureKit
    /// supports it; this process's own windows are always left out there.
    pub exclude_app_pid: Option<u32>,
    /// Also leave out every other TwinCursor process (see [`select::is_twincursor`]), such as
    /// a viewer on the same Mac. Off only for checks that must see the desktop app's windows.
    pub leave_out_twincursor: bool,
    /// For [`Source::Window`]: when the window is resized, the output follows its new pixel
    /// size through this tier ([`Tier::size`]) and [`CaptureHandle::resized`] reports it.
    /// `None` keeps the output size fixed.
    pub follow_tier: Option<Tier>,
}

/// A shared window changed size, and so did the frames captured from it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Resize {
    /// The window's new size in pixels.
    pub window: (u32, u32),
    /// The size frames now arrive at: `follow_tier.size(window)`.
    pub output: (u32, u32),
}

/// An on-screen window, in points from the top-left corner of the captured display.
#[derive(Debug, Clone, PartialEq)]
pub struct WindowRect {
    pub id: u32,
    pub title: String,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// The captured display's size in points and the on-screen windows of the app with process id
/// `pid` on it, so a check can find where they would appear in a frame.
pub fn app_windows(pid: u32) -> Option<((f64, f64), Vec<WindowRect>)> {
    #[cfg(target_os = "macos")]
    {
        sck::app_windows(pid)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = pid;
        None
    }
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

    /// A copy scaled to `width` x `height` (libyuv, on the CPU).
    pub fn scaled(&mut self, width: u32, height: u32) -> FrameBuffer {
        match self {
            Self::Nv12(b) => Self::Nv12(b.scale(width as i32, height as i32)),
            Self::I420(b) => Self::I420(b.scale(width as i32, height as i32)),
        }
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
    #[error("{0} is no longer available to capture")]
    Gone(String),
    #[error("only ScreenCaptureKit (macOS) can capture a single window or app")]
    Unsupported,
}

/// Keeps a capture running; capture stops when this is dropped.
pub struct CaptureHandle {
    _inner: Box<dyn Send>,
    /// Capture stopping by itself.
    pub ended: Ended,
    /// A shared window's new size taking effect.
    pub resized: Resized,
}

/// See [`Ended::wait`].
pub struct Ended(Option<tokio::sync::oneshot::Receiver<String>>);

impl Ended {
    /// Resolves, with the reason, if capture stops on its own: for example when the shared
    /// window is closed. Never resolves for captures that can't end by themselves.
    pub async fn wait(&mut self) -> String {
        match self.0.as_mut() {
            Some(rx) => {
                let reason = rx.await;
                self.0 = None;
                match reason {
                    Ok(reason) => reason,
                    Err(_) => std::future::pending().await,
                }
            }
            None => std::future::pending().await,
        }
    }
}

/// See [`Resized::next`].
pub struct Resized(tokio::sync::mpsc::UnboundedReceiver<Resize>);

impl Resized {
    /// Resolves when a shared window's new size has taken effect (see
    /// [`CaptureConfig::follow_tier`]). Never resolves for other captures.
    pub async fn next(&mut self) -> Resize {
        match self.0.recv().await {
            Some(resize) => resize,
            None => std::future::pending().await,
        }
    }
}

/// Every window that could be shared with [`Source::Window`], front to back, and the apps
/// that own them (for [`Source::App`]).
pub fn shareable() -> Result<(Vec<select::WindowInfo>, Vec<select::AppInfo>), CaptureError> {
    #[cfg(target_os = "macos")]
    {
        sck::shareable()
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err(CaptureError::Unsupported)
    }
}

/// Starts capturing `config.source` and calls `sink` for every new frame.
pub fn start(
    backend: Backend,
    config: CaptureConfig,
    sink: impl Fn(Frame) + Send + Sync + 'static,
) -> Result<CaptureHandle, CaptureError> {
    let (end_tx, end_rx) = tokio::sync::oneshot::channel();
    let (resize_tx, resize_rx) = tokio::sync::mpsc::unbounded_channel();
    let inner: Box<dyn Send> = match backend {
        #[cfg(target_os = "macos")]
        Backend::ScreenCaptureKit => Box::new(sck::start(config, sink, end_tx, resize_tx)?),
        _ if config.source != Source::Display => return Err(CaptureError::Unsupported),
        Backend::LiveKit => Box::new(lk::start(config, sink)?),
        Backend::Pattern => Box::new(synthetic::start(config, sink)),
    };
    #[cfg(not(target_os = "macos"))]
    drop((end_tx, resize_tx));
    Ok(CaptureHandle {
        _inner: inner,
        ended: Ended(Some(end_rx)),
        resized: Resized(resize_rx),
    })
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
    fn tier_2160() {
        assert_eq!(Tier::P2160.size((3840, 2160)), (3840, 2160));
        // Shorter than 2160 lines: the display's own size, never upscaled.
        assert_eq!(Tier::P2160.size(RETINA_14), (3024, 1964));
        assert_eq!(Tier::P2160.size(RETINA_14), Tier::Native.size(RETINA_14));
        // 5K: scaled down to 4K.
        assert_eq!(Tier::P2160.size((5120, 2880)), (3840, 2160));
        assert_eq!(Tier::P1440.size((5120, 2880)), (2560, 1440));
        assert_eq!(Tier::Native.size((5120, 2880)), (5120, 2880));
    }

    #[test]
    fn pattern_tiers_are_16_by_9() {
        let sizes = [
            Tier::P720,
            Tier::P1080,
            Tier::P1440,
            Tier::P2160,
            Tier::Native,
        ]
        .map(|t| t.size(PATTERN_DISPLAY));
        assert_eq!(
            sizes,
            [
                (1280, 720),
                (1920, 1080),
                (2560, 1440),
                (3840, 2160),
                (3840, 2160)
            ]
        );
    }

    #[test]
    fn tiers_on_16_by_9() {
        assert_eq!(Tier::P1080.size((1920, 1080)), (1920, 1080));
        assert_eq!(Tier::P1440.size((2560, 1440)), (2560, 1440));
        assert_eq!(Tier::P720.size((3840, 2160)), (1280, 720));
    }

    #[test]
    fn a_window_keeps_its_own_size_within_the_rounding() {
        // A 754x523-point window on a Retina display.
        assert_eq!(Tier::Native.size((1508, 1046)), (1504, 1044));
        assert_eq!(Tier::P1080.size((1508, 1046)), (1504, 1044));
        // Taller than the tier: scaled down like a display.
        assert_eq!(Tier::P720.size((1508, 1046)), (1040, 720));
    }

    #[test]
    fn never_upscales() {
        assert_eq!(Tier::P1440.size((1920, 1080)), (1920, 1080));
        assert_eq!(Tier::Native.size((1366, 768)), (1360, 768));
    }

    #[test]
    fn widths_are_multiples_of_16_and_heights_of_4() {
        for native in [
            (3024, 1964),
            (2560, 1600),
            (1440, 900),
            (1366, 768),
            (1001, 777),
            (3840, 2160),
            (5120, 2880),
            (3440, 1440),
        ] {
            for tier in [
                Tier::P720,
                Tier::P1080,
                Tier::P1440,
                Tier::P2160,
                Tier::Native,
            ] {
                let (w, h) = tier.size(native);
                assert_eq!(w % 16, 0, "{tier:?} on {native:?}");
                assert_eq!(h % 4, 0, "{tier:?} on {native:?}");
                assert!(w <= native.0 && h <= native.1, "{tier:?} on {native:?}");
            }
        }
    }

    #[test]
    fn fits_a_viewport_without_upscaling() {
        // The pattern's 4K display into common viewports: exactly the tier sizes.
        assert_eq!(fit_within(PATTERN_DISPLAY, (1920, 1080)), (1920, 1080));
        assert_eq!(fit_within(PATTERN_DISPLAY, (2560, 1440)), (2560, 1440));
        // Bigger than the source: the source's own size.
        assert_eq!(fit_within(PATTERN_DISPLAY, (7680, 4320)), (3840, 2160));
        assert_eq!(fit_within(RETINA_14, (5120, 2880)), (3024, 1964));
        // A maximised viewer on a Retina MacBook (95% x 88% of 3024x1964), sharer on the same
        // kind of Mac: limited by the height.
        assert_eq!(fit_within(RETINA_14, (2872, 1728)), (2656, 1728));
        // A viewer window of 1508x1046 pixels: limited by the width, rounding stays inside.
        assert_eq!(fit_within(RETINA_14, (1508, 1046)), (1504, 980));
        // A tall, narrow viewport: limited by the width.
        assert_eq!(fit_within(PATTERN_DISPLAY, (1000, 2000)), (992, 560));
    }

    #[test]
    fn fitted_sizes_stay_inside_the_viewport_and_the_rounding() {
        for native in [
            RETINA_14,
            PATTERN_DISPLAY,
            (1508, 1046),
            (3440, 1440),
            (1366, 768),
        ] {
            for w in (64..=4000).step_by(97) {
                for h in (64..=2400).step_by(89) {
                    let (fw, fh) = fit_within(native, (w, h));
                    assert!(
                        fw <= w.max(16) && fh <= h,
                        "{native:?} into {w}x{h}: {fw}x{fh}"
                    );
                    assert!(fw <= native.0 && fh <= native.1, "{native:?}: {fw}x{fh}");
                    assert_eq!((fw % 16, fh % 4), (0, 0), "{native:?} into {w}x{h}");
                }
            }
        }
    }

    #[test]
    fn fitting_a_tier_size_again_changes_nothing() {
        // Convergence: a viewer whose viewport is exactly what it receives asks for the same.
        for native in [RETINA_14, PATTERN_DISPLAY, (1508, 1046), (5120, 2880)] {
            for tier in [
                Tier::P720,
                Tier::P1080,
                Tier::P1440,
                Tier::P2160,
                Tier::Native,
            ] {
                let size = tier.size(native);
                assert_eq!(fit_within(native, size), size, "{tier:?} on {native:?}");
                let fitted = fit_within(native, (size.0 + 7, size.1 + 3));
                assert_eq!(fit_within(native, fitted), fitted, "{tier:?} on {native:?}");
            }
        }
    }

    #[test]
    fn caps_each_side() {
        assert_eq!(capped((3840, 2160), Some((1920, 1080))), (1920, 1080));
        assert_eq!(capped((1280, 720), Some((1920, 1080))), (1280, 720));
        assert_eq!(capped((1280, 720), None), (1280, 720));
    }

    #[test]
    fn bitrate_grows_with_tier() {
        let rates: Vec<_> = [Tier::P720, Tier::P1080, Tier::P1440, Tier::P2160]
            .map(Tier::default_bitrate_kbps)
            .to_vec();
        assert!(rates.windows(2).all(|w| w[0] < w[1]));
        assert!(Tier::Native.default_bitrate_kbps() > Tier::P1440.default_bitrate_kbps());
    }
}
