//! Real input on macOS, through CGEvent (via `enigo`).
//!
//! Needs the Accessibility permission for the app that runs it (for `tc-spike`, the terminal).
//! Events come from a private event source, so the sharer's own held keys don't leak into the
//! guest's input, and the other way round.
//!
//! **Nothing is injected while the screen is locked.** Typed keys would land in the lock screen's
//! password field, which would let a guest try to unlock the sharer's Mac. Releases still go
//! through, so nothing stays held.

use std::time::{Duration, Instant};

use core_foundation::base::TCFType;
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
use core_foundation::string::CFString;
use core_graphics::display::CGDisplay;
use core_graphics::event::CGEvent;
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use enigo::{Axis, Coordinate, Direction, Enigo, Keyboard, Mouse, Settings};
use protocol::{Button, ScrollUnit};

use crate::{Action, Injector, Point};

/// How long a lock-screen check or the display's bounds are trusted before asking again.
const RECHECK: Duration = Duration::from_millis(50);
const BOUNDS_RECHECK: Duration = Duration::from_secs(2);

#[derive(Debug, thiserror::Error)]
pub enum MacError {
    #[error(
        "this app may not control the computer. Allow it in System Settings → Privacy & \
         Security → Accessibility (for tc-spike, allow the terminal you run it from), then \
         start again"
    )]
    NoPermission,
    #[error("couldn't set up input injection: {0}")]
    Setup(String),
}

pub struct MacInjector {
    enigo: Enigo,
    /// The main display in global points: x, y, width, height.
    bounds: (f64, f64, f64, f64),
    bounds_at: Instant,
    locked: bool,
    locked_at: Option<Instant>,
    /// Scroll left over from fractional deltas, so slow trackpad scrolls aren't lost.
    carry: [(f64, f64); 2],
    /// Actions held back because the screen was locked.
    pub paused: u64,
    /// Actions the system refused.
    pub failed: u64,
}

impl MacInjector {
    pub fn new() -> Result<Self, MacError> {
        let settings = Settings {
            open_prompt_to_get_permissions: false,
            independent_of_keyboard_state: true,
            release_keys_when_dropped: true,
            ..Settings::default()
        };
        let enigo = Enigo::new(&settings).map_err(|e| match e {
            enigo::NewConError::NoPermission => MacError::NoPermission,
            other => MacError::Setup(other.to_string()),
        })?;
        Ok(Self {
            enigo,
            bounds: main_bounds(),
            bounds_at: Instant::now(),
            locked: false,
            locked_at: None,
            carry: [(0.0, 0.0); 2],
            paused: 0,
            failed: 0,
        })
    }

    fn screen_locked(&mut self) -> bool {
        if self.locked_at.is_none_or(|t| t.elapsed() >= RECHECK) {
            let locked = screen_is_locked();
            if locked != self.locked {
                if locked {
                    tracing::warn!("the screen is locked: remote input is paused");
                } else {
                    tracing::info!("the screen is unlocked: remote input resumes");
                }
            }
            self.locked = locked;
            self.locked_at = Some(Instant::now());
        }
        self.locked
    }

    fn display(&mut self) -> (f64, f64, f64, f64) {
        if self.bounds_at.elapsed() >= BOUNDS_RECHECK {
            self.bounds = main_bounds();
            self.bounds_at = Instant::now();
        }
        self.bounds
    }

    fn apply(&mut self, action: &Action) -> enigo::InputResult<()> {
        match action {
            Action::MoveTo(p) => {
                let (x, y) = to_points(*p, self.display());
                self.enigo.move_mouse(x, y, Coordinate::Abs)
            }
            Action::Button { button, pressed } => self
                .enigo
                .button(enigo_button(*button), direction(*pressed)),
            Action::Scroll { dx, dy, unit } => {
                let slot = usize::from(*unit == ScrollUnit::Line);
                let (sx, sy) = &mut self.carry[slot];
                let (x, y) = (take_whole(sx, *dx), take_whole(sy, *dy));
                // enigo's positive lengths scroll right and down, as the protocol's do.
                for (len, axis) in [(y, Axis::Vertical), (x, Axis::Horizontal)] {
                    if len != 0 {
                        match unit {
                            ScrollUnit::Pixel => self.enigo.smooth_scroll(len, axis)?,
                            ScrollUnit::Line => self.enigo.scroll(len, axis)?,
                        }
                    }
                }
                Ok(())
            }
            Action::Key { code, pressed } => match virtual_key(code) {
                Some(vk) => self.enigo.raw(vk, direction(*pressed)),
                None => {
                    tracing::debug!(code, "no such key on a Mac");
                    Ok(())
                }
            },
            Action::Text { text } => self.enigo.text(text),
        }
    }
}

impl Injector for MacInjector {
    fn inject(&mut self, action: &Action) {
        let release = matches!(
            action,
            Action::Key { pressed: false, .. } | Action::Button { pressed: false, .. }
        );
        if !release && self.screen_locked() {
            self.paused += 1;
            return;
        }
        if let Err(e) = self.apply(action) {
            self.failed += 1;
            tracing::warn!(?action, "input not injected: {e}");
        }
    }

    fn pointer(&self) -> Option<Point> {
        let source = CGEventSource::new(CGEventSourceStateID::CombinedSessionState).ok()?;
        let at = CGEvent::new(source).ok()?.location();
        let (x0, y0, w, h) = self.bounds;
        Some(Point {
            x: ((at.x - x0) / w).clamp(0.0, 1.0),
            y: ((at.y - y0) / h).clamp(0.0, 1.0),
        })
    }
}

impl Drop for MacInjector {
    fn drop(&mut self) {
        tracing::info!(
            paused = self.paused,
            failed = self.failed,
            "real input injection stopped"
        );
    }
}

fn main_bounds() -> (f64, f64, f64, f64) {
    let b = CGDisplay::main().bounds();
    (b.origin.x, b.origin.y, b.size.width, b.size.height)
}

/// A point on the shared surface (the main display) in global display points.
fn to_points(p: Point, (x0, y0, w, h): (f64, f64, f64, f64)) -> (i32, i32) {
    // The last point is width - 1: a pointer at x = width would be on the next display.
    let x = x0 + (p.x * w).min(w - 1.0);
    let y = y0 + (p.y * h).min(h - 1.0);
    (x.round() as i32, y.round() as i32)
}

/// Adds `delta` to `carry` and takes out the whole part.
fn take_whole(carry: &mut f64, delta: f64) -> i32 {
    *carry += delta;
    // A hair of tolerance, so ten scrolls of 0.3 make 3 and not 2.999….
    let whole = (*carry + carry.signum() * 1e-9).trunc();
    *carry -= whole;
    whole as i32
}

fn direction(pressed: bool) -> Direction {
    if pressed {
        Direction::Press
    } else {
        Direction::Release
    }
}

fn enigo_button(b: Button) -> enigo::Button {
    match b {
        Button::Left => enigo::Button::Left,
        Button::Right => enigo::Button::Right,
        Button::Middle => enigo::Button::Middle,
        Button::Back => enigo::Button::Back,
        Button::Forward => enigo::Button::Forward,
    }
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGSessionCopyCurrentDictionary() -> CFDictionaryRef;
}

/// Whether the login session's screen is locked (or the session isn't on the console).
pub fn screen_is_locked() -> bool {
    // SAFETY: a Copy function: we own the returned dictionary, which may be null.
    let raw = unsafe { CGSessionCopyCurrentDictionary() };
    if raw.is_null() {
        // No session dictionary: nobody is logged in at the console.
        return true;
    }
    // SAFETY: non-null, and created with a +1 retain count that wrap_under_create_rule takes.
    let dict: CFDictionary<CFString, CFBoolean> =
        unsafe { CFDictionary::wrap_under_create_rule(raw) };
    dict.find(CFString::from_static_string("CGSSessionScreenIsLocked"))
        .is_some_and(|v| bool::from(v.clone()))
}

/// The Mac virtual key code (`kVK_*` in HIToolbox's Events.h) for a key named as
/// `KeyboardEvent.code` does. `None` for keys a Mac keyboard doesn't have.
pub fn virtual_key(code: &str) -> Option<u16> {
    let vk = match code {
        "KeyA" => 0x00,
        "KeyS" => 0x01,
        "KeyD" => 0x02,
        "KeyF" => 0x03,
        "KeyH" => 0x04,
        "KeyG" => 0x05,
        "KeyZ" => 0x06,
        "KeyX" => 0x07,
        "KeyC" => 0x08,
        "KeyV" => 0x09,
        "IntlBackslash" => 0x0A,
        "KeyB" => 0x0B,
        "KeyQ" => 0x0C,
        "KeyW" => 0x0D,
        "KeyE" => 0x0E,
        "KeyR" => 0x0F,
        "KeyY" => 0x10,
        "KeyT" => 0x11,
        "Digit1" => 0x12,
        "Digit2" => 0x13,
        "Digit3" => 0x14,
        "Digit4" => 0x15,
        "Digit6" => 0x16,
        "Digit5" => 0x17,
        "Equal" => 0x18,
        "Digit9" => 0x19,
        "Digit7" => 0x1A,
        "Minus" => 0x1B,
        "Digit8" => 0x1C,
        "Digit0" => 0x1D,
        "BracketRight" => 0x1E,
        "KeyO" => 0x1F,
        "KeyU" => 0x20,
        "BracketLeft" => 0x21,
        "KeyI" => 0x22,
        "KeyP" => 0x23,
        "Enter" => 0x24,
        "KeyL" => 0x25,
        "KeyJ" => 0x26,
        "Quote" => 0x27,
        "KeyK" => 0x28,
        "Semicolon" => 0x29,
        "Backslash" => 0x2A,
        "Comma" => 0x2B,
        "Slash" => 0x2C,
        "KeyN" => 0x2D,
        "KeyM" => 0x2E,
        "Period" => 0x2F,
        "Tab" => 0x30,
        "Space" => 0x31,
        "Backquote" => 0x32,
        "Backspace" => 0x33,
        "Escape" => 0x35,
        "MetaRight" => 0x36,
        "MetaLeft" => 0x37,
        "ShiftLeft" => 0x38,
        "CapsLock" => 0x39,
        "AltLeft" => 0x3A,
        "ControlLeft" => 0x3B,
        "ShiftRight" => 0x3C,
        "AltRight" => 0x3D,
        "ControlRight" => 0x3E,
        "Fn" => 0x3F,
        "F17" => 0x40,
        "NumpadDecimal" => 0x41,
        "NumpadMultiply" => 0x43,
        "NumpadAdd" => 0x45,
        "NumLock" => 0x47,
        "AudioVolumeUp" => 0x48,
        "AudioVolumeDown" => 0x49,
        "AudioVolumeMute" => 0x4A,
        "NumpadDivide" => 0x4B,
        "NumpadEnter" => 0x4C,
        "NumpadSubtract" => 0x4E,
        "F18" => 0x4F,
        "F19" => 0x50,
        "NumpadEqual" => 0x51,
        "Numpad0" => 0x52,
        "Numpad1" => 0x53,
        "Numpad2" => 0x54,
        "Numpad3" => 0x55,
        "Numpad4" => 0x56,
        "Numpad5" => 0x57,
        "Numpad6" => 0x58,
        "Numpad7" => 0x59,
        "F20" => 0x5A,
        "Numpad8" => 0x5B,
        "Numpad9" => 0x5C,
        "IntlYen" => 0x5D,
        "IntlRo" => 0x5E,
        "NumpadComma" => 0x5F,
        "F5" => 0x60,
        "F6" => 0x61,
        "F7" => 0x62,
        "F3" => 0x63,
        "F8" => 0x64,
        "F9" => 0x65,
        "Lang2" => 0x66,
        "F11" => 0x67,
        "Lang1" => 0x68,
        "F13" => 0x69,
        "F16" => 0x6A,
        "F14" => 0x6B,
        "F10" => 0x6D,
        "F12" => 0x6F,
        "F15" => 0x71,
        "Insert" | "Help" => 0x72,
        "Home" => 0x73,
        "PageUp" => 0x74,
        "Delete" => 0x75,
        "F4" => 0x76,
        "End" => 0x77,
        "F2" => 0x78,
        "PageDown" => 0x79,
        "F1" => 0x7A,
        "ArrowLeft" => 0x7B,
        "ArrowRight" => 0x7C,
        "ArrowDown" => 0x7D,
        "ArrowUp" => 0x7E,
        _ => return None,
    };
    Some(vk)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_the_surface_onto_display_points() {
        let display = (0.0, 0.0, 1512.0, 982.0);
        assert_eq!(to_points(Point { x: 0.0, y: 0.0 }, display), (0, 0));
        assert_eq!(to_points(Point { x: 0.5, y: 0.5 }, display), (756, 491));
        assert_eq!(
            to_points(Point { x: 1.0, y: 1.0 }, display),
            (1511, 981),
            "stays on this display"
        );
        // A main display that isn't at the origin of the global space.
        assert_eq!(
            to_points(Point { x: 0.25, y: 0.0 }, (-1920.0, 100.0, 1920.0, 1080.0)),
            (-1440, 100)
        );
    }

    #[test]
    fn slow_scrolls_add_up_instead_of_vanishing() {
        let mut carry = 0.0;
        let steps: Vec<i32> = (0..10).map(|_| take_whole(&mut carry, 0.3)).collect();
        assert_eq!(steps.iter().sum::<i32>(), 3);
        let mut carry = 0.0;
        assert_eq!(take_whole(&mut carry, -2.5), -2);
        assert_eq!(take_whole(&mut carry, -0.5), -1);
    }

    #[test]
    fn every_key_the_viewer_sends_for_typing_has_a_mac_key() {
        for c in 'A'..='Z' {
            assert!(virtual_key(&format!("Key{c}")).is_some(), "Key{c}");
        }
        for d in 0..=9 {
            assert!(virtual_key(&format!("Digit{d}")).is_some(), "Digit{d}");
        }
        for code in [
            "Enter",
            "Tab",
            "Space",
            "Backspace",
            "Delete",
            "Escape",
            "ArrowLeft",
            "ArrowRight",
            "ArrowUp",
            "ArrowDown",
            "Home",
            "End",
            "PageUp",
            "PageDown",
            "ShiftLeft",
            "ShiftRight",
            "ControlLeft",
            "AltLeft",
            "MetaLeft",
            "MetaRight",
            "F1",
            "F12",
        ] {
            assert!(virtual_key(code).is_some(), "{code}");
        }
        assert_eq!(virtual_key("PrintScreen"), None);
        // No two names share a code, except the aliases for Insert.
        let mut seen = std::collections::HashMap::new();
        for code in KNOWN {
            if let Some(vk) = virtual_key(code) {
                if let Some(other) = seen.insert(vk, *code) {
                    panic!("{code} and {other} are both {vk:#x}");
                }
            }
        }
    }

    const KNOWN: &[&str] = &[
        "KeyA",
        "KeyS",
        "KeyQ",
        "Digit1",
        "Digit0",
        "Equal",
        "Minus",
        "BracketLeft",
        "BracketRight",
        "Enter",
        "Quote",
        "Semicolon",
        "Backslash",
        "Comma",
        "Slash",
        "Period",
        "Tab",
        "Space",
        "Backquote",
        "Backspace",
        "Escape",
        "MetaRight",
        "MetaLeft",
        "ShiftLeft",
        "CapsLock",
        "AltLeft",
        "ControlLeft",
        "ShiftRight",
        "AltRight",
        "ControlRight",
        "Fn",
        "Insert",
        "Home",
        "PageUp",
        "Delete",
        "End",
        "PageDown",
        "ArrowLeft",
        "ArrowRight",
        "ArrowDown",
        "ArrowUp",
        "F1",
        "F2",
        "F3",
        "F4",
        "F5",
        "F6",
        "F7",
        "F8",
        "F9",
        "F10",
        "F11",
        "F12",
        "IntlBackslash",
        "NumpadEnter",
        "Numpad0",
        "Numpad9",
    ];

    #[test]
    fn reads_the_lock_state_without_crashing() {
        // The answer depends on the machine; this only proves the call and the dictionary
        // lookup are sound.
        eprintln!("screen locked: {}", screen_is_locked());
    }
}
