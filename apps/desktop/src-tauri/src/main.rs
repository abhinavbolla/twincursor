//! The TwinCursor desktop app: a menu bar app with two small windows, the panel (from the menu
//! bar icon) and the call bar (during a call). The call itself runs in the engine, `tc-engine`
//! (inside the bundle), which does everything that touches the screen, the mic and input, and
//! shows the shared screen in its own native window. The webview only shows text and controls
//! (docs/adr/0007-shell-and-engine.md).
//!
//! `calls.rs` runs calls with accounts (sharing, joining by link, letting in, ringing) and owns
//! the engine of every call; `engine.rs` starts and stops the engine and reads its events;
//! `windows.rs` makes the windows and the menu bar icon and decides when they show (with
//! `surfaces.rs`, `native.rs` and `mark.rs`); `settings.rs` keeps where the bar was left;
//! `accounts.rs` signs in and out (`backend.rs` says where); `platform.rs` is what differs
//! between macOS and Windows beyond the windows; `applog.rs` is the log file; `script.rs` is the
//! test-only `--script` mode. This file connects them.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod accounts;
mod applog;
mod backend;
mod calls;
mod engine;
mod link;
mod mark;
mod native;
mod platform;
mod script;
mod seams;
mod settings;
mod snapshot;
mod surfaces;
mod windows;

use std::path::PathBuf;
use std::sync::{mpsc, Arc};

use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager, RunEvent, WebviewWindow};

use accounts::{AccountShell, AccountView};
use calls::{Calls, HomeView, Options, Setup};
use engine::{Output, Quality, Sink, TestHooks};
use seams::{
    EnginePermits, MemoryClipboard, RealEngines, ScriptPermits, SystemClipboard, SystemClock,
};
use surfaces::Hold;
use windows::{HeldPermits, Windows};

/// The webview hears about the call on this event.
const ENGINE_EVENT: &str = "engine";
/// And about accounts and calls on this one: the whole [`HomeView`], whenever it changes.
const HOME_EVENT: &str = "home";
/// A session file for this run instead of the one in the app's config folder (for tests).
const SESSION_FILE_ENV: &str = "TWINCURSOR_SESSION_FILE";

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

/// The page in the calling window is `height` points tall: the window follows (`windows.rs`).
#[tauri::command]
fn fit_window(window: WebviewWindow, windows: tauri::State<'_, Windows>, height: f64) {
    windows.fit(&window, height);
}

/// Escape in the panel.
#[tauri::command]
fn hide_panel(windows: tauri::State<'_, Windows>) {
    windows.hide_panel();
}

/// Quit TwinCursor (the panel's menu): any call is ended or left properly first.
#[tauri::command]
fn quit(windows: tauri::State<'_, Windows>) {
    windows.quit();
}

/// One command for the engine, as the person clicked it. Typed, so only commands the protocol
/// knows get through; an allow names the request the person answered, and is refused if it
/// comes within a second of that request being shown.
#[tauri::command]
async fn send_command(calls: tauri::State<'_, Calls>, command: ipc::Command) -> Result<(), String> {
    calls.send_command(&command).await
}

#[tauri::command]
fn leave_call(calls: tauri::State<'_, Calls>) {
    calls.leave_engine();
}

#[tauri::command]
fn home_state(calls: tauri::State<'_, Calls>) -> HomeView {
    calls.home()
}

/// The current call's engine output, for a window opened mid-call to replay.
#[tauri::command]
fn call_snapshot(calls: tauri::State<'_, Calls>) -> Vec<Output> {
    calls.snapshot()
}

#[tauri::command]
async fn share(calls: tauri::State<'_, Calls>) -> Result<HomeView, String> {
    calls.share().await
}

/// Paste link: the app reads the clipboard itself; the window gets only the outcome.
#[tauri::command]
async fn join_from_clipboard(calls: tauri::State<'_, Calls>) -> Result<HomeView, String> {
    calls.join_from_clipboard().await
}

#[tauri::command]
async fn join_link(calls: tauri::State<'_, Calls>, text: String) -> Result<HomeView, String> {
    calls.join_link(&text).await
}

#[tauri::command]
async fn cancel_knock(calls: tauri::State<'_, Calls>) -> Result<HomeView, String> {
    calls.cancel_knock().await
}

#[tauri::command(rename_all = "snake_case")]
async fn answer_knock(
    calls: tauri::State<'_, Calls>,
    user_id: String,
    let_in: bool,
    control: bool,
) -> Result<HomeView, String> {
    calls.answer_knock(&user_id, let_in, control).await
}

#[tauri::command]
async fn give_control(
    calls: tauri::State<'_, Calls>,
    identity: String,
) -> Result<HomeView, String> {
    calls.give_control(&identity).await
}

#[tauri::command(rename_all = "snake_case")]
async fn ring(calls: tauri::State<'_, Calls>, user_id: String) -> Result<HomeView, String> {
    calls.ring(&user_id).await
}

#[tauri::command(rename_all = "snake_case")]
async fn answer_ring(
    calls: tauri::State<'_, Calls>,
    call_id: String,
    accept: bool,
) -> Result<HomeView, String> {
    calls.answer_ring(&call_id, accept).await
}

/// Ask to be invited, on a server where this person may not start calls.
#[tauri::command]
async fn ask_to_host(calls: tauri::State<'_, Calls>) -> Result<HomeView, String> {
    calls.ask_to_host().await
}

#[tauri::command]
async fn end(calls: tauri::State<'_, Calls>) -> Result<HomeView, String> {
    calls.end().await
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
        engine::run_once(&exe, args)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Opens the System Settings privacy pane for one permission (macOS; elsewhere a sentence).
#[tauri::command]
fn open_settings_pane(pane: platform::Pane) -> Result<(), String> {
    platform::open_settings_pane(pane)
}

/// What this platform can do: the panel offers sharing and calling only where it can share.
#[tauri::command]
fn platform_info() -> platform::PlatformInfo {
    platform::INFO
}

/// The self-hosting guide, which the panel offers when the build names no backend.
const SELF_HOSTING_GUIDE: &str =
    "https://github.com/abhinavbolla/twincursor/blob/main/docs/self-hosting.md";

/// Opens the self-hosting guide with `open`. The address is fixed here, so the window can't
/// make the app open any other page.
fn open_guide(open: impl FnOnce(&str) -> Result<(), String>) -> Result<(), String> {
    open(SELF_HOSTING_GUIDE).map_err(|e| format!("The guide didn't open: {e}."))
}

/// The panel's "self-hosting guide" link, in the default browser. Takes nothing from the window.
#[tauri::command]
fn open_self_hosting_guide() -> Result<(), String> {
    open_guide(accounts::open_browser)
}

#[tauri::command]
async fn account_state(
    account: tauri::State<'_, Arc<AccountShell>>,
) -> Result<AccountView, String> {
    Ok(account.view().await)
}

/// Signs in with GitHub in the default browser. Resolves once signed in, or with a sentence.
/// The panel stays open meanwhile (the browser takes focus) and comes back when it's over.
#[tauri::command]
async fn sign_in(
    account: tauri::State<'_, Arc<AccountShell>>,
    calls: tauri::State<'_, Calls>,
    windows: tauri::State<'_, Windows>,
) -> Result<AccountView, String> {
    windows.hold(Hold::SignIn);
    let view = account.sign_in(accounts::open_browser).await;
    windows.release(Hold::SignIn);
    calls.refresh().await;
    view
}

#[tauri::command]
fn cancel_sign_in(account: tauri::State<'_, Arc<AccountShell>>) {
    account.cancel_sign_in();
}

/// Signs out, ending or leaving any call first.
#[tauri::command]
async fn sign_out(
    account: tauri::State<'_, Arc<AccountShell>>,
    calls: tauri::State<'_, Calls>,
) -> Result<AccountView, String> {
    if calls.busy() {
        let _ = calls.end().await;
    }
    let view = account.sign_out().await;
    calls.refresh().await;
    view
}

/// The account backend this run names, if any, with its session file in the app's config
/// directory (or where `TWINCURSOR_SESSION_FILE` says). A backend that is set up wrongly is
/// reported and left out.
fn account_shell(app: &AppHandle) -> AccountShell {
    let backend = match backend::from_environment() {
        Ok(Some(backend)) => backend,
        Ok(None) => {
            applog::line("backend: none named, so the app isn't connected to a server");
            return AccountShell::none();
        }
        Err(e) => {
            eprintln!("accounts: {e}");
            applog::line(format!("backend: set up wrongly, left out: {e}"));
            return AccountShell::none();
        }
    };
    applog::line(format!("backend: {}", applog::host_of(backend.url())));
    let session_file = match std::env::var_os(SESSION_FILE_ENV).filter(|v| !v.is_empty()) {
        Some(path) => Ok(PathBuf::from(path)),
        None => platform::session_dir(app).map(|dir| dir.join(accounts::SESSION_FILE)),
    };
    if let Ok(file) = &session_file {
        applog::line(format!(
            "session file: {} ({})",
            file.display(),
            if file.exists() { "there" } else { "not yet" }
        ));
    }
    let shell = session_file.and_then(|file| AccountShell::new(backend, file));
    shell.unwrap_or_else(|e| {
        eprintln!("accounts: {e}");
        applog::line(format!("accounts: {e}"));
        AccountShell::none()
    })
}

/// What `--script` mode reads before the app starts.
struct ScriptRun {
    script: script::Script,
    report: Arc<script::Report>,
    outputs: mpsc::Receiver<Output>,
    tee: mpsc::Sender<Output>,
}

fn load_script(
    script_path: &std::path::Path,
    report_path: &std::path::Path,
) -> Result<ScriptRun, String> {
    let report = Arc::new(script::Report::create(report_path)?);
    report.write(serde_json::json!({ "kind": "app", "pid": std::process::id() }));
    let script = script::Script::read(script_path).inspect_err(|e| {
        report.write(serde_json::json!({ "kind": "script", "ok": false, "error": e }));
    })?;
    let (tee, outputs) = mpsc::channel();
    Ok(ScriptRun {
        script,
        report,
        outputs,
        tee,
    })
}

/// What of the engine's output goes to the log: its start, exit and restarts, its own log lines,
/// and the events that say where the call is. Never a command's or the call's secrets: the
/// engine gets those in its environment, which isn't logged.
fn log_engine(output: &Output) {
    match output {
        Output::Started {
            role, codec, pid, ..
        } => applog::line(format!("engine: started ({role:?}, {codec:?}, pid {pid})")),
        Output::Exited { code } => applog::line(format!("engine: exited ({code:?})")),
        Output::Restarting { codec, message } => {
            applog::line(format!("engine: restarting in {codec:?}: {message}"))
        }
        Output::Log { line } => applog::line(format!("engine| {line}")),
        Output::Event { event } => {
            let kind = event["event"].as_str().unwrap_or("");
            if matches!(kind, "hello" | "connection" | "ended" | "error" | "control") {
                applog::line(format!("engine event: {event}"));
            }
        }
    }
}

/// The call flows, wired to the windows (and, in script mode, to the report).
fn make_calls(
    app: &AppHandle,
    windows: &Windows,
    account: Arc<AccountShell>,
    run: Option<&ScriptRun>,
) -> Calls {
    let window = app.clone();
    let surfaces = windows.clone();
    let report = run.map(|r| r.report.clone());
    let tee = run.map(|r| r.tee.clone());
    let on_engine: Sink = {
        let report = report.clone();
        let tee = tee.map(std::sync::Mutex::new);
        Arc::new(move |output: Output| {
            log_engine(&output);
            if let Some(report) = &report {
                report.output(&output);
            }
            if let Some(tee) = &tee {
                let _ = tee.lock().unwrap().send(output.clone());
            }
            let asked =
                matches!(&output, Output::Event { event } if event["event"] == "control_asked");
            let _ = window.emit(ENGINE_EVENT, output);
            if asked {
                surfaces.control_asked();
            }
        })
    };
    let window = app.clone();
    let on_home = {
        let report = report.clone();
        let surfaces = windows.clone();
        Arc::new(move |home: &HomeView| {
            if let Some(report) = &report {
                report.write(serde_json::json!({ "kind": "home", "home": home }));
            }
            let _ = window.emit(HOME_EVENT, home);
            surfaces.on_home(home);
        })
    };
    let trace: Arc<dyn Fn(Value) + Send + Sync> = match report {
        Some(report) => Arc::new(move |v: Value| {
            report.write(serde_json::json!({ "kind": "trace", "trace": v }))
        }),
        None => Arc::new(|_| {}),
    };
    let runtime = tauri::async_runtime::handle().inner().clone();
    let setup = match run {
        // Pattern capture, recorded input, voice off, a clipboard in memory, and the
        // permissions those need, which are none.
        Some(run) => Setup {
            account,
            engines: Arc::new(RealEngines {
                hooks: run.script.hooks(),
            }),
            clipboard: Arc::new(MemoryClipboard::default()),
            permits: Arc::new(ScriptPermits),
            clock: Arc::new(SystemClock),
            options: Options {
                quality: run.script.quality(),
                voice: false,
                can_share: platform::INFO.can_share,
            },
            on_home,
            on_engine,
            trace,
            runtime,
        },
        None => Setup {
            account,
            engines: Arc::new(RealEngines {
                hooks: TestHooks::default(),
            }),
            clipboard: Arc::new(SystemClipboard),
            permits: Arc::new(HeldPermits {
                inner: EnginePermits,
                windows: windows.clone(),
            }),
            clock: Arc::new(SystemClock),
            options: Options {
                quality: Quality::K4,
                voice: true,
                can_share: platform::INFO.can_share,
            },
            on_home,
            on_engine,
            trace,
            runtime,
        },
    };
    Calls::new(setup)
}

/// Plays the script's steps, then maybe quits.
fn run_script(app: AppHandle, run: ScriptRun) {
    let ScriptRun {
        script,
        report,
        outputs,
        ..
    } = run;
    let calls = app.state::<Calls>().inner().clone();
    let windows = app.state::<Windows>().inner().clone();
    // What the app found at start, so a test can check it without a window.
    let info = engine_info();
    report.write(serde_json::json!({
        "kind": "engine_info",
        "found": info.path.is_some(),
        "path": info.path,
        "error": info.error,
        "platform": platform::INFO,
    }));
    let fail = |app: &AppHandle, message: String| {
        eprintln!("script: {message}");
        app.exit(2);
    };
    // Saved settings are left out: a test talks to the server its environment names, or the
    // local dev server.
    if let Some(plan) = script.plan() {
        if let Err(e) = calls.start_plan(plan, Vec::new()) {
            report.write(serde_json::json!({ "kind": "script", "ok": false, "error": e }));
            return fail(&app, e);
        }
    }
    std::thread::spawn(move || {
        let runtime = tauri::async_runtime::handle();
        let send = |command: &ipc::Command| runtime.block_on(calls.send_command(command));
        let home = || serde_json::to_value(calls.home()).unwrap_or(Value::Null);
        let act = |act: script::Act| -> Result<Value, String> {
            let view = runtime.block_on(async {
                match act {
                    script::Act::Share => calls.share().await,
                    script::Act::JoinLink(text) => calls.join_link(&text).await,
                    script::Act::CancelKnock => calls.cancel_knock().await,
                    script::Act::AnswerKnock {
                        user_id,
                        let_in,
                        control,
                    } => calls.answer_knock(&user_id, let_in, control).await,
                    script::Act::GiveControl { name } => match calls.guest_identity(&name) {
                        Some(identity) => calls.give_control(&identity).await,
                        None => Err(calls::NOT_IN_THE_CALL.into()),
                    },
                    script::Act::Ring { user_id } => calls.ring(&user_id).await,
                    script::Act::AnswerRing { call_id, accept } => {
                        calls.answer_ring(&call_id, accept).await
                    }
                    script::Act::End => calls.end().await,
                    script::Act::Show(surface) => {
                        windows.show_unprompted(match surface {
                            script::Surface::Panel => surfaces::Kind::Panel,
                            script::Surface::Bar => surfaces::Kind::Bar,
                        });
                        Ok(calls.home())
                    }
                }
            })?;
            Ok(serde_json::to_value(view).unwrap_or(Value::Null))
        };
        let write = |v: Value| report.write(v);
        let host = script::Host {
            outputs: &outputs,
            send: &send,
            report: &write,
            home: &home,
            act: &act,
            timeout: script::STEP_TIMEOUT,
        };
        let result = script::play(&script.steps, &host);
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

/// Opens the log and writes what a failed run on someone else's computer needs first: which
/// build, where it runs from, the webview, and where the engine is.
fn start_log(script: bool) {
    match applog::init() {
        Some(Ok(path)) => eprintln!("log: {}", path.display()),
        Some(Err(e)) => eprintln!("log: couldn't open {e}"),
        None => {}
    }
    applog::line(format!(
        "TwinCursor {} starting: {} {}, pid {}{}",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
        std::process::id(),
        if script { ", script mode" } else { "" }
    ));
    if let Ok(exe) = std::env::current_exe() {
        applog::line(format!("app: {}", exe.display()));
    }
    applog::line(format!(
        "platform: {} (can share: {})",
        platform::INFO.os,
        platform::INFO.can_share
    ));
    match tauri::webview_version() {
        Ok(v) => applog::line(format!("webview: {v}")),
        Err(e) => applog::line(format!(
            "webview: not found ({e}). On Windows, TwinCursor needs the Microsoft Edge WebView2 Runtime."
        )),
    }
    match engine::engine_path() {
        Ok(path) => applog::line(format!("engine: found at {}", path.display())),
        Err(e) => applog::line(format!("engine: not found: {e}")),
    }
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        applog::line(format!("panic: {info}"));
        default_hook(info);
    }));
}

fn main() {
    let script_args = script::args_from(std::env::args_os().skip(1));
    start_log(script_args.is_some());
    let run = match &script_args {
        Some((script_path, report_path)) => match load_script(script_path, report_path) {
            Ok(run) => Some(run),
            Err(e) => {
                eprintln!("script: {e}");
                std::process::exit(2);
            }
        },
        None => None,
    };
    let run = std::sync::Mutex::new(run);
    let app = tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            engine_info,
            fit_window,
            hide_panel,
            quit,
            send_command,
            leave_call,
            permissions,
            open_settings_pane,
            platform_info,
            open_self_hosting_guide,
            account_state,
            sign_in,
            cancel_sign_in,
            sign_out,
            home_state,
            call_snapshot,
            share,
            join_from_clipboard,
            join_link,
            cancel_knock,
            answer_knock,
            give_control,
            ring,
            answer_ring,
            ask_to_host,
            end
        ])
        .on_menu_event(|app, event| {
            if let Some(windows) = app.try_state::<Windows>() {
                windows.on_app_menu(&event);
            }
        })
        .setup(move |app| {
            let account = Arc::new(account_shell(app.handle()));
            let run = run.lock().unwrap().take();
            // In script mode the windows exist but stay out of sight, unless the script asks.
            let live = run.as_ref().is_none_or(|r| r.script.windows);
            let windows = Windows::create(app.handle(), live)?;
            let calls = make_calls(app.handle(), &windows, account.clone(), run.as_ref());
            windows.attach(calls.clone());
            app.manage(account);
            app.manage(calls.clone());
            app.manage(windows.clone());
            calls.start_polling();
            if run.is_none() {
                // Launching the app opens the panel, as a click on the icon does.
                windows.show_panel(surfaces::ShowWhy::Launch);
            }
            if let Some(run) = run {
                let handle = app.handle().clone();
                // Give the window a moment to appear: capture leaves out apps with a window.
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_secs(1));
                    run_script(handle, run);
                });
            }
            Ok(())
        })
        .build(tauri::generate_context!());
    let app = match app {
        Ok(app) => app,
        Err(e) => {
            // Without the WebView2 Runtime, Tauri has shown its own message by now.
            eprintln!("TwinCursor couldn't start: {e}");
            applog::line(format!("couldn't start: {e}"));
            std::process::exit(1);
        }
    };
    app.run(|app, event| match event {
        // Windows closing never quits a menu bar app; Quit and a finished script do.
        RunEvent::ExitRequested { code, api, .. } => {
            let quitting = app.try_state::<Windows>().is_some_and(|w| w.quitting());
            if !surfaces::allow_exit(code, quitting) {
                api.prevent_exit();
            }
        }
        // The Dock icon (or opening the app again): the panel.
        #[cfg(target_os = "macos")]
        RunEvent::Reopen { .. } => {
            if let Some(windows) = app.try_state::<Windows>() {
                windows.show_panel(surfaces::ShowWhy::Launch);
            }
        }
        RunEvent::Exit => {
            applog::line("exiting");
            // Closing the engine's stdin ends it; the kernel does the same if we die instead.
            if let Some(calls) = app.try_state::<Calls>() {
                calls.shutdown();
            }
        }
        _ => {}
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_guide_command_opens_exactly_the_guide() {
        let mut opened = Vec::new();
        open_guide(|url| {
            opened.push(url.to_owned());
            Ok(())
        })
        .unwrap();
        assert_eq!(
            opened,
            ["https://github.com/abhinavbolla/twincursor/blob/main/docs/self-hosting.md"]
        );
        assert_eq!(
            open_guide(|_| Err("open failed".into())).unwrap_err(),
            "The guide didn't open: open failed."
        );
    }
}
