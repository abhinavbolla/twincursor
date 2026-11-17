//! ScreenCaptureKit backend. SCK scales and converts to NV12 on the GPU, so the CPU only
//! copies planes out of the pixel buffer.

use livekit::webrtc::video_frame::NV12Buffer;
use screencapturekit::cm::CMSampleBufferExt;
use screencapturekit::cv::CVPixelBufferLockFlags;
use screencapturekit::prelude::*;
use screencapturekit::stream::configuration::{color_matrix, SCCaptureResolutionType};

use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use screencapturekit::stream::ErrorHandler;
use tokio::sync::{mpsc as tokio_mpsc, oneshot};

use super::select::{self, is_twincursor, AppInfo, Process, WindowInfo};
use super::{
    wall_clock_us, CaptureConfig, CaptureError, Frame, FrameBuffer, Resize, Source, Tier,
    WindowRect,
};

/// How often a display capture looks for TwinCursor windows that appeared since it started
/// (a viewer started after the sharer), to leave them out too.
const WATCH_DISPLAY_EVERY: Duration = Duration::from_secs(1);

/// How often a window capture checks the window's size.
const WATCH_WINDOW_EVERY: Duration = Duration::from_millis(200);

/// A new window size is applied once it has held this long, so dragging a window's edge
/// reconfigures the stream once, after the drag stops, rather than on every frame.
const WINDOW_SETTLE: Duration = Duration::from_millis(600);

/// Capture is stopped when the stream is dropped.
pub struct SckCapture {
    stream: SCStream,
    /// Dropping it stops the thread that keeps the filter or the size up to date.
    _watch: Option<mpsc::Sender<()>>,
}

// SCStream wraps an Objective-C object that SCK allows to be used from any thread.
unsafe impl Send for SckCapture {}

impl Drop for SckCapture {
    fn drop(&mut self) {
        let _ = self.stream.stop_capture();
    }
}

fn backend_err(e: impl std::fmt::Display) -> CaptureError {
    CaptureError::Backend(e.to_string())
}

/// Path of a process's executable.
fn exe_path(pid: i32) -> Option<PathBuf> {
    let mut buf = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    // SAFETY: the buffer is as large as we say it is.
    let len = unsafe { libc::proc_pidpath(pid, buf.as_mut_ptr().cast(), buf.len() as u32) };
    if len <= 0 {
        return None;
    }
    buf.truncate(len as usize);
    let path = PathBuf::from(std::ffi::OsString::from_vec(buf));
    Some(path.canonicalize().unwrap_or(path))
}

/// SCK's process ids are `pid_t`. An id that doesn't fit matches no app.
fn pid_i32(pid: u32) -> i32 {
    i32::try_from(pid).unwrap_or(-1)
}

/// Which apps a display capture leaves out: always this process; with `ours`, every
/// TwinCursor process ([`is_twincursor`]); and `extra`, an app named by process id (the
/// desktop app that started this one, `--exclude-app-pid`).
struct Exclusion {
    pid: i32,
    exe: Option<PathBuf>,
    ours: bool,
    extra: Option<i32>,
}

impl Exclusion {
    fn new(ours: bool, extra: Option<u32>) -> Self {
        let exe = std::env::current_exe()
            .ok()
            .map(|p| p.canonicalize().unwrap_or(p));
        Self {
            pid: pid_i32(std::process::id()),
            exe,
            ours,
            extra: extra.map(pid_i32),
        }
    }

    fn leaves_out(&self, app: &SCRunningApplication) -> bool {
        let pid = app.process_id();
        if pid == self.pid || Some(pid) == self.extra {
            return true;
        }
        if !self.ours {
            return false;
        }
        let path = exe_path(pid);
        let bundle_id = app.bundle_identifier();
        let candidate = Process {
            pid,
            exe: path.as_deref(),
            bundle_id: &bundle_id,
        };
        let me = Process {
            pid: self.pid,
            exe: self.exe.as_deref(),
            bundle_id: "",
        };
        is_twincursor(&candidate, &me)
    }

    /// The running apps to leave out. An app is only listed while it has a window: a
    /// command-line program without one needs nothing left out.
    fn apps(&self, content: &SCShareableContent) -> Vec<SCRunningApplication> {
        content
            .applications()
            .into_iter()
            .filter(|app| self.leaves_out(app))
            .collect()
    }
}

/// Windows a person would pick from: on screen, at the normal level, with an owner and big
/// enough to see.
fn is_shareable(window: &SCWindow) -> bool {
    let frame = window.frame();
    window.window_layer() == 0
        && window.owning_application().is_some()
        && frame.size.width >= 40.0
        && frame.size.height >= 40.0
}

/// Pixel size of a window: its frame in points times its display's scale, as SCK reports
/// for a single-window filter.
fn window_pixels(window: &SCWindow) -> Option<(u32, u32)> {
    let filter = SCContentFilter::create().with_window(window).build().ok()?;
    let size = filter_pixels(&filter);
    (size.0 > 0 && size.1 > 0).then_some(size)
}

/// A filter's size and scale come from CoreGraphics, which asserts unless this process has
/// connected to the window server; command-line tools haven't yet. This connects.
fn connect_window_server() {
    // SAFETY: plain CoreGraphics call.
    unsafe { CGMainDisplayID() };
}

pub fn shareable() -> Result<(Vec<WindowInfo>, Vec<AppInfo>), CaptureError> {
    connect_window_server();
    let content = SCShareableContent::create()
        .with_on_screen_windows_only(true)
        .with_exclude_desktop_windows(true)
        .get()
        .map_err(|_| CaptureError::NoDisplay)?;
    // Never offer TwinCursor's own windows (a viewer): sharing one is the mirror again.
    let ours = pids_of(&Exclusion::new(true, None).apps(&content));
    let windows = content
        .windows()
        .iter()
        .filter(|w| is_shareable(w))
        .filter_map(|w| {
            let app = w.owning_application()?;
            if ours.contains(&app.process_id()) {
                return None;
            }
            Some(WindowInfo {
                id: w.window_id(),
                app: app.application_name(),
                pid: app.process_id(),
                title: w.title().unwrap_or_default(),
                pixels: window_pixels(w)?,
            })
        })
        .collect::<Vec<_>>();
    let mut apps: Vec<AppInfo> = Vec::new();
    for app in content.applications() {
        // Only apps with a window someone could see.
        if windows.iter().any(|w| w.pid == app.process_id()) {
            apps.push(AppInfo {
                pid: app.process_id(),
                name: app.application_name(),
                bundle_id: app.bundle_identifier(),
            });
        }
    }
    Ok((windows, apps))
}

/// The whole display without the apps `exclusion` leaves out: this app's own windows, such
/// as the remote-control overlay, a viewer running on this Mac, which would otherwise show
/// the picture inside itself, and the desktop app that started this one. Also returns the
/// process ids left out.
fn display_filter(
    content: &SCShareableContent,
    display: &SCDisplay,
    exclusion: &Exclusion,
) -> (SCResult<SCContentFilter>, Vec<i32>) {
    let left_out = exclusion.apps(content);
    let pids = pids_of(&left_out);
    if !left_out.is_empty() {
        let names: Vec<String> = left_out
            .iter()
            .map(|a| format!("{} ({})", a.application_name(), a.process_id()))
            .collect();
        tracing::info!("leaving out of the capture: {}", names.join(", "));
    }
    if let Some(pid) = exclusion.extra.filter(|pid| !pids.contains(pid)) {
        tracing::info!("app {pid} has no windows yet; it's left out once it has one");
    }
    let refs: Vec<&SCRunningApplication> = left_out.iter().collect();
    let builder = SCContentFilter::create().with_display(display);
    let builder = if refs.is_empty() {
        builder.with_excluding_windows(&[])
    } else {
        builder.with_excluding_applications(&refs, &[])
    };
    (builder.build(), pids)
}

/// Keeps a display capture's filter up to date as apps to leave out start and quit:
/// ScreenCaptureKit can only leave out an app that has a window when the filter is made.
fn watch_display(stream: SCStream, exclusion: Exclusion, mut pids: Vec<i32>) -> mpsc::Sender<()> {
    let (stop, stopped) = mpsc::channel::<()>();
    std::thread::spawn(move || {
        while let Err(mpsc::RecvTimeoutError::Timeout) = stopped.recv_timeout(WATCH_DISPLAY_EVERY) {
            let Ok(content) = SCShareableContent::get() else {
                continue;
            };
            let Some(display) = content.displays().into_iter().next() else {
                continue;
            };
            if pids_of(&exclusion.apps(&content)) == pids {
                continue;
            }
            let (filter, now) = display_filter(&content, &display, &exclusion);
            match filter.map(|f| stream.update_content_filter(&f)) {
                Ok(Ok(())) => pids = now,
                Ok(Err(e)) => tracing::warn!("couldn't update the capture filter: {e}"),
                Err(e) => tracing::warn!("couldn't build the capture filter: {e}"),
            }
        }
    });
    stop
}

/// The size to capture a window at and the size to send, for a window of `window` pixels:
/// `tier.size(window)`, captured 1:1 and cropped when that's within `Tier::size`'s rounding
/// (a few pixels), scaled by ScreenCaptureKit otherwise.
struct WindowSize {
    capture: (u32, u32),
    crop: Option<(usize, usize)>,
}

fn window_size(window: (u32, u32), output: (u32, u32)) -> WindowSize {
    let fits = (output.0..output.0 + 16).contains(&window.0)
        && (output.1..output.1 + 4).contains(&window.1);
    if fits {
        WindowSize {
            capture: window,
            crop: Some((output.0 as usize, output.1 as usize)),
        }
    } else {
        WindowSize {
            capture: output,
            crop: None,
        }
    }
}

fn stream_config(
    config: &CaptureConfig,
    size: &WindowSize,
) -> Result<SCStreamConfiguration, CaptureError> {
    let stream_config = SCStreamConfiguration::new()
        .with_width(size.capture.0)
        .with_height(size.capture.1)
        // Video-range NV12 matches what WebRTC's I420 pipeline expects; BT.601 matches
        // libyuv's default conversion on the viewer.
        .with_pixel_format(PixelFormat::YCbCr_420v)
        .with_color_matrix(color_matrix::ITU_R_601_4)
        .map_err(backend_err)?
        .with_minimum_frame_interval(&CMTime::new(1, config.max_fps as i32))
        .with_shows_cursor(config.show_cursor)
        .with_queue_depth(3);
    if size.crop.is_some() {
        stream_config
            .with_capture_resolution_type(SCCaptureResolutionType::Best)
            .map_err(backend_err)
    } else {
        Ok(stream_config)
    }
}

/// Makes a window capture follow the window's size: once a new size has held for
/// [`WINDOW_SETTLE`], reconfigures the stream for `tier.size(new)` and reports it on `resized`.
#[allow(clippy::too_many_arguments)]
fn watch_window(
    stream: SCStream,
    config: CaptureConfig,
    id: u32,
    tier: Tier,
    mut current: (u32, u32),
    crop: Arc<Mutex<Option<(usize, usize)>>>,
    resized: tokio_mpsc::UnboundedSender<Resize>,
) -> mpsc::Sender<()> {
    let (stop, stopped) = mpsc::channel::<()>();
    std::thread::spawn(move || {
        let mut pending: Option<((u32, u32), Instant)> = None;
        while let Err(mpsc::RecvTimeoutError::Timeout) = stopped.recv_timeout(WATCH_WINDOW_EVERY) {
            // A closed window ends the capture through the stream's delegate; nothing to do.
            let Some(window) = SCShareableContent::get()
                .ok()
                .and_then(|c| c.windows().into_iter().find(|w| w.window_id() == id))
            else {
                continue;
            };
            let Ok(filter) = SCContentFilter::create().with_window(&window).build() else {
                continue;
            };
            let now = filter_pixels(&filter);
            if now == current || now.0 < 16 || now.1 < 4 {
                pending = None;
                continue;
            }
            match pending {
                Some((size, since)) if size == now => {
                    if since.elapsed() < WINDOW_SETTLE {
                        continue;
                    }
                }
                _ => {
                    pending = Some((now, Instant::now()));
                    continue;
                }
            }
            pending = None;
            let output = tier.size(now);
            let size = window_size(now, output);
            let applied = stream_config(&config, &size)
                .and_then(|c| stream.update_configuration(&c).map_err(backend_err));
            match applied {
                Ok(()) => {
                    *crop.lock().unwrap() = size.crop;
                    current = now;
                    tracing::info!(
                        "window resized to {}x{} px, sending {}x{} ({})",
                        now.0,
                        now.1,
                        output.0,
                        output.1,
                        if size.crop.is_some() {
                            "1:1, cropped"
                        } else {
                            "scaled"
                        }
                    );
                    let _ = resized.send(Resize {
                        window: now,
                        output,
                    });
                }
                Err(e) => tracing::warn!("couldn't follow the window's new size: {e}"),
            }
        }
    });
    stop
}

fn pids_of(apps: &[SCRunningApplication]) -> Vec<i32> {
    let mut pids: Vec<i32> = apps.iter().map(SCRunningApplication::process_id).collect();
    pids.sort_unstable();
    pids
}

/// The filter for `source`, and a name for it in messages.
fn filter_for(
    source: Source,
    content: &SCShareableContent,
    display: &SCDisplay,
    exclusion: &Exclusion,
) -> Result<(SCContentFilter, String), CaptureError> {
    let (filter, name) = match source {
        Source::Display => {
            let (filter, _) = display_filter(content, display, exclusion);
            (filter, "the display".to_owned())
        }
        Source::Window(id) => {
            let window = content
                .windows()
                .into_iter()
                .find(|w| w.window_id() == id)
                .ok_or_else(|| CaptureError::Gone(format!("window {id}")))?;
            let name = format!("window {id} ({})", window.title().unwrap_or_default());
            (SCContentFilter::create().with_window(&window).build(), name)
        }
        Source::App(pid) => {
            let app = content
                .applications()
                .into_iter()
                .find(|a| a.process_id() == pid)
                .ok_or_else(|| CaptureError::Gone(format!("app with process id {pid}")))?;
            let name = app.application_name();
            let filter = SCContentFilter::create()
                .with_display(display)
                .with_including_applications(&[&app], &[])
                .build();
            (filter, name)
        }
    };
    Ok((filter.map_err(backend_err)?, name))
}

/// The first display's size in points, and the on-screen windows of app `pid` on it.
pub fn app_windows(pid: u32) -> Option<((f64, f64), Vec<WindowRect>)> {
    let content = SCShareableContent::get().ok()?;
    let display = content.displays().into_iter().next()?;
    let screen = display.frame();
    let pid = pid_i32(pid);
    let windows = content
        .windows()
        .into_iter()
        .filter(|w| w.is_on_screen())
        .filter(|w| {
            w.owning_application()
                .is_some_and(|app| app.process_id() == pid)
        })
        .map(|w| {
            let frame = w.frame();
            WindowRect {
                id: w.window_id(),
                title: w.title().unwrap_or_default(),
                x: frame.origin.x - screen.origin.x,
                y: frame.origin.y - screen.origin.y,
                width: frame.size.width,
                height: frame.size.height,
            }
        })
        .collect();
    Some(((screen.size.width, screen.size.height), windows))
}

pub fn start(
    config: CaptureConfig,
    sink: impl Fn(Frame) + Send + Sync + 'static,
    ended: oneshot::Sender<String>,
    resized: tokio_mpsc::UnboundedSender<Resize>,
) -> Result<SckCapture, CaptureError> {
    let content = SCShareableContent::get().map_err(|_| CaptureError::NoDisplay)?;
    let display = content
        .displays()
        .into_iter()
        .next()
        .ok_or(CaptureError::NoDisplay)?;
    connect_window_server();
    let exclusion = Exclusion::new(config.leave_out_twincursor, config.exclude_app_pid);
    let (filter, name) = filter_for(config.source, &content, &display, &exclusion)?;

    // A window that fits in the chosen tier is captured at its own pixel size and cropped
    // to the encoder's (`Tier::size` rounds the width down to a multiple of 16 and the height
    // to 4), rather than scaled by a fraction of a percent, which would blur every pixel.
    let output = (config.width, config.height);
    let window_px = match config.source {
        Source::Window(_) => Some(filter_pixels(&filter)),
        _ => None,
    };
    let size = match window_px {
        Some(px) => window_size(px, output),
        None => WindowSize {
            capture: output,
            crop: None,
        },
    };
    let stream_config = stream_config(&config, &size)?;
    match window_px {
        Some((w, h)) => tracing::info!(
            "capturing {name}: {w}x{h} px, sent as {}x{} ({})",
            output.0,
            output.1,
            if size.crop.is_some() {
                "1:1, cropped"
            } else {
                "scaled"
            }
        ),
        None => tracing::info!("capturing {name}"),
    }
    let crop = Arc::new(Mutex::new(size.crop));

    // Capture ends by itself when the window or app goes away.
    let ended = Mutex::new(Some(ended));
    let delegate = ErrorHandler::new(move |e: SCError| {
        if let Some(tx) = ended.lock().unwrap().take() {
            let _ = tx.send(format!(
                "capture of {name} stopped (closed, or its app quit?): {e}"
            ));
        }
    });
    let mut stream =
        SCStream::new_with_delegate(&filter, &stream_config, delegate).map_err(backend_err)?;
    let frame_crop = crop.clone();
    stream
        .add_output_handler(
            move |sample: CMSampleBuffer, _: SCStreamOutputType| {
                let crop = *frame_crop.lock().unwrap();
                if let Some(frame) = to_frame(&sample, crop) {
                    sink(frame);
                }
            },
            SCStreamOutputType::Screen,
        )
        .map_err(backend_err)?;
    stream.start_capture().map_err(backend_err)?;
    let watch = match (config.source, window_px, config.follow_tier) {
        (Source::Display, _, _) => {
            let pids = pids_of(&exclusion.apps(&content));
            Some(watch_display(stream.clone(), exclusion, pids))
        }
        (Source::Window(id), Some(px), Some(tier)) => Some(watch_window(
            stream.clone(),
            config,
            id,
            tier,
            px,
            crop,
            resized,
        )),
        _ => None,
    };
    Ok(SckCapture {
        stream,
        _watch: watch,
    })
}

/// Pixel size of what a filter captures.
fn filter_pixels(filter: &SCContentFilter) -> (u32, u32) {
    let rect = filter.content_rect();
    let scale = f64::from(filter.point_pixel_scale());
    select::window_pixels((rect.size.width, rect.size.height), scale)
}

/// Copies an NV12 sample into a LiveKit buffer, keeping only the top-left `crop` pixels if
/// given. Returns `None` for idle samples, which SCK sends without pixels when the screen
/// hasn't changed.
fn to_frame(sample: &CMSampleBuffer, crop: Option<(usize, usize)>) -> Option<Frame> {
    let captured_at_us = capture_time_us(sample);
    let pixels = sample.pixel_buffer()?;
    let guard = pixels.lock(CVPixelBufferLockFlags::READ_ONLY).ok()?;
    if guard.plane_count() != 2 {
        return None;
    }
    let (mut width, mut height) = (guard.width(), guard.height());
    if let Some((w, h)) = crop {
        (width, height) = (width.min(w), height.min(h) & !1);
    }
    let mut buffer = NV12Buffer::new(width as u32, height as u32);
    let (dst_stride_y, dst_stride_uv) = buffer.strides();
    let (dst_y, dst_uv) = buffer.data_mut();
    for (plane, dst, dst_stride, rows) in [
        (0, dst_y, dst_stride_y as usize, height),
        (1, dst_uv, dst_stride_uv as usize, height / 2),
    ] {
        // SAFETY: the buffer is locked for reading until `guard` is dropped.
        let src = unsafe { guard.plane_data(plane) }?;
        let src_stride = guard.bytes_per_row_of_plane(plane);
        let row_len = width.min(dst_stride).min(src_stride);
        for row in 0..rows {
            dst[row * dst_stride..row * dst_stride + row_len]
                .copy_from_slice(&src[row * src_stride..row * src_stride + row_len]);
        }
    }
    Some(Frame {
        buffer: FrameBuffer::Nv12(buffer),
        captured_at_us,
    })
}

extern "C" {
    // Darwin libc; not exposed by the `libc` crate.
    fn clock_gettime_nsec_np(clock_id: libc::clockid_t) -> u64;
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGDisplayCopyDisplayMode(display: u32) -> *mut std::ffi::c_void;
    fn CGDisplayModeGetPixelWidth(mode: *mut std::ffi::c_void) -> usize;
    fn CGDisplayModeGetPixelHeight(mode: *mut std::ffi::c_void) -> usize;
    fn CGDisplayModeRelease(mode: *mut std::ffi::c_void);
    fn CGMainDisplayID() -> u32;
}

/// Pixel size of the first display. `SCDisplay` reports points, which is half the pixels
/// on a Retina panel, so ask CoreGraphics for the current mode's pixel size.
pub fn display_pixels() -> Option<(u32, u32)> {
    let content = SCShareableContent::get().ok()?;
    let display = content.displays().into_iter().next()?;
    // SAFETY: plain CoreGraphics calls; the mode is released after use.
    unsafe {
        let mode = CGDisplayCopyDisplayMode(display.display_id());
        if mode.is_null() {
            return None;
        }
        let size = (
            CGDisplayModeGetPixelWidth(mode) as u32,
            CGDisplayModeGetPixelHeight(mode) as u32,
        );
        CGDisplayModeRelease(mode);
        (size.0 > 0 && size.1 > 0).then_some(size)
    }
}

/// Wall-clock time at which SCK captured the sample, so the measured latency includes
/// SCK's own delay. SCK timestamps samples on the host clock (`CLOCK_UPTIME_RAW`).
fn capture_time_us(sample: &CMSampleBuffer) -> u64 {
    let now_wall = wall_clock_us();
    let pts = sample.presentation_timestamp();
    if pts.timescale <= 0 {
        return now_wall;
    }
    let pts_ns = pts.value as i128 * 1_000_000_000 / pts.timescale as i128;
    // SAFETY: plain libc call with a valid clock id.
    let now_host_ns = unsafe { clock_gettime_nsec_np(libc::CLOCK_UPTIME_RAW) } as i128;
    let age_us = (now_host_ns - pts_ns) / 1000;
    // Only trust a plausible age; otherwise fall back to the arrival time.
    if (0..1_000_000).contains(&age_us) {
        now_wall - age_us as u64
    } else {
        now_wall
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_window_within_the_rounding_is_captured_1_to_1_and_cropped() {
        let size = window_size((1508, 1046), (1504, 1044));
        assert_eq!(size.capture, (1508, 1046));
        assert_eq!(size.crop, Some((1504, 1044)));
    }

    #[test]
    fn a_window_bigger_than_its_tier_is_scaled() {
        let size = window_size((3000, 1900), (1664, 1052));
        assert_eq!(size.capture, (1664, 1052));
        assert_eq!(size.crop, None);
    }
}
