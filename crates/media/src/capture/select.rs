//! Choosing what to capture: which running processes are TwinCursor (left out of a display
//! capture so a viewer on the same Mac doesn't show up in its own picture), and which window
//! or app a `--window` or `--app` query means. Pure functions, so they're tested without a
//! screen.

use std::path::Path;

use serde::Serialize;

/// Bundle identifiers of TwinCursor apps.
pub const BUNDLE_IDS: &[&str] = &["dev.twincursor.desktop"];

/// Executable names of TwinCursor programs. A copy renamed with a suffix (`tc-spike-old`)
/// counts too.
pub const EXECUTABLES: &[&str] = &[
    "tc-spike",
    "tc-engine",
    "TwinCursor",
    "twincursor",
    "twincursor-desktop",
];

/// A running process, as far as telling TwinCursor apart needs.
#[derive(Debug, Clone, Copy)]
pub struct Process<'a> {
    pub pid: i32,
    /// Path of its executable, when it can be read.
    pub exe: Option<&'a Path>,
    pub bundle_id: &'a str,
}

/// Whether `candidate` is TwinCursor: this process itself, another copy of the same
/// executable, or any TwinCursor program by bundle identifier or executable name.
pub fn is_twincursor(candidate: &Process, me: &Process) -> bool {
    if candidate.pid == me.pid {
        return true;
    }
    if let (Some(a), Some(b)) = (candidate.exe, me.exe) {
        if a == b {
            return true;
        }
    }
    if BUNDLE_IDS
        .iter()
        .any(|id| id.eq_ignore_ascii_case(candidate.bundle_id))
    {
        return true;
    }
    candidate
        .exe
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            EXECUTABLES.iter().any(|exe| {
                name == *exe
                    || name
                        .strip_prefix(exe)
                        .is_some_and(|rest| rest.starts_with('-'))
            })
        })
}

/// A window that can be shared.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WindowInfo {
    /// Window id (CoreGraphics), stable while the window exists.
    pub id: u32,
    pub app: String,
    pub pid: i32,
    pub title: String,
    /// Size in pixels: points × the display's scale.
    pub pixels: (u32, u32),
}

/// An application that can be shared.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AppInfo {
    pub pid: i32,
    pub name: String,
    pub bundle_id: String,
}

/// Why a query didn't pick exactly one thing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pick<T> {
    One(T),
    None,
    /// Several matched; they're listed so the user can be more specific.
    Many(Vec<T>),
}

/// Contains `query`, ignoring case.
fn contains(haystack: &str, query: &str) -> bool {
    haystack.to_lowercase().contains(&query.to_lowercase())
}

/// The candidates matching `query`; when several do, the ones matching it exactly win.
fn narrow<T>(items: &[T], matches: impl Fn(&T) -> bool, exact: impl Fn(&T) -> bool) -> Pick<&T> {
    let found: Vec<&T> = items.iter().filter(|i| matches(i)).collect();
    match found.len() {
        0 => Pick::None,
        1 => Pick::One(found[0]),
        _ => {
            let exact: Vec<&T> = found.iter().copied().filter(|i| exact(i)).collect();
            if exact.len() == 1 {
                Pick::One(exact[0])
            } else {
                Pick::Many(found)
            }
        }
    }
}

/// The window `query` means: a window id, or text the window's title or app name contains
/// (ignoring case). When several contain it, one whose title or app name is exactly `query`
/// wins.
pub fn pick_window<'a>(windows: &'a [WindowInfo], query: &str) -> Pick<&'a WindowInfo> {
    let query = query.trim();
    if let Ok(id) = query.parse::<u32>() {
        if let Some(w) = windows.iter().find(|w| w.id == id) {
            return Pick::One(w);
        }
    }
    if query.is_empty() {
        return Pick::None;
    }
    narrow(
        windows,
        |w| contains(&w.title, query) || contains(&w.app, query),
        |w| w.title.eq_ignore_ascii_case(query),
    )
}

/// The application `query` means: its name contains `query` (ignoring case), or its bundle
/// identifier is `query`. When several contain it, the one named exactly `query` wins.
pub fn pick_app<'a>(apps: &'a [AppInfo], query: &str) -> Pick<&'a AppInfo> {
    let query = query.trim();
    if query.is_empty() {
        return Pick::None;
    }
    narrow(
        apps,
        |a| contains(&a.name, query) || a.bundle_id.eq_ignore_ascii_case(query),
        |a| a.name.eq_ignore_ascii_case(query) || a.bundle_id.eq_ignore_ascii_case(query),
    )
}

/// Pixel size of a window `points` big on a display with `scale` pixels per point.
pub fn window_pixels(points: (f64, f64), scale: f64) -> (u32, u32) {
    let px = |v: f64| (v * scale).round().max(0.0) as u32;
    (px(points.0), px(points.1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn me() -> (PathBuf, i32) {
        (
            PathBuf::from("/Users/a/twincursor/target/release/tc-spike"),
            100,
        )
    }

    #[test]
    fn this_process_is_twincursor() {
        let (exe, pid) = me();
        let me = Process {
            pid,
            exe: Some(&exe),
            bundle_id: "",
        };
        assert!(is_twincursor(&me, &me));
        // Even when its path can't be read.
        let unreadable = Process { exe: None, ..me };
        assert!(is_twincursor(&unreadable, &me));
    }

    #[test]
    fn another_copy_of_the_same_program_is_twincursor() {
        let (exe, pid) = me();
        let me = Process {
            pid,
            exe: Some(&exe),
            bundle_id: "",
        };
        let viewer = Process { pid: 200, ..me };
        assert!(is_twincursor(&viewer, &me));
        // A tc-spike built elsewhere (another checkout), a renamed copy, and the engine.
        let other = PathBuf::from("/tmp/old/tc-spike");
        let renamed = PathBuf::from("/tmp/bin/tc-spike-before");
        let engine = PathBuf::from("/opt/twincursor/tc-engine");
        for exe in [&other, &renamed, &engine] {
            let p = Process {
                pid: 300,
                exe: Some(exe),
                bundle_id: "",
            };
            assert!(is_twincursor(&p, &me), "{exe:?}");
        }
    }

    #[test]
    fn the_app_bundle_is_twincursor() {
        let (exe, pid) = me();
        let me = Process {
            pid,
            exe: Some(&exe),
            bundle_id: "",
        };
        let app = PathBuf::from("/Applications/TwinCursor.app/Contents/MacOS/TwinCursor");
        let by_name = Process {
            pid: 400,
            exe: Some(&app),
            bundle_id: "",
        };
        let by_bundle = Process {
            pid: 401,
            exe: None,
            bundle_id: "dev.twincursor.desktop",
        };
        assert!(is_twincursor(&by_name, &me));
        assert!(is_twincursor(&by_bundle, &me));
    }

    #[test]
    fn other_apps_are_not_twincursor() {
        let (exe, pid) = me();
        let me = Process {
            pid,
            exe: Some(&exe),
            bundle_id: "",
        };
        let terminal =
            PathBuf::from("/System/Applications/Utilities/Terminal.app/Contents/MacOS/Terminal");
        let spiky = PathBuf::from("/usr/local/bin/tc-spiked");
        for (exe, bundle) in [
            (Some(terminal.as_path()), "com.apple.Terminal"),
            (Some(spiky.as_path()), ""),
            (None, ""),
        ] {
            let p = Process {
                pid: 500,
                exe,
                bundle_id: bundle,
            };
            assert!(!is_twincursor(&p, &me), "{exe:?} {bundle}");
        }
    }

    fn window(id: u32, app: &str, title: &str) -> WindowInfo {
        WindowInfo {
            id,
            app: app.into(),
            pid: id as i32,
            title: title.into(),
            pixels: (1000, 800),
        }
    }

    fn windows() -> Vec<WindowInfo> {
        vec![
            window(11, "Terminal", "abhi — zsh — 80x24"),
            window(12, "Terminal", "build — cargo — 120x40"),
            window(20, "Safari", "TwinCursor docs"),
            window(30, "TextEdit", "notes.txt"),
            window(31, "TextEdit", "notes"),
        ]
    }

    #[test]
    fn picks_a_window_by_title_ignoring_case() {
        let w = windows();
        assert_eq!(pick_window(&w, "CARGO"), Pick::One(&w[1]));
        assert_eq!(pick_window(&w, "twincursor docs"), Pick::One(&w[2]));
    }

    #[test]
    fn picks_a_window_by_app_name() {
        let w = windows();
        assert_eq!(pick_window(&w, "safari"), Pick::One(&w[2]));
    }

    #[test]
    fn several_matches_are_listed() {
        let w = windows();
        assert_eq!(pick_window(&w, "terminal"), Pick::Many(vec![&w[0], &w[1]]));
    }

    #[test]
    fn an_exact_title_wins_over_partial_ones() {
        let w = windows();
        assert_eq!(pick_window(&w, "Notes"), Pick::One(&w[4]));
        assert_eq!(pick_window(&w, "notes.TXT"), Pick::One(&w[3]));
    }

    #[test]
    fn a_window_id_picks_that_window() {
        let w = windows();
        assert_eq!(pick_window(&w, "12"), Pick::One(&w[1]));
        // A number that isn't an id is matched as text.
        assert_eq!(pick_window(&w, "80"), Pick::One(&w[0]));
    }

    #[test]
    fn no_match() {
        let w = windows();
        assert_eq!(pick_window(&w, "Xcode"), Pick::None);
        assert_eq!(pick_window(&w, "  "), Pick::None);
        assert_eq!(pick_window(&[], "Terminal"), Pick::None);
    }

    fn app(pid: i32, name: &str, bundle: &str) -> AppInfo {
        AppInfo {
            pid,
            name: name.into(),
            bundle_id: bundle.into(),
        }
    }

    #[test]
    fn picks_an_app() {
        let apps = vec![
            app(1, "Code", "com.microsoft.VSCode"),
            app(2, "Xcode", "com.apple.dt.Xcode"),
            app(3, "Safari", "com.apple.Safari"),
        ];
        assert_eq!(pick_app(&apps, "safari"), Pick::One(&apps[2]));
        assert_eq!(pick_app(&apps, "code"), Pick::One(&apps[0]));
        assert_eq!(pick_app(&apps, "com.apple.dt.xcode"), Pick::One(&apps[1]));
        assert_eq!(pick_app(&apps, "o"), Pick::Many(vec![&apps[0], &apps[1]]));
        assert_eq!(pick_app(&apps, "Mail"), Pick::None);
    }

    #[test]
    fn window_pixels_follow_the_display_scale() {
        assert_eq!(window_pixels((754.0, 491.0), 2.0), (1508, 982));
        assert_eq!(window_pixels((1280.0, 800.0), 1.0), (1280, 800));
        assert_eq!(window_pixels((100.25, 50.5), 2.0), (201, 101));
    }
}
