//! Sizing the stream to what viewers can show.
//!
//! Each viewer reports its viewport: the largest area, in physical pixels, the picture can
//! take on its screen (`protocol::Message::Viewport`). The sharer sends the source fitted
//! inside the viewport that gives the largest picture, so the picture is drawn 1:1 wherever
//! the source has the pixels, and nobody's pixels go to waste. The sharer's own setting
//! (`--resolution`) and the auto-throttle stay upper limits on top of that.
//!
//! The viewer reports something that doesn't depend on the picture it receives (the monitor's
//! usable area, or the window size its guest chose), so a size sent never makes it report a
//! different viewport: the loop settles after one step.

use std::collections::HashMap;

use crate::capture::{capped, fit_within};

/// A viewer that joined this recently and hasn't reported a viewport yet is left out, so each
/// join doesn't send the largest size for the moment it takes to report.
pub const JOIN_GRACE_S: f64 = 2.0;
/// The viewport the size follows changes at most this often, whatever viewers send.
pub const MIN_GAP_S: f64 = 1.0;

/// Every viewer's viewport, and the one the size follows.
#[derive(Debug, Default)]
pub struct Viewports {
    viewers: HashMap<String, Viewer>,
    /// The viewport the size follows now; `None` means none (the sharer's own size).
    chosen: Option<(u32, u32)>,
    changed_at: Option<f64>,
}

#[derive(Debug, Clone, Copy)]
struct Viewer {
    joined_at: f64,
    viewport: Option<(u32, u32)>,
}

impl Viewports {
    /// Someone joined at `t_s` (seconds on any steady clock).
    pub fn joined(&mut self, identity: &str, t_s: f64) {
        self.viewers.entry(identity.to_owned()).or_insert(Viewer {
            joined_at: t_s,
            viewport: None,
        });
    }

    pub fn left(&mut self, identity: &str) {
        self.viewers.remove(identity);
    }

    /// `identity` reported `viewport` (already validated by `protocol::decode`).
    pub fn reported(&mut self, identity: &str, viewport: (u32, u32), t_s: f64) {
        self.joined(identity, t_s);
        if let Some(viewer) = self.viewers.get_mut(identity) {
            viewer.viewport = Some(viewport);
        }
    }

    /// The viewport the size should follow for a source of `source` pixels: the one giving the
    /// largest picture. `None` while nobody has reported one, or while anyone in the call
    /// (past [`JOIN_GRACE_S`]) hasn't: a viewer that doesn't report, such as one from before
    /// viewports existed, gets the sharer's own size as before.
    pub fn wanted(&self, source: (u32, u32), t_s: f64) -> Option<(u32, u32)> {
        let mut best: Option<((u32, u32), (u32, u32))> = None;
        for viewer in self.viewers.values() {
            match viewer.viewport {
                Some(viewport) => {
                    let fit = fit_within(source, viewport);
                    // Ties go to the larger viewport, so the choice doesn't depend on order.
                    if best.is_none_or(|(f, v)| (fit.1, fit.0, viewport) > (f.1, f.0, v)) {
                        best = Some((fit, viewport));
                    }
                }
                None if t_s - viewer.joined_at < JOIN_GRACE_S => {}
                None => return None,
            }
        }
        best.map(|(_, viewport)| viewport)
    }

    /// Moves to [`Viewports::wanted`] when it gives a different size, at most once every
    /// [`MIN_GAP_S`]. Returns whether it moved. Call it on every report, join and leave, and
    /// now and then, so a change held back by the rate limit is made when it's due.
    pub fn update(&mut self, source: (u32, u32), t_s: f64) -> bool {
        let wanted = self.wanted(source, t_s);
        let fit = |v: Option<(u32, u32)>| v.map(|v| fit_within(source, v));
        if fit(wanted) == fit(self.chosen) {
            // Same picture: follow the new viewport quietly, so the next comparison is fresh.
            self.chosen = wanted;
            return false;
        }
        if self.changed_at.is_some_and(|at| t_s - at < MIN_GAP_S) {
            return false;
        }
        self.chosen = wanted;
        self.changed_at = Some(t_s);
        true
    }

    /// The viewport the size follows now.
    pub fn chosen(&self) -> Option<(u32, u32)> {
        self.chosen
    }

    /// The picture size viewers allow for a source of `source` pixels, if they limit it.
    pub fn limit(&self, source: (u32, u32)) -> Option<(u32, u32)> {
        self.chosen.map(|v| fit_within(source, v))
    }
}

/// What decided the size sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SizedBy {
    /// The sharer's `--resolution` (or the source's own size, when smaller).
    Setting,
    /// The auto-throttle stepped down from it.
    AutoThrottle,
    /// The largest viewer's viewport, of this many pixels.
    Viewer((u32, u32)),
}

impl std::fmt::Display for SizedBy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SizedBy::Setting => f.write_str("the sharer's setting"),
            SizedBy::AutoThrottle => f.write_str("the auto-throttle"),
            SizedBy::Viewer((w, h)) => write!(f, "the viewer ({w}x{h} viewport)"),
        }
    }
}

/// The size to send: `level` (the setting, or the auto-throttle's current size, which is
/// `top` when it is the setting) limited by the viewers' `limit`, and what decided it.
pub fn send_size(
    level: (u32, u32),
    top: bool,
    limit: Option<(u32, u32)>,
    viewport: Option<(u32, u32)>,
) -> ((u32, u32), SizedBy) {
    let size = capped(level, limit);
    let by = match viewport {
        Some(v) if size != level => SizedBy::Viewer(v),
        _ if top => SizedBy::Setting,
        _ => SizedBy::AutoThrottle,
    };
    (size, by)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::PATTERN_DISPLAY as UHD;

    const FHD: (u32, u32) = (1920, 1080);
    const QHD: (u32, u32) = (2560, 1440);

    #[test]
    fn nobody_reporting_means_no_limit() {
        let mut v = Viewports::default();
        assert_eq!(v.wanted(UHD, 0.0), None);
        v.joined("a", 0.0);
        assert!(!v.update(UHD, 10.0));
        assert_eq!(v.limit(UHD), None);
    }

    #[test]
    fn one_viewer_sets_the_size() {
        let mut v = Viewports::default();
        v.joined("a", 0.0);
        v.reported("a", FHD, 0.5);
        assert!(v.update(UHD, 0.5));
        assert_eq!(v.limit(UHD), Some(FHD));
    }

    #[test]
    fn the_largest_picture_wins_and_a_leaver_is_forgotten() {
        let mut v = Viewports::default();
        v.reported("small", FHD, 0.0);
        v.reported("big", QHD, 0.0);
        assert!(v.update(UHD, 0.0));
        assert_eq!(v.limit(UHD), Some(QHD));
        v.left("big");
        // Held back by the rate limit, then made.
        assert!(!v.update(UHD, 0.5));
        assert_eq!(v.limit(UHD), Some(QHD));
        assert!(v.update(UHD, 1.0));
        assert_eq!(v.limit(UHD), Some(FHD));
        v.left("small");
        assert!(v.update(UHD, 2.0));
        assert_eq!(v.limit(UHD), None);
    }

    #[test]
    fn the_largest_picture_not_the_largest_area() {
        // On a 16:9 source, a tall narrow viewport gives a smaller picture than a short wide one
        // with fewer pixels.
        let mut v = Viewports::default();
        v.reported("tall", (1200, 2400), 0.0);
        v.reported("wide", (1920, 1080), 0.0);
        assert_eq!(v.wanted(UHD, 0.0), Some((1920, 1080)));
    }

    #[test]
    fn a_viewer_that_never_reports_gets_the_full_size_after_the_grace() {
        let mut v = Viewports::default();
        v.reported("new", FHD, 0.0);
        v.joined("old", 0.0);
        assert!(
            v.update(UHD, 0.5),
            "within the grace, the reporting one decides"
        );
        assert_eq!(v.limit(UHD), Some(FHD));
        assert!(v.update(UHD, JOIN_GRACE_S));
        assert_eq!(v.limit(UHD), None);
    }

    #[test]
    fn spam_changes_the_size_at_most_once_a_second() {
        let mut v = Viewports::default();
        let mut changes = 0;
        for i in 0..1000 {
            let t = f64::from(i) * 0.01;
            let size = if i % 2 == 0 { FHD } else { (1280, 720) };
            v.reported("spammer", size, t);
            changes += u32::from(v.update(UHD, t));
        }
        assert!(changes <= 11, "{changes} changes in 10 s");
    }

    #[test]
    fn a_viewport_above_the_source_asks_for_no_more_than_the_source() {
        let mut v = Viewports::default();
        v.reported("a", (16_384, 16_384), 0.0);
        v.update(UHD, 0.0);
        assert_eq!(v.limit(UHD), Some(UHD));
        // Against a 1080p setting, the setting decides.
        let (size, by) = send_size(FHD, true, v.limit(UHD), v.chosen());
        assert_eq!((size, by), (FHD, SizedBy::Setting));
    }

    #[test]
    fn a_different_viewport_with_the_same_picture_is_no_change() {
        let mut v = Viewports::default();
        v.reported("a", FHD, 0.0);
        assert!(v.update(UHD, 0.0));
        // A few pixels more don't round to a larger picture.
        v.reported("a", (1925, 1083), 5.0);
        assert!(!v.update(UHD, 5.0));
        assert_eq!(v.limit(UHD), Some(FHD));
    }

    #[test]
    fn says_what_decided_the_size() {
        assert_eq!(
            send_size(UHD, true, Some(FHD), Some(FHD)),
            (FHD, SizedBy::Viewer(FHD))
        );
        assert_eq!(send_size(UHD, true, None, None), (UHD, SizedBy::Setting));
        assert_eq!(
            send_size(QHD, false, Some(UHD), Some(UHD)),
            (QHD, SizedBy::AutoThrottle)
        );
        assert_eq!(
            SizedBy::Viewer(FHD).to_string(),
            "the viewer (1920x1080 viewport)"
        );
    }

    #[test]
    fn a_resized_source_is_fitted_again() {
        // A shared window grows: the same viewport now allows a larger picture of it.
        let mut v = Viewports::default();
        v.reported("a", (1600, 1000), 0.0);
        v.update((1200, 800), 0.0);
        assert_eq!(v.limit((1200, 800)), Some((1200, 800)));
        assert_eq!(v.limit((2400, 1600)), Some((1504, 1000)));
    }
}
