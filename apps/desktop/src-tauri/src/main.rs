//! The TwinCursor desktop app: one window to start or join a call. The call itself runs in the
//! engine, `tc-engine` (inside the bundle), which does everything that touches the screen, the
//! mic and input, and shows the shared screen in its own native window. The webview only shows
//! text and controls (docs/adr/0007-shell-and-engine.md).
//!
//! `engine.rs` starts and stops the engine and reads its events, `settings.rs` keeps the server
//! overrides, and `script.rs` is the test-only `--script` mode. This file connects them to the
//! window.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod engine;
mod script;
mod settings;

use std::path::PathBuf;
use std::sync::{mpsc, Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager, RunEvent};

use engine::{Call, Launcher, Output, Plan, Sink, TestHooks};
use settings::{Settings, SettingsChange, SettingsView};

/// The webview hears about the call on this event.
const ENGINE_EVENT: &str = "engine";

#[derive(Default)]
struct AppState {
    call: Mutex<Option<Call>>,
}

#[derive(Serialize)]
struct EngineInfo {
    path: Option<String>,
    error: Option<String>,
    in_bundle: bool,
    app_pid: u32,
}

fn settings_path(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map(|dir| dir.join("settings.json"))
        .map_err(|e| format!("No settings folder: {e}."))
}

fn load_settings(app: &AppHandle) -> Result<Settings, String> {
    Settings::load(&settings_path(app)?)
}

#[tauri::command]
fn engine_info() -> EngineInfo {
    let app_pid = std::process::id();
    match engine::engine_path() {
        Ok(path) => EngineInfo {
            in_bundle: engine::in_bundle(&path),
            path: Some(path.display().to_string()),
            error: None,
            app_pid,
        },
        Err(e) => EngineInfo {
            path: None,
            error: Some(e),
            in_bundle: false,
            app_pid,
        },
    }
}

/// Starts a call unless one is running. Its output goes to the window, and to `also`.
fn begin(
    app: &AppHandle,
    plan: Plan,
    env: Vec<(String, String)>,
    hooks: TestHooks,
    also: Option<Sink>,
) -> Result<(), String> {
    let plan = plan.checked()?;
    let state = app.state::<AppState>();
    let mut slot = state.call.lock().unwrap();
    if slot.as_ref().is_some_and(|call| !call.is_done()) {
        return Err("A call is already running.".into());
    }
    if let Some(old) = slot.take() {
        old.shutdown();
    }
    let launcher = Launcher {
        exe: engine::engine_path()?,
        env,
        app_pid: std::process::id(),
        hooks,
    };
    let window = app.clone();
    let sink: Sink = Arc::new(move |output: Output| {
        if let Some(also) = &also {
            also(output.clone());
        }
        let _ = window.emit(ENGINE_EVENT, output);
    });
    *slot = Some(Call::start(launcher, plan, sink)?);
    Ok(())
}

#[tauri::command]
fn start_call(app: AppHandle, plan: Plan) -> Result<(), String> {
    let env = load_settings(&app)?.env();
    begin(&app, plan, env, TestHooks::default(), None)
}

/// One command for the engine, as the person clicked it. Typed, so only commands the protocol
/// knows get through; an allow names the request the person answered.
#[tauri::command]
fn send_command(state: tauri::State<'_, AppState>, command: ipc::Command) -> Result<(), String> {
    match state.call.lock().unwrap().as_ref() {
        Some(call) if !call.is_done() => call.send(&command),
        _ => Err("No call is running.".into()),
    }
}

#[tauri::command]
fn leave_call(state: tauri::State<'_, AppState>) {
    if let Some(call) = state.call.lock().unwrap().as_ref() {
        call.leave();
    }
}

/// `tc-engine permissions`, in a fresh process each time: macOS applies a new grant only to
/// processes started after it. With `request`, asks macOS for what is missing.
#[tauri::command]
async fn permissions(request: bool) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let exe = engine::engine_path()?;
        let args: &[&str] = if request {
            &["permissions", "--request"]
        } else {
            &["permissions"]
        };
        engine::run_once(&exe, args, &[], &[])
    })
    .await
    .map_err(|e| e.to_string())?
}

/// The server the engine uses when nothing is overridden: what the build baked in, or the local
/// dev server. Never the secret.
#[tauri::command]
async fn engine_config() -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let exe = engine::engine_path()?;
        engine::run_once(
            &exe,
            &["config"],
            &[],
            &[settings::URL_ENV, settings::KEY_ENV, settings::SECRET_ENV],
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Pane {
    ScreenRecording,
    Accessibility,
    Microphone,
}

/// Opens the System Settings privacy pane for one permission.
#[tauri::command]
fn open_settings_pane(pane: Pane) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let anchor = match pane {
            Pane::ScreenRecording => "Privacy_ScreenCapture",
            Pane::Accessibility => "Privacy_Accessibility",
            Pane::Microphone => "Privacy_Microphone",
        };
        let url = format!("x-apple.systempreferences:com.apple.preference.security?{anchor}");
        std::process::Command::new("/usr/bin/open")
            .arg(url)
            .status()
            .map_err(|e| format!("System Settings didn't open: {e}."))
            .map(|_| ())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = pane;
        Err("Permissions are only needed on macOS.".into())
    }
}

#[tauri::command]
fn read_settings(app: AppHandle) -> Result<SettingsView, String> {
    Ok(load_settings(&app)?.view())
}

/// Saves overrides. The answer says whether a secret is set, never what it is.
#[tauri::command]
fn save_settings(app: AppHandle, change: SettingsChange) -> Result<SettingsView, String> {
    let path = settings_path(&app)?;
    let mut settings = Settings::load(&path)?;
    settings.apply(change);
    settings.save(&path)?;
    Ok(settings.view())
}

/// `--script PATH --report PATH`: share as the script says, write what happens, maybe quit.
fn run_script(app: AppHandle, script_path: PathBuf, report_path: PathBuf) {
    let fail = |app: &AppHandle, message: String| {
        eprintln!("script: {message}");
        app.exit(2);
    };
    let report = match script::Report::create(&report_path) {
        Ok(report) => Arc::new(report),
        Err(e) => return fail(&app, e),
    };
    report.write(serde_json::json!({ "kind": "app", "pid": std::process::id() }));
    let script = match script::Script::read(&script_path) {
        Ok(script) => script,
        Err(e) => return fail(&app, e),
    };
    let (tx, rx) = mpsc::channel::<Output>();
    let tee = {
        let report = report.clone();
        Arc::new(move |output: Output| {
            report.output(&output);
            let _ = tx.send(output);
        }) as Sink
    };
    // Saved settings are left out: a test talks to the server its environment names, or the
    // local dev server.
    if let Err(e) = begin(&app, script.plan(), Vec::new(), script.hooks(), Some(tee)) {
        report.write(serde_json::json!({ "kind": "script", "ok": false, "error": e }));
        return fail(&app, e);
    }
    std::thread::spawn(move || {
        let send = |command: &ipc::Command| -> Result<(), String> {
            let state = app.state::<AppState>();
            let slot = state.call.lock().unwrap();
            slot.as_ref()
                .ok_or_else(|| "No call is running.".to_owned())?
                .send(command)
        };
        let result = script::play(
            &script.steps,
            &rx,
            &send,
            &|v| report.write(v),
            script::STEP_TIMEOUT,
        );
        report.write(serde_json::json!({
            "kind": "script",
            "done": true,
            "ok": result.is_ok(),
            "error": result.as_ref().err(),
        }));
        if script.quit {
            app.exit(if result.is_ok() { 0 } else { 1 });
        }
    });
}

fn main() {
    let script = script::args_from(std::env::args_os().skip(1));
    let app = tauri::Builder::default()
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            engine_info,
            start_call,
            send_command,
            leave_call,
            permissions,
            engine_config,
            open_settings_pane,
            read_settings,
            save_settings
        ])
        .setup(move |app| {
            if let Some((script_path, report_path)) = script.clone() {
                let handle = app.handle().clone();
                // Give the window a moment to appear: capture leaves out apps with a window.
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_secs(1));
                    run_script(handle, script_path, report_path);
                });
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("starting TwinCursor");
    app.run(|app, event| {
        if let RunEvent::Exit = event {
            // Closing the engine's stdin ends it; the kernel does the same if we die instead.
            if let Some(call) = app.state::<AppState>().call.lock().unwrap().take() {
                call.shutdown();
            }
        }
    });
}
