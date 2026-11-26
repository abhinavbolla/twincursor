//! Which display a share is about. Decided once, when the share starts, by [`choose`], and
//! handed to everything that needs a screen: the capture (`CaptureConfig::display`), the
//! injector ([`crate::sharer::Surface::Display`]), and the sharer's label and guest-pointer tag
//! ([`crate::hotkey::SharerUi::new`]). None of them picks a display on its own.

use control::display::Display;
use media::capture::Backend;
use serde::Serialize;

/// The display a share captures, and the one remote-control input goes to: `requested` if
/// given (a `CGDirectDisplayID`, as `displays` lists it), else the main display (the one with
/// the menu bar). For the screen backends it must be one ScreenCaptureKit lists: otherwise the
/// share fails with a sentence saying so, rather than capturing some other display.
///
/// The synthetic pattern is no display's picture. It keeps what benches have always had: input
/// maps onto the main display (or the requested one), and the capture list isn't consulted.
#[cfg(target_os = "macos")]
pub fn choose(backend: Backend, requested: Option<u32>) -> Result<Display, String> {
    use control::display::{by_id, main_display, shared_display};
    let displays = control::macos::displays();
    let chosen = match backend {
        Backend::Pattern => match requested {
            Some(id) => by_id(&displays, id),
            None => main_display(&displays),
        },
        _ => {
            let capturable = media::capture::display_ids().map_err(|e| e.to_string())?;
            shared_display(&displays, &capturable, requested)
        }
    };
    let shared = chosen.map_err(|e| format!("can't tell which display to share: {e}"))?;
    tracing::info!(
        main = shared.main,
        "sharing display {} ({}x{} points at {}, {})",
        shared.id,
        shared.width,
        shared.height,
        shared.x,
        shared.y,
    );
    Ok(shared)
}

#[cfg(not(target_os = "macos"))]
pub fn choose(backend: Backend, requested: Option<u32>) -> Result<Display, String> {
    let _ = (backend, requested);
    Err("sharing a display is macOS only for now".into())
}

/// A display as `displays` lists it.
#[derive(Debug, Clone, Serialize)]
pub struct DisplayInfo {
    /// What `--display` takes.
    pub id: u32,
    /// The main display, the one a share is about by default.
    pub main: bool,
    /// Whether screen capture can see it (ScreenCaptureKit lists it).
    pub capturable: bool,
    /// Top left and size in points, in the global space (the main display's top left is 0, 0).
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    /// Size in pixels, when known.
    pub pixels: Option<(u32, u32)>,
}

/// Every active display, main first.
pub fn list() -> Vec<DisplayInfo> {
    #[cfg(target_os = "macos")]
    {
        let capturable = media::capture::display_ids().unwrap_or_default();
        let mut out: Vec<DisplayInfo> = control::macos::displays()
            .into_iter()
            .map(|d| DisplayInfo {
                id: d.id,
                main: d.main,
                capturable: capturable.contains(&d.id),
                x: d.x,
                y: d.y,
                width: d.width,
                height: d.height,
                pixels: media::capture::display_pixels(d.id),
            })
            .collect();
        out.sort_by_key(|d| !d.main);
        out
    }
    #[cfg(not(target_os = "macos"))]
    {
        Vec::new()
    }
}

/// [`list`] as a table.
pub fn table(displays: &[DisplayInfo]) -> String {
    let mut out =
        String::from("id          main  capture  points (x, y, w x h)             pixels\n");
    for d in displays {
        let pixels = d
            .pixels
            .map_or_else(|| "?".to_owned(), |(w, h)| format!("{w}x{h}"));
        out.push_str(&format!(
            "{:<11} {:<5} {:<8} {:<32} {pixels}\n",
            d.id,
            if d.main { "yes" } else { "" },
            if d.capturable { "yes" } else { "no" },
            format!("{}, {}, {} x {}", d.x, d.y, d.width, d.height),
        ));
    }
    out
}
