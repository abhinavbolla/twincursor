//! Turns the viewer window's mouse, keyboard and wheel events into remote-control input.
//!
//! Pure functions, so the mapping is tested without a window.

use protocol::{Button, InputEvent, ScrollUnit};
use winit::event::{MouseButton, MouseScrollDelta};
use winit::keyboard::KeyCode;

/// Where the frame is drawn inside the window, in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// The largest rectangle with the frame's aspect ratio that fits in the window, centred.
/// Dimensions stay even, as I420 scaling needs.
pub fn letterbox(frame: (u32, u32), window: (u32, u32)) -> Rect {
    let (fw, fh) = (f64::from(frame.0.max(1)), f64::from(frame.1.max(1)));
    let (ww, wh) = (window.0.max(2), window.1.max(2));
    let scale = f64::min(f64::from(ww) / fw, f64::from(wh) / fh);
    let even = |v: f64, max: u32| ((v.round() as u32) & !1).clamp(2, max & !1);
    let (w, h) = (even(fw * scale, ww), even(fh * scale, wh));
    Rect {
        x: (ww - w) / 2,
        y: (wh - h) / 2,
        w,
        h,
    }
}

/// A window position as a point on the shared surface (0..1), or `None` outside the frame.
pub fn to_surface(rect: Rect, (px, py): (f64, f64)) -> Option<(f64, f64)> {
    let x = (px - f64::from(rect.x)) / f64::from(rect.w);
    let y = (py - f64::from(rect.y)) / f64::from(rect.h);
    ((0.0..=1.0).contains(&x) && (0.0..=1.0).contains(&y)).then_some((x, y))
}

/// Like [`to_surface`], but pulls points outside the frame to its edge. Used for releases, which
/// must reach the sharer wherever the pointer is.
pub fn to_surface_clamped(rect: Rect, (px, py): (f64, f64)) -> (f64, f64) {
    let x = (px - f64::from(rect.x)) / f64::from(rect.w);
    let y = (py - f64::from(rect.y)) / f64::from(rect.h);
    (x.clamp(0.0, 1.0), y.clamp(0.0, 1.0))
}

pub fn button(b: MouseButton) -> Option<Button> {
    match b {
        MouseButton::Left => Some(Button::Left),
        MouseButton::Right => Some(Button::Right),
        MouseButton::Middle => Some(Button::Middle),
        MouseButton::Back => Some(Button::Back),
        MouseButton::Forward => Some(Button::Forward),
        MouseButton::Other(_) => None,
    }
}

/// A wheel or trackpad event. winit's positive values move the content right and down, which is
/// scrolling left and up; the protocol uses the web's convention, where positive scrolls right
/// and down. Pixel deltas are converted to logical pixels, so a Retina guest scrolls as far as
/// any other.
pub fn scroll(delta: MouseScrollDelta, scale_factor: f64) -> Option<InputEvent> {
    let (dx, dy, unit) = match delta {
        MouseScrollDelta::LineDelta(x, y) => (f64::from(x), f64::from(y), ScrollUnit::Line),
        MouseScrollDelta::PixelDelta(p) => {
            (p.x / scale_factor, p.y / scale_factor, ScrollUnit::Pixel)
        }
    };
    let max = protocol::MAX_SCROLL;
    (dx != 0.0 || dy != 0.0).then_some(InputEvent::Scroll {
        dx: (-dx).clamp(-max, max),
        dy: (-dy).clamp(-max, max),
        unit,
    })
}

/// The key's name as `KeyboardEvent.code` gives it. winit uses the same names, except that it
/// calls ⌘ and the Windows key "Super" where the web says "Meta".
pub fn key_code(code: KeyCode) -> String {
    match code {
        KeyCode::SuperLeft => "MetaLeft".into(),
        KeyCode::SuperRight => "MetaRight".into(),
        other => format!("{other:?}"),
    }
}

/// Text a key press produced, if it's something to type. Function keys and the like produce
/// none; control characters are left to the physical key (ADR 0004).
pub fn key_text(text: Option<&str>) -> Option<String> {
    text.filter(|t| !t.is_empty() && t.chars().count() <= protocol::MAX_TEXT_CHARS)
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use winit::dpi::PhysicalPosition;

    #[test]
    fn letterboxes_a_wide_frame_in_a_tall_window() {
        assert_eq!(
            letterbox((1920, 1080), (1000, 1000)),
            Rect {
                x: 0,
                y: 219,
                w: 1000,
                h: 562
            }
        );
        assert_eq!(
            letterbox((1920, 1080), (1920, 1080)),
            Rect {
                x: 0,
                y: 0,
                w: 1920,
                h: 1080
            }
        );
        let r = letterbox((1280, 720), (3000, 1000));
        assert_eq!((r.h, r.w % 2, r.x), (1000, 0, (3000 - r.w) / 2));
    }

    #[test]
    fn maps_window_points_through_the_letterbox() {
        let r = Rect {
            x: 100,
            y: 50,
            w: 800,
            h: 400,
        };
        assert_eq!(to_surface(r, (100.0, 50.0)), Some((0.0, 0.0)));
        assert_eq!(to_surface(r, (500.0, 250.0)), Some((0.5, 0.5)));
        assert_eq!(to_surface(r, (900.0, 450.0)), Some((1.0, 1.0)));
        assert_eq!(to_surface(r, (99.0, 250.0)), None, "in the black bar");
        assert_eq!(to_surface_clamped(r, (0.0, 1000.0)), (0.0, 1.0));
    }

    #[test]
    fn every_key_name_passes_validation() {
        // A sample across winit's key codes, including the renamed ones.
        for code in [
            KeyCode::KeyA,
            KeyCode::Digit0,
            KeyCode::ShiftLeft,
            KeyCode::ControlRight,
            KeyCode::AltLeft,
            KeyCode::SuperLeft,
            KeyCode::Enter,
            KeyCode::ArrowUp,
            KeyCode::F12,
            KeyCode::Backquote,
            KeyCode::IntlBackslash,
            KeyCode::NumpadEnter,
        ] {
            let message = protocol::Message::Input {
                seq: 1,
                sent_at_us: 0,
                event: InputEvent::Key {
                    code: key_code(code),
                    pressed: true,
                    text: None,
                },
            };
            protocol::decode(&protocol::encode(&message)).expect("valid");
        }
        assert_eq!(key_code(KeyCode::SuperRight), "MetaRight");
        assert_eq!(key_code(KeyCode::KeyA), "KeyA");
        assert_eq!(key_code(KeyCode::ArrowLeft), "ArrowLeft");
    }

    #[test]
    fn scrolls_use_the_web_direction_in_logical_pixels() {
        // Two-finger swipe up on a Retina trackpad: content moves up, the page scrolls down.
        let up = MouseScrollDelta::PixelDelta(PhysicalPosition::new(0.0, -40.0));
        assert_eq!(
            scroll(up, 2.0),
            Some(InputEvent::Scroll {
                dx: 0.0,
                dy: 20.0,
                unit: ScrollUnit::Pixel
            })
        );
        // A wheel notch towards the user: scroll down one line.
        assert_eq!(
            scroll(MouseScrollDelta::LineDelta(0.0, -1.0), 1.0),
            Some(InputEvent::Scroll {
                dx: 0.0,
                dy: 1.0,
                unit: ScrollUnit::Line
            })
        );
        assert_eq!(scroll(MouseScrollDelta::LineDelta(0.0, 0.0), 1.0), None);
    }

    #[test]
    fn text_is_kept_only_when_typed() {
        assert_eq!(key_text(Some("é")), Some("é".into()));
        assert_eq!(key_text(Some("")), None);
        assert_eq!(key_text(None), None);
    }
}
