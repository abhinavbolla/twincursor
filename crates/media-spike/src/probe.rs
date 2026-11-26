//! `tc-spike probe`: a full-screen window that records the input it receives.
//!
//! It turns window events into protocol events with the same code the viewer uses, so its
//! recording can be compared with what a guest sent (`tc-spike replay --probe`). That closes the
//! loop on real injection: guest → network → sharer → CGEvent → macOS → this window.
//!
//! Real input lands wherever it lands, so the window must be where it goes before any is sent.
//! It covers the display the injector targets ([`control::macos::input_display`], the injector's
//! own answer), and its first record says so: `{"ready": true, …}` once it is the key window of
//! the frontmost app, on that display and covering it, and has stayed so for a moment. If that
//! doesn't happen within `--ready-timeout` it records `not_ready` and exits non-zero. After
//! that it keeps checking, and records `lost_focus` the moment any of it stops being true.
//! `tc-spike probe-gate` reads these records for `bench/control-inject.sh`.

use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use clap::Args;
use control::display::Display;
use engine::input;
use protocol::InputEvent;
use serde::{Deserialize, Serialize};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::PhysicalKey;
use winit::monitor::MonitorHandle;
use winit::window::{Fullscreen, Window, WindowId};

#[derive(Args)]
pub struct ProbeArgs {
    /// Where to write what the window receives, one JSON line per event, after a first line
    /// that says the window is ready.
    #[arg(long)]
    out: PathBuf,
    /// Close after this many seconds.
    #[arg(long, default_value_t = 60)]
    duration: u64,
    /// Give up (and exit non-zero) if the window isn't ready this many seconds after it opens.
    #[arg(long, default_value_t = 6)]
    ready_timeout: u64,
}

/// How long everything has to hold before the window says it is ready: a full-screen window
/// takes a moment to settle, and focus can flicker while it does.
const SETTLE: Duration = Duration::from_millis(600);
/// How often the window checks it is still in front.
const RECHECK: Duration = Duration::from_millis(50);

/// Where the window is, as a fraction of the target display, top left at 0, 0.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Frame {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Frame {
    /// Whether the window covers the display: inside it, its full width, and all of its
    /// height but a notch's worth (a full-screen window starts below the camera housing).
    pub fn covers_display(&self) -> bool {
        const EDGE: f64 = 0.005;
        self.x >= -EDGE
            && self.y >= -EDGE
            && self.x + self.width <= 1.0 + EDGE
            && self.y + self.height <= 1.0 + EDGE
            && self.width >= 0.99
            && self.height >= 0.9
    }

    /// Whether a point on the display (0..1) is on the window, `margin` in from its edges.
    pub fn contains(&self, (x, y): (f64, f64), margin: f64) -> bool {
        x >= self.x + margin
            && y >= self.y + margin
            && x <= self.x + self.width - margin
            && y <= self.y + self.height - margin
    }
}

/// What the window knows about where it is.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Placement {
    /// On the display input goes to.
    pub on_target: bool,
    /// The key window: keys go to it.
    pub key: bool,
    /// This app is the frontmost one, and active.
    pub frontmost: bool,
    /// Where the window is on the target display.
    pub frame: Option<Frame>,
}

impl Placement {
    pub fn ok(&self) -> bool {
        self.on_target
            && self.key
            && self.frontmost
            && self.frame.is_some_and(|f| f.covers_display())
    }

    /// What is wrong, in words.
    pub fn problems(&self) -> String {
        let mut out = Vec::new();
        if !self.on_target {
            out.push("not on the display input goes to");
        }
        if !self.frontmost {
            out.push("not the frontmost app");
        }
        if !self.key {
            out.push("not the key window");
        }
        if !self.frame.is_some_and(|f| f.covers_display()) {
            out.push("not covering the display");
        }
        out.join(", ")
    }
}

/// One line of the probe's output file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ProbeLine {
    /// The first line, once the window can receive input: the display input goes to, and where
    /// the window is on it.
    Ready {
        ready: bool,
        display: Display,
        frame: Frame,
        key: bool,
        frontmost: bool,
    },
    /// The window stopped being in front, key or on the display after it was ready.
    LostFocus { lost_focus: Placement },
    /// The window never got ready.
    NotReady { not_ready: String },
    /// The window is closing.
    Closed { closed: bool },
    /// Input the window received.
    Event(InputEvent),
}

/// What changed in whether the window can receive input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    Ready,
    Lost,
    TimedOut,
}

/// Turns a stream of checks (is everything in place now?) into the ready and lost records:
/// ready once it has held for `settle`, lost at every fall after that, timed out if it was never
/// ready by the deadline.
#[derive(Debug)]
pub struct Readiness {
    settle: Duration,
    deadline: Instant,
    ok_since: Option<Instant>,
    ready: bool,
    ok: bool,
    timed_out: bool,
}

impl Readiness {
    pub fn new(settle: Duration, deadline: Instant) -> Self {
        Self {
            settle,
            deadline,
            ok_since: None,
            ready: false,
            ok: false,
            timed_out: false,
        }
    }

    /// Whether the ready record has been written: input before it isn't recorded, so the
    /// ready line is always the first.
    pub fn is_ready(&self) -> bool {
        self.ready
    }

    pub fn observe(&mut self, ok: bool, now: Instant) -> Option<Change> {
        if self.timed_out {
            return None;
        }
        if self.ready {
            let was = std::mem::replace(&mut self.ok, ok);
            return (was && !ok).then_some(Change::Lost);
        }
        if ok {
            let since = *self.ok_since.get_or_insert(now);
            if now.duration_since(since) >= self.settle {
                self.ready = true;
                self.ok = true;
                return Some(Change::Ready);
            }
        } else {
            self.ok_since = None;
        }
        if now >= self.deadline {
            self.timed_out = true;
            return Some(Change::TimedOut);
        }
        None
    }
}

pub fn run(args: ProbeArgs) -> anyhow::Result<()> {
    let target = target_display()?;
    let event_loop = EventLoop::new()?;
    let opened = Instant::now();
    let mut probe = Probe {
        out: std::fs::File::create(&args.out)?,
        until: opened + Duration::from_secs(args.duration),
        target,
        monitor: None,
        window: None,
        cursor: None,
        moves: 0,
        readiness: Readiness::new(SETTLE, opened + Duration::from_secs(args.ready_timeout)),
        failed: None,
    };
    event_loop.run_app(&mut probe)?;
    probe.write(&ProbeLine::Closed { closed: true });
    tracing::info!(moves = probe.moves, "probe closed");
    match probe.failed {
        Some(message) => anyhow::bail!(message),
        None => Ok(()),
    }
}

/// The display input goes to: the injector's own answer. A probe anywhere else proves nothing
/// and puts real input on whatever is under it, so it refuses to start instead.
#[cfg(target_os = "macos")]
fn target_display() -> anyhow::Result<Display> {
    if control::macos::screen_is_locked() {
        anyhow::bail!("the screen is locked: unlock it first (nothing is injected while it is)");
    }
    control::macos::input_display().map_err(|e| {
        anyhow::anyhow!(
            "can't tell which display input would go to ({e}), so no test window was opened"
        )
    })
}

#[cfg(not(target_os = "macos"))]
fn target_display() -> anyhow::Result<Display> {
    anyhow::bail!("the input probe is macOS only for now")
}

#[cfg(target_os = "macos")]
fn monitor_id(monitor: &MonitorHandle) -> Option<u32> {
    use winit::platform::macos::MonitorHandleExtMacOS;
    Some(monitor.native_id())
}

#[cfg(not(target_os = "macos"))]
fn monitor_id(_: &MonitorHandle) -> Option<u32> {
    None
}

struct Probe {
    out: std::fs::File,
    until: Instant,
    target: Display,
    monitor: Option<MonitorHandle>,
    window: Option<Window>,
    cursor: Option<(f64, f64)>,
    moves: u64,
    readiness: Readiness,
    failed: Option<String>,
}

impl Probe {
    fn write(&mut self, line: &ProbeLine) {
        let line = serde_json::to_string(line).expect("records serialize");
        let _ = writeln!(self.out, "{line}");
        let _ = self.out.flush();
    }

    fn record(&mut self, event: InputEvent) {
        if !self.readiness.is_ready() {
            return;
        }
        self.write(&ProbeLine::Event(event));
    }

    /// Where the window is on the target display, in its pixels.
    fn frame(&self) -> Option<Frame> {
        let (window, monitor) = (self.window.as_ref()?, self.monitor.as_ref()?);
        let (origin, size) = (monitor.position(), monitor.size());
        let (inner, inner_size) = (window.inner_position().ok()?, window.inner_size());
        let (w, h) = (f64::from(size.width.max(1)), f64::from(size.height.max(1)));
        Some(Frame {
            x: f64::from(inner.x - origin.x) / w,
            y: f64::from(inner.y - origin.y) / h,
            width: f64::from(inner_size.width) / w,
            height: f64::from(inner_size.height) / h,
        })
    }

    fn placement(&self) -> Placement {
        let window = self.window.as_ref();
        Placement {
            on_target: window
                .and_then(Window::current_monitor)
                .and_then(|m| monitor_id(&m))
                == Some(self.target.id),
            key: window.is_some_and(Window::has_focus),
            frontmost: engine::focus::frontmost() == Some(true),
            frame: self.frame(),
        }
    }

    fn check(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_none() {
            return;
        }
        let placement = self.placement();
        match self.readiness.observe(placement.ok(), Instant::now()) {
            Some(Change::Ready) => {
                let frame = placement.frame.expect("ready means placed");
                tracing::info!(?frame, display = ?self.target, "probe ready");
                self.write(&ProbeLine::Ready {
                    ready: true,
                    display: self.target,
                    frame,
                    key: placement.key,
                    frontmost: placement.frontmost,
                });
            }
            Some(Change::Lost) => {
                tracing::warn!("probe lost focus: {}", placement.problems());
                self.write(&ProbeLine::LostFocus {
                    lost_focus: placement,
                });
            }
            Some(Change::TimedOut) => {
                let message = format!(
                    "the test window didn't get focus on the display input goes to (display {} at {}, {}): {}",
                    self.target.id,
                    self.target.x,
                    self.target.y,
                    placement.problems()
                );
                self.write(&ProbeLine::NotReady {
                    not_ready: message.clone(),
                });
                self.failed = Some(message);
                event_loop.exit();
            }
            None => {}
        }
    }

    /// Where a point in the window is on the shared surface, which is the whole display. The
    /// window's own size isn't the display's: on a Mac with a notch, a full-screen window starts
    /// below the camera housing (32 pt on a 14-inch MacBook Pro), so dividing by the window's
    /// height would put every click a few percent too high.
    fn on_display(&self, (x, y): (f64, f64)) -> (f64, f64) {
        let Some(frame) = self.frame() else {
            return (0.0, 0.0);
        };
        let Some(monitor) = &self.monitor else {
            return (0.0, 0.0);
        };
        let size = monitor.size();
        let surface = input::Rect {
            x: 0,
            y: 0,
            w: size.width.max(1),
            h: size.height.max(1),
        };
        let at = (
            x + frame.x * f64::from(size.width),
            y + frame.y * f64::from(size.height),
        );
        input::to_surface_clamped(surface, at)
    }
}

impl ApplicationHandler for Probe {
    fn new_events(&mut self, event_loop: &ActiveEventLoop, _: StartCause) {
        let now = Instant::now();
        if now >= self.until {
            event_loop.exit();
        } else {
            event_loop.set_control_flow(ControlFlow::WaitUntil(self.until.min(now + RECHECK)));
        }
    }

    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let Some(monitor) = event_loop
            .available_monitors()
            .find(|m| monitor_id(m) == Some(self.target.id))
        else {
            let message = format!(
                "display {} (where input goes) isn't one this window can open on",
                self.target.id
            );
            self.write(&ProbeLine::NotReady {
                not_ready: message.clone(),
            });
            self.failed = Some(message);
            event_loop.exit();
            return;
        };
        let attributes = Window::default_attributes()
            .with_title("TwinCursor input probe")
            .with_position(monitor.position())
            .with_fullscreen(Some(Fullscreen::Borderless(Some(monitor.clone()))));
        let window = event_loop.create_window(attributes).expect("probe window");
        window.focus_window();
        tracing::info!(
            target = ?self.target,
            window = ?window.inner_position().ok().zip(Some(window.inner_size())),
            display = ?window.current_monitor().map(|m| (m.position(), m.size())),
            "probe window placed"
        );
        self.monitor = Some(monitor);
        self.window = Some(window);
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.check(event_loop);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Focused(_) => self.check(event_loop),
            WindowEvent::KeyboardInput { event, .. } => {
                let PhysicalKey::Code(code) = event.physical_key else {
                    return;
                };
                let pressed = event.state == ElementState::Pressed;
                self.record(InputEvent::Key {
                    code: input::key_code(code),
                    pressed,
                    text: if pressed {
                        input::key_text(event.text.as_deref())
                    } else {
                        None
                    },
                });
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = Some((position.x, position.y));
                self.moves += 1;
                let (x, y) = self.on_display((position.x, position.y));
                self.record(InputEvent::Move { x, y });
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if let (Some(button), Some(at)) = (input::button(button), self.cursor) {
                    let (x, y) = self.on_display(at);
                    self.record(InputEvent::Button {
                        button,
                        pressed: state == ElementState::Pressed,
                        x,
                        y,
                    });
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let scale = self.window.as_ref().map_or(1.0, Window::scale_factor);
                if let Some(event) = input::scroll(delta, scale) {
                    self.record(event);
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn placed(frame: Frame) -> Placement {
        Placement {
            on_target: true,
            key: true,
            frontmost: true,
            frame: Some(frame),
        }
    }

    const FULL: Frame = Frame {
        x: 0.0,
        y: 0.0,
        width: 1.0,
        height: 1.0,
    };

    #[test]
    fn a_window_covers_the_display_only_when_it_fills_it() {
        assert!(FULL.covers_display());
        // Below a notch.
        let notched = Frame {
            y: 0.033,
            height: 0.967,
            ..FULL
        };
        assert!(notched.covers_display());
        assert!(!Frame { width: 0.5, ..FULL }.covers_display(), "half");
        assert!(!Frame { x: 0.5, ..FULL }.covers_display(), "off the edge");
        assert!(notched.contains((0.5, 0.4), 0.01));
        assert!(!notched.contains((0.5, 0.02), 0.01), "in the menu bar");
    }

    #[test]
    fn ready_needs_every_condition() {
        assert!(placed(FULL).ok());
        for broken in [
            Placement {
                on_target: false,
                ..placed(FULL)
            },
            Placement {
                key: false,
                ..placed(FULL)
            },
            Placement {
                frontmost: false,
                ..placed(FULL)
            },
            Placement {
                frame: None,
                ..placed(FULL)
            },
            placed(Frame {
                height: 0.5,
                ..FULL
            }),
        ] {
            assert!(!broken.ok(), "{broken:?}");
            assert!(!broken.problems().is_empty());
        }
    }

    #[test]
    fn ready_once_settled_then_lost_at_every_fall() {
        let t0 = Instant::now();
        let ms = |n| t0 + Duration::from_millis(n);
        let mut r = Readiness::new(Duration::from_millis(500), ms(5000));
        assert_eq!(r.observe(true, ms(0)), None);
        // A flicker restarts the wait.
        assert_eq!(r.observe(false, ms(300)), None);
        assert_eq!(r.observe(true, ms(400)), None);
        assert_eq!(r.observe(true, ms(800)), None);
        assert_eq!(r.observe(true, ms(900)), Some(Change::Ready));
        assert_eq!(r.observe(true, ms(950)), None);
        assert_eq!(r.observe(false, ms(1000)), Some(Change::Lost));
        assert_eq!(r.observe(false, ms(1050)), None, "once per fall");
        assert_eq!(r.observe(true, ms(1100)), None);
        assert_eq!(r.observe(false, ms(1150)), Some(Change::Lost));
    }

    #[test]
    fn never_ready_times_out_once() {
        let t0 = Instant::now();
        let ms = |n| t0 + Duration::from_millis(n);
        let mut r = Readiness::new(Duration::from_millis(500), ms(2000));
        assert_eq!(r.observe(false, ms(1000)), None);
        assert_eq!(r.observe(true, ms(1800)), None);
        assert_eq!(r.observe(true, ms(2000)), Some(Change::TimedOut));
        assert_eq!(r.observe(true, ms(2600)), None, "too late to be ready");
    }

    #[test]
    fn records_read_back() {
        let lines = [
            r#"{"ready":true,"display":{"id":1,"x":0.0,"y":0.0,"width":1512.0,"height":982.0,"main":true},"frame":{"x":0.0,"y":0.03,"width":1.0,"height":0.97},"key":true,"frontmost":true}"#,
            r#"{"kind":"move","x":0.5,"y":0.4}"#,
            r#"{"lost_focus":{"on_target":true,"key":false,"frontmost":false,"frame":null}}"#,
            r#"{"not_ready":"no"}"#,
            r#"{"closed":true}"#,
        ];
        let parsed: Vec<ProbeLine> = lines
            .iter()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert!(matches!(parsed[0], ProbeLine::Ready { ready: true, .. }));
        assert!(matches!(
            parsed[1],
            ProbeLine::Event(InputEvent::Move { .. })
        ));
        assert!(matches!(parsed[2], ProbeLine::LostFocus { .. }));
        assert!(matches!(parsed[3], ProbeLine::NotReady { .. }));
        assert!(matches!(parsed[4], ProbeLine::Closed { closed: true }));
        // Written the same way.
        assert_eq!(serde_json::to_string(&parsed[1]).unwrap(), lines[1]);
    }
}
