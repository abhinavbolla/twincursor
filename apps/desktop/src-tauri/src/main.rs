//! The TwinCursor desktop app. It shows a window and starts the helper (`tc-spike`, inside the
//! bundle), which does everything that touches the screen, the mic and input. The window only
//! shows text: no media runs in the webview (see docs/adr/0007-shell-and-engine.md).
//!
//! The helper's stdin stays open for as long as this app runs. The helper is started with
//! `--exit-on-stdin-close`, so when the app quits or dies, the kernel closes that pipe and the
//! helper leaves too.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager, RunEvent, WebviewUrl, WebviewWindowBuilder};

/// The helper's file name, next to this app's executable.
const HELPER: &str = "tc-spike";
/// Title of the probe windows; the UI looks for it in the helper's report.
const PROBE_TITLE: &str = "TwinCursor capture probe";
/// The helper prints this line once the capture that leaves us out is running
/// (`CAPTURING` in `crates/media-spike/src/permissions.rs`).
const CAPTURING: &str = "permissions: capture running";
/// How long the helper may take to exit once its stdin closes.
const EXIT_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Clone, Serialize)]
struct OutputLine {
    source: String,
    line: String,
}

#[derive(Serialize)]
struct HelperInfo {
    path: Option<String>,
    error: Option<String>,
    in_bundle: bool,
    app_pid: u32,
}

#[derive(Serialize)]
struct StdinCloseResult {
    exited_after_ms: Option<u64>,
}

/// The long-running helper. Holding `stdin` keeps it alive.
struct Watcher {
    child: Child,
    stdin: Option<ChildStdin>,
}

#[derive(Default)]
struct AppState {
    watcher: Mutex<Option<Watcher>>,
}

/// Where the helper is: next to this executable (in the bundle, `Contents/MacOS`), or in a
/// development checkout, the workspace's release build.
fn helper_path() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("can't find this app: {e}"))?;
    let mut tried = Vec::new();
    if let Some(dir) = exe.parent() {
        tried.push(dir.join(HELPER));
    }
    tried.push(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../target/release")
            .join(HELPER),
    );
    for path in &tried {
        if path.is_file() {
            return Ok(path.canonicalize().unwrap_or_else(|_| path.clone()));
        }
    }
    let tried: Vec<_> = tried.iter().map(|p| p.display().to_string()).collect();
    Err(format!(
        "no {HELPER} at {} (run `cargo build --release -p twincursor-media-spike`)",
        tried.join(" or ")
    ))
}

fn in_bundle(path: &Path) -> bool {
    path.parent()
        .is_some_and(|dir| dir.ends_with("Contents/MacOS"))
}

/// Starts the helper with `args`, its stdin kept open, and streams its stderr to the window.
/// `NO_COLOR` keeps its log lines free of terminal colors.
fn spawn_helper(app: &AppHandle, source: &str, args: &[String]) -> Result<Child, String> {
    let path = helper_path()?;
    let mut child = Command::new(&path)
        .args(args)
        .arg("--exit-on-stdin-close")
        .env("NO_COLOR", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("couldn't start {}: {e}", path.display()))?;
    emit_line(app, source, &format!("$ {HELPER} {}", args.join(" ")));
    if let Some(stderr) = child.stderr.take() {
        forward(app.clone(), source.to_owned(), stderr, |_| {});
    }
    Ok(child)
}

fn emit_line(app: &AppHandle, source: &str, line: &str) {
    let _ = app.emit(
        "helper-output",
        OutputLine {
            source: source.to_owned(),
            line: line.to_owned(),
        },
    );
}

/// Sends each line of `from` to the window, and to `on_line`, on a thread of its own.
fn forward(
    app: AppHandle,
    source: String,
    from: impl Read + Send + 'static,
    mut on_line: impl FnMut(&str) + Send + 'static,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        for line in BufReader::new(from).lines() {
            let Ok(line) = line else { break };
            emit_line(&app, &source, &line);
            on_line(&line);
        }
    })
}

/// The last line of `lines` that is a JSON object.
fn last_json(lines: &[String]) -> Option<Value> {
    lines
        .iter()
        .rev()
        .filter(|l| l.starts_with('{'))
        .find_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(Value::is_object)
}

/// Runs the helper to the end and returns the JSON object it printed last. `on_line` sees each
/// line of its stdout as it comes.
fn run_helper(
    app: &AppHandle,
    source: &str,
    args: &[&str],
    on_line: impl FnMut(&str) + Send + 'static,
) -> Result<Value, String> {
    let args: Vec<String> = args.iter().map(|s| (*s).to_owned()).collect();
    let mut child = spawn_helper(app, source, &args)?;
    // Hold stdin until the helper is done, so it isn't told to leave early.
    let stdin = child.stdin.take();
    let lines = std::sync::Arc::new(Mutex::new(Vec::new()));
    let reader = child.stdout.take().map(|stdout| {
        let lines = lines.clone();
        let mut on_line = on_line;
        forward(app.clone(), source.to_owned(), stdout, move |line| {
            lines.lock().unwrap().push(line.to_owned());
            on_line(line);
        })
    });
    let status = child.wait().map_err(|e| e.to_string())?;
    if let Some(reader) = reader {
        let _ = reader.join();
    }
    drop(stdin);
    let lines = lines.lock().unwrap();
    last_json(&lines).ok_or_else(|| {
        format!("the helper exited ({status}) without a report; see its output below")
    })
}

#[tauri::command]
fn helper_info() -> HelperInfo {
    let app_pid = std::process::id();
    match helper_path() {
        Ok(path) => HelperInfo {
            in_bundle: in_bundle(&path),
            path: Some(path.display().to_string()),
            error: None,
            app_pid,
        },
        Err(e) => HelperInfo {
            path: None,
            error: Some(e),
            in_bundle: false,
            app_pid,
        },
    }
}

fn permissions(app: &AppHandle, request: bool) -> Result<Value, String> {
    let args: &[&str] = if request {
        &["permissions", "--request"]
    } else {
        &["permissions"]
    };
    run_helper(app, "permissions", args, |_| {})
}

#[tauri::command]
async fn check_permissions(app: AppHandle, request: bool) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || permissions(&app, request))
        .await
        .map_err(|e| e.to_string())?
}

/// A plain magenta, always-on-top window the helper looks for in what it captures.
fn open_probe(app: &AppHandle, label: &str, x: f64) -> Result<(), String> {
    WebviewWindowBuilder::new(app, label, WebviewUrl::App("probe.html".into()))
        .title(PROBE_TITLE)
        .inner_size(280.0, 180.0)
        .position(x, 140.0)
        .background_color(tauri::window::Color(255, 0, 255, 255))
        .decorations(false)
        .shadow(false)
        .resizable(false)
        .always_on_top(true)
        .focused(false)
        .skip_taskbar(true)
        .build()
        .map(|_| ())
        .map_err(|e| format!("couldn't open a probe window: {e}"))
}

fn close_probes(app: &AppHandle) {
    for label in ["probe-early", "probe-late"] {
        if let Some(w) = app.get_webview_window(label) {
            let _ = w.destroy();
        }
    }
}

/// One probe window before the helper starts capturing, and a second one once it is: the
/// second shows whether windows opened mid-capture stay out too.
fn capture(app: &AppHandle) -> Result<Value, String> {
    open_probe(app, "probe-early", 80.0)?;
    // Let it reach the screen before the first capture.
    std::thread::sleep(Duration::from_millis(800));
    let pid = std::process::id().to_string();
    let late = app.clone();
    let result = run_helper(
        app,
        "capture",
        &["permissions", "--capture", "--exclude-app-pid", &pid],
        move |line| {
            if line == CAPTURING {
                if let Err(e) = open_probe(&late, "probe-late", 420.0) {
                    emit_line(&late, "capture", &e);
                }
            }
        },
    );
    close_probes(app);
    result
}

#[tauri::command]
async fn check_capture(app: AppHandle) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || capture(&app))
        .await
        .map_err(|e| e.to_string())?
}

/// Starts a helper that would run until stopped, closes its stdin, and times its exit.
fn stdin_close(app: &AppHandle) -> Result<StdinCloseResult, String> {
    let args = ["permissions".to_owned(), "--watch".to_owned()];
    let mut child = spawn_helper(app, "stdin", &args)?;
    // Wait until it's up and printing, so we time the exit, not the start.
    let stdout = child.stdout.take().ok_or("no stdout")?;
    let mut stdout = BufReader::new(stdout);
    let mut first = String::new();
    stdout.read_line(&mut first).map_err(|e| e.to_string())?;
    emit_line(app, "stdin", first.trim_end());
    let rest = forward(app.clone(), "stdin".into(), stdout, |_| {});
    emit_line(app, "stdin", "closing the helper's stdin");
    let closed = Instant::now();
    drop(child.stdin.take());
    let exited = loop {
        if child.try_wait().map_err(|e| e.to_string())?.is_some() {
            break Some(closed.elapsed().as_millis() as u64);
        }
        if closed.elapsed() > EXIT_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    let _ = rest.join();
    Ok(StdinCloseResult {
        exited_after_ms: exited,
    })
}

#[tauri::command]
async fn check_stdin_close(app: AppHandle) -> Result<StdinCloseResult, String> {
    tauri::async_runtime::spawn_blocking(move || stdin_close(&app))
        .await
        .map_err(|e| e.to_string())?
}

/// Starts the helper that stays for the app's whole life and reports permission changes.
fn start_watcher(app: &AppHandle) -> Result<Watcher, String> {
    let args = ["permissions".to_owned(), "--watch".to_owned()];
    let mut child = spawn_helper(app, "helper", &args)?;
    if let Some(stdout) = child.stdout.take() {
        let status = app.clone();
        forward(app.clone(), "helper".into(), stdout, move |line| {
            if let Ok(value) = serde_json::from_str::<Value>(line) {
                let _ = status.emit("helper-status", value);
            }
        });
    }
    let stdin = child.stdin.take();
    Ok(Watcher { child, stdin })
}

/// Stops a watcher by closing its stdin, as the app's exit would, and kills it if it lingers.
fn stop_watcher(mut watcher: Watcher) {
    drop(watcher.stdin.take());
    let start = Instant::now();
    while start.elapsed() < EXIT_TIMEOUT {
        if let Ok(Some(_)) = watcher.child.try_wait() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let _ = watcher.child.kill();
    let _ = watcher.child.wait();
}

/// macOS applies a new grant to processes started after it, so after the person answers a
/// prompt the long-running helper is replaced by a fresh one rather than restarting the app.
fn restart(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    if let Some(old) = state.watcher.lock().unwrap().take() {
        stop_watcher(old);
    }
    let new = start_watcher(app)?;
    *state.watcher.lock().unwrap() = Some(new);
    Ok(())
}

#[tauri::command]
async fn restart_helper(app: AppHandle) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || restart(&app))
        .await
        .map_err(|e| e.to_string())?
}

/// `--run-checks PATH`: run the checks that need no prompt, write the results to PATH as JSON,
/// and quit. Never asks macOS for anything, so it can run unattended.
fn run_checks_to(app: AppHandle, out: PathBuf) {
    std::thread::spawn(move || {
        // Give the main window a moment to appear, as it would for a person.
        std::thread::sleep(Duration::from_secs(1));
        let to_json =
            |r: Result<Value, String>| r.unwrap_or_else(|e| serde_json::json!({ "error": e }));
        let report = serde_json::json!({
            "app_pid": std::process::id(),
            "helper": helper_info(),
            "permissions": to_json(permissions(&app, false)),
            "capture": to_json(capture(&app)),
            "stdin_close": to_json(stdin_close(&app).and_then(|r| serde_json::to_value(r).map_err(|e| e.to_string()))),
        });
        let text = serde_json::to_string_pretty(&report).unwrap_or_default();
        let code = match std::fs::write(&out, text) {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("couldn't write {}: {e}", out.display());
                1
            }
        };
        app.exit(code);
    });
}

fn run_checks_arg() -> Option<PathBuf> {
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--run-checks" {
            return args.next().map(PathBuf::from);
        }
    }
    None
}

fn main() {
    let run_checks = run_checks_arg();
    let app = tauri::Builder::default()
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            helper_info,
            check_permissions,
            check_capture,
            check_stdin_close,
            restart_helper
        ])
        .setup(move |app| {
            let handle = app.handle().clone();
            match start_watcher(&handle) {
                Ok(watcher) => *app.state::<AppState>().watcher.lock().unwrap() = Some(watcher),
                Err(e) => emit_line(&handle, "helper", &e),
            }
            if let Some(out) = run_checks.clone() {
                run_checks_to(handle, out);
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("starting TwinCursor");
    app.run(|app, event| {
        if let RunEvent::Exit = event {
            // Closing stdin is enough; the kernel does the same if we die instead.
            if let Some(watcher) = app.state::<AppState>().watcher.lock().unwrap().take() {
                stop_watcher(watcher);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_report_among_log_lines() {
        let lines = [
            "2026-11-14T09:00:00Z INFO capture started".to_owned(),
            r#"{"screen_recording":"granted"}"#.to_owned(),
            "{not json".to_owned(),
        ];
        assert_eq!(last_json(&lines).unwrap()["screen_recording"], "granted");
        assert!(last_json(&lines[..1]).is_none());
        assert!(last_json(&["[1, 2]".to_owned()]).is_none());
    }

    #[test]
    fn knows_a_bundled_helper() {
        assert!(in_bundle(Path::new(
            "/Applications/TwinCursor.app/Contents/MacOS/tc-spike"
        )));
        assert!(!in_bundle(Path::new(
            "/src/twincursor/target/release/tc-spike"
        )));
    }

    #[test]
    fn the_capture_marker_matches_the_helper() {
        let helper = include_str!("../../../../crates/media-spike/src/permissions.rs");
        assert!(helper.contains(&format!("pub const CAPTURING: &str = \"{CAPTURING}\";")));
        let ui = include_str!("../../src/checks.ts");
        assert!(ui.contains(&format!("export const PROBE_TITLE = \"{PROBE_TITLE}\";")));
    }
}
