//! What the windows need from the platform that Tauri doesn't offer, as one set of functions
//! per platform.
//!
//! macOS (AppKit): no traffic lights, kept out of the Window menu and the window cycle, the
//! call bar on every Space and over full-screen apps, showing a window without making it key,
//! the displays' usable areas, and frames set in one step. Public AppKit API only (no private
//! API, no transparent windows).
//!
//! Windows (Win32, `win` below): undecorated windows kept off the taskbar (Tauri's builder),
//! shown without taking focus (`SW_SHOWNOACTIVATE`), moved and sized in one `SetWindowPos`,
//! displays from Tauri's monitors in points, the light or dark setting from the registry, and
//! rounded corners on Windows 11 (`DWMWA_WINDOW_CORNER_PREFERENCE`; Windows 10 keeps square
//! ones, and the page draws a hairline inside the edge). The panel stands on the taskbar.
//!
//! Everything here must run on the main thread; off it, it does nothing. Rects are in
//! [`crate::surfaces`]' space: points, origin at the main display's top left, y downward.
//! AppKit's has its origin at the bottom left, so they are flipped here; Windows' are physical
//! pixels, converted with each display's scale.

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
    use tauri::{Runtime, WebviewWindow, WebviewWindowBuilder};

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

    /// The window's look, at build time: a title bar that shows nothing (it gives the
    /// system's corners, hairline and shadow).
    pub fn style<'a, R: Runtime, M: tauri::Manager<R>>(
        builder: WebviewWindowBuilder<'a, R, M>,
        _kind: Kind,
    ) -> WebviewWindowBuilder<'a, R, M> {
        builder
            .title_bar_style(tauri::TitleBarStyle::Overlay)
            .hidden_title(true)
    }

    /// The panel hangs from the menu bar icon and keeps its top edge as it grows.
    pub const PANEL_KEEPS_TOP: bool = true;
    /// The app menu (Quit, and Edit for copy and paste) is the menu bar's.
    pub const APP_MENU: bool = true;

    /// The menu bar icon: a template image, 18 pt at 2×, tinted by macOS.
    pub fn tray_image() -> (Vec<u8>, u32, bool) {
        (crate::mark::template_rgba(36), 36, true)
    }

    pub fn panel_origin(
        icon: Option<Rect>,
        _height: f64,
        displays: &[Display],
    ) -> Option<(f64, f64)> {
        crate::surfaces::panel_origin(icon, displays)
    }

    /// The displays, the main one (with the menu bar) first.
    pub fn displays(_w: &WebviewWindow) -> Vec<Display> {
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

#[cfg(windows)]
mod win {
    use tauri::{Runtime, WebviewWindow, WebviewWindowBuilder};
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
    };
    use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SetWindowPos, ShowWindow, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
        SWP_NOZORDER, SWP_SHOWWINDOW, SW_SHOWNOACTIVATE,
    };

    use super::{Display, Kind, Rect};
    use crate::surfaces::{to_pixels, to_points};

    fn hwnd(w: &WebviewWindow) -> Option<HWND> {
        let h = w.hwnd().ok()?;
        let h = h.0 as HWND;
        (!h.is_null()).then_some(h)
    }

    /// No title bar or frame, off the taskbar (the tray icon is the app's place), with the
    /// system's shadow.
    pub fn style<'a, R: Runtime, M: tauri::Manager<R>>(
        builder: WebviewWindowBuilder<'a, R, M>,
        _kind: Kind,
    ) -> WebviewWindowBuilder<'a, R, M> {
        builder.decorations(false).skip_taskbar(true)
    }

    /// The panel stands on the taskbar, so as it grows its top edge moves, not its bottom.
    pub const PANEL_KEEPS_TOP: bool = false;
    /// No menu bar: a menu set on the app would show in every window.
    pub const APP_MENU: bool = false;

    /// A full-colour icon (Windows doesn't tint tray icons): white mark on an indigo tile.
    pub fn tray_image() -> (Vec<u8>, u32, bool) {
        (crate::mark::badge_rgba(32), 32, false)
    }

    pub fn panel_origin(
        icon: Option<Rect>,
        height: f64,
        displays: &[Display],
    ) -> Option<(f64, f64)> {
        crate::surfaces::panel_origin_at_taskbar(icon, height, displays)
    }

    /// Rounded corners where Windows offers them (11). Windows 10 ignores the request.
    pub fn dress(w: &WebviewWindow, _kind: Kind) {
        let Some(h) = hwnd(w) else { return };
        let round = DWMWCP_ROUND;
        // SAFETY: a live window handle and a 4-byte value of the documented type. A failure
        // (Windows 10) changes nothing.
        let result = unsafe {
            DwmSetWindowAttribute(
                h,
                DWMWA_WINDOW_CORNER_PREFERENCE as u32,
                &round as *const _ as *const core::ffi::c_void,
                std::mem::size_of_val(&round) as u32,
            )
        };
        crate::applog::line(format!(
            "window {}: rounded corners {}",
            w.label(),
            if result == 0 {
                "on"
            } else {
                "not offered (square)"
            }
        ));
    }

    /// On screen and on top, without activating it: the app the person is in keeps the
    /// keyboard.
    pub fn show_without_focus(w: &WebviewWindow) {
        let Some(h) = hwnd(w) else {
            let _ = w.show();
            return;
        };
        // SAFETY: a live window handle; no activation flags.
        unsafe {
            ShowWindow(h, SW_SHOWNOACTIVATE);
            SetWindowPos(
                h,
                HWND_TOPMOST,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
        }
    }

    /// The monitors in points (each divided by its own scale), the primary one first.
    pub fn displays(w: &WebviewWindow) -> Vec<Display> {
        let primary = w.primary_monitor().ok().flatten();
        let mut monitors = w.available_monitors().unwrap_or_default();
        if let Some(p) = &primary {
            if let Some(i) = monitors.iter().position(|m| m.position() == p.position()) {
                let m = monitors.remove(i);
                monitors.insert(0, m);
            }
        }
        monitors
            .iter()
            .map(|m| {
                let s = m.scale_factor();
                let (p, z) = (m.position(), m.size());
                let work = m.work_area();
                let pt = |x: i32, y: i32, w: u32, h: u32| {
                    Rect::new(
                        f64::from(x) / s,
                        f64::from(y) / s,
                        f64::from(w) / s,
                        f64::from(h) / s,
                    )
                };
                Display {
                    frame: pt(p.x, p.y, z.width, z.height),
                    work: pt(
                        work.position.x,
                        work.position.y,
                        work.size.width,
                        work.size.height,
                    ),
                    scale: s,
                }
            })
            .collect()
    }

    pub fn frame(w: &WebviewWindow) -> Option<Rect> {
        let p = w.outer_position().ok()?;
        let z = w.outer_size().ok()?;
        let pixels = Rect::new(
            f64::from(p.x),
            f64::from(p.y),
            f64::from(z.width),
            f64::from(z.height),
        );
        Some(to_points(pixels, &displays(w)))
    }

    /// Moves and sizes the window in pixels of the display it goes to. Sized a second time:
    /// moving onto a display with another scale makes Windows rescale the window first.
    pub fn set_frame(w: &WebviewWindow, r: Rect) {
        let (x, y, cx, cy) = to_pixels(r, &displays(w));
        let Some(h) = hwnd(w) else { return };
        // SAFETY: a live window handle; no activation, no change of z-order.
        unsafe {
            SetWindowPos(
                h,
                std::ptr::null_mut(),
                x,
                y,
                cx,
                cy,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
            SetWindowPos(
                h,
                std::ptr::null_mut(),
                0,
                0,
                cx,
                cy,
                SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOMOVE,
            );
        }
    }

    /// Whether apps are drawn dark: `AppsUseLightTheme` is 0.
    pub fn dark() -> bool {
        let key: Vec<u16> = "Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize\0"
            .encode_utf16()
            .collect();
        let name: Vec<u16> = "AppsUseLightTheme\0".encode_utf16().collect();
        let mut value: u32 = 1;
        let mut size = std::mem::size_of::<u32>() as u32;
        // SAFETY: NUL-terminated names and a 4-byte buffer whose size is passed.
        let status = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                name.as_ptr(),
                RRF_RT_REG_DWORD,
                std::ptr::null_mut(),
                &mut value as *mut u32 as *mut core::ffi::c_void,
                &mut size,
            )
        };
        status == 0 && value == 0
    }

    pub fn force_appearance(_dark: bool) {}

    pub fn control_down() -> bool {
        false
    }
}

#[cfg(windows)]
pub use win::*;

#[cfg(not(any(target_os = "macos", windows)))]
mod other {
    use tauri::{LogicalPosition, LogicalSize, Runtime, WebviewWindow, WebviewWindowBuilder};

    use super::{Display, Kind, Rect};

    pub fn style<'a, R: Runtime, M: tauri::Manager<R>>(
        builder: WebviewWindowBuilder<'a, R, M>,
        _kind: Kind,
    ) -> WebviewWindowBuilder<'a, R, M> {
        builder
    }

    pub const PANEL_KEEPS_TOP: bool = true;
    pub const APP_MENU: bool = false;

    pub fn tray_image() -> (Vec<u8>, u32, bool) {
        (crate::mark::badge_rgba(32), 32, false)
    }

    pub fn panel_origin(
        icon: Option<Rect>,
        _height: f64,
        displays: &[Display],
    ) -> Option<(f64, f64)> {
        crate::surfaces::panel_origin(icon, displays)
    }

    pub fn dress(_w: &WebviewWindow, _kind: Kind) {}

    pub fn show_without_focus(w: &WebviewWindow) {
        let _ = w.show();
    }

    pub fn displays(_w: &WebviewWindow) -> Vec<Display> {
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

#[cfg(not(any(target_os = "macos", windows)))]
pub use other::*;
