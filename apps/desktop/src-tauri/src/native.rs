//! What the windows need from AppKit that Tauri doesn't offer: no traffic lights, kept out of
//! the Window menu and the window cycle, the call bar on every Space and over full-screen apps,
//! showing a window without making it key, the displays' usable areas, and frames set in one
//! step. Public AppKit API only (no private API, no transparent windows). Everything here must
//! run on the main thread; off it, it does nothing. Other platforms get Tauri's nearest
//! equivalents until their milestone.
//!
//! Rects are in [`crate::surfaces`]' space: points, origin at the main display's top left, y
//! downward. AppKit's has its origin at the bottom left, so they are flipped here.

use crate::surfaces::{Display, Kind, Rect};

#[cfg(target_os = "macos")]
mod mac {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{
        NSAppearance, NSAppearanceNameAqua, NSAppearanceNameDarkAqua, NSApplication, NSEvent,
        NSEventModifierFlags, NSScreen, NSTitlebarSeparatorStyle, NSWindow, NSWindowButton,
        NSWindowCollectionBehavior,
    };
    use objc2_foundation::{NSArray, NSPoint, NSRect, NSSize};
    use tauri::WebviewWindow;

    use super::{Display, Kind, Rect};

    /// Runs `f` with the window's `NSWindow`, on the main thread only.
    fn with<T>(w: &WebviewWindow, f: impl FnOnce(&NSWindow, MainThreadMarker) -> T) -> Option<T> {
        let mtm = MainThreadMarker::new()?;
        let ptr = w.ns_window().ok()?;
        if ptr.is_null() {
            return None;
        }
        // SAFETY: Tauri hands out the live NSWindow of this window; it outlives this call,
        // which is on the main thread (checked above), where AppKit wants it used.
        let window = unsafe { &*(ptr as *const NSWindow) };
        Some(f(window, mtm))
    }

    /// The window's native behaviour, once, right after it is made.
    pub fn dress(w: &WebviewWindow, kind: Kind) {
        with(w, |window, _| {
            // The title bar stays (it gives the system's corners, hairline and shadow) but shows
            // nothing: no title (Tauri's `hiddenTitle`), no separator, no traffic lights.
            for button in [
                NSWindowButton::CloseButton,
                NSWindowButton::MiniaturizeButton,
                NSWindowButton::ZoomButton,
            ] {
                if let Some(button) = window.standardWindowButton(button) {
                    button.setHidden(true);
                }
            }
            window.setTitlebarSeparatorStyle(NSTitlebarSeparatorStyle::None);
            window.setExcludedFromWindowsMenu(true);
            let behaviour = match kind {
                // Shown on whichever Space is current, including a full-screen app's.
                Kind::Panel => {
                    NSWindowCollectionBehavior::MoveToActiveSpace
                        | NSWindowCollectionBehavior::Transient
                        | NSWindowCollectionBehavior::IgnoresCycle
                        | NSWindowCollectionBehavior::FullScreenAuxiliary
                }
                // On every Space, over full-screen apps, left alone by Mission Control.
                Kind::Bar => {
                    NSWindowCollectionBehavior::CanJoinAllSpaces
                        | NSWindowCollectionBehavior::Stationary
                        | NSWindowCollectionBehavior::IgnoresCycle
                        | NSWindowCollectionBehavior::FullScreenAuxiliary
                }
            };
            window.setCollectionBehavior(behaviour);
            // The panel hangs from the menu bar icon and never moves; the bar moves only by its
            // drag region (`start_dragging`), since the web content covers the title bar.
            window.setMovable(kind == Kind::Bar);
        });
    }

    /// On screen, in front, without becoming key: the app the person is in keeps the keyboard.
    pub fn show_without_focus(w: &WebviewWindow) {
        with(w, |window, _| window.orderFrontRegardless());
    }

    fn main_height(mtm: MainThreadMarker) -> f64 {
        NSScreen::screens(mtm)
            .firstObject()
            .map_or(0.0, |s| s.frame().size.height)
    }

    fn from_ns(r: NSRect, main_height: f64) -> Rect {
        Rect::new(
            r.origin.x,
            main_height - (r.origin.y + r.size.height),
            r.size.width,
            r.size.height,
        )
    }

    fn to_ns(r: Rect, main_height: f64) -> NSRect {
        NSRect::new(
            NSPoint::new(r.x, main_height - r.y - r.h),
            NSSize::new(r.w, r.h),
        )
    }

    /// The displays, the main one (with the menu bar) first.
    pub fn displays() -> Vec<Display> {
        let Some(mtm) = MainThreadMarker::new() else {
            return Vec::new();
        };
        let h = main_height(mtm);
        NSScreen::screens(mtm)
            .iter()
            .map(|s| Display {
                frame: from_ns(s.frame(), h),
                work: from_ns(s.visibleFrame(), h),
                scale: s.backingScaleFactor(),
            })
            .collect()
    }

    pub fn frame(w: &WebviewWindow) -> Option<Rect> {
        with(w, |window, mtm| from_ns(window.frame(), main_height(mtm)))
    }

    /// Moves and sizes the window in one step, so its top edge doesn't move first and its size
    /// after (Tauri's separate calls each take a frame).
    pub fn set_frame(w: &WebviewWindow, r: Rect) {
        with(w, |window, mtm| {
            window.setFrame_display(to_ns(r, main_height(mtm)), true)
        });
    }

    /// Whether the app is drawn dark right now.
    pub fn dark() -> bool {
        let Some(mtm) = MainThreadMarker::new() else {
            return false;
        };
        let app = NSApplication::sharedApplication(mtm);
        // SAFETY: AppKit's appearance name constants, always set.
        let (aqua, dark) = unsafe { (NSAppearanceNameAqua, NSAppearanceNameDarkAqua) };
        let names = NSArray::from_slice(&[aqua, dark]);
        app.effectiveAppearance()
            .bestMatchFromAppearancesWithNames(&names)
            .is_some_and(|name| &*name == dark)
    }

    /// Draws the app light or dark whatever the system says (`TWINCURSOR_APPEARANCE`), to look
    /// at the other appearance without changing System Settings.
    pub fn force_appearance(dark: bool) {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        // SAFETY: as above.
        let name = unsafe {
            if dark {
                NSAppearanceNameDarkAqua
            } else {
                NSAppearanceNameAqua
            }
        };
        let appearance = NSAppearance::appearanceNamed(name);
        NSApplication::sharedApplication(mtm).setAppearance(appearance.as_deref());
    }

    /// Whether Control is held right now (a Control-click on the menu bar icon opens its menu).
    pub fn control_down() -> bool {
        NSEvent::modifierFlags_class().contains(NSEventModifierFlags::Control)
    }
}

#[cfg(target_os = "macos")]
pub use mac::*;

#[cfg(not(target_os = "macos"))]
mod other {
    use tauri::{LogicalPosition, LogicalSize, WebviewWindow};

    use super::{Display, Kind, Rect};

    pub fn dress(_w: &WebviewWindow, _kind: Kind) {}

    pub fn show_without_focus(w: &WebviewWindow) {
        let _ = w.show();
    }

    pub fn displays() -> Vec<Display> {
        Vec::new()
    }

    pub fn frame(w: &WebviewWindow) -> Option<Rect> {
        let scale = w.scale_factor().ok()?;
        let p = w.outer_position().ok()?.to_logical::<f64>(scale);
        let s = w.outer_size().ok()?.to_logical::<f64>(scale);
        Some(Rect::new(p.x, p.y, s.width, s.height))
    }

    pub fn set_frame(w: &WebviewWindow, r: Rect) {
        let _ = w.set_size(LogicalSize::new(r.w, r.h));
        let _ = w.set_position(LogicalPosition::new(r.x, r.y));
    }

    pub fn dark() -> bool {
        false
    }

    pub fn force_appearance(_dark: bool) {}

    pub fn control_down() -> bool {
        false
    }
}

#[cfg(not(target_os = "macos"))]
pub use other::*;
