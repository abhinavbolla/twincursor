//! Which display real input goes to.
//!
//! The injector maps the shared surface (0..1 on both axes) onto one display, and anything that
//! needs to know where injected input will land (the `tc-spike probe` test window) asks the same
//! function, [`input_target`], rather than guessing. The platform code only lists the displays.

use serde::{Deserialize, Serialize};

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

/// Why [`input_target`] couldn't say where input goes.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NoTarget {
    #[error("no display is active")]
    NoDisplay,
    #[error("no display is marked as the main one")]
    NoMain,
    #[error("more than one display is marked as the main one")]
    TwoMains,
    #[error("the main display has no size")]
    Empty,
}

/// The display real input is injected onto: the main display, whatever the others are and
/// wherever they sit. Its position isn't part of the rule (an external display above or to the
/// left has negative coordinates; one made the main display takes the origin), so a layout this
/// can't settle is an error rather than a guess.
pub fn input_target(displays: &[Display]) -> Result<Display, NoTarget> {
    if displays.is_empty() {
        return Err(NoTarget::NoDisplay);
    }
    let mut mains = displays.iter().filter(|d| d.main);
    let main = *mains.next().ok_or(NoTarget::NoMain)?;
    if mains.next().is_some() {
        return Err(NoTarget::TwoMains);
    }
    if main.width < 1.0 || main.height < 1.0 {
        return Err(NoTarget::Empty);
    }
    Ok(main)
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

    #[test]
    fn one_display_is_the_target() {
        assert_eq!(input_target(&[BUILT_IN]), Ok(BUILT_IN));
    }

    #[test]
    fn an_external_display_beside_the_main_one_is_never_the_target() {
        // Above (the layout that sent a probe window to the wrong display), left, right, and
        // listed first.
        for ext in [
            external(-205.0, -1080.0),
            external(-1920.0, 0.0),
            external(1512.0, -100.0),
        ] {
            assert_eq!(input_target(&[BUILT_IN, ext]), Ok(BUILT_IN), "{ext:?}");
            assert_eq!(input_target(&[ext, BUILT_IN]), Ok(BUILT_IN), "{ext:?}");
        }
    }

    #[test]
    fn an_external_display_made_main_is_the_target() {
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
        assert_eq!(input_target(&[built_in, ext]), Ok(ext));
    }

    #[test]
    fn a_layout_that_cant_be_settled_is_an_error() {
        assert_eq!(input_target(&[]), Err(NoTarget::NoDisplay));
        assert_eq!(
            input_target(&[external(0.0, 0.0), external(1920.0, 0.0)]),
            Err(NoTarget::NoMain)
        );
        let second = Display {
            id: 2,
            x: 1512.0,
            ..BUILT_IN
        };
        assert_eq!(input_target(&[BUILT_IN, second]), Err(NoTarget::TwoMains));
        let empty = Display {
            width: 0.0,
            ..BUILT_IN
        };
        assert_eq!(input_target(&[empty]), Err(NoTarget::Empty));
    }
}
