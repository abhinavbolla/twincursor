//! The app's windows and its menu bar icon: a menu bar app with two small windows
//! (docs/ui.md, "In the app").
//!
//! - **The panel** (`panel`, the page with `?view=panel`) hangs under the menu bar icon. A click
//!   on the icon opens or closes it; it hides when it loses focus, except while something it
//!   started needs it open (a sign-in in the browser, a macOS permission prompt), and on Escape.
//! - **The call bar** (`bar`, `?view=bar`) is on screen whenever a call is (any stage but idle),
//!   on top of everything, on every Space. It is shown without taking the keyboard from the app
//!   the person is in, and it is where knocks and control requests show, so arming counts from
//!   when it came on screen ([`Calls::set_bar_visible`]). It goes where it was last dragged.
//!
//! Both are as tall as their content (`fit_window`), keep their top edge where it is when they
//! grow, and get the system's rounded corners and shadow from a real title bar that shows
//! nothing (`native::dress`). Closing or hiding them never quits; Quit does.
//!
//! Where things go and when is decided in `surfaces.rs`; this file only applies it.

use std::collections::HashSet;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use tauri::image::Image;
use tauri::menu::{Menu, MenuBuilder, MenuEvent, MenuItemBuilder, SubmenuBuilder};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::window::Color;
use tauri::{
    AppHandle, Manager, Theme, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent,
};

use crate::calls::{Calls, HomeView};
use crate::native;
use crate::seams::{Permission, Permits};
use crate::settings::Settings;
use crate::surfaces::{
    bar_origin, bar_wanted, clamp_height, icon_in_points, Hold, Holds, Kind, PanelToggle, Rect,
    TrayClick, BAR_MIN_HEIGHT,
};

pub const PANEL: &str = "panel";
pub const BAR: &str = "bar";
const TRAY: &str = "tray";
const MENU_OPEN: &str = "open";
const MENU_QUIT: &str = "quit";
/// How long a panel asked to show waits for its page to say how tall it is.
const READY_TIMEOUT: Duration = Duration::from_millis(1500);
/// How long after the bar stops moving its place is saved.
const SAVE_AFTER: Duration = Duration::from_millis(500);
/// How long Quit waits for a call to end properly before quitting anyway.
pub const QUIT_TIMEOUT: Duration = Duration::from_secs(4);
/// The panel's height before its page has measured itself.
const PANEL_FIRST_HEIGHT: f64 = 240.0;

/// The surfaces' background (`--float` in styles.css), so no other colour shows at the corners
/// or before the page has drawn.
fn surface_colour(dark: bool) -> Color {
    if dark {
        Color(0x1c, 0x1c, 0x1e, 0xff)
    } else {
        Color(0xff, 0xff, 0xff, 0xff)
    }
}

fn kind_of(label: &str) -> Option<Kind> {
    match label {
        PANEL => Some(Kind::Panel),
        BAR => Some(Kind::Bar),
        _ => None,
    }
}

#[derive(Default)]
struct State {
    toggle: PanelToggle,
    holds: Holds,
    /// The panel's page has measured itself once.
    panel_ready: bool,
    /// Show the panel as soon as it is ready; `true` to give it focus.
    panel_pending: Option<bool>,
    panel_height: f64,
    bar_height: f64,
    /// Whether the bar should be on screen now (the stage isn't idle).
    bar_wanted: bool,
    bar_shown: bool,
    /// Where the bar goes the first time it shows in this run (from the settings file).
    bar_saved: Option<(f64, f64)>,
    bar_placed: bool,
    /// Where the app itself last put the bar, so only a person's drag is remembered.
    bar_set: Option<Rect>,
    /// Rings and knocks already seen, so only new ones call for attention.
    rings: HashSet<String>,
    knocks: HashSet<String>,
    quitting: bool,
    /// Bumped on every move of the bar, so only the last one in a while is saved.
    moves: u64,
}

/// The windows. Cheap to clone; every clone is the same windows.
#[derive(Clone)]
pub struct Windows {
    inner: Arc<Inner>,
}

struct Inner {
    app: AppHandle,
    /// Off in script mode (unless the script asks for windows): they exist, but never show.
    live: bool,
    calls: OnceLock<Calls>,
    state: Mutex<State>,
}

impl Windows {
    /// Makes both windows, hidden, so each is ready the moment it is needed. With `live`, also
    /// the menu bar icon and the app menu, and the bar's visibility starts counting for arming.
    pub fn create(app: &AppHandle, live: bool) -> tauri::Result<Windows> {
        let bar_saved = crate::settings_path(app)
            .ok()
            .and_then(|p| Settings::load(&p).ok())
            .and_then(|s| s.bar_position)
            .map(|(x, y)| (f64::from(x), f64::from(y)));
        let windows = Windows {
            inner: Arc::new(Inner {
                app: app.clone(),
                live,
                calls: OnceLock::new(),
                state: Mutex::new(State {
                    panel_height: PANEL_FIRST_HEIGHT,
                    bar_height: BAR_MIN_HEIGHT,
                    bar_saved,
                    ..State::default()
                }),
            }),
        };
        if let Ok(appearance) = std::env::var("TWINCURSOR_APPEARANCE") {
            match appearance.as_str() {
                "dark" => native::force_appearance(true),
                "light" => native::force_appearance(false),
                _ => {}
            }
        }
        let dark = native::dark();
        for kind in [Kind::Panel, Kind::Bar] {
            let window = windows.build(kind, dark).inspect_err(|e| {
                crate::applog::line(format!("window {kind:?}: couldn't be made: {e}"));
            })?;
            crate::applog::line(format!("window {kind:?}: made (hidden)"));
            native::dress(&window, kind);
            let me = windows.clone();
            let w = window.clone();
            window.on_window_event(move |event| me.on_window_event(&w, kind, event));
        }
        if live {
            windows.menus().inspect_err(|e| {
                crate::applog::line(format!("tray icon: couldn't be made: {e}"));
            })?;
            crate::applog::line("tray icon: made");
        }
        Ok(windows)
    }

    fn build(&self, kind: Kind, dark: bool) -> tauri::Result<WebviewWindow> {
        let (label, height) = match kind {
            Kind::Panel => (PANEL, PANEL_FIRST_HEIGHT),
            Kind::Bar => (BAR, BAR_MIN_HEIGHT),
        };
        let url = WebviewUrl::App(format!("index.html?view={label}").into());
        let builder = WebviewWindowBuilder::new(&self.inner.app, label, url);
        native::style(builder, kind)
            .title("TwinCursor")
            .inner_size(kind.width(), height)
            .resizable(false)
            .maximizable(false)
            .minimizable(false)
            .closable(false)
            .visible(false)
            .focused(false)
            .always_on_top(true)
            .visible_on_all_workspaces(kind == Kind::Bar)
            .accept_first_mouse(true)
            .shadow(true)
            .background_color(surface_colour(dark))
            // Keep a hidden window's page running, so it measures itself before it shows.
            .background_throttling(tauri::utils::config::BackgroundThrottlingPolicy::Disabled)
            .build()
    }

    /// The menu bar icon, and the app menu (its Quit ends a call properly first).
    fn menus(&self) -> tauri::Result<()> {
        let app = &self.inner.app;
        if native::APP_MENU {
            self.app_menu()?;
        }
        let tray_menu: Menu<tauri::Wry> = MenuBuilder::new(app)
            .text(MENU_OPEN, "Open TwinCursor")
            .separator()
            .text(MENU_QUIT, "Quit TwinCursor")
            .build()?;
        let me = self.clone();
        let on_tray = move |tray: &TrayIcon, event: TrayIconEvent| me.on_tray(tray, event);
        let me = self.clone();
        let (rgba, pixels, template) = native::tray_image();
        TrayIconBuilder::with_id(TRAY)
            .icon(Image::new_owned(rgba, pixels, pixels))
            .icon_as_template(template)
            .tooltip("TwinCursor")
            .menu(&tray_menu)
            .show_menu_on_left_click(false)
            .on_tray_icon_event(on_tray)
            .on_menu_event(move |_, event| me.on_menu(&event))
            .build(app)?;
        Ok(())
    }

    /// The app menu: Quit (which ends a call properly first) and Edit, for copy and paste.
    fn app_menu(&self) -> tauri::Result<()> {
        let app = &self.inner.app;
        let quit = MenuItemBuilder::with_id(MENU_QUIT, "Quit TwinCursor")
            .accelerator("CmdOrCtrl+Q")
            .build(app)?;
        let app_menu = SubmenuBuilder::new(app, "TwinCursor")
            .about(None)
            .separator()
            .hide()
            .hide_others()
            .show_all()
            .separator()
            .item(&quit)
            .build()?;
        // The panel's sign-in and join fields need the Edit menu's shortcuts for copy and paste.
        let edit = SubmenuBuilder::new(app, "Edit")
            .undo()
            .redo()
            .separator()
            .cut()
            .copy()
            .paste()
            .select_all()
            .build()?;
        let menu = MenuBuilder::new(app).items(&[&app_menu, &edit]).build()?;
        app.set_menu(menu)?;
        Ok(())
    }

    /// Connects the call flows (made after the windows, since they report to them).
    pub fn attach(&self, calls: Calls) {
        if self.inner.live {
            calls.set_bar_visible(false);
        }
        let _ = self.inner.calls.set(calls);
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.inner.state.lock().unwrap()
    }

    fn window(&self, kind: Kind) -> Option<WebviewWindow> {
        self.inner.app.get_webview_window(match kind {
            Kind::Panel => PANEL,
            Kind::Bar => BAR,
        })
    }

    /// Runs `f` on the main thread (at once when already on it), where AppKit wants windows used.
    fn on_main(&self, f: impl FnOnce(&Windows) + Send + 'static) {
        let me = self.clone();
        let _ = self.inner.app.run_on_main_thread(move || f(&me));
    }

    // Events.

    fn on_window_event(&self, window: &WebviewWindow, kind: Kind, event: &WindowEvent) {
        match event {
            WindowEvent::CloseRequested { api, .. } => {
                api.prevent_close();
                let _ = window.hide();
                if kind == Kind::Bar {
                    self.set_bar_seen(false);
                }
            }
            WindowEvent::Focused(false) if kind == Kind::Panel => {
                if !window.is_visible().unwrap_or(false) {
                    return;
                }
                let hide = {
                    let mut s = self.lock();
                    let held = s.holds.held();
                    s.toggle.blurred(Instant::now(), held)
                };
                if hide {
                    let _ = window.hide();
                }
            }
            WindowEvent::ThemeChanged(theme) => {
                let _ = window.set_background_color(Some(surface_colour(*theme == Theme::Dark)));
            }
            WindowEvent::Moved(_) if kind == Kind::Bar => self.bar_moved(window),
            _ => {}
        }
    }

    fn on_tray(&self, tray: &TrayIcon, event: TrayIconEvent) {
        let TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Down,
            ..
        } = event
        else {
            return;
        };
        // A Control-click is a right click on a Mac: the menu.
        if native::control_down() {
            let _ = tray.with_inner_tray_icon(|icon| icon.show_menu());
            return;
        }
        let Some(panel) = self.window(Kind::Panel) else {
            return;
        };
        let visible = panel.is_visible().unwrap_or(false);
        let click = self.lock().toggle.clicked(Instant::now(), visible);
        match click {
            TrayClick::Show => self.show_panel(true),
            TrayClick::Hide => {
                let _ = panel.hide();
            }
            TrayClick::Nothing => {}
        }
    }

    fn on_menu(&self, event: &MenuEvent) {
        match event.id().as_ref() {
            MENU_OPEN => self.show_panel(true),
            MENU_QUIT => self.quit(),
            _ => {}
        }
    }

    /// The app menu's events (Quit, with Command-Q).
    pub fn on_app_menu(&self, event: &MenuEvent) {
        self.on_menu(event);
    }

    // The panel.

    /// Shows the panel under the menu bar icon; with `focus`, it takes the keyboard (a click
    /// on the icon, the Dock icon, the end of a sign-in); without, it only comes on screen (a
    /// ring). Waits for its page to be ready, briefly.
    pub fn show_panel(&self, focus: bool) {
        if !self.inner.live {
            return;
        }
        let ready = {
            let mut s = self.lock();
            if !s.panel_ready {
                s.panel_pending = Some(s.panel_pending.unwrap_or(false) || focus);
            }
            s.panel_ready
        };
        if !ready {
            let me = self.clone();
            std::thread::spawn(move || {
                std::thread::sleep(READY_TIMEOUT);
                if let Some(focus) = me.lock().panel_pending.take() {
                    me.on_main(move |me| me.present_panel(focus));
                }
            });
            return;
        }
        self.on_main(move |me| me.present_panel(focus));
    }

    /// Where the panel goes for its height: by the tray icon.
    fn panel_at(&self, height: f64, displays: &[crate::surfaces::Display]) -> Option<(f64, f64)> {
        let icon = self
            .inner
            .app
            .tray_by_id(TRAY)
            .and_then(|tray| tray.rect().ok().flatten())
            .and_then(|r| {
                let p = r.position.to_physical::<f64>(1.0);
                let s = r.size.to_physical::<f64>(1.0);
                icon_in_points(Rect::new(p.x, p.y, s.width, s.height), displays)
            });
        native::panel_origin(icon, height, displays)
    }

    fn present_panel(&self, focus: bool) {
        let Some(panel) = self.window(Kind::Panel) else {
            return;
        };
        let displays = native::displays(&panel);
        let height = self.lock().panel_height;
        if let Some((x, y)) = self.panel_at(height, &displays) {
            native::set_frame(&panel, Rect::new(x, y, Kind::Panel.width(), height));
            crate::applog::line(format!(
                "window Panel: shown at ({x:.0}, {y:.0}) pt, {height:.0} pt tall{}",
                if focus { ", focused" } else { "" }
            ));
        }
        if focus {
            let _ = panel.show();
            let _ = panel.set_focus();
        } else if !panel.is_visible().unwrap_or(false) {
            native::show_without_focus(&panel);
        }
    }

    pub fn hide_panel(&self) {
        if let Some(panel) = self.window(Kind::Panel) {
            let _ = panel.hide();
        }
    }

    /// Keeps the panel open while it doesn't have focus, for `hold`.
    pub fn hold(&self, hold: Hold) {
        self.lock().holds.add(hold);
    }

    /// Lets go of `hold`. After a sign-in (done or not), the panel comes back with focus.
    pub fn release(&self, hold: Hold) {
        let last = self.lock().holds.release(hold);
        if last && hold == Hold::SignIn {
            self.show_panel(true);
        }
    }

    // The bar.

    fn set_bar_seen(&self, seen: bool) {
        self.lock().bar_shown = seen;
        if let Some(calls) = self.inner.calls.get() {
            calls.set_bar_visible(seen);
        }
    }

    /// Shows or hides the bar for where the call is. On the main thread.
    fn apply_bar(&self) {
        let Some(bar) = self.window(Kind::Bar) else {
            return;
        };
        let (wanted, shown) = {
            let s = self.lock();
            (s.bar_wanted, s.bar_shown)
        };
        if wanted && !shown {
            self.place_bar(&bar);
            native::show_without_focus(&bar);
            self.set_bar_seen(true);
        } else if !wanted && shown {
            let _ = bar.hide();
            self.set_bar_seen(false);
        }
    }

    /// Puts the bar where it was left (or its first place), onto a display that is there.
    fn place_bar(&self, bar: &WebviewWindow) {
        let displays = native::displays(bar);
        let (from, height) = {
            let mut s = self.lock();
            let from = if s.bar_placed {
                native::frame(bar).map(|r| (r.x, r.y))
            } else {
                s.bar_placed = true;
                s.bar_saved
            };
            (from, s.bar_height)
        };
        if let Some((x, y)) = bar_origin(from, height, &displays) {
            self.set_bar_frame(bar, Rect::new(x, y, Kind::Bar.width(), height));
        }
    }

    fn set_bar_frame(&self, bar: &WebviewWindow, frame: Rect) {
        self.lock().bar_set = Some(frame);
        native::set_frame(bar, frame);
    }

    fn bar_moved(&self, bar: &WebviewWindow) {
        let Some(frame) = native::frame(bar) else {
            return;
        };
        let moves = {
            let mut s = self.lock();
            let ours = s
                .bar_set
                .is_some_and(|set| (set.x - frame.x).abs() < 0.5 && (set.y - frame.y).abs() < 0.5);
            if !s.bar_placed || ours {
                return;
            }
            s.moves += 1;
            s.moves
        };
        let me = self.clone();
        std::thread::spawn(move || {
            std::thread::sleep(SAVE_AFTER);
            if me.lock().moves != moves {
                return;
            }
            let place = (frame.x.round() as i32, frame.y.round() as i32);
            me.lock().bar_saved = Some((frame.x, frame.y));
            if let Ok(path) = crate::settings_path(&me.inner.app) {
                if let Ok(mut settings) = Settings::load(&path) {
                    if settings.bar_position != Some(place) {
                        settings.bar_position = Some(place);
                        let _ = settings.save(&path);
                    }
                }
            }
        });
    }

    /// A control request: the bar must be on screen (never focused), and the Dock icon asks
    /// for attention once.
    pub fn control_asked(&self) {
        if !self.inner.live {
            return;
        }
        self.on_main(|me| {
            me.apply_bar();
            if let Some(bar) = me.window(Kind::Bar) {
                let _ = bar.request_user_attention(Some(tauri::UserAttentionType::Informational));
            }
        });
    }

    /// Every change to the home view: the bar follows the stage, a new knock makes sure the
    /// bar is on screen, and a new ring while not in a call shows the panel without focus.
    pub fn on_home(&self, home: &HomeView) {
        if !self.inner.live {
            return;
        }
        let wanted = bar_wanted(&home.stage);
        let ring_ids: HashSet<String> = home.rings.iter().map(|r| r.call_id.clone()).collect();
        let knock_ids: HashSet<String> = home.knocks.iter().map(|k| k.user_id.clone()).collect();
        let (bar_changed, new_ring, new_knock) = {
            let mut s = self.lock();
            let bar_changed = s.bar_wanted != wanted;
            s.bar_wanted = wanted;
            let new_ring = ring_ids.iter().any(|id| !s.rings.contains(id));
            let new_knock = knock_ids.iter().any(|id| !s.knocks.contains(id));
            s.rings = ring_ids;
            s.knocks = knock_ids;
            (bar_changed, new_ring, new_knock)
        };
        if bar_changed || new_knock {
            self.on_main(|me| me.apply_bar());
        }
        if new_ring && !wanted {
            self.show_panel(false);
        }
    }

    // Both.

    /// The page in `window` is `height` tall: the window follows, keeping its top edge (the
    /// panel hangs from the icon, the bar grows downward).
    pub fn fit(&self, window: &WebviewWindow, height: f64) {
        let Some(kind) = kind_of(window.label()) else {
            return;
        };
        let window = window.clone();
        self.on_main(move |me| {
            let displays = native::displays(&window);
            let Some(frame) = native::frame(&window) else {
                return;
            };
            let usable = displays
                .iter()
                .find(|d| d.frame.x <= frame.x && frame.x < d.frame.right())
                .or(displays.first())
                .map(|d| d.work.h - 2.0 * crate::surfaces::BAR_TOP_GAP);
            let h = clamp_height(kind, height, usable);
            let mut at = (frame.x, frame.y);
            let pending = {
                let mut s = me.lock();
                match kind {
                    Kind::Panel => {
                        if !s.panel_ready {
                            crate::applog::line(format!("window Panel: page drawn ({h} pt tall)"));
                        }
                        s.panel_height = h;
                        s.panel_ready = true;
                        s.panel_pending.take()
                    }
                    Kind::Bar => {
                        s.bar_height = h;
                        None
                    }
                }
            };
            if kind == Kind::Bar {
                if let Some(origin) = bar_origin(Some(at), h, &displays) {
                    at = origin;
                }
            }
            // A panel standing on the taskbar (Windows) keeps its bottom edge by the icon.
            if kind == Kind::Panel && !native::PANEL_KEEPS_TOP {
                if let Some(origin) = me.panel_at(h, &displays) {
                    at = origin;
                }
            }
            if (frame.h - h).abs() >= 0.5 || (frame.x, frame.y) != at {
                let rect = Rect::new(at.0, at.1, kind.width(), h);
                match kind {
                    Kind::Bar => me.set_bar_frame(&window, rect),
                    Kind::Panel => native::set_frame(&window, rect),
                }
            }
            if let Some(focus) = pending {
                me.present_panel(focus);
            }
        });
    }

    // Quitting.

    pub fn quitting(&self) -> bool {
        self.lock().quitting
    }

    /// Quit TwinCursor: leaves or ends any call properly (briefly), then exits. Once.
    pub fn quit(&self) {
        {
            let mut s = self.lock();
            if s.quitting {
                return;
            }
            s.quitting = true;
        }
        let app = self.inner.app.clone();
        let calls = self.inner.calls.get().cloned();
        tauri::async_runtime::spawn(async move {
            if let Some(calls) = calls.filter(Calls::busy) {
                let _ = tokio::time::timeout(QUIT_TIMEOUT, calls.end()).await;
            }
            app.exit(0);
        });
    }
}

/// The engine's permissions, with the panel held open while macOS shows its prompt: the prompt
/// takes focus, and the panel that asked should still be there after it.
pub struct HeldPermits<P> {
    pub inner: P,
    pub windows: Windows,
}

impl<P: Permits> Permits for HeldPermits<P> {
    fn granted(&self, which: Permission) -> Result<bool, String> {
        self.inner.granted(which)
    }

    fn request(&self, which: Permission) -> Result<bool, String> {
        self.windows.hold(Hold::Permission);
        let granted = self.inner.request(which);
        self.windows.release(Hold::Permission);
        granted
    }
}
