//! What the sharer sees on their own screen during remote control (hard rule 4).
//!
//! - **A label** at the top of the screen for as long as control lasts: who is controlling, and
//!   how to take it back. While a guest is asking, it says so.
//! - **A name tag** that follows the guest's pointer, so the sharer can tell whose pointer is
//!   whose, even while their own mouse has the real pointer.
//!
//! Both are small opaque windows: always on top, on every Space and over full-screen apps,
//! click-through, and never focused. Their pixels are drawn with the HUD's 8x8 font. The sharer's
//! screen capture leaves this app's windows out, so the guest doesn't see them in the stream.

use std::num::NonZeroU32;
use std::sync::{Arc, Mutex};

use control::Point;
use font8x8::legacy::{BASIC_LEGACY, LATIN_LEGACY};
use winit::dpi::{LogicalPosition, LogicalSize};
use winit::event_loop::{ActiveEventLoop, EventLoopProxy};
use winit::window::{Window, WindowLevel};

/// What the overlay should show. The control thread sets it; the main thread draws it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OverlayState {
    /// A guest is asking for control.
    pub asking: Option<String>,
    /// A guest has control.
    pub controller: Option<String>,
    /// Where the guest is pointing, 0..1 over the shared display.
    pub pointer: Option<Point>,
}

/// What wakes the sharer's main thread.
#[derive(Debug, Clone, Copy)]
pub enum UiEvent {
    /// The overlay state changed.
    Redraw,
    /// Sharing ended: close.
    Done,
}

/// The control thread's way to update the overlay.
#[derive(Clone)]
pub struct OverlayHandle {
    state: Arc<Mutex<OverlayState>>,
    proxy: EventLoopProxy<UiEvent>,
}

impl OverlayHandle {
    pub fn new(proxy: EventLoopProxy<UiEvent>) -> (Self, Arc<Mutex<OverlayState>>) {
        let state = Arc::new(Mutex::new(OverlayState::default()));
        (
            Self {
                state: state.clone(),
                proxy,
            },
            state,
        )
    }

    pub fn show(&self, next: OverlayState) {
        let mut state = self.state.lock().unwrap();
        if *state != next {
            *state = next;
            let _ = self.proxy.send_event(UiEvent::Redraw);
        }
    }
}

/// How the revoke hotkey reads in the label: the 8x8 font has no ⌃⌥⌘ glyphs.
const HOTKEY_TEXT: &str = "Ctrl+Option+Cmd+R";

const PAD: u32 = 10;
const LABEL_TOP: f64 = 34.0;
const TAG_OFFSET: (f64, f64) = (14.0, 16.0);

const WHITE: u32 = 0x00ff_ffff;
const CONTROL_BG: u32 = 0x00b4_2318;
const ASKING_BG: u32 = 0x0024_5bb4;
const TAG_BG: u32 = 0x00e8_7a10;

/// The text and background of the label, or none.
pub fn label(state: &OverlayState) -> Option<(String, u32)> {
    if let Some(name) = &state.controller {
        Some((
            format!("{name} is controlling this computer.  {HOTKEY_TEXT} takes it back."),
            CONTROL_BG,
        ))
    } else {
        state.asking.as_ref().map(|name| {
            (
                format!("{name} asks to control this computer. Answer in the terminal."),
                ASKING_BG,
            )
        })
    }
}

const LABEL_TITLE: &str = "TwinCursor control label";
const TAG_TITLE: &str = "TwinCursor guest pointer";

/// The overlay's windows, owned by the main thread.
pub struct Overlay {
    state: Arc<Mutex<OverlayState>>,
    label: Option<Panel>,
    tag: Option<Panel>,
}

impl Overlay {
    pub fn new(state: Arc<Mutex<OverlayState>>) -> Self {
        Self {
            state,
            label: None,
            tag: None,
        }
    }

    /// Creates both panels, hidden. Call it before screen capture starts: ScreenCaptureKit only
    /// leaves this app's windows out of the stream if the app has windows when capture starts,
    /// and panels first made on a grant would be captured.
    pub fn prepare(&mut self, event_loop: &ActiveEventLoop) {
        self.label
            .get_or_insert_with(|| Panel::new(event_loop, LABEL_TITLE));
        self.tag
            .get_or_insert_with(|| Panel::new(event_loop, TAG_TITLE));
    }

    /// Brings the windows in line with the current state.
    pub fn update(&mut self, event_loop: &ActiveEventLoop) {
        let state = self.state.lock().unwrap().clone();
        let Some(display) = event_loop.primary_monitor() else {
            return;
        };
        let scale = display.scale_factor();
        let screen: LogicalSize<f64> = display.size().to_logical(scale);
        let origin: LogicalPosition<f64> = display.position().to_logical(scale);

        match label(&state) {
            Some((text, bg)) => {
                let panel = self
                    .label
                    .get_or_insert_with(|| Panel::new(event_loop, LABEL_TITLE));
                let (w, _) = panel.show(&text, bg, scale);
                let x = origin.x + (screen.width - w) / 2.0;
                place(&panel.window, x, origin.y + LABEL_TOP);
            }
            None => {
                if let Some(panel) = &self.label {
                    panel.hide();
                }
            }
        }

        match (&state.controller, state.pointer) {
            (Some(name), Some(p)) => {
                let panel = self
                    .tag
                    .get_or_insert_with(|| Panel::new(event_loop, TAG_TITLE));
                panel.show(name, TAG_BG, scale);
                let x = origin.x + p.x * screen.width + TAG_OFFSET.0;
                let y = origin.y + p.y * screen.height + TAG_OFFSET.1;
                place(&panel.window, x, y);
            }
            _ => {
                if let Some(panel) = &self.tag {
                    panel.hide();
                }
            }
        }
    }
}

struct Panel {
    window: Arc<Window>,
    surface: softbuffer::Surface<Arc<Window>, Arc<Window>>,
    shown: Option<(String, u32)>,
    size: (f64, f64),
}

impl Panel {
    fn new(event_loop: &ActiveEventLoop, title: &str) -> Self {
        let attributes = Window::default_attributes()
            .with_title(title)
            .with_decorations(false)
            .with_resizable(false)
            .with_visible(false)
            .with_active(false)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_inner_size(LogicalSize::new(10.0, 10.0));
        let window = Arc::new(
            event_loop
                .create_window(attributes)
                .expect("overlay window"),
        );
        // Clicks go to whatever is underneath.
        let _ = window.set_cursor_hittest(false);
        #[cfg(target_os = "macos")]
        macos::float_everywhere(&window);
        let context = softbuffer::Context::new(window.clone()).expect("softbuffer context");
        let surface = softbuffer::Surface::new(&context, window.clone()).expect("surface");
        Self {
            window,
            surface,
            shown: None,
            size: (10.0, 10.0),
        }
    }

    /// Draws `text` if it changed, and shows the window. Returns its logical size.
    fn show(&mut self, text: &str, bg: u32, scale: f64) -> (f64, f64) {
        let wanted = (text.to_owned(), bg);
        if self.shown.as_ref() != Some(&wanted) {
            let glyph = glyph_px(scale);
            let pad = (f64::from(PAD) * scale).round() as u32;
            let (w, h) = text_size(text, glyph, pad);
            self.size = (f64::from(w) / scale, f64::from(h) / scale);
            let _ = self
                .window
                .request_inner_size(LogicalSize::new(self.size.0, self.size.1));
            if self
                .surface
                .resize(NonZeroU32::new(w).unwrap(), NonZeroU32::new(h).unwrap())
                .is_ok()
            {
                if let Ok(mut buffer) = self.surface.buffer_mut() {
                    render(&mut buffer, w as usize, h as usize, text, bg, glyph, pad);
                    let _ = buffer.present();
                }
            }
            self.shown = Some(wanted);
        }
        set_shown(&self.window, true);
        self.size
    }

    fn hide(&self) {
        set_shown(&self.window, false);
    }
}

/// Puts a panel's top-left corner at (`x`, `y`), in logical points from the top-left of the
/// main display.
fn place(window: &Window, x: f64, y: f64) {
    #[cfg(target_os = "macos")]
    macos::place(window, x, y);
    #[cfg(not(target_os = "macos"))]
    window.set_outer_position(LogicalPosition::new(x, y));
}

/// Shows or hides a panel without activating this app or taking focus.
fn set_shown(window: &Window, shown: bool) {
    #[cfg(target_os = "macos")]
    macos::order(window, shown);
    #[cfg(not(target_os = "macos"))]
    window.set_visible(shown);
}

/// Glyph size in physical pixels: 16 logical pixels, a whole multiple of 8 so it stays crisp.
fn glyph_px(scale: f64) -> u32 {
    ((2.0 * scale).round().max(1.0) as u32) * 8
}

fn text_size(text: &str, glyph: u32, pad: u32) -> (u32, u32) {
    let cols = text.chars().count() as u32;
    (cols * glyph + 2 * pad, glyph + 2 * pad)
}

/// The 8x8 glyph for `c`: ASCII and Latin-1, or a box for anything else.
fn glyph(c: char) -> [u8; 8] {
    match u32::from(c) {
        n @ 0..=0x7f => BASIC_LEGACY[n as usize],
        n @ 0xa0..=0xff => LATIN_LEGACY[(n - 0xa0) as usize],
        _ => [0x7e, 0x42, 0x42, 0x42, 0x42, 0x42, 0x7e, 0x00],
    }
}

/// Fills a `w` x `h` 0RGB buffer with `bg` and writes `text` on it in white.
pub fn render(
    pixels: &mut [u32],
    w: usize,
    h: usize,
    text: &str,
    bg: u32,
    glyph_size: u32,
    pad: u32,
) {
    pixels.fill(bg);
    let dot = (glyph_size / 8) as usize;
    let (pad, glyph_size) = (pad as usize, glyph_size as usize);
    for (i, c) in text.chars().enumerate() {
        let left = pad + i * glyph_size;
        for (row, bits) in glyph(c).iter().enumerate() {
            for col in 0..8 {
                if bits >> col & 1 == 1 {
                    for y in pad + row * dot..(pad + (row + 1) * dot).min(h) {
                        for x in left + col * dot..(left + (col + 1) * dot).min(w) {
                            pixels[y * w + x] = WHITE;
                        }
                    }
                }
            }
        }
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use objc2::encode::{Encode, Encoding};
    use objc2::msg_send;
    use objc2::runtime::AnyObject;
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use winit::window::Window;

    /// NSWindowCollectionBehavior: on every Space, over full-screen apps, left out of ⌘`.
    /// Not "stationary": macOS keeps a stationary window in the on-screen window list after it's
    /// ordered out (found on macOS 26).
    const CAN_JOIN_ALL_SPACES: usize = 1 << 0;
    const IGNORES_CYCLE: usize = 1 << 6;
    const FULL_SCREEN_AUXILIARY: usize = 1 << 8;
    /// NSStatusWindowLevel: above ordinary floating windows and full-screen apps' windows.
    const STATUS_LEVEL: isize = 25;

    /// winit's NSWindow, or null.
    fn ns_window(window: &Window) -> *mut AnyObject {
        let Ok(handle) = window.window_handle() else {
            return std::ptr::null_mut();
        };
        let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
            return std::ptr::null_mut();
        };
        let view = appkit.ns_view.as_ptr().cast::<AnyObject>();
        // SAFETY: winit's NSView, on the main thread that owns it; NSView responds to `window`.
        unsafe { msg_send![view, window] }
    }

    pub fn float_everywhere(window: &Window) {
        let ns_window = ns_window(window);
        if ns_window.is_null() {
            return;
        }
        let behavior = CAN_JOIN_ALL_SPACES | IGNORES_CYCLE | FULL_SCREEN_AUXILIARY;
        // SAFETY: an NSWindow on its main thread; both setters take these plain integer types.
        unsafe {
            let () = msg_send![ns_window, setCollectionBehavior: behavior];
            let () = msg_send![ns_window, setLevel: STATUS_LEVEL];
        }
    }

    #[repr(C)]
    struct NSPoint {
        x: f64,
        y: f64,
    }

    // SAFETY: NSPoint is CGPoint, two doubles, laid out as declared.
    unsafe impl Encode for NSPoint {
        const ENCODING: Encoding = Encoding::Struct("CGPoint", &[f64::ENCODING, f64::ENCODING]);
    }

    /// Cocoa measures from the bottom-left of the main display, so the top-left corner is
    /// flipped against its height. Setting the corner directly keeps it put when the window's
    /// size changes, which `set_outer_position` on a window being resized doesn't.
    pub fn place(window: &Window, x: f64, y: f64) {
        let ns_window = ns_window(window);
        if ns_window.is_null() {
            return;
        }
        let height = core_graphics_main_height();
        let corner = NSPoint { x, y: height - y };
        // SAFETY: an NSWindow on its main thread; `setFrameTopLeftPoint:` takes an NSPoint.
        unsafe {
            let () = msg_send![ns_window, setFrameTopLeftPoint: corner];
        }
    }

    fn core_graphics_main_height() -> f64 {
        #[link(name = "CoreGraphics", kind = "framework")]
        extern "C" {
            fn CGMainDisplayID() -> u32;
            fn CGDisplayBounds(display: u32) -> CGRect;
        }
        #[repr(C)]
        struct CGRect {
            x: f64,
            y: f64,
            w: f64,
            h: f64,
        }
        // SAFETY: plain C calls with no pointers; CGRect is four doubles.
        unsafe { CGDisplayBounds(CGMainDisplayID()).h }
    }

    /// Puts the window on screen without making this app active, or takes it off.
    pub fn order(window: &Window, shown: bool) {
        let ns_window = ns_window(window);
        if ns_window.is_null() {
            return;
        }
        let none: *mut AnyObject = std::ptr::null_mut();
        // SAFETY: an NSWindow on its main thread. Both methods exist on NSWindow; `orderOut:`
        // takes a sender, which may be nil.
        unsafe {
            if shown {
                let () = msg_send![ns_window, orderFrontRegardless];
            } else {
                let () = msg_send![ns_window, orderOut: none];
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(asking: Option<&str>, controller: Option<&str>) -> OverlayState {
        OverlayState {
            asking: asking.map(Into::into),
            controller: controller.map(Into::into),
            pointer: None,
        }
    }

    #[test]
    fn the_label_names_the_guest_and_the_way_out() {
        assert_eq!(label(&OverlayState::default()), None);
        let (text, bg) = label(&named(Some("André"), None)).unwrap();
        assert!(text.starts_with("André asks to control"), "{text}");
        assert_eq!(bg, ASKING_BG);
        let (text, bg) = label(&named(None, Some("André"))).unwrap();
        assert_eq!(
            text,
            "André is controlling this computer.  Ctrl+Option+Cmd+R takes it back."
        );
        assert_eq!(bg, CONTROL_BG);
    }

    #[test]
    fn renders_text_in_white_on_the_background() {
        let size = glyph_px(2.0);
        assert_eq!(size, 32, "16 logical pixels on Retina");
        let (w, h) = text_size("Hé!", size, 20);
        let mut pixels = vec![0; (w * h) as usize];
        render(
            &mut pixels,
            w as usize,
            h as usize,
            "Hé!",
            CONTROL_BG,
            size,
            20,
        );
        let white = pixels.iter().filter(|&&p| p == WHITE).count();
        assert!(white > 100, "{white} white pixels");
        assert!(pixels.iter().all(|&p| p == WHITE || p == CONTROL_BG));
        // Padding stays background.
        assert!(pixels[..w as usize * 20].iter().all(|&p| p == CONTROL_BG));
        // é is drawn (Latin-1), not the box for unknown characters.
        assert_ne!(glyph('é'), glyph('✓'));
    }
}
