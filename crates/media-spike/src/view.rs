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
use media::subscribe::{self, DecodedFrame, ViewConfig};
use serde_json::json;
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

use crate::Connection;

const REPORT_EVERY: Duration = Duration::from_secs(10);

#[derive(Args)]
pub struct ViewArgs {
    #[command(flatten)]
    connection: Connection,
    /// Interval between latency ticks, in milliseconds.
    #[arg(long, default_value_t = 250)]
    tick_ms: u64,
    /// Stop automatically after this many seconds.
    #[arg(long)]
    duration: Option<u64>,
    /// Label stored in the results file, e.g. "sck-vp8-static".
    #[arg(long, default_value = "run")]
    label: String,
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

pub fn run(args: ViewArgs) -> anyhow::Result<()> {
    let event_loop = EventLoop::<UserEvent>::with_user_event().build()?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let proxy = event_loop.create_proxy();

    let latest = Arc::new(Latest::default());
    let recorder = Arc::new(Mutex::new(Recorder::default()));
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();

    let config = ViewConfig {
        url: args.connection.url.clone(),
        token: args.connection.token("viewer")?,
        tick_interval: Duration::from_millis(args.tick_ms),
    };
    let network = {
        let latest = latest.clone();
        let recorder = recorder.clone();
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
            let result = runtime.block_on(subscribe::run(config, recorder, on_frame, shutdown));
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
        window: None,
        surface: None,
        size: (0, 0),
        last_report: Instant::now(),
        frames_since_report: 0,
    };
    event_loop.run_app(&mut app)?;

    let _ = stop_tx.send(());
    let _ = network.join();
    let recorder = recorder.lock().unwrap();
    write_results(&args, &recorder, started.elapsed(), app.size)
}

struct App {
    latest: Arc<Latest>,
    recorder: Arc<Mutex<Recorder>>,
    window: Option<Arc<Window>>,
    surface: Option<softbuffer::Surface<Arc<Window>, Arc<Window>>>,
    size: (u32, u32),
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
        if let WindowEvent::CloseRequested = event {
            event_loop.exit();
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

        let (w, h) = (frame.i420.width(), frame.i420.height());
        if (w, h) != self.size {
            let _ = window.request_inner_size(PhysicalSize::new(w, h));
            surface
                .resize(NonZeroU32::new(w).unwrap(), NonZeroU32::new(h).unwrap())
                .expect("resize surface");
            self.size = (w, h);
            tracing::info!("receiving {w}x{h}");
        }

        let mut buffer = surface.buffer_mut().expect("buffer");
        // softbuffer pixels are 0RGB u32s, i.e. B,G,R,X bytes on little-endian, which is
        // what libyuv calls ARGB.
        // SAFETY: reinterpreting initialized u32s as bytes; alignment only gets looser.
        let bytes = unsafe {
            std::slice::from_raw_parts_mut(buffer.as_mut_ptr().cast::<u8>(), buffer.len() * 4)
        };
        let (y, u, v) = frame.i420.data();
        let (sy, su, sv) = frame.i420.strides();
        yuv_helper::i420_to_argb(y, sy, u, su, v, sv, bytes, w * 4, w as i32, h as i32);
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
        let report = self.recorder.lock().unwrap().report();
        let fmt = |s: Option<media::stats::Summary>| s.map_or("n/a".to_owned(), |s| s.to_string());
        tracing::info!(
            "{fps:.1} fps | RTT (tick→presented) {} | one-way (capture→presented) {}",
            fmt(report.rtt_presented),
            fmt(report.one_way_presented),
        );
        self.last_report = Instant::now();
        self.frames_since_report = 0;
    }
}

fn write_results(
    args: &ViewArgs,
    recorder: &Recorder,
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
