//! The `view` subcommand: a winit + softbuffer window that shows the newest decoded frame.
//!
//! winit must own the main thread on macOS, so LiveKit runs on a Tokio runtime in a
//! background thread. Frames are handed over through a single "latest frame" slot, so a
//! slow redraw drops old frames instead of queueing them.

use std::num::NonZeroU32;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Context;
use clap::Args;
use livekit::webrtc::native::yuv_helper;
use livekit::webrtc::video_frame::VideoBuffer;
use media::capture::wall_clock_us;
use media::latency::Recorder;
use media::netstats::Board;
use media::quality::Snapshots;
use media::stats::{summarize, Summary};
use media::subscribe::{self, DecodedFrame, ViewConfig};
use serde_json::json;
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::Key;
use winit::window::{Window, WindowId};

use crate::{hud, Connection};

const REPORT_EVERY: Duration = Duration::from_secs(10);

#[derive(Args)]
pub struct ViewArgs {
    #[command(flatten)]
    connection: Connection,
    /// Interval between latency ticks, in milliseconds. Not a multiple of the frame time:
    /// at 250 ms (15 frames at 60 fps) every tick lands at the same point of the frame clock.
    #[arg(long, default_value_t = 247)]
    tick_ms: u64,
    /// Stop automatically after this many seconds.
    #[arg(long)]
    duration: Option<u64>,
    /// Label stored in the results file, e.g. "sck-vp8-static".
    #[arg(long, default_value = "run")]
    label: String,
    /// Don't poll network stats or show the HUD.
    #[arg(long)]
    no_stats: bool,
    /// Decode and measure without opening a window (for benchmarks and the network lab).
    /// Latency is then measured at decode.
    #[arg(long)]
    headless: bool,
    /// Score frames against the synthetic pattern (needs `share --capture pattern`).
    #[arg(long)]
    measure_quality: bool,
    /// Measure the data-packet path remote-control input takes (reliable vs lossy).
    #[arg(long)]
    measure_input: bool,
    /// Save the frame carrying every Nth latency tick as a PGM (see `tc-spike compare`).
    #[arg(long, default_value_t = 0)]
    snapshot_every: u32,
    #[arg(long, default_value = "bench/snapshots")]
    snapshot_dir: PathBuf,
    /// Directory for the JSON results file.
    #[arg(long, default_value = "bench/results")]
    out: PathBuf,
}

#[derive(Debug)]
enum UserEvent {
    NewFrame,
    Stop,
}

/// Newest decoded frame, plus a flag so we only wake the event loop once per redraw.
#[derive(Default)]
struct Latest {
    frame: Mutex<Option<DecodedFrame>>,
    wake_pending: AtomicBool,
}

fn view_config(args: &ViewArgs) -> anyhow::Result<ViewConfig> {
    Ok(ViewConfig {
        url: args.connection.url.clone(),
        token: args.connection.token("viewer")?,
        tick_interval: Duration::from_millis(args.tick_ms),
        net_stats: !args.no_stats,
        measure_quality: args.measure_quality,
        measure_input: args.measure_input,
        snapshots: (args.snapshot_every > 0).then(|| Snapshots {
            every: args.snapshot_every,
            dir: args.snapshot_dir.clone(),
        }),
    })
}

pub fn run(args: ViewArgs) -> anyhow::Result<()> {
    if args.headless {
        return run_headless(args);
    }
    let event_loop = EventLoop::<UserEvent>::with_user_event().build()?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let proxy = event_loop.create_proxy();

    let latest = Arc::new(Latest::default());
    let recorder = Arc::new(Mutex::new(Recorder::default()));
    let board = Arc::new(Mutex::new(Board::default()));
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();

    let config = view_config(&args)?;
    let network = {
        let latest = latest.clone();
        let recorder = recorder.clone();
        let board = board.clone();
        let proxy = proxy.clone();
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
            let wake = proxy.clone();
            let on_frame = move |frame: DecodedFrame| {
                *latest.frame.lock().unwrap() = Some(frame);
                if !latest.wake_pending.swap(true, Ordering::AcqRel) {
                    let _ = wake.send_event(UserEvent::NewFrame);
                }
            };
            let shutdown = async {
                let _ = stop_rx.await;
            };
            let result =
                runtime.block_on(subscribe::run(config, recorder, board, on_frame, shutdown));
            if let Err(e) = &result {
                tracing::error!("viewer failed: {e}");
            }
            let _ = proxy.send_event(UserEvent::Stop);
        })
    };

    if let Some(secs) = args.duration {
        let proxy = proxy.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(secs));
            let _ = proxy.send_event(UserEvent::Stop);
        });
    }

    let started = Instant::now();
    let mut app = App {
        latest,
        recorder: recorder.clone(),
        board: board.clone(),
        hud: !args.no_stats,
        recent_rtt: None,
        rtt_mark: 0,
        window: None,
        surface: None,
        size: (0, 0),
        frame_size: (0, 0),
        last_report: Instant::now(),
        frames_since_report: 0,
    };
    event_loop.run_app(&mut app)?;

    let _ = stop_tx.send(());
    let _ = network.join();
    let recorder = recorder.lock().unwrap();
    let board = board.lock().unwrap();
    write_results(&args, &recorder, &board, started.elapsed(), app.frame_size)
}

struct App {
    latest: Arc<Latest>,
    recorder: Arc<Mutex<Recorder>>,
    board: Arc<Mutex<Board>>,
    /// HUD visibility, toggled with the S key.
    hud: bool,
    /// Tick round trip over the last report window, shown in the HUD.
    recent_rtt: Option<Summary>,
    /// Index into the recorder's RTT samples where the current report window starts.
    rtt_mark: usize,
    window: Option<Arc<Window>>,
    surface: Option<softbuffer::Surface<Arc<Window>, Arc<Window>>>,
    /// Size we draw at: the frame size, or smaller if the frame doesn't fit on the monitor.
    size: (u32, u32),
    /// Size of the frames we receive.
    frame_size: (u32, u32),
    last_report: Instant,
    frames_since_report: u64,
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attributes = Window::default_attributes()
            .with_title("TwinCursor M1 viewer")
            .with_inner_size(PhysicalSize::new(1662, 1080));
        let window = Arc::new(event_loop.create_window(attributes).expect("create window"));
        let context = softbuffer::Context::new(window.clone()).expect("softbuffer context");
        self.surface = Some(softbuffer::Surface::new(&context, window.clone()).expect("surface"));
        self.window = Some(window);
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::NewFrame => self.present_latest(),
            UserEvent::Stop => event_loop.exit(),
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::KeyboardInput { event, .. }
                if event.state == ElementState::Pressed
                    && matches!(&event.logical_key, Key::Character(c) if c.eq_ignore_ascii_case("s")) =>
            {
                self.hud = !self.hud;
            }
            _ => {}
        }
    }
}

impl App {
    /// Converts the newest frame straight into the window's pixel buffer and presents it.
    /// Presenting here rather than on `RedrawRequested` avoids waiting for the next vsync.
    fn present_latest(&mut self) {
        self.latest.wake_pending.store(false, Ordering::Release);
        let Some(frame) = self.latest.frame.lock().unwrap().take() else {
            return;
        };
        let (Some(window), Some(surface)) = (&self.window, &mut self.surface) else {
            return;
        };

        let frame_size = (frame.i420.width(), frame.i420.height());
        if frame_size != self.frame_size {
            // Show frames 1:1 for the sharpest text, unless they don't fit on this monitor.
            // The window only grows: when the sharer's auto-throttle drops to a lower tier,
            // frames are scaled up to the window instead of the window jumping around.
            if frame_size.0 > self.size.0 || frame_size.1 > self.size.1 {
                let monitor = window
                    .current_monitor()
                    .map(|m| (m.size().width, m.size().height));
                let (w, h) = monitor.map_or(frame_size, |m| fit_within(frame_size, usable(m)));
                let _ = window.request_inner_size(PhysicalSize::new(w, h));
                surface
                    .resize(NonZeroU32::new(w).unwrap(), NonZeroU32::new(h).unwrap())
                    .expect("resize surface");
                self.size = (w, h);
            }
            self.frame_size = frame_size;
            tracing::info!(
                "receiving {}x{}, showing at {}x{}",
                frame_size.0,
                frame_size.1,
                self.size.0,
                self.size.1
            );
        }
        let (w, h) = self.size;
        let mut i420 = frame.i420;
        if (w, h) != frame_size {
            i420 = i420.scale(w as i32, h as i32);
        }

        let mut buffer = surface.buffer_mut().expect("buffer");
        // softbuffer pixels are 0RGB u32s, i.e. B,G,R,X bytes on little-endian, which is
        // what libyuv calls ARGB.
        // SAFETY: reinterpreting initialized u32s as bytes; alignment only gets looser.
        let bytes = unsafe {
            std::slice::from_raw_parts_mut(buffer.as_mut_ptr().cast::<u8>(), buffer.len() * 4)
        };
        let (y, u, v) = i420.data();
        let (sy, su, sv) = i420.strides();
        yuv_helper::i420_to_argb(y, sy, u, su, v, sv, bytes, w * 4, w as i32, h as i32);
        if self.hud {
            let lines = hud::lines(&self.board.lock().unwrap(), self.recent_rtt);
            hud::draw(&mut buffer, w as usize, h as usize, &lines);
        }
        buffer.present().expect("present");

        let presented_at = Instant::now();
        self.recorder.lock().unwrap().frame_presented(
            frame.marker,
            frame.decoded_at,
            presented_at,
            frame.captured_at_us,
            wall_clock_us(),
        );
        self.frames_since_report += 1;
        if self.last_report.elapsed() >= REPORT_EVERY {
            self.report();
        }
    }

    fn report(&mut self) {
        let fps = self.frames_since_report as f64 / self.last_report.elapsed().as_secs_f64();
        self.recent_rtt = log_report(&self.recorder, &self.board, &mut self.rtt_mark, fps);
        self.last_report = Instant::now();
        self.frames_since_report = 0;
    }
}

/// Logs the latency, quality and network summary for the last window. Returns the round trip
/// over that window, which the HUD shows.
fn log_report(
    recorder: &Mutex<Recorder>,
    board: &Mutex<Board>,
    rtt_mark: &mut usize,
    fps: f64,
) -> Option<Summary> {
    let (recent, report) = {
        let recorder = recorder.lock().unwrap();
        let samples = &recorder.rtt_presented;
        let recent = summarize(&samples[(*rtt_mark).min(samples.len())..]);
        *rtt_mark = samples.len();
        (recent, recorder.report())
    };
    let fmt = |s: Option<Summary>| s.map_or("n/a".to_owned(), |s| s.to_string());
    tracing::info!(
        "{fps:.1} fps | RTT (tick→presented) last 10 s {} | run {} | one-way (capture→presented) {}",
        fmt(recent),
        fmt(report.rtt_presented),
        fmt(report.one_way_presented),
    );
    if let (Some(ssim), Some(psnr)) = (report.ssim, report.psnr_db) {
        let fast = report.ssim_fast_scroll.map_or("n/a".to_owned(), |f| {
            format!("p50 {:.4} p5 {:.4}", f.p50, f.p5)
        });
        tracing::info!(
            "  quality SSIM p50 {:.4} p5 {:.4} min {:.4} | fast scroll {fast} | PSNR p50 {:.1} dB p5 {:.1} dB (n={})",
            ssim.p50,
            ssim.p5,
            ssim.min,
            psnr.p50,
            psnr.p5,
            ssim.count
        );
    }
    for stream in &report.input {
        tracing::info!(
            "  input {}: RTT {} | lost {}/{} | out of order {}",
            stream.stream,
            fmt(stream.rtt),
            stream.sent - stream.received as u64,
            stream.sent,
            stream.out_of_order,
        );
    }
    for line in hud::lines(&board.lock().unwrap(), recent) {
        tracing::info!("  {}", line.text);
    }
    recent
}

/// Viewer without a window: frames count as presented when decoded.
fn run_headless(args: ViewArgs) -> anyhow::Result<()> {
    let recorder = Arc::new(Mutex::new(Recorder::default()));
    let board = Arc::new(Mutex::new(Board::default()));
    let frame_size = Arc::new(Mutex::new((0u32, 0u32)));
    let config = view_config(&args)?;
    let started = Instant::now();

    let on_frame = {
        let recorder = recorder.clone();
        let frame_size = frame_size.clone();
        move |frame: DecodedFrame| {
            *frame_size.lock().unwrap() = (frame.i420.width(), frame.i420.height());
            recorder.lock().unwrap().frame_presented(
                frame.marker,
                frame.decoded_at,
                frame.decoded_at,
                frame.captured_at_us,
                wall_clock_us(),
            );
        }
    };
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async {
        let reporter = {
            let (recorder, board) = (recorder.clone(), board.clone());
            tokio::spawn(async move {
                let mut interval = tokio::time::interval(REPORT_EVERY);
                interval.tick().await;
                let (mut rtt_mark, mut frames) = (0, 0);
                loop {
                    interval.tick().await;
                    let total = recorder.lock().unwrap().frames;
                    let fps = (total - frames) as f64 / REPORT_EVERY.as_secs_f64();
                    frames = total;
                    log_report(&recorder, &board, &mut rtt_mark, fps);
                }
            })
        };
        let duration = args.duration;
        let shutdown = async move {
            match duration {
                Some(secs) => tokio::time::sleep(Duration::from_secs(secs)).await,
                None => {
                    let _ = tokio::signal::ctrl_c().await;
                }
            }
        };
        let result =
            subscribe::run(config, recorder.clone(), board.clone(), on_frame, shutdown).await;
        reporter.abort();
        result
    })?;

    let recorder = recorder.lock().unwrap();
    let board = board.lock().unwrap();
    let size = *frame_size.lock().unwrap();
    write_results(&args, &recorder, &board, started.elapsed(), size)
}

/// Room on a monitor for the window's content, leaving space for the title bar and dock.
fn usable((w, h): (u32, u32)) -> (u32, u32) {
    (w * 95 / 100, h * 88 / 100)
}

/// Largest size with the frame's aspect ratio that fits in `max`, never upscaling.
/// Dimensions stay even, as I420 scaling needs.
fn fit_within(frame: (u32, u32), max: (u32, u32)) -> (u32, u32) {
    if frame.0 <= max.0 && frame.1 <= max.1 {
        return frame;
    }
    let scale = f64::min(
        f64::from(max.0) / f64::from(frame.0),
        f64::from(max.1) / f64::from(frame.1),
    );
    let even = |v: f64| ((v as u32) & !1).max(2);
    (
        even(f64::from(frame.0) * scale),
        even(f64::from(frame.1) * scale),
    )
}

fn write_results(
    args: &ViewArgs,
    recorder: &Recorder,
    board: &Board,
    elapsed: Duration,
    size: (u32, u32),
) -> anyhow::Result<()> {
    let report = recorder.report();
    let secs = elapsed.as_secs_f64();
    let results = json!({
        "label": args.label,
        "finished_at_unix_ms": wall_clock_us() / 1000,
        "duration_s": secs,
        "resolution": [size.0, size.1],
        "avg_fps": report.frames as f64 / secs,
        "tick_ms": args.tick_ms,
        "report": report,
        "net": board.history,
        "sharer_net_last": board.remote,
    });
    std::fs::create_dir_all(&args.out)?;
    let path = args.out.join(format!(
        "{}-{}.json",
        args.label,
        wall_clock_us() / 1_000_000
    ));
    std::fs::write(&path, serde_json::to_string_pretty(&results)?)
        .with_context(|| format!("writing {}", path.display()))?;
    tracing::info!("results written to {}", path.display());
    println!("{}", serde_json::to_string_pretty(&results)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fits_without_scaling_when_room() {
        assert_eq!(fit_within((1664, 1080), (2872, 1728)), (1664, 1080));
    }

    #[test]
    fn shrinks_1440p_onto_a_1080p_screen() {
        let (w, h) = fit_within((2224, 1440), usable((1920, 1080)));
        assert_eq!(h, 950);
        assert!(w <= 1824 && w % 2 == 0, "{w}");
        assert!((f64::from(w) / f64::from(h) - 2224.0 / 1440.0).abs() < 0.01);
    }

    #[test]
    fn limited_by_width() {
        assert_eq!(fit_within((3000, 1000), (1500, 1000)), (1500, 500));
    }
}
