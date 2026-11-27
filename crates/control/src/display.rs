//! Which display a share is about, and where on it input lands.
//!
//! A share is about one display, decided once when it starts ([`shared_display`]) and handed to
//! everything that needs a screen: the capture, the injector's mapping, the sharer's label and
//! the guest-pointer tag. Nothing picks a display on its own. The platform code only lists the
//! displays; the rules here are pure, so every layout is tested without a second screen.

use serde::{Deserialize, Serialize};

use crate::Point;

/// A display in the global coordinate space, in points (macOS: the space `CGDisplayBounds`
/// uses, with the main display's top left at the origin and y growing downwards).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Display {
    /// The platform's display id (macOS: `CGDirectDisplayID`).
    pub id: u32,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    /// The platform's main display (macOS: the one with the menu bar).
    pub main: bool,
}

impl Display {
    pub fn bounds(&self) -> (f64, f64, f64, f64) {
        (self.x, self.y, self.width, self.height)
    }
}

/// Why no display could be settled on.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NoTarget {
    #[error("no display is active")]
    NoDisplay,
    #[error("no display is marked as the main one")]
    NoMain,
    #[error("more than one display is marked as the main one")]
    TwoMains,
    #[error("display {0} has no size")]
    Empty(u32),
    #[error("no display with id {0} is connected")]
    Gone(u32),
    #[error(
        "display {0} can't be captured (ScreenCaptureKit doesn't list it), so nothing was shared"
    )]
    NotCapturable(u32),
}

/// The main display, whatever the others are and wherever they sit. Its position isn't part of
/// the rule (an external display above or to the left has negative coordinates; one made the
/// main display takes the origin), so a layout this can't settle is an error rather than a guess.
pub fn main_display(displays: &[Display]) -> Result<Display, NoTarget> {
    if displays.is_empty() {
        return Err(NoTarget::NoDisplay);
    }
    let mut mains = displays.iter().filter(|d| d.main);
    let main = *mains.next().ok_or(NoTarget::NoMain)?;
    if mains.next().is_some() {
        return Err(NoTarget::TwoMains);
    }
    sized(main)
}

/// The display with this id, as it is now.
pub fn by_id(displays: &[Display], id: u32) -> Result<Display, NoTarget> {
    let display = *displays
        .iter()
        .find(|d| d.id == id)
        .ok_or(NoTarget::Gone(id))?;
    sized(display)
}

/// The display a share is about: `requested` if given, else the main display. It must be one
/// the capture can see (`capturable`, the ids ScreenCaptureKit lists): sharing some other
/// display instead would show the guest one screen while their input lands on another, so a
/// display the capture doesn't list is an error.
pub fn shared_display(
    displays: &[Display],
    capturable: &[u32],
    requested: Option<u32>,
) -> Result<Display, NoTarget> {
    let display = match requested {
        Some(id) => by_id(displays, id)?,
        None => main_display(displays)?,
    };
    if !capturable.contains(&display.id) {
        return Err(NoTarget::NotCapturable(display.id));
    }
    Ok(display)
}

fn sized(display: Display) -> Result<Display, NoTarget> {
    if display.width < 1.0 || display.height < 1.0 {
        return Err(NoTarget::Empty(display.id));
    }
    Ok(display)
}

/// A point on the shared surface (0..1 over the shared display) in global display points.
pub fn to_points(p: Point, (x0, y0, w, h): (f64, f64, f64, f64)) -> (i32, i32) {
    // The last point is width - 1: a pointer at x = width would be on the next display.
    let x = x0 + (p.x * w).min(w - 1.0);
    let y = y0 + (p.y * h).min(h - 1.0);
    (x.round() as i32, y.round() as i32)
}

/// A point in global display points as a position on the shared surface, clamped to it: a
/// pointer on another display reads as the nearest edge of this one.
pub fn to_surface((x, y): (f64, f64), (x0, y0, w, h): (f64, f64, f64, f64)) -> Point {
    Point {
        x: ((x - x0) / w).clamp(0.0, 1.0),
        y: ((y - y0) / h).clamp(0.0, 1.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BUILT_IN: Display = Display {
        id: 1,
        x: 0.0,
        y: 0.0,
        width: 1512.0,
        height: 982.0,
        main: true,
    };

    fn external(x: f64, y: f64) -> Display {
        Display {
            id: 7,
            x,
            y,
            width: 1920.0,
            height: 1080.0,
            main: false,
        }
    }

    /// Both displays can be captured.
    const BOTH: &[u32] = &[1, 7];

    #[test]
    fn one_display_is_shared() {
        assert_eq!(main_display(&[BUILT_IN]), Ok(BUILT_IN));
        assert_eq!(shared_display(&[BUILT_IN], &[1], None), Ok(BUILT_IN));
    }

    #[test]
    fn an_external_display_beside_the_main_one_is_never_shared_by_default() {
        // Above (the layout that sent a probe window to the wrong display), left, right, and
        // listed first, by CoreGraphics or by ScreenCaptureKit.
        for ext in [
            external(-205.0, -1080.0),
            external(-1920.0, 0.0),
            external(1512.0, -100.0),
        ] {
            for layout in [[BUILT_IN, ext], [ext, BUILT_IN]] {
                assert_eq!(main_display(&layout), Ok(BUILT_IN), "{ext:?}");
                for capturable in [&[1, 7][..], &[7, 1][..]] {
                    assert_eq!(
                        shared_display(&layout, capturable, None),
                        Ok(BUILT_IN),
                        "{ext:?} {capturable:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn an_external_display_made_main_is_shared() {
        let ext = Display {
            main: true,
            ..external(0.0, 0.0)
        };
        let built_in = Display {
            main: false,
            x: 204.0,
            y: 1080.0,
            ..BUILT_IN
        };
        assert_eq!(main_display(&[built_in, ext]), Ok(ext));
        assert_eq!(shared_display(&[built_in, ext], BOTH, None), Ok(ext));
    }

    #[test]
    fn a_display_the_capture_doesnt_list_is_an_error_not_another_display() {
        let layout = [external(-1920.0, 0.0), BUILT_IN];
        // ScreenCaptureKit lists only the external one: share nothing rather than it.
        assert_eq!(
            shared_display(&layout, &[7], None),
            Err(NoTarget::NotCapturable(1))
        );
        assert_eq!(
            shared_display(&layout, &[], None),
            Err(NoTarget::NotCapturable(1))
        );
    }

    #[test]
    fn a_requested_display_is_shared_if_it_exists_and_can_be_captured() {
        let ext = external(-1920.0, 0.0);
        let layout = [BUILT_IN, ext];
        assert_eq!(shared_display(&layout, BOTH, Some(7)), Ok(ext));
        assert_eq!(shared_display(&layout, BOTH, Some(1)), Ok(BUILT_IN));
        assert_eq!(
            shared_display(&layout, BOTH, Some(9)),
            Err(NoTarget::Gone(9))
        );
        assert_eq!(
            shared_display(&layout, &[1], Some(7)),
            Err(NoTarget::NotCapturable(7))
        );
    }

    #[test]
    fn a_layout_that_cant_be_settled_is_an_error() {
        assert_eq!(main_display(&[]), Err(NoTarget::NoDisplay));
        assert_eq!(shared_display(&[], &[], None), Err(NoTarget::NoDisplay));
        assert_eq!(
            main_display(&[external(0.0, 0.0), external(1920.0, 0.0)]),
            Err(NoTarget::NoMain)
        );
        let second = Display {
            id: 2,
            x: 1512.0,
            ..BUILT_IN
        };
        assert_eq!(main_display(&[BUILT_IN, second]), Err(NoTarget::TwoMains));
        let empty = Display {
            width: 0.0,
            ..BUILT_IN
        };
        assert_eq!(main_display(&[empty]), Err(NoTarget::Empty(1)));
        assert_eq!(by_id(&[empty], 1), Err(NoTarget::Empty(1)));
    }

    #[test]
    fn maps_the_surface_onto_the_shared_display() {
        let display = BUILT_IN.bounds();
        assert_eq!(to_points(Point { x: 0.0, y: 0.0 }, display), (0, 0));
        assert_eq!(to_points(Point { x: 0.5, y: 0.5 }, display), (756, 491));
        assert_eq!(
            to_points(Point { x: 1.0, y: 1.0 }, display),
            (1511, 981),
            "stays on this display"
        );
    }

    #[test]
    fn maps_onto_a_display_away_from_the_origin() {
        // Left of the main display, and lower.
        let left = (-1920.0, 100.0, 1920.0, 1080.0);
        assert_eq!(to_points(Point { x: 0.25, y: 0.0 }, left), (-1440, 100));
        assert_eq!(to_points(Point { x: 1.0, y: 1.0 }, left), (-1, 1179));
        // Above it, off-centre: both coordinates negative.
        let above = (-205.0, -1080.0, 1920.0, 1080.0);
        assert_eq!(to_points(Point { x: 0.0, y: 0.0 }, above), (-205, -1080));
        assert_eq!(to_points(Point { x: 0.5, y: 0.5 }, above), (755, -540));
        assert_eq!(
            to_points(Point { x: 1.0, y: 1.0 }, above),
            (1714, -1),
            "never on the main display below it"
        );
        // A 4K panel at "looks like 1920x1080" (scale 2) and a 1080p one at scale 1 have the
        // same size in points: the mapping is in points, so the scale makes no difference.
        let right = (1512.0, -98.0, 1920.0, 1080.0);
        assert_eq!(to_points(Point { x: 0.5, y: 0.5 }, right), (2472, 442));
        // A scaled mode with fractional points.
        let scaled = (1512.0, 0.0, 1706.5, 960.0);
        assert_eq!(to_points(Point { x: 1.0, y: 0.0 }, scaled), (3218, 0));
    }

    #[test]
    fn a_point_read_back_is_where_it_was_sent() {
        for bounds in [
            BUILT_IN.bounds(),
            (-1920.0, 100.0, 1920.0, 1080.0),
            (-205.0, -1080.0, 1920.0, 1080.0),
        ] {
            for p in [Point { x: 0.0, y: 0.0 }, Point { x: 0.3, y: 0.7 }] {
                let (x, y) = to_points(p, bounds);
                let back = to_surface((f64::from(x), f64::from(y)), bounds);
                assert!(back.near(p), "{p:?} on {bounds:?} came back as {back:?}");
            }
        }
        // A pointer on another display reads as this display's nearest edge.
        let back = to_surface((-50.0, 2000.0), BUILT_IN.bounds());
        assert_eq!(back, Point { x: 0.0, y: 1.0 });
    }
}
