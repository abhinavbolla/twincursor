//! Where the two windows go and when they show, as plain functions with no window in them, so
//! the rules can be tested: the panel hangs under the menu bar icon, the call bar remembers
//! where it was put, both are as tall as their content, a click on the icon while the panel is
//! open closes it, and the app only quits when asked to (docs/ui.md, "In the app").
//!
//! Geometry is in points, in the global display space with the origin at the top left of the
//! main display and y growing downward (what Core Graphics uses). `windows.rs` converts.

use std::time::{Duration, Instant};

use crate::calls::StageView;

/// The panel's width, and the range its height is fitted into (its list scrolls inside).
pub const PANEL_WIDTH: f64 = 340.0;
pub const PANEL_MIN_HEIGHT: f64 = 120.0;
pub const PANEL_MAX_HEIGHT: f64 = 560.0;
/// The call bar's width, and the range of its height: one 48 pt row, growing downward.
pub const BAR_WIDTH: f64 = 440.0;
pub const BAR_MIN_HEIGHT: f64 = 48.0;
pub const BAR_MAX_HEIGHT: f64 = 640.0;
/// The gap between the menu bar and the panel hanging from it.
pub const PANEL_GAP: f64 = 6.0;
/// The panel keeps this far from a display's side edges.
pub const EDGE: f64 = 8.0;
/// The call bar's first place: top centre of the main display, this far under the menu bar.
pub const BAR_TOP_GAP: f64 = 12.0;
/// A tray click this soon after the panel hid because it lost focus is the same click: the
/// person clicked the icon to close the panel, so it stays closed.
pub const BLUR_DEBOUNCE: Duration = Duration::from_millis(300);

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub const fn new(x: f64, y: f64, w: f64, h: f64) -> Rect {
        Rect { x, y, w, h }
    }

    pub fn right(&self) -> f64 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f64 {
        self.y + self.h
    }

    fn centre(&self) -> (f64, f64) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }

    fn contains(&self, (x, y): (f64, f64)) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }

    fn overlap(&self, other: &Rect) -> f64 {
        let w = self.right().min(other.right()) - self.x.max(other.x);
        let h = self.bottom().min(other.bottom()) - self.y.max(other.y);
        if w > 0.0 && h > 0.0 {
            w * h
        } else {
            0.0
        }
    }
}

/// One display: all of it, the part windows may use (below the menu bar, beside the Dock), and
/// how many pixels a point is. The main display, the one with the menu bar, comes first.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Display {
    pub frame: Rect,
    pub work: Rect,
    pub scale: f64,
}

/// Which window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Panel,
    Bar,
}

impl Kind {
    pub fn width(self) -> f64 {
        match self {
            Kind::Panel => PANEL_WIDTH,
            Kind::Bar => BAR_WIDTH,
        }
    }
}

/// The height a window gets for content this tall: within its range, and never taller than the
/// display's usable height when that is known.
pub fn clamp_height(kind: Kind, content: f64, usable: Option<f64>) -> f64 {
    let (min, max) = match kind {
        Kind::Panel => (PANEL_MIN_HEIGHT, PANEL_MAX_HEIGHT),
        Kind::Bar => (BAR_MIN_HEIGHT, BAR_MAX_HEIGHT),
    };
    let max = usable.map_or(max, |u| max.min(u.max(min)));
    if content.is_finite() {
        content.round().clamp(min, max)
    } else {
        min
    }
}

/// The menu bar icon's rect in points. The tray reports it in pixels of the display it is on,
/// so the display is the one whose frame holds the rect once divided by that display's scale.
pub fn icon_in_points(pixels: Rect, displays: &[Display]) -> Option<Rect> {
    displays.iter().find_map(|d| {
        let s = if d.scale > 0.0 { d.scale } else { 1.0 };
        let r = Rect::new(pixels.x / s, pixels.y / s, pixels.w / s, pixels.h / s);
        d.frame.contains(r.centre()).then_some(r)
    })
}

fn clamp_x(x: f64, width: f64, work: &Rect) -> f64 {
    let low = work.x + EDGE;
    let high = work.right() - width - EDGE;
    if high < low {
        work.x + (work.w - width) / 2.0
    } else {
        x.clamp(low, high)
    }
}

/// Where the panel's top-left corner goes: centred under the menu bar icon, on the icon's
/// display, kept off that display's side edges. Without an icon (its place isn't known yet):
/// top right of the main display. `None` without displays.
pub fn panel_origin(icon: Option<Rect>, displays: &[Display]) -> Option<(f64, f64)> {
    let main = displays.first()?;
    match icon {
        Some(icon) => {
            let d = displays
                .iter()
                .find(|d| d.frame.contains(icon.centre()))
                .unwrap_or(main);
            let (cx, _) = icon.centre();
            let x = clamp_x(cx - PANEL_WIDTH / 2.0, PANEL_WIDTH, &d.work);
            let y = icon.bottom().max(d.work.y) + PANEL_GAP;
            Some((x, y))
        }
        None => Some((
            main.work.right() - PANEL_WIDTH - EDGE,
            main.work.y + PANEL_GAP,
        )),
    }
}

/// The call bar's first place: top centre of the main display, under the menu bar.
pub fn bar_default(displays: &[Display]) -> Option<(f64, f64)> {
    let main = displays.first()?;
    Some((
        main.work.x + (main.work.w - BAR_WIDTH) / 2.0,
        main.work.y + BAR_TOP_GAP,
    ))
}

/// Where the call bar goes for a remembered (or current) top-left corner and its height: onto
/// the display it overlaps most, moved just enough to be wholly inside that display's usable
/// area. On no display any more (a display was unplugged): its first place.
pub fn bar_origin(
    saved: Option<(f64, f64)>,
    height: f64,
    displays: &[Display],
) -> Option<(f64, f64)> {
    let Some((x, y)) = saved.filter(|(x, y)| x.is_finite() && y.is_finite()) else {
        return bar_default(displays);
    };
    let bar = Rect::new(x, y, BAR_WIDTH, height);
    let best = displays
        .iter()
        .map(|d| (d, bar.overlap(&d.work)))
        .filter(|(_, area)| *area > 0.0)
        .max_by(|a, b| a.1.total_cmp(&b.1));
    let Some((d, _)) = best else {
        return bar_default(displays);
    };
    let w = &d.work;
    let x = if w.w < BAR_WIDTH {
        w.x
    } else {
        x.clamp(w.x, w.right() - BAR_WIDTH)
    };
    let y = if w.h < height {
        w.y
    } else {
        y.clamp(w.y, w.bottom() - height)
    };
    Some((x, y))
}

/// Whether the call bar is on screen for this stage: during anything to do with a call.
pub fn bar_wanted(stage: &StageView) -> bool {
    !matches!(stage, StageView::Idle)
}

/// What a click on the menu bar icon does to the panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayClick {
    Show,
    Hide,
    /// The click that just took focus from the panel (and so hid it): leave it hidden.
    Nothing,
}

/// The panel's show-and-hide with the icon, against its hiding when it loses focus. Clicking
/// the icon while the panel is open can reach the app after the panel lost focus to that very
/// click, so a click right after a blur-hide closes rather than reopens.
#[derive(Debug, Default)]
pub struct PanelToggle {
    blur_hidden_at: Option<Instant>,
}

impl PanelToggle {
    /// The panel lost focus. Returns whether to hide it: not while something it started needs
    /// it open (`held`).
    pub fn blurred(&mut self, now: Instant, held: bool) -> bool {
        if held {
            return false;
        }
        self.blur_hidden_at = Some(now);
        true
    }

    pub fn clicked(&mut self, now: Instant, visible: bool) -> TrayClick {
        let recent = self
            .blur_hidden_at
            .take()
            .is_some_and(|at| now.saturating_duration_since(at) < BLUR_DEBOUNCE);
        if visible {
            TrayClick::Hide
        } else if recent {
            TrayClick::Nothing
        } else {
            TrayClick::Show
        }
    }
}

/// What the panel is kept open for while it doesn't have focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hold {
    /// A sign-in in the browser.
    SignIn,
    /// A macOS permission prompt the panel's action brought up.
    Permission,
}

/// The reasons the panel is held open right now (each can overlap itself: two prompts).
#[derive(Debug, Default)]
pub struct Holds {
    sign_in: u32,
    permission: u32,
}

impl Holds {
    pub fn add(&mut self, hold: Hold) {
        *self.count(hold) += 1;
    }

    /// Returns whether that was the last of its kind.
    pub fn release(&mut self, hold: Hold) -> bool {
        let n = self.count(hold);
        *n = n.saturating_sub(1);
        *n == 0
    }

    pub fn held(&self) -> bool {
        self.sign_in > 0 || self.permission > 0
    }

    fn count(&mut self, hold: Hold) -> &mut u32 {
        match hold {
            Hold::SignIn => &mut self.sign_in,
            Hold::Permission => &mut self.permission,
        }
    }
}

/// Whether an exit request goes ahead. Closing windows never quits a menu bar app: a request
/// with no code (the last window went) is refused, unless the app is quitting on purpose. One
/// with a code is the app's own `exit` (Quit, or the end of a script) and goes ahead.
pub fn allow_exit(code: Option<i32>, quitting: bool) -> bool {
    code.is_some() || quitting
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounts::Me;

    /// A 1512 × 982 laptop display (menu bar 37 pt with the notch) and, to its right, a 2560 ×
    /// 1440 external one at scale 1 (menu bar 25 pt), tops aligned.
    fn two() -> Vec<Display> {
        vec![
            Display {
                frame: Rect::new(0.0, 0.0, 1512.0, 982.0),
                work: Rect::new(0.0, 37.0, 1512.0, 945.0),
                scale: 2.0,
            },
            Display {
                frame: Rect::new(1512.0, 0.0, 2560.0, 1440.0),
                work: Rect::new(1512.0, 25.0, 2560.0, 1415.0),
                scale: 1.0,
            },
        ]
    }

    #[test]
    fn heights_are_kept_in_range() {
        assert_eq!(clamp_height(Kind::Panel, 300.4, None), 300.0);
        assert_eq!(clamp_height(Kind::Panel, 40.0, None), PANEL_MIN_HEIGHT);
        assert_eq!(clamp_height(Kind::Panel, 900.0, None), PANEL_MAX_HEIGHT);
        assert_eq!(clamp_height(Kind::Bar, 10.0, None), BAR_MIN_HEIGHT);
        assert_eq!(clamp_height(Kind::Bar, 212.0, None), 212.0);
        assert_eq!(clamp_height(Kind::Bar, 2000.0, None), BAR_MAX_HEIGHT);
        // A short display caps it, but never under the minimum.
        assert_eq!(clamp_height(Kind::Panel, 500.0, Some(400.0)), 400.0);
        assert_eq!(clamp_height(Kind::Bar, 500.0, Some(20.0)), BAR_MIN_HEIGHT);
        assert_eq!(clamp_height(Kind::Bar, f64::NAN, None), BAR_MIN_HEIGHT);
    }

    #[test]
    fn the_icon_rect_comes_back_to_points_on_its_own_display() {
        let d = two();
        // On the Retina display: pixels are twice the points.
        let laptop = icon_in_points(Rect::new(2400.0, 0.0, 64.0, 74.0), &d).unwrap();
        assert_eq!(laptop, Rect::new(1200.0, 0.0, 32.0, 37.0));
        // On the external display at scale 1: as given.
        let external = icon_in_points(Rect::new(3800.0, 0.0, 30.0, 25.0), &d).unwrap();
        assert_eq!(external, Rect::new(3800.0, 0.0, 30.0, 25.0));
        assert_eq!(
            icon_in_points(Rect::new(-500.0, -500.0, 10.0, 10.0), &d),
            None
        );
    }

    #[test]
    fn the_panel_hangs_centred_under_the_icon() {
        let d = two();
        let icon = Rect::new(1200.0, 0.0, 32.0, 37.0);
        assert_eq!(
            panel_origin(Some(icon), &d),
            Some((1216.0 - PANEL_WIDTH / 2.0, 37.0 + PANEL_GAP))
        );
    }

    #[test]
    fn the_panel_stays_on_screen_at_the_edges() {
        let d = two();
        // An icon near the laptop's right edge: the panel moves left to fit.
        let right = Rect::new(1480.0, 0.0, 28.0, 37.0);
        assert_eq!(
            panel_origin(Some(right), &d),
            Some((1512.0 - PANEL_WIDTH - EDGE, 37.0 + PANEL_GAP))
        );
        // Near the left edge: it moves right.
        let left = Rect::new(4.0, 0.0, 28.0, 37.0);
        assert_eq!(panel_origin(Some(left), &d), Some((EDGE, 37.0 + PANEL_GAP)));
    }

    #[test]
    fn the_panel_goes_to_the_display_the_icon_is_on() {
        let d = two();
        // The menu bar on the external display (its own, shorter menu bar).
        let icon = Rect::new(3000.0, 0.0, 30.0, 25.0);
        let (x, y) = panel_origin(Some(icon), &d).unwrap();
        assert_eq!(y, 25.0 + PANEL_GAP);
        assert_eq!(x, 3015.0 - PANEL_WIDTH / 2.0);
        // At that display's right edge, it is kept inside that display, not the laptop's.
        let edge = Rect::new(4050.0, 0.0, 22.0, 25.0);
        let (x, _) = panel_origin(Some(edge), &d).unwrap();
        assert_eq!(x, 1512.0 + 2560.0 - PANEL_WIDTH - EDGE);
    }

    #[test]
    fn a_display_left_of_or_above_the_main_one() {
        // An external display to the left of the main one and higher: negative coordinates.
        let d = vec![
            Display {
                frame: Rect::new(0.0, 0.0, 1440.0, 900.0),
                work: Rect::new(0.0, 24.0, 1440.0, 876.0),
                scale: 2.0,
            },
            Display {
                frame: Rect::new(-1920.0, -300.0, 1920.0, 1080.0),
                work: Rect::new(-1920.0, -276.0, 1920.0, 1056.0),
                scale: 1.0,
            },
        ];
        let icon = Rect::new(-1910.0, -300.0, 24.0, 24.0);
        assert_eq!(
            panel_origin(Some(icon), &d),
            Some((-1920.0 + EDGE, -276.0 + PANEL_GAP))
        );
    }

    #[test]
    fn without_an_icon_the_panel_goes_top_right_of_the_main_display() {
        let d = two();
        assert_eq!(
            panel_origin(None, &d),
            Some((1512.0 - PANEL_WIDTH - EDGE, 37.0 + PANEL_GAP))
        );
        assert_eq!(panel_origin(None, &[]), None);
    }

    #[test]
    fn the_bar_starts_top_centre_under_the_menu_bar() {
        let d = two();
        assert_eq!(
            bar_origin(None, 48.0, &d),
            Some(((1512.0 - BAR_WIDTH) / 2.0, 37.0 + BAR_TOP_GAP))
        );
        assert_eq!(bar_origin(None, 48.0, &[]), None);
    }

    #[test]
    fn the_bar_comes_back_where_it_was_left() {
        let d = two();
        assert_eq!(
            bar_origin(Some((300.0, 700.0)), 48.0, &d),
            Some((300.0, 700.0))
        );
        // On the external display.
        assert_eq!(
            bar_origin(Some((2000.0, 1200.0)), 48.0, &d),
            Some((2000.0, 1200.0))
        );
    }

    #[test]
    fn the_bar_is_clamped_onto_a_visible_display() {
        let d = two();
        // Half off the bottom of the laptop: pulled up to fit.
        assert_eq!(
            bar_origin(Some((300.0, 960.0)), 48.0, &d),
            Some((300.0, 982.0 - 48.0))
        );
        // Under the menu bar: pushed down out of it.
        assert_eq!(
            bar_origin(Some((300.0, 10.0)), 48.0, &d),
            Some((300.0, 37.0))
        );
        // Across both displays, mostly on the external one: it moves onto that one.
        assert_eq!(
            bar_origin(Some((1400.0, 500.0)), 48.0, &d),
            Some((1512.0, 500.0))
        );
        // Taller now (a knock): kept inside.
        assert_eq!(
            bar_origin(Some((300.0, 900.0)), 200.0, &d),
            Some((300.0, 982.0 - 200.0))
        );
    }

    #[test]
    fn a_bar_left_on_an_unplugged_display_starts_over() {
        let laptop = vec![two()[0]];
        assert_eq!(
            bar_origin(Some((2000.0, 1200.0)), 48.0, &laptop),
            Some(((1512.0 - BAR_WIDTH) / 2.0, 37.0 + BAR_TOP_GAP))
        );
        assert_eq!(
            bar_origin(Some((f64::NAN, 3.0)), 48.0, &laptop),
            bar_default(&laptop)
        );
    }

    #[test]
    fn the_bar_shows_for_everything_but_idle() {
        let host = Me {
            id: "a".into(),
            name: "Alex".into(),
            handle: None,
            avatar_url: None,
        };
        assert!(!bar_wanted(&StageView::Idle));
        for stage in [
            StageView::Starting,
            StageView::Hosting {
                call_id: "c".into(),
                link: "twincursor://call/x".into(),
            },
            StageView::Knocking { host: host.clone() },
            StageView::Joined { host },
            StageView::Ending,
        ] {
            assert!(bar_wanted(&stage), "{stage:?}");
        }
    }

    #[test]
    fn a_click_on_the_icon_toggles_the_panel() {
        let mut t = PanelToggle::default();
        let now = Instant::now();
        assert_eq!(t.clicked(now, false), TrayClick::Show);
        assert_eq!(t.clicked(now, true), TrayClick::Hide);
    }

    #[test]
    fn the_click_that_blurred_the_panel_closes_it_for_good() {
        let mut t = PanelToggle::default();
        let t0 = Instant::now();
        // Open panel; the click on the icon takes its focus first, which hides it.
        assert!(t.blurred(t0, false));
        assert_eq!(
            t.clicked(t0 + Duration::from_millis(40), false),
            TrayClick::Nothing
        );
        // The next click opens it again.
        assert_eq!(
            t.clicked(t0 + Duration::from_millis(400), false),
            TrayClick::Show
        );
    }

    #[test]
    fn a_click_long_after_a_blur_opens_the_panel() {
        let mut t = PanelToggle::default();
        let t0 = Instant::now();
        assert!(t.blurred(t0, false));
        assert_eq!(t.clicked(t0 + BLUR_DEBOUNCE, false), TrayClick::Show);
    }

    #[test]
    fn a_held_panel_doesnt_hide_on_blur() {
        let mut t = PanelToggle::default();
        let t0 = Instant::now();
        assert!(!t.blurred(t0, true));
        // Still open, so a click closes it.
        assert_eq!(
            t.clicked(t0 + Duration::from_millis(10), true),
            TrayClick::Hide
        );
    }

    #[test]
    fn holds_count_each_reason() {
        let mut h = Holds::default();
        assert!(!h.held());
        h.add(Hold::SignIn);
        h.add(Hold::Permission);
        h.add(Hold::Permission);
        assert!(h.held());
        assert!(h.release(Hold::SignIn));
        assert!(!h.release(Hold::Permission));
        assert!(h.held());
        assert!(h.release(Hold::Permission));
        assert!(!h.held());
        // Releasing more than was held stays at none.
        assert!(h.release(Hold::SignIn));
        assert!(!h.held());
    }

    #[test]
    fn only_asking_to_quit_quits() {
        // The last window going away doesn't quit a menu bar app.
        assert!(!allow_exit(None, false));
        // Quit from the panel, the menu bar icon's menu or Command-Q.
        assert!(allow_exit(None, true));
        // The app's own exit: a script that is done, or Quit.
        assert!(allow_exit(Some(0), false));
        assert!(allow_exit(Some(2), false));
    }
}
