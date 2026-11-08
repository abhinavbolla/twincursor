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
use control::guest::GuestState;
use livekit::webrtc::native::yuv_helper;
use livekit::webrtc::video_frame::VideoBuffer;
use media::capture::wall_clock_us;
use media::latency::Recorder;
use media::netstats::Board;
use media::quality::Snapshots;
use media::stats::{summarize, Summary};
use media::subscribe::{self, DecodedFrame, ViewConfig};
use protocol::InputEvent;
use serde_json::json;
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, KeyCode, PhysicalKey};
use winit::window::{Window, WindowId};

use crate::guest::{Guest, Ui};
use crate::input::{self, Rect};
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
    /// Time the markers in the sharer's voice (needs `share --audio pattern`).
    #[arg(long)]
    measure_audio: bool,
    #[command(flatten)]
    voice: crate::voice::VoiceArgs,
    /// Save the frame carrying every Nth latency tick as a PGM (see `tc-spike compare`).
    #[arg(long, default_value_t = 0)]
    snapshot_every: u32,
    #[arg(long, default_value = "bench/snapshots")]
    snapshot_dir: PathBuf,
    /// Directory for the JSON results file.
    #[arg(long, default_value = "bench/results")]
    out: PathBuf,
    /// Play a remote-control script (JSON steps; see `bench/control/`), then stop. Needs
    /// `--headless` and a sharer started with `--allow-control`.
    #[arg(long, requires = "headless")]
    control_script: Option<PathBuf>,
    /// Write every control message sent as a line of JSON (see `tc-spike replay`).
    #[arg(long)]
    sent_log: Option<PathBuf>,
    /// The name the sharer sees when you ask for control.
    #[arg(long, default_value = "guest")]
    name: String,
}

#[derive(Debug)]
enum UserEvent {
    NewFrame,
    Stop,
}

const TITLE: &str = "TwinCursor viewer";

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
        measure_audio: args.measure_audio,
        audio: args.voice.mode(false)?,
        voice: args.voice.options(),
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
    let mut builder = EventLoop::<UserEvent>::with_user_event();
    #[cfg(target_os = "macos")]
    {
        // Without the default menu, ⌘Q and ⌘H reach the sharer instead of closing this window.
        use winit::platform::macos::EventLoopBuilderExtMacOS;
        builder.with_default_menu(false);
    }
    let event_loop = builder.build()?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let proxy = event_loop.create_proxy();

    let latest = Arc::new(Latest::default());
    let recorder = Arc::new(Mutex::new(Recorder::default()));
    let board = Arc::new(Mutex::new(Board::default()));
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();

    let config = view_config(&args)?;
    let (guest, link, guest_thread) = Guest::start(args.name.clone(), args.sent_log.as_deref())?;
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
            let result = runtime.block_on(subscribe::run(
                config,
                recorder,
                board,
                Some(link),
                on_frame,
                shutdown,
            ));
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
        guest: guest.clone(),
        shown_state: GuestState::Idle,
        rect: Rect {
            x: 0,
            y: 0,
            w: 1,
            h: 1,
        },
        cursor: None,
        surface_size: (0, 0),
    };
    tracing::info!("press F9 to ask for control, and F9 again to give it back");
    event_loop.run_app(&mut app)?;

    guest.stop(Duration::from_secs(2));
    let _ = guest_thread.join();
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
    guest: Guest,
    /// The control state the title last showed.
    shown_state: GuestState,
    /// Where the frame is drawn in the window.
    rect: Rect,
    /// The pointer in window pixels, while it's over the window.
    cursor: Option<(f64, f64)>,
    surface_size: (u32, u32),
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attributes = Window::default_attributes()
            .with_title(TITLE)
            .with_inner_size(PhysicalSize::new(1662, 1080));
        let window = Arc::new(event_loop.create_window(attributes).expect("create window"));
        let context = softbuffer::Context::new(window.clone()).expect("softbuffer context");
        self.surface = Some(softbuffer::Surface::new(&context, window.clone()).expect("surface"));
        self.window = Some(window);
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::NewFrame => {
                self.present_latest();
                self.show_state();
            }
            UserEvent::Stop => event_loop.exit(),
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        let controlling = self.guest.status.get() == GuestState::InControl;
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => self.resize(size),
            WindowEvent::Focused(false) => {
                // Keys held when the window loses focus would never see their key-up here.
                self.guest.send(Ui::Input(InputEvent::ReleaseAll));
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let pressed = event.state == ElementState::Pressed;
                let PhysicalKey::Code(code) = event.physical_key else {
                    return;
                };
                match code {
                    KeyCode::F9 => {
                        if pressed && !event.repeat {
                            self.guest.send(Ui::Toggle);
                        }
                    }
                    KeyCode::F10 => {
                        if pressed && !event.repeat {
                            self.hud = !self.hud;
                        }
                    }
                    _ if controlling => {
                        let text = if pressed {
                            input::key_text(event.text.as_deref())
                        } else {
                            None
                        };
                        self.guest.send(Ui::Input(InputEvent::Key {
                            code: input::key_code(code),
                            pressed,
                            text,
                        }));
                    }
                    _ => {
                        let is_s = matches!(&event.logical_key, Key::Character(c) if c.eq_ignore_ascii_case("s"));
                        if pressed && is_s {
                            self.hud = !self.hud;
                        }
                    }
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                let at = (position.x, position.y);
                self.cursor = Some(at);
                if controlling {
                    if let Some((x, y)) = input::to_surface(self.rect, at) {
                        self.guest.send(Ui::Input(InputEvent::Move { x, y }));
                    }
                }
            }
            WindowEvent::CursorLeft { .. } => self.cursor = None,
            WindowEvent::MouseInput { state, button, .. } if controlling => {
                let (Some(button), Some(at)) = (input::button(button), self.cursor) else {
                    return;
                };
                let pressed = state == ElementState::Pressed;
                // A press needs to be on the frame; a release goes wherever the pointer is, so
                // nothing stays held on the sharer.
                let point = if pressed {
                    input::to_surface(self.rect, at)
                } else {
                    Some(input::to_surface_clamped(self.rect, at))
                };
                if let Some((x, y)) = point {
                    self.guest.send(Ui::Input(InputEvent::Button {
                        button,
                        pressed,
                        x,
                        y,
                    }));
                }
            }
            WindowEvent::MouseWheel { delta, .. } if controlling => {
                let scale = self.window.as_ref().map_or(1.0, |w| w.scale_factor());
                if let Some(event) = input::scroll(delta, scale) {
                    self.guest.send(Ui::Input(event));
                }
            }
            _ => {}
        }
        self.show_state();
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
            // After the first frame the window only grows on its own: when the sharer's
            // auto-throttle drops to a lower tier, frames are scaled up instead of the window
            // jumping around. The guest can resize it; the frame is then letterboxed.
            let first = self.frame_size == (0, 0);
            if first || frame_size.0 > self.size.0 || frame_size.1 > self.size.1 {
                let monitor = window
                    .current_monitor()
                    .map(|m| (m.size().width, m.size().height));
                let (w, h) = monitor.map_or(frame_size, |m| fit_within(frame_size, usable(m)));
                if let Some(size) = window.request_inner_size(PhysicalSize::new(w, h)) {
                    self.size = (size.width, size.height);
                }
            }
            self.frame_size = frame_size;
            tracing::info!(
                "receiving {}x{}, window {}x{}",
                frame_size.0,
                frame_size.1,
                self.size.0,
                self.size.1
            );
        }
        let (w, h) = self.size;
        if w < 2 || h < 2 {
            return;
        }
        let rect = input::letterbox(frame_size, (w, h));
        if rect != self.rect {
            tracing::info!(
                "drawing {}x{} at ({}, {}) in a {w}x{h} window",
                rect.w,
                rect.h,
                rect.x,
                rect.y
            );
            self.rect = rect;
        }
        let mut i420 = frame.i420;
        if (rect.w, rect.h) != frame_size {
            i420 = i420.scale(rect.w as i32, rect.h as i32);
        }

        if self.surface_size != (w, h) {
            if surface
                .resize(NonZeroU32::new(w).unwrap(), NonZeroU32::new(h).unwrap())
                .is_err()
            {
                return;
            }
            self.surface_size = (w, h);
        }
        let mut buffer = surface.buffer_mut().expect("buffer");
        if (rect.w, rect.h) != (w, h) {
            buffer.fill(0);
        }
        // softbuffer pixels are 0RGB u32s, i.e. B,G,R,X bytes on little-endian, which is
        // what libyuv calls ARGB.
        // SAFETY: reinterpreting initialized u32s as bytes; alignment only gets looser.
        let bytes = unsafe {
            std::slice::from_raw_parts_mut(buffer.as_mut_ptr().cast::<u8>(), buffer.len() * 4)
        };
        let offset = (rect.y * w + rect.x) as usize * 4;
        let (y, u, v) = i420.data();
        let (sy, su, sv) = i420.strides();
        yuv_helper::i420_to_argb(
            y,
            sy,
            u,
            su,
            v,
            sv,
            &mut bytes[offset..],
            w * 4,
            rect.w as i32,
            rect.h as i32,
        );
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

    fn resize(&mut self, size: PhysicalSize<u32>) {
        self.size = (size.width, size.height);
    }

    /// Shows in the title whether this guest has control, so they never type blind.
    fn show_state(&mut self) {
        let state = self.guest.status.get();
        if state == self.shown_state {
            return;
        }
        self.shown_state = state;
        let title = match state {
            GuestState::Idle => format!("{TITLE} (F9 asks for control)"),
            GuestState::Requested => format!("{TITLE}: asking for control…"),
            GuestState::InControl => format!("{TITLE}: you're in control (F9 gives it back)"),
        };
        if let Some(window) = &self.window {
            window.set_title(&title);
        }
        tracing::info!("{title}");
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
            "  input {}: RTT {} | one-way {} | lost {}/{} | out of order {}",
            stream.stream,
            fmt(stream.rtt),
            fmt(stream.one_way),
            stream.sent - stream.received as u64,
            stream.sent,
            stream.out_of_order,
        );
    }
    if let Some(audio) = &report.audio {
        tracing::info!(
            "  audio: RTT {} | one-way {} | A/V {} | heard {} rejected {}",
            fmt(audio.rtt),
            fmt(audio.one_way),
            fmt(audio.av_offset),
            audio.bursts_heard,
            audio.bursts_rejected,
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
    let script = args
        .control_script
        .as_ref()
        .map(crate::guest::read_script)
        .transpose()?;
    let (script_done_tx, script_done_rx) = tokio::sync::oneshot::channel::<()>();
    let mut link = None;
    let mut player = None;
    if let Some(steps) = script {
        let (guest, control, thread) =
            crate::guest::Guest::start(args.name.clone(), args.sent_log.as_deref())?;
        link = Some(control);
        player = Some(std::thread::spawn(move || {
            let result = crate::guest::play(&guest, &steps);
            // Waits until the sharer confirms it has applied everything the script sent.
            guest.stop(Duration::from_secs(60));
            let _ = thread.join();
            if guest.unsent() > 0 {
                tracing::warn!("some control messages were still unsent when the viewer left");
            }
            let _ = script_done_tx.send(());
            result
        }));
    }

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
        let scripted = player.is_some();
        let shutdown = async move {
            let timer = async {
                match duration {
                    Some(secs) => tokio::time::sleep(Duration::from_secs(secs)).await,
                    None => {
                        let _ = tokio::signal::ctrl_c().await;
                    }
                }
            };
            tokio::select! {
                _ = timer => {}
                Ok(()) = script_done_rx, if scripted => {}
            }
        };
        let result = subscribe::run(
            config,
            recorder.clone(),
            board.clone(),
            link,
            on_frame,
            shutdown,
        )
        .await;
        reporter.abort();
        result
    })?;

    let recorder = recorder.lock().unwrap();
    let board = board.lock().unwrap();
    let size = *frame_size.lock().unwrap();
    write_results(&args, &recorder, &board, started.elapsed(), size)?;
    if let Some(player) = player {
        player
            .join()
            .map_err(|_| anyhow::anyhow!("control script panicked"))?
            .context("control script failed")?;
        tracing::info!("control script finished");
    }
    Ok(())
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
    let mut report = recorder.report();
    if let Some(audio) = report.audio.as_mut() {
        audio.concealed_pct = board
            .local
            .as_ref()
            .and_then(|s| s.audio_recv.as_ref())
            .map(|a| a.concealed_run_pct);
    }
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
