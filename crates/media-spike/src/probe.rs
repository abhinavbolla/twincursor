//! `tc-spike probe`: a full-screen window that records the input it receives.
//!
//! It turns window events into protocol events with the same code the viewer uses, so its
//! recording can be compared with what a guest sent (`tc-spike replay --probe`). That closes the
//! loop on real injection: guest → network → sharer → CGEvent → macOS → this window.

use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use clap::Args;
use protocol::InputEvent;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::PhysicalKey;
use winit::window::{Fullscreen, Window, WindowId};

use crate::input;

#[derive(Args)]
pub struct ProbeArgs {
    /// Where to write what the window receives, one JSON line per event.
    #[arg(long)]
    out: PathBuf,
    /// Close after this many seconds.
    #[arg(long, default_value_t = 60)]
    duration: u64,
}

pub fn run(args: ProbeArgs) -> anyhow::Result<()> {
    let event_loop = EventLoop::new()?;
    let mut probe = Probe {
        out: std::fs::File::create(&args.out)?,
        until: Instant::now() + Duration::from_secs(args.duration),
        window: None,
        cursor: None,
        moves: 0,
    };
    event_loop.run_app(&mut probe)?;
    tracing::info!(moves = probe.moves, "probe closed");
    Ok(())
}

struct Probe {
    out: std::fs::File,
    until: Instant,
    window: Option<Window>,
    cursor: Option<(f64, f64)>,
    moves: u64,
}

impl Probe {
    fn record(&mut self, event: &InputEvent) {
        let line = serde_json::to_string(event).expect("events serialize");
        let _ = writeln!(self.out, "{line}");
    }

    /// The window covers the main display, so its size is the shared surface.
    fn surface(&self) -> input::Rect {
        let size = self.window.as_ref().map(|w| w.inner_size());
        let (w, h) = size.map_or((1, 1), |s| (s.width.max(1), s.height.max(1)));
        input::Rect { x: 0, y: 0, w, h }
    }
}

impl ApplicationHandler for Probe {
    fn new_events(&mut self, event_loop: &ActiveEventLoop, _: StartCause) {
        if Instant::now() >= self.until {
            event_loop.exit();
        } else {
            event_loop.set_control_flow(ControlFlow::WaitUntil(self.until));
        }
    }

    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attributes = Window::default_attributes()
            .with_title("TwinCursor input probe")
            .with_fullscreen(Some(Fullscreen::Borderless(None)));
        let window = event_loop.create_window(attributes).expect("probe window");
        window.focus_window();
        self.window = Some(window);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::KeyboardInput { event, .. } => {
                let PhysicalKey::Code(code) = event.physical_key else {
                    return;
                };
                let pressed = event.state == ElementState::Pressed;
                self.record(&InputEvent::Key {
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
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if let (Some(button), Some(at)) = (input::button(button), self.cursor) {
                    let (x, y) = input::to_surface_clamped(self.surface(), at);
                    self.record(&InputEvent::Button {
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
                    self.record(&event);
                }
            }
            WindowEvent::RedrawRequested => {}
            _ => {}
        }
    }
}
