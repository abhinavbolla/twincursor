//! The viewer: a winit + softbuffer window that shows the newest decoded frame, or a headless
//! viewer that only decodes and measures.
//!
//! winit must own the main thread on macOS, so LiveKit runs on a Tokio runtime in a
//! background thread. Frames are handed over through a single "latest frame" slot, so a
//! slow redraw drops old frames instead of queueing them.

use std::num::NonZeroU32;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use control::guest::GuestState;
use livekit::webrtc::native::yuv_helper;
use livekit::webrtc::video_frame::VideoBuffer;
use media::audio::VoiceState;
use media::capture::wall_clock_us;
use media::latency::Recorder;
use media::netstats::Board;
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

use crate::app::Events;
use crate::guest::{Guest, Ui};
use crate::input::{self, Rect};
use crate::{hud, Error};

const REPORT_EVERY: Duration = Duration::from_secs(10);

/// What [`run`] needs: the call to join, and how to show and measure it.
pub struct ViewOptions {
    /// The call: server, token, latency ticks, voice and what to measure.
    pub config: ViewConfig,
    /// Mute, and what is known about both voices.
    pub voice: Arc<VoiceState>,
    /// Decode and measure without opening a window. Latency is then measured at decode.
    pub headless: bool,
    /// Stop automatically after this many seconds.
    pub duration: Option<u64>,
    /// Label stored in the results file, e.g. "sck-vp8-static".
    pub label: String,
    /// Directory for the JSON results file, which is also printed to stdout. `None` writes
    /// and prints nothing.
    pub out: Option<PathBuf>,
    /// Play this remote-control script, then stop (headless only).
    pub control_script: Option<PathBuf>,
    /// Write every control message sent as a line of JSON.
    pub sent_log: Option<PathBuf>,
    /// The name the sharer sees when this guest asks for control.
    pub name: String,
    /// The desktop app, when it drives this viewer.
    pub app: Option<ViewApp>,
    /// Headless only: viewports to report to the sharer, each this long after starting, as a
    /// window would report its own. Empty, a headless viewer reports none, and the sharer
    /// sends its own size.
    pub viewports: Vec<(Duration, (u32, u32))>,
}

/// How the desktop app drives a viewer: commands in, events out.
pub struct ViewApp {
    pub events: Events,
    pub commands: mpsc::Receiver<ipc::Command>,
}

/// Carries out the app's commands until they stop coming. `leave` ends the call.
fn obey(
    commands: mpsc::Receiver<ipc::Command>,
    events: Events,
    guest: Option<Guest>,
    voice: Arc<VoiceState>,
    left: Arc<AtomicBool>,
    leave: impl FnOnce() + Send + 'static,
) -> std::thread::JoinHandle<()> {
    use ipc::Command;
    std::thread::spawn(move || {
        let mut leave = Some(leave);
        for command in commands {
            match command {
                Command::Mute => voice.set_muted(true),
                Command::Unmute => voice.set_muted(false),
                Command::SetMic { device } => voice.choose_mic(device.map(|d| d.parse().unwrap())),
                Command::SetSpeaker { device } => {
                    voice.choose_speaker(device.map(|d| d.parse().unwrap()))
                }
                Command::RequestControl | Command::ReleaseControl => match &guest {
                    Some(guest) => guest.send(if command == Command::RequestControl {
                        Ui::Request
                    } else {
                        Ui::Release
                    }),
                    None => events(ipc::Event::Error {
                        message: "Remote control isn't available in this call.".into(),
                    }),
                },
                Command::AllowControl { .. }
                | Command::DenyControl { .. }
                | Command::RevokeControl
                | Command::TrustControl { .. }
                | Command::TrustOnArrival { .. } => events(ipc::Event::Error {
                    message: "Only the sharer can answer requests for control.".into(),
                }),
                Command::Leave => {
                    left.store(true, Ordering::Release);
                    if let Some(leave) = leave.take() {
                        leave();
                    }
                }
            }
        }
    })
}

/// Tells the app about the voice when it changes, and about the video about once a second.
/// Runs until `done` is set.
fn report_to_app(
    events: Events,
    voice: Arc<VoiceState>,
    board: Arc<Mutex<Board>>,
    shown_pct: Arc<AtomicU32>,
    done: Arc<AtomicBool>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut last_voice = None;
        let mut ticks = 0u32;
        while !done.load(Ordering::Acquire) {
            let status = voice.status();
            let now = (status.sending, status.muted, status.mic, status.speaker);
            if last_voice.as_ref() != Some(&now) {
                last_voice = Some(now.clone());
                let (sending, muted, mic, speaker) = now;
                events(ipc::Event::Voice {
                    sending,
                    muted,
                    mic,
                    speaker,
                });
            }
            if let Some(message) = voice.take_device_error() {
                events(ipc::Event::Error { message });
            }
            ticks += 1;
            if ticks.is_multiple_of(4) {
                let board = board.lock().unwrap();
                if let Some(local) = &board.local {
                    if let Some(recv) = &local.recv {
                        let pct = shown_pct.load(Ordering::Acquire);
                        events(ipc::Event::Stats {
                            width: recv.width,
                            height: recv.height,
                            fps: recv.fps,
                            kbps: recv.kbps,
                            rtt_ms: local.rtt_ms,
                            codec: crate::app::codec_name(&recv.codec),
                            shown_pct: (pct > 0).then_some(pct),
                        });
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    })
}

/// The app's last words about a call: the connection is gone, why, and that it ended.
fn tell_ended(events: &Events, left: bool, error: Option<String>) {
    events(ipc::Event::Connection {
        state: ipc::ConnectionState::Disconnected,
    });
    let reason = match error {
        Some(message) => {
            events(ipc::Event::Error { message });
            ipc::EndReason::Failed
        }
        None if left => ipc::EndReason::Left,
        None => ipc::EndReason::Disconnected,
    };
    events(ipc::Event::Ended { reason });
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

/// Joins the call and shows the sharer's screen until the window closes, the call ends or
/// `duration` passes, then writes the results file. Must run on the main thread.
pub fn run(mut args: ViewOptions) -> Result<(), Error> {
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

    let config = args.config.clone();
    let voice = args.voice.clone();
    let mut app_link = args.app.take();
    let events = app_link.as_ref().map(|a| a.events.clone());
    let (guest, link, guest_thread) =
        Guest::start(args.name.clone(), args.sent_log.as_deref(), events.clone())?;
    let failure = Arc::new(Mutex::new(None::<String>));
    let left = Arc::new(AtomicBool::new(false));
    let shown_pct = Arc::new(AtomicU32::new(0));
    let reporting = Arc::new(AtomicBool::new(false));
    let mut helpers = Vec::new();
    if let Some(app) = app_link.take() {
        let proxy = proxy.clone();
        helpers.push(obey(
            app.commands,
            app.events.clone(),
            Some(guest.clone()),
            voice.clone(),
            left.clone(),
            move || {
                let _ = proxy.send_event(UserEvent::Stop);
            },
        ));
        report_to_app(
            app.events,
            voice.clone(),
            board.clone(),
            shown_pct.clone(),
            reporting.clone(),
        );
    }
    let network = {
        let failure = failure.clone();
        let latest = latest.clone();
        let recorder = recorder.clone();
        let board = board.clone();
        let voice = voice.clone();
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
                voice,
                Some(link),
                on_frame,
                shutdown,
            ));
            if let Err(e) = &result {
                tracing::error!("viewer failed: {e}");
                *failure.lock().unwrap() = Some(format!("The call failed: {e}."));
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
        hud: args.config.net_stats,
        recent_rtt: None,
        rtt_mark: 0,
        voice,
        voice_one_way: None,
        window: None,
        surface: None,
        size: (0, 0),
        frame_size: (0, 0),
        sizes: Sizes::new(started),
        last_report: Instant::now(),
        frames_since_report: 0,
        guest: guest.clone(),
        shown: (GuestState::Idle, false),
        rect: Rect {
            x: 0,
            y: 0,
            w: 1,
            h: 1,
        },
        cursor: None,
        surface_size: (0, 0),
        shown_pct: shown_pct.clone(),
        requested: None,
        manual_size: false,
        viewport: None,
    };
    tracing::info!(
        "press F9 to ask for control, and F9 again to give it back; M mutes your mic (F8 while \
         in control)"
    );
    event_loop.run_app(&mut app)?;

    guest.stop(Duration::from_secs(2));
    let _ = guest_thread.join();
    let _ = stop_tx.send(());
    let _ = network.join();
    reporting.store(true, Ordering::Release);
    drop(helpers);
    if let Some(events) = &events {
        let error = failure.lock().unwrap().take();
        tell_ended(events, left.load(Ordering::Acquire), error);
    }
    let recorder = recorder.lock().unwrap();
    let board = board.lock().unwrap();
    write_results(&args, &recorder, &board, started.elapsed(), &app.sizes)
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
    /// Mute, and what is known about both voices.
    voice: Arc<VoiceState>,
    /// The sharer's voice, mic to speakers, over the last report window, when it's measured.
    voice_one_way: Option<f64>,
    window: Option<Arc<Window>>,
    surface: Option<softbuffer::Surface<Arc<Window>, Arc<Window>>>,
    /// Size we draw at: the frame size, or smaller if the frame doesn't fit on the monitor.
    size: (u32, u32),
    /// Size of the frames we receive.
    frame_size: (u32, u32),
    /// Every size received, and when, for the results file.
    sizes: Sizes,
    last_report: Instant,
    frames_since_report: u64,
    guest: Guest,
    /// The control state and whether the mic was muted, as the title last showed them.
    shown: (GuestState, bool),
    /// Where the frame is drawn in the window.
    rect: Rect,
    /// The pointer in window pixels, while it's over the window.
    cursor: Option<(f64, f64)>,
    surface_size: (u32, u32),
    /// How the frame is drawn, in percent of its size (as the HUD's "shown" line says), for
    /// the app's stats.
    shown_pct: Arc<AtomicU32>,
    /// The window size we last asked for, to tell our resizes from the guest's.
    requested: Option<(u32, u32)>,
    /// The guest resized the window by hand, so it stays as they left it.
    manual_size: bool,
    /// The viewport last handed to the guest session for the sharer.
    viewport: Option<(u32, u32)>,
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
        self.report_viewport();
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
            WindowEvent::Resized(size) => {
                self.resize(size);
                self.report_viewport();
            }
            WindowEvent::Moved(_) | WindowEvent::ScaleFactorChanged { .. } => {
                // Possibly onto another monitor, with room for a different size.
                self.report_viewport();
            }
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
                    KeyCode::F8 => {
                        if pressed && !event.repeat {
                            self.toggle_mute();
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
                        let is = |key: &str| matches!(&event.logical_key, Key::Character(c) if c.eq_ignore_ascii_case(key));
                        if pressed && is("s") {
                            self.hud = !self.hud;
                        }
                        if pressed && !event.repeat && is("m") {
                            self.toggle_mute();
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
            // The window follows the frame size (a shared window resized on the sharer's
            // side, or the auto-throttle changing tier) unless the guest has resized it by
            // hand: then frames are scaled into the size they chose.
            let first = self.frame_size == (0, 0);
            if first || !self.manual_size {
                let monitor = window
                    .current_monitor()
                    .map(|m| (m.size().width, m.size().height));
                let (w, h) = monitor.map_or(frame_size, |m| fit_within(frame_size, usable(m)));
                self.requested = Some((w, h));
                if let Some(size) = window.request_inner_size(PhysicalSize::new(w, h)) {
                    self.size = (size.width, size.height);
                }
            }
            self.frame_size = frame_size;
            self.sizes.saw(frame_size);
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
        let pct = hud::shown_pct(frame_size, (rect.w, rect.h)).round() as u32;
        self.shown_pct.store(pct, Ordering::Release);
        if rect != self.rect {
            tracing::info!(
                "drawing {}x{} at ({}, {}) in a {w}x{h} window: {}",
                rect.w,
                rect.h,
                rect.x,
                rect.y,
                hud::shown_line(frame_size, (rect.w, rect.h)).text
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
        let stride = w as usize * 4;
        let row = rect.w as usize * 4;
        if bytes.len() - offset >= stride * rect.h as usize {
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
        } else {
            // Bars at the sides and none above or below: the last row ends before a whole
            // stride does, which libyuv's wrapper rejects. Convert into a tight buffer and copy
            // the rows across instead.
            let mut tight = vec![0u8; row * rect.h as usize];
            yuv_helper::i420_to_argb(
                y,
                sy,
                u,
                su,
                v,
                sv,
                &mut tight,
                rect.w * 4,
                rect.w as i32,
                rect.h as i32,
            );
            for (i, src) in tight.chunks_exact(row).enumerate() {
                let at = offset + i * stride;
                bytes[at..at + row].copy_from_slice(src);
            }
        }
        if self.hud {
            let voice = hud::Voice {
                status: self.voice.status(),
                one_way_ms: self.voice_one_way,
            };
            let mut lines = hud::lines(&self.board.lock().unwrap(), self.recent_rtt, &voice);
            lines.push(hud::shown_line(frame_size, (rect.w, rect.h)));
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
        // Any size we didn't ask for once frames arrive is the guest's choice: keep it.
        if self.frame_size != (0, 0) && self.requested != Some(self.size) {
            if !self.manual_size {
                tracing::info!(
                    "viewer resized by hand to {}x{}: it no longer follows the picture's size",
                    size.width,
                    size.height
                );
            }
            self.manual_size = true;
        }
    }

    /// Tells the sharer, through the guest session, the largest picture this window can show
    /// 1:1. While the window follows the picture's size, that's the room on the monitor (what
    /// the window could grow to); once the guest has sized it by hand, it's the window's own
    /// size. Neither depends on the picture received, so the sharer's answer never changes it.
    fn report_viewport(&mut self) {
        let Some(window) = &self.window else { return };
        let inner = window.inner_size();
        let monitor = window
            .current_monitor()
            .map(|m| (m.size().width, m.size().height));
        let viewport = match monitor {
            Some(m) if !self.manual_size => usable(m),
            _ => (inner.width, inner.height),
        };
        if viewport.0 < 2 || viewport.1 < 2 || self.viewport == Some(viewport) {
            return;
        }
        self.viewport = Some(viewport);
        tracing::info!("viewport {}x{}", viewport.0, viewport.1);
        self.guest.send(Ui::Viewport(viewport));
    }

    /// Mutes or unmutes this side's mic.
    fn toggle_mute(&mut self) {
        if !self.voice.status().sending {
            tracing::info!("your mic isn't on in this call (--audio), so there is nothing to mute");
            return;
        }
        if self.voice.toggle_mute() {
            tracing::info!("your mic is muted (M, or F8 while in control, unmutes)");
        } else {
            tracing::info!("your mic is on");
        }
    }

    /// Shows in the title whether this guest has control, so they never type blind, and
    /// whether their mic is muted, so they never talk to no one.
    fn show_state(&mut self) {
        let state = self.guest.status.get();
        let muted = self.voice.wants_muted() && self.voice.status().sending;
        if (state, muted) == self.shown {
            return;
        }
        self.shown = (state, muted);
        let mut title = match state {
            GuestState::Idle => format!("{TITLE} (F9 asks for control)"),
            GuestState::Requested => format!("{TITLE}: asking for control…"),
            GuestState::InControl => format!("{TITLE}: you're in control (F9 gives it back)"),
        };
        if muted {
            title += " · mic muted";
        }
        if let Some(window) = &self.window {
            window.set_title(&title);
        }
        tracing::info!("{title}");
    }

    fn report(&mut self) {
        let fps = self.frames_since_report as f64 / self.last_report.elapsed().as_secs_f64();
        let mut voice = hud::Voice {
            status: self.voice.status(),
            one_way_ms: None,
        };
        self.recent_rtt = log_report(
            &self.recorder,
            &self.board,
            &mut voice,
            &mut self.rtt_mark,
            fps,
        );
        self.voice_one_way = voice.one_way_ms;
        self.last_report = Instant::now();
        self.frames_since_report = 0;
    }
}

/// Logs the latency, quality and network summary for the last window. Returns the round trip
/// over that window, which the HUD shows, and fills in the voice's measured one-way delay.
fn log_report(
    recorder: &Mutex<Recorder>,
    board: &Mutex<Board>,
    voice: &mut hud::Voice,
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
    voice.one_way_ms = report.audio.as_ref().and_then(|a| a.one_way).map(|s| s.p50);
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
    for line in hud::lines(&board.lock().unwrap(), recent, voice) {
        tracing::info!("  {}", line.text);
    }
    recent
}

/// Viewer without a window: frames count as presented when decoded.
fn run_headless(mut args: ViewOptions) -> Result<(), Error> {
    let recorder = Arc::new(Mutex::new(Recorder::default()));
    let board = Arc::new(Mutex::new(Board::default()));
    let started = Instant::now();
    let sizes = Arc::new(Mutex::new(Sizes::new(started)));
    let config = args.config.clone();
    let voice = args.voice.clone();

    let on_frame = {
        let recorder = recorder.clone();
        let sizes = sizes.clone();
        move |frame: DecodedFrame| {
            let size = (frame.i420.width(), frame.i420.height());
            if sizes.lock().unwrap().saw(size) {
                tracing::info!("receiving {}x{}", size.0, size.1);
            }
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
    let (leave_tx, leave_rx) = tokio::sync::oneshot::channel::<()>();
    let app_link = args.app.take();
    let events = app_link.as_ref().map(|a| a.events.clone());
    let left = Arc::new(AtomicBool::new(false));
    let reporting = Arc::new(AtomicBool::new(false));
    let mut link = None;
    let mut player = None;
    let mut guest_for_app = None;
    let mut guest_thread = None;
    let mut viewport_guest = None;
    if script.is_some() || app_link.is_some() || !args.viewports.is_empty() {
        let (guest, control, thread) = crate::guest::Guest::start(
            args.name.clone(),
            args.sent_log.as_deref(),
            events.clone(),
        )?;
        link = Some(control);
        guest_for_app = Some(guest.clone());
        viewport_guest = Some(guest.clone());
        if let Some(steps) = script {
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
        } else {
            guest_thread = Some((guest, thread));
        }
    }
    if let Some(app) = app_link {
        obey(
            app.commands,
            app.events.clone(),
            guest_for_app,
            args.voice.clone(),
            left.clone(),
            move || {
                let _ = leave_tx.send(());
            },
        );
        report_to_app(
            app.events,
            args.voice.clone(),
            board.clone(),
            Arc::new(AtomicU32::new(0)),
            reporting.clone(),
        );
    }
    if let Some(guest) = viewport_guest.filter(|_| !args.viewports.is_empty()) {
        let schedule = args.viewports.clone();
        std::thread::spawn(move || {
            for (at, viewport) in schedule {
                if let Some(rest) = at.checked_sub(started.elapsed()) {
                    std::thread::sleep(rest);
                }
                tracing::info!("viewport {}x{}", viewport.0, viewport.1);
                guest.send(Ui::Viewport(viewport));
            }
        });
    }
    // With the app, the call lasts until it says to leave, even after a script ends.
    let ends_with_script = player.is_some() && events.is_none();

    let runtime = tokio::runtime::Runtime::new()?;
    let result = runtime.block_on(async {
        let reporter = {
            let (recorder, board, voice) = (recorder.clone(), board.clone(), voice.clone());
            tokio::spawn(async move {
                let mut interval = tokio::time::interval(REPORT_EVERY);
                interval.tick().await;
                let (mut rtt_mark, mut frames) = (0, 0);
                loop {
                    interval.tick().await;
                    let total = recorder.lock().unwrap().frames;
                    let fps = (total - frames) as f64 / REPORT_EVERY.as_secs_f64();
                    frames = total;
                    let mut voice = hud::Voice {
                        status: voice.status(),
                        one_way_ms: None,
                    };
                    log_report(&recorder, &board, &mut voice, &mut rtt_mark, fps);
                }
            })
        };
        let duration = args.duration;
        let scripted = ends_with_script;
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
                Ok(()) = leave_rx => {}
            }
        };
        let result = subscribe::run(
            config,
            recorder.clone(),
            board.clone(),
            voice,
            link,
            on_frame,
            shutdown,
        )
        .await;
        reporter.abort();
        result
    });
    if let Some((guest, thread)) = guest_thread {
        guest.stop(Duration::from_secs(2));
        let _ = thread.join();
    }
    reporting.store(true, Ordering::Release);
    if let Some(events) = &events {
        let error = result
            .as_ref()
            .err()
            .map(|e| format!("The call failed: {e}."));
        tell_ended(events, left.load(Ordering::Acquire), error);
    }
    result?;

    let recorder = recorder.lock().unwrap();
    let board = board.lock().unwrap();
    let sizes = sizes.lock().unwrap();
    write_results(&args, &recorder, &board, started.elapsed(), &sizes)?;
    if let Some(player) = player {
        player
            .join()
            .map_err(|_| Error::ScriptPanicked)?
            .map_err(Error::Script)?;
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

/// The sizes frames arrived at, each with when it began, in seconds since the viewer started:
/// on the same clock as `ViewOptions::viewports`, so a check can time how long a new viewport
/// took to show.
struct Sizes {
    started: Instant,
    changes: Vec<(f64, (u32, u32))>,
}

impl Sizes {
    fn new(started: Instant) -> Self {
        Self {
            started,
            changes: Vec::new(),
        }
    }

    /// Notes the size of a frame. Returns whether it's a new one.
    fn saw(&mut self, size: (u32, u32)) -> bool {
        if self.changes.last().map(|&(_, s)| s) == Some(size) {
            return false;
        }
        let t = self.started.elapsed().as_secs_f64();
        self.changes.push((t, size));
        true
    }

    /// The size frames arrive at now.
    fn last(&self) -> (u32, u32) {
        self.changes.last().map_or((0, 0), |&(_, s)| s)
    }
}

fn write_results(
    args: &ViewOptions,
    recorder: &Recorder,
    board: &Board,
    elapsed: Duration,
    sizes: &Sizes,
) -> Result<(), Error> {
    let size = sizes.last();
    let mut report = recorder.report();
    if let Some(audio) = report.audio.as_mut() {
        audio.concealed_pct = board
            .local
            .as_ref()
            .and_then(|s| s.audio_recv.as_ref())
            .map(|a| a.concealed_run_pct);
    }
    let Some(out) = &args.out else {
        return Ok(());
    };
    let secs = elapsed.as_secs_f64();
    let results = json!({
        "label": args.label,
        "finished_at_unix_ms": wall_clock_us() / 1000,
        "duration_s": secs,
        "resolution": [size.0, size.1],
        "sizes": sizes
            .changes
            .iter()
            .map(|(t, (w, h))| json!({ "t_s": t, "size": [w, h] }))
            .collect::<Vec<_>>(),
        "viewports": args
            .viewports
            .iter()
            .map(|(t, (w, h))| json!({ "t_s": t.as_secs_f64(), "size": [w, h] }))
            .collect::<Vec<_>>(),
        "avg_fps": report.frames as f64 / secs,
        "tick_ms": args.config.tick_interval.as_millis() as u64,
        "report": report,
        "media": media::netstats::media_summary(&board.history),
        "net": board.history,
        "sharer_net_last": board.remote,
    });
    std::fs::create_dir_all(out)?;
    let path = out.join(format!(
        "{}-{}.json",
        args.label,
        wall_clock_us() / 1_000_000
    ));
    let json = serde_json::to_string_pretty(&results)?;
    if let Err(source) = std::fs::write(&path, json) {
        return Err(Error::Write { path, source });
    }
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
