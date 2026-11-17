//! ScreenCaptureKit backend. SCK scales and converts to NV12 on the GPU, so the CPU only
//! copies planes out of the pixel buffer.

use livekit::webrtc::video_frame::NV12Buffer;
use screencapturekit::cm::CMSampleBufferExt;
use screencapturekit::cv::CVPixelBufferLockFlags;
use screencapturekit::prelude::*;
use screencapturekit::stream::configuration::{color_matrix, SCCaptureResolutionType};

use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;
use std::sync::{mpsc, Mutex};
use std::time::Duration;

use screencapturekit::stream::ErrorHandler;
use tokio::sync::oneshot;

use super::select::{self, is_twincursor, AppInfo, Process, WindowInfo};
use super::{wall_clock_us, CaptureConfig, CaptureError, Frame, FrameBuffer, Source};

/// How often a display capture looks for TwinCursor windows that appeared since it started
/// (a viewer started after the sharer), to leave them out too.
const WATCH_EVERY: Duration = Duration::from_secs(1);

/// Capture is stopped when the stream is dropped.
pub struct SckCapture {
    stream: SCStream,
    /// Dropping it stops the thread that keeps TwinCursor windows out of a display capture.
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

/// This process, for telling TwinCursor's windows apart from everyone else's.
fn me() -> (i32, Option<PathBuf>) {
    let exe = std::env::current_exe()
        .ok()
        .map(|p| p.canonicalize().unwrap_or(p));
    (i32::try_from(std::process::id()).unwrap_or(-1), exe)
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

/// The running apps that are TwinCursor: this process and any other copy of it.
fn twincursor_apps(content: &SCShareableContent) -> Vec<SCRunningApplication> {
    let (pid, exe) = me();
    let me = Process {
        pid,
        exe: exe.as_deref(),
        bundle_id: "",
    };
    content
        .applications()
        .into_iter()
        .filter(|app| {
            let path = exe_path(app.process_id());
            let bundle_id = app.bundle_identifier();
            let candidate = Process {
                pid: app.process_id(),
                exe: path.as_deref(),
                bundle_id: &bundle_id,
            };
            is_twincursor(&candidate, &me)
        })
        .collect()
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
    let windows = content
        .windows()
        .iter()
        .filter(|w| is_shareable(w))
        .filter_map(|w| {
            let app = w.owning_application()?;
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

/// The whole display without any TwinCursor window: this app's own, such as the
/// remote-control overlay, and a viewer running on this Mac, which would otherwise show the
/// picture inside itself. Also returns the process ids left out. A command-line program
/// without windows isn't listed, so it needs nothing left out.
fn display_filter(
    content: &SCShareableContent,
    display: &SCDisplay,
) -> (SCResult<SCContentFilter>, Vec<i32>) {
    let ours = twincursor_apps(content);
    let pids = pids_of(&ours);
    if !ours.is_empty() {
        let names: Vec<String> = ours
            .iter()
            .map(|a| format!("{} ({})", a.application_name(), a.process_id()))
            .collect();
        tracing::info!(
            "leaving TwinCursor windows out of the capture: {}",
            names.join(", ")
        );
    }
    let refs: Vec<&SCRunningApplication> = ours.iter().collect();
    let builder = SCContentFilter::create().with_display(display);
    let builder = if refs.is_empty() {
        builder.with_excluding_windows(&[])
    } else {
        builder.with_excluding_applications(&refs, &[])
    };
    (builder.build(), pids)
}

/// Keeps a display capture's filter up to date as TwinCursor apps start and quit, until
/// `stop` is dropped. Runs off the frame path: SCK applies a new filter between frames.
fn watch_ours(stream: SCStream, mut pids: Vec<i32>) -> mpsc::Sender<()> {
    let (stop, stopped) = mpsc::channel::<()>();
    std::thread::spawn(move || {
        while let Err(mpsc::RecvTimeoutError::Timeout) = stopped.recv_timeout(WATCH_EVERY) {
            let Ok(content) = SCShareableContent::get() else {
                continue;
            };
            let Some(display) = content.displays().into_iter().next() else {
                continue;
            };
            if pids_of(&twincursor_apps(&content)) == pids {
                continue;
            }
            let (filter, now) = display_filter(&content, &display);
            match filter.map(|f| stream.update_content_filter(&f)) {
                Ok(Ok(())) => pids = now,
                Ok(Err(e)) => tracing::warn!("couldn't update the capture filter: {e}"),
                Err(e) => tracing::warn!("couldn't build the capture filter: {e}"),
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
) -> Result<(SCContentFilter, String), CaptureError> {
    let (filter, name) = match source {
        Source::Display => {
            let (filter, _) = display_filter(content, display);
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

pub fn start(
    config: CaptureConfig,
    sink: impl Fn(Frame) + Send + Sync + 'static,
    ended: oneshot::Sender<String>,
) -> Result<SckCapture, CaptureError> {
    let content = SCShareableContent::get().map_err(|_| CaptureError::NoDisplay)?;
    let display = content
        .displays()
        .into_iter()
        .next()
        .ok_or(CaptureError::NoDisplay)?;
    connect_window_server();
    let (filter, name) = filter_for(config.source, &content, &display)?;

    // A window that fits in the chosen tier is captured at its own pixel size and cropped
    // to the encoder's (`Tier::size` rounds the width down to a multiple of 16 and the height
    // to 4), rather than scaled by a fraction of a percent, which would blur every pixel.
    let window_px = match config.source {
        Source::Window(_) => Some(filter_pixels(&filter)),
        _ => None,
    };
    let crop = window_px.filter(|&(w, h)| {
        (config.width..config.width + 16).contains(&w)
            && (config.height..config.height + 4).contains(&h)
    });
    let (width, height) = crop.unwrap_or((config.width, config.height));
    let mut stream_config = SCStreamConfiguration::new()
        .with_width(width)
        .with_height(height)
        // Video-range NV12 matches what WebRTC's I420 pipeline expects; BT.601 matches
        // libyuv's default conversion on the viewer.
        .with_pixel_format(PixelFormat::YCbCr_420v)
        .with_color_matrix(color_matrix::ITU_R_601_4)
        .map_err(backend_err)?
        .with_minimum_frame_interval(&CMTime::new(1, config.max_fps as i32))
        .with_shows_cursor(config.show_cursor)
        .with_queue_depth(3);
    if crop.is_some() {
        stream_config = stream_config
            .with_capture_resolution_type(SCCaptureResolutionType::Best)
            .map_err(backend_err)?;
    }
    let crop = crop.map(|_| (config.width as usize, config.height as usize));
    match window_px {
        Some((w, h)) => tracing::info!(
            "capturing {name}: {w}x{h} px, sent as {}x{} ({})",
            config.width,
            config.height,
            if crop.is_some() {
                "1:1, cropped"
            } else {
                "scaled"
            }
        ),
        None => tracing::info!("capturing {name}"),
    }

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
    stream
        .add_output_handler(
            move |sample: CMSampleBuffer, _: SCStreamOutputType| {
                if let Some(frame) = to_frame(&sample, crop) {
                    sink(frame);
                }
            },
            SCStreamOutputType::Screen,
        )
        .map_err(backend_err)?;
    stream.start_capture().map_err(backend_err)?;
    let watch = (config.source == Source::Display)
        .then(|| watch_ours(stream.clone(), pids_of(&twincursor_apps(&content))));
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
