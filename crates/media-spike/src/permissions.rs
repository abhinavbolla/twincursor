//! `tc-spike permissions`: what macOS lets this process do, as one JSON object, for a desktop
//! app that starts this tool and needs to know whether its grants reach it. It reads Screen
//! Recording, Accessibility and Microphone access, can ask for the ones not yet decided, and can
//! check that capture works and leaves the desktop app's windows out. It never injects input.
//!
//! The permission report itself lives in `engine::permission`, which `tc-engine permissions`
//! prints too.

use std::io::Write;
#[cfg(target_os = "macos")]
use std::sync::{Arc, Mutex};
use std::time::Duration;
#[cfg(target_os = "macos")]
use std::time::Instant;

use clap::Args;
use serde::Serialize;

use engine::permission::{status, Access, Status};

/// Printed on its own line once the capture that leaves the app out is running, so the app can
/// open a window that wasn't there when capture started.
pub const CAPTURING: &str = "permissions: capture running";

#[derive(Args)]
pub struct PermissionsArgs {
    /// Ask macOS for the access not yet given. Screen Recording and Accessibility show their
    /// prompts and return at once; the microphone waits for its answer.
    #[arg(long)]
    request: bool,
    /// Keep running and print a new line whenever the answers change.
    #[arg(long, conflicts_with_all = ["request", "capture"])]
    watch: bool,
    /// Also capture the screen, to show the Screen Recording grant works in practice.
    #[arg(long)]
    capture: bool,
    /// With --capture: check that this app's windows stay out of the capture, including a
    /// window it opens after capture starts (watch stdout for the line it prints then).
    #[arg(long, requires = "capture")]
    exclude_app_pid: Option<u32>,
}

#[derive(Serialize)]
struct Report {
    #[serde(flatten)]
    status: Status,
    #[serde(skip_serializing_if = "Option::is_none")]
    capture: Option<CaptureReport>,
}

pub fn run(args: PermissionsArgs) -> anyhow::Result<()> {
    if args.watch {
        let mut last = None;
        loop {
            let now = status();
            if last.as_ref() != Some(&now) {
                print_line(&serde_json::to_string(&now)?);
                last = Some(now);
            }
            std::thread::sleep(Duration::from_secs(1));
        }
    }
    if args.request {
        engine::permission::request_missing(&status());
    }
    let status = status();
    let capture = args.capture.then(|| {
        if status.screen_recording == Access::Granted {
            capture_check(args.exclude_app_pid)
        } else {
            CaptureReport::failed("Screen Recording isn't allowed for this app")
        }
    });
    print_line(&serde_json::to_string(&Report { status, capture })?);
    Ok(())
}

/// Prints and flushes, so an app reading the pipe sees each line at once. A closed pipe means
/// nobody is listening any more, which isn't an error worth a panic.
fn print_line(line: &str) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

/// Exits the process when stdin reaches end of file: the app that started us has closed its end,
/// or died. Nothing else may read stdin while this runs.
pub fn exit_on_stdin_close() {
    engine::app::watch_stdin(|_| {}, || {});
}

#[derive(Debug, Default, Serialize)]
struct CaptureReport {
    /// Size of the frames captured, once one arrived.
    frame: Option<(u32, u32)>,
    error: Option<String>,
    /// The app's windows in a capture that doesn't leave it out, so the check is known to see
    /// them when they're there.
    included: Vec<WindowSample>,
    /// The same app's windows in a capture that leaves it out.
    excluded: Vec<WindowSample>,
}

impl CaptureReport {
    fn failed(error: &str) -> Self {
        Self {
            error: Some(error.to_owned()),
            ..Self::default()
        }
    }
}

#[derive(Debug, Serialize)]
struct WindowSample {
    id: u32,
    title: String,
    /// The window wasn't there yet when the capture started.
    opened_after_start: bool,
    /// Share of the window's middle that shows the probe color (see [`is_probe_color`]).
    probe_color: f64,
}

/// Probe windows are filled with pure magenta (#ff00ff). In video-range BT.601 NV12 that's
/// Cb ≈ 202 and Cr ≈ 222 on an sRGB display and a little less on a P3 one; few other colors
/// are high on both.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn is_probe_color(cb: u8, cr: u8) -> bool {
    cb >= 160 && cr >= 165
}

/// Share of the middle of a rectangle (in frame pixels) whose chroma is the probe color. The
/// outer fifth on each side is skipped, for window shadows and rounded corners.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn probe_share(uv: &[u8], stride_uv: usize, frame: (u32, u32), rect: [f64; 4]) -> f64 {
    let [x, y, w, h] = rect;
    let clamp = |v: f64, max: u32| (v.max(0.0) as u32).min(max);
    let (x0, x1) = (clamp(x + w * 0.2, frame.0), clamp(x + w * 0.8, frame.0));
    let (y0, y1) = (clamp(y + h * 0.2, frame.1), clamp(y + h * 0.8, frame.1));
    let (mut hits, mut total) = (0u64, 0u64);
    for cy in (y0 / 2)..(y1 / 2) {
        for cx in (x0 / 2)..(x1 / 2) {
            let i = cy as usize * stride_uv + cx as usize * 2;
            let (Some(&cb), Some(&cr)) = (uv.get(i), uv.get(i + 1)) else {
                continue;
            };
            total += 1;
            hits += u64::from(is_probe_color(cb, cr));
        }
    }
    if total == 0 {
        0.0
    } else {
        hits as f64 / total as f64
    }
}

#[cfg(target_os = "macos")]
fn capture_check(exclude: Option<u32>) -> CaptureReport {
    let mut report = CaptureReport::default();
    if let Err(e) = capture_check_into(exclude, &mut report) {
        report.error = Some(format!("{e:#}"));
    }
    report
}

#[cfg(not(target_os = "macos"))]
fn capture_check(_exclude: Option<u32>) -> CaptureReport {
    CaptureReport::failed("the capture check is macOS only")
}

/// The latest frame's chroma plane, copied out of the capture callback.
#[cfg(target_os = "macos")]
#[derive(Default)]
struct Latest {
    size: (u32, u32),
    stride_uv: usize,
    uv: Vec<u8>,
    frames: u64,
}

#[cfg(target_os = "macos")]
struct Grab {
    _handle: media::capture::CaptureHandle,
    latest: Arc<Mutex<Latest>>,
}

#[cfg(target_os = "macos")]
impl Grab {
    fn start(size: (u32, u32), display: u32, exclude_app_pid: Option<u32>) -> anyhow::Result<Self> {
        use media::capture::{self, Backend, CaptureConfig, FrameBuffer};
        let latest = Arc::new(Mutex::new(Latest::default()));
        let sink = latest.clone();
        let config = CaptureConfig {
            width: size.0,
            height: size.1,
            max_fps: 30,
            show_cursor: false,
            source: capture::Source::Display,
            display: Some(display),
            exclude_app_pid,
            // The check must see the desktop app's windows when it doesn't leave them out, and
            // the desktop app is TwinCursor: only `exclude_app_pid` decides here.
            leave_out_twincursor: false,
            follow_tier: None,
        };
        let handle = capture::start(Backend::ScreenCaptureKit, config, move |frame| {
            let FrameBuffer::Nv12(buffer) = &frame.buffer else {
                return;
            };
            let (_, uv) = buffer.data();
            let (_, stride_uv) = buffer.strides();
            let mut latest = sink.lock().unwrap();
            latest.size = (frame.buffer.width(), frame.buffer.height());
            latest.stride_uv = stride_uv as usize;
            latest.uv.clear();
            latest.uv.extend_from_slice(uv);
            latest.frames += 1;
        })?;
        Ok(Self {
            _handle: handle,
            latest,
        })
    }

    /// Waits for the first frame, then `settle` more for later ones.
    fn wait(&self, settle: Duration) -> anyhow::Result<()> {
        let start = Instant::now();
        while self.latest.lock().unwrap().frames == 0 {
            anyhow::ensure!(
                start.elapsed() < Duration::from_secs(5),
                "no frame arrived within 5 s"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        std::thread::sleep(settle);
        Ok(())
    }

    fn measure(
        &self,
        pid: u32,
        display: u32,
        known_before: &[u32],
    ) -> anyhow::Result<Vec<WindowSample>> {
        let (screen, windows) = media::capture::app_windows(pid, display)
            .ok_or_else(|| anyhow::anyhow!("no display"))?;
        let latest = self.latest.lock().unwrap();
        let sx = f64::from(latest.size.0) / screen.0;
        let sy = f64::from(latest.size.1) / screen.1;
        Ok(windows
            .into_iter()
            .map(|w| WindowSample {
                probe_color: probe_share(
                    &latest.uv,
                    latest.stride_uv,
                    latest.size,
                    [w.x * sx, w.y * sy, w.width * sx, w.height * sy],
                ),
                opened_after_start: !known_before.contains(&w.id),
                id: w.id,
                title: w.title,
            })
            .collect())
    }
}

#[cfg(target_os = "macos")]
fn capture_check_into(exclude: Option<u32>, report: &mut CaptureReport) -> anyhow::Result<()> {
    // The display a share would capture, so the check sees what a guest would.
    let display = engine::screen::choose(media::capture::Backend::ScreenCaptureKit, None)
        .map_err(anyhow::Error::msg)?
        .id;
    let pixels =
        media::capture::display_pixels(display).unwrap_or(media::capture::FALLBACK_DISPLAY);
    // Half size is plenty to find a window, and cheap.
    let size = ((pixels.0 / 2) / 16 * 16, (pixels.1 / 2) & !1);
    let window_ids = |pid| {
        media::capture::app_windows(pid, display)
            .map(|(_, w)| w.into_iter().map(|w| w.id).collect::<Vec<_>>())
            .unwrap_or_default()
    };

    let Some(pid) = exclude else {
        let grab = Grab::start(size, display, None)?;
        grab.wait(Duration::ZERO)?;
        report.frame = Some(grab.latest.lock().unwrap().size);
        return Ok(());
    };

    // 1. Without leaving the app out: its probe windows must show up.
    {
        let grab = Grab::start(size, display, None)?;
        grab.wait(Duration::from_millis(500))?;
        report.frame = Some(grab.latest.lock().unwrap().size);
        let known = window_ids(pid);
        report.included = grab.measure(pid, display, &known)?;
    }

    // 2. Leaving it out, while it opens another window.
    let known = window_ids(pid);
    let grab = Grab::start(size, display, Some(pid))?;
    grab.wait(Duration::ZERO)?;
    print_line(CAPTURING);
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(5)
        && window_ids(pid).iter().all(|id| known.contains(id))
    {
        std::thread::sleep(Duration::from_millis(100));
    }
    // ScreenCaptureKit sends a frame whenever what it captures changes, so a window that leaked
    // into the capture would be in the latest frame by now.
    std::thread::sleep(Duration::from_secs(1));
    report.excluded = grab.measure(pid, display, &known)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A chroma plane of `w`x`h` chroma samples, neutral gray, with the probe color in `rect`
    /// (in chroma samples).
    fn plane(w: usize, h: usize, rect: (usize, usize, usize, usize)) -> Vec<u8> {
        let mut uv = vec![128u8; w * h * 2];
        for y in rect.1..rect.1 + rect.3 {
            for x in rect.0..rect.0 + rect.2 {
                uv[y * w * 2 + x * 2] = 202;
                uv[y * w * 2 + x * 2 + 1] = 222;
            }
        }
        uv
    }

    #[test]
    fn probe_color_is_magenta_only() {
        assert!(is_probe_color(202, 222)); // sRGB magenta
        assert!(is_probe_color(188, 194)); // magenta on a P3 display
        assert!(!is_probe_color(128, 128)); // grays
        assert!(!is_probe_color(90, 240)); // red
        assert!(!is_probe_color(240, 110)); // blue
    }

    #[test]
    fn finds_a_probe_window_and_its_absence() {
        // 200x100 frame, so 100x50 chroma samples; probe at frame pixels (40, 20) 80x40.
        let uv = plane(100, 50, (20, 10, 40, 20));
        let share = probe_share(&uv, 200, (200, 100), [40.0, 20.0, 80.0, 40.0]);
        assert!(share > 0.99, "{share}");
        let elsewhere = probe_share(&uv, 200, (200, 100), [130.0, 50.0, 60.0, 40.0]);
        assert_eq!(elsewhere, 0.0);
    }

    #[test]
    fn rectangles_off_the_frame_are_clamped() {
        let uv = plane(100, 50, (0, 0, 0, 0));
        assert_eq!(
            probe_share(&uv, 200, (200, 100), [-50.0, -50.0, 1000.0, 1000.0]),
            0.0
        );
        assert_eq!(
            probe_share(&uv, 200, (200, 100), [500.0, 500.0, 10.0, 10.0]),
            0.0
        );
    }
}
