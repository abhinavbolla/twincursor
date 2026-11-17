//! `tc-engine`: the call engine the desktop app starts. `share` and `view` speak the
//! `twincursor-ipc` protocol with `--ipc`: commands as JSON lines on stdin, events as JSON lines
//! on stdout, logs on stderr. The engine exits when its stdin closes, so it never outlives the
//! app. `permissions` and `devices` print one JSON object.
//!
//! `tc-spike` is the same engine as a development tool, with its terminal prompt and its
//! measurement flags; this binary carries only what the app needs.

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use anyhow::Context;
use clap::{Args, Parser, Subcommand, ValueEnum};
use engine::app::{self, Events};
use engine::sharer::{Answer, Consent, Inject, Sharer};
use engine::{hotkey, view};
use ipc::{Command, ConnectionState, EndReason, Event, Role};
use livekit::options::VideoCodec;
use media::audio::{AudioMode, DeviceChoice, DeviceRef, VoiceOptions, VoiceState};
use media::capture::{self, select, Backend, CaptureConfig, Source, Tier};
use media::publish::{self, AdaptConfig, ShareConfig, ShareStats};
use media::subscribe::ViewConfig;
use media::token;

const URL_ENV: &str = "TWINCURSOR_SERVER_URL";
const KEY_ENV: &str = "TWINCURSOR_API_KEY";
const SECRET_ENV: &str = "TWINCURSOR_API_SECRET";

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Share this Mac's screen.
    Share(ShareArgs),
    /// Watch the sharer's screen in a window.
    View(ViewArgs),
    /// Print this process's Screen Recording, Accessibility and Microphone access as JSON.
    Permissions {
        /// Ask macOS for the access not yet given.
        #[arg(long)]
        request: bool,
    },
    /// Print the microphones and speakers as JSON.
    Devices,
    /// Print the windows `share --window` can share as JSON: id, app, pid, title, pixels.
    Windows,
}

/// Where the call is, and who this person says they are.
#[derive(Args)]
struct Call {
    /// The server's URL [env: TWINCURSOR_SERVER_URL; default: the local dev server].
    #[arg(long)]
    url: Option<String>,
    /// The server's API key [env: TWINCURSOR_API_KEY; default: the dev server's].
    #[arg(long)]
    api_key: Option<String>,
    /// The server's API secret [env: TWINCURSOR_API_SECRET; default: the dev server's]. Prefer
    /// the environment: other users of this Mac can read a process's flags.
    #[arg(long)]
    api_secret: Option<String>,
    /// The room to join.
    #[arg(long)]
    room: String,
    /// The name the others see. They can't check it, and neither can we.
    #[arg(long)]
    name: String,
    /// Speak the app's protocol on stdin and stdout, and exit when stdin closes.
    #[arg(long)]
    ipc: bool,
    #[arg(long, value_enum, default_value = "mic")]
    audio: Audio,
    /// The microphone, by number or part of its name (see `devices`). Default: the system's.
    #[arg(long)]
    mic: Option<DeviceRef>,
    /// The speakers or headphones, as for --mic.
    #[arg(long)]
    speaker: Option<DeviceRef>,
}

impl Call {
    fn setting(flag: &Option<String>, env: &str, default: &str) -> String {
        flag.clone()
            .or_else(|| std::env::var(env).ok().filter(|v| !v.is_empty()))
            .unwrap_or_else(|| default.to_owned())
    }

    fn url(&self) -> String {
        Self::setting(&self.url, URL_ENV, token::DEV_URL)
    }

    /// A token for a fresh identity with `role`'s prefix. The secret goes into the token's
    /// signature and nowhere else.
    fn token(&self, role: Role) -> anyhow::Result<(String, String)> {
        let identity = app::new_identity(role);
        let key = Self::setting(&self.api_key, KEY_ENV, token::DEV_API_KEY);
        let secret = Self::setting(&self.api_secret, SECRET_ENV, token::DEV_API_SECRET);
        let jwt = token::mint(&key, &secret, &self.room, &identity, &self.name)
            .context("couldn't make an access token")?;
        Ok((identity, jwt))
    }

    fn audio(&self) -> anyhow::Result<AudioMode> {
        Ok(match self.audio {
            Audio::Off => {
                anyhow::ensure!(
                    self.mic.is_none() && self.speaker.is_none(),
                    "--mic and --speaker need --audio mic"
                );
                AudioMode::Off
            }
            Audio::Mic => {
                engine::permission::check_mic_permission()?;
                AudioMode::Mic(DeviceChoice {
                    mic: self.mic.clone(),
                    speaker: self.speaker.clone(),
                })
            }
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Audio {
    /// Your mic, and the other side's voice on your speakers.
    Mic,
    Off,
}

#[derive(Clone, Copy, ValueEnum)]
enum Codec {
    H264,
    /// HEVC: hardware on macOS, for 4K (docs/adr/0008).
    H265,
}

#[derive(Clone, Copy, ValueEnum)]
enum Resolution {
    #[value(name = "720")]
    P720,
    #[value(name = "1080")]
    P1080,
    #[value(name = "1440")]
    P1440,
    #[value(name = "2160")]
    P2160,
    Native,
}

#[derive(Clone, Copy, ValueEnum)]
enum CaptureSource {
    /// The main display (ScreenCaptureKit).
    Screen,
    /// A synthetic picture, for tests: needs no Screen Recording permission.
    Pattern,
}

#[derive(Args)]
struct ShareArgs {
    #[command(flatten)]
    call: Call,
    #[arg(long, value_enum, default_value = "1080")]
    resolution: Resolution,
    #[arg(long, value_enum, default_value = "h264")]
    codec: Codec,
    /// Share one window: its id, or text its title or app name contains (see `windows`).
    #[arg(long, conflicts_with = "app")]
    window: Option<String>,
    /// Share one app: the display with only this app's windows on it (name or bundle id).
    #[arg(long)]
    app: Option<String>,
    /// Let guests ask for control. Each request goes to the app as `control_asked`, and nothing
    /// is granted until it answers `allow_control`. Without this, requests are turned down.
    #[arg(long)]
    allow_control: bool,
    /// What granted input does: `real` input (macOS, needs Accessibility), or `log` it, or
    /// `record:PATH` it as JSON lines, for tests.
    #[arg(long, default_value = "real", requires = "allow_control")]
    inject: Inject,
    /// Leave this app's windows out of the capture, by process id (the desktop app).
    #[arg(long)]
    exclude_app_pid: Option<u32>,
    #[arg(long, value_enum, default_value = "screen", hide = true)]
    capture: CaptureSource,
    /// Stamp the latency markers into the top left of the picture (for measuring only).
    #[arg(long, hide = true)]
    markers: bool,
    /// No label and no hotkeys, for machines without a display. Only with --inject log or
    /// record: real control always shows the sharer who is in control.
    #[arg(long, hide = true)]
    headless: bool,
}

#[derive(Args)]
struct ViewArgs {
    #[command(flatten)]
    call: Call,
    /// Decode without a window, for tests.
    #[arg(long, hide = true)]
    headless: bool,
    /// Play a remote-control script (see bench/control/), for tests. Needs --headless.
    #[arg(long, hide = true, requires = "headless")]
    control_script: Option<std::path::PathBuf>,
    /// Write every control message sent as a line of JSON, for tests.
    #[arg(long, hide = true)]
    sent_log: Option<std::path::PathBuf>,
}

fn main() -> anyhow::Result<()> {
    // Logs go to stderr: stdout carries only events.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,livekit=warn,libwebrtc=warn".into()),
        )
        .init();
    match Cli::parse().command {
        Cmd::Share(args) => share(args),
        Cmd::View(args) => watch(args),
        Cmd::Permissions { request } => {
            if request {
                engine::permission::request_missing(&engine::permission::status());
            }
            print_line(&serde_json::to_string(&engine::permission::status())?);
            Ok(())
        }
        Cmd::Windows => {
            let (windows, _) = capture::shareable().context("listing windows")?;
            print_line(&serde_json::to_string(&windows)?);
            Ok(())
        }
        Cmd::Devices => {
            let devices = media::audio::device::list()?;
            let json = serde_json::json!({"mics": devices.mics, "speakers": devices.speakers});
            print_line(&json.to_string());
            Ok(())
        }
    }
}

/// Prints and flushes one line. A closed pipe means nobody is listening, which is no reason to
/// panic.
fn print_line(line: &str) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

/// With `--ipc`, events are lines on stdout; without, they are logged.
fn events(ipc: bool) -> Events {
    let lock = Arc::new(Mutex::new(()));
    Arc::new(move |event: Event| {
        let line = ipc::encode(&event);
        if ipc {
            let _guard = lock.lock().unwrap();
            print_line(&line);
        } else {
            tracing::info!("event {line}");
        }
    })
}

/// Reads the app's commands from stdin, and exits when it closes after `on_close`.
fn read_commands(
    events: Events,
    mut on_command: impl FnMut(Command) + Send + 'static,
    on_close: impl FnOnce() + Send + 'static,
) {
    app::watch_stdin(
        move |line| match ipc::decode_command(line) {
            None => {}
            Some(Ok(command)) => on_command(command),
            Some(Err(message)) => events(Event::Error { message }),
        },
        on_close,
    );
}

fn hello(events: &Events, role: Role) {
    events(Event::Hello {
        version: ipc::VERSION,
        role,
        engine: env!("CARGO_PKG_VERSION").to_owned(),
    });
}

fn ended(events: &Events, left: bool, error: Option<String>) {
    events(Event::Connection {
        state: ConnectionState::Disconnected,
    });
    let reason = match error {
        Some(message) => {
            events(Event::Error { message });
            EndReason::Failed
        }
        None if left => EndReason::Left,
        None => EndReason::Disconnected,
    };
    events(Event::Ended { reason });
}

/// Fails before the call starts: tell the app why, then end.
fn failed_early(events: &Events, error: anyhow::Error) -> anyhow::Error {
    events(Event::Error {
        message: format!("{error:#}."),
    });
    events(Event::Ended {
        reason: EndReason::Failed,
    });
    error
}

fn share(args: ShareArgs) -> anyhow::Result<()> {
    let events = events(args.call.ipc);
    hello(&events, Role::Sharer);
    let setup = share_config(&args).map_err(|e| failed_early(&events, e))?;
    let (config, voice) = setup;
    let revoke = control::RevokeSwitch::default();
    if args.headless {
        anyhow::ensure!(
            !matches!(args.inject, Inject::Real) || !args.allow_control,
            "--headless only works with --inject log or record: real control needs the label"
        );
    }
    // Without --allow-control nothing can be granted, so nothing is ever injected.
    let inject = if args.allow_control {
        args.inject.clone()
    } else {
        Inject::Log
    };
    let ui = if args.headless {
        None
    } else {
        let mut ui = hotkey::SharerUi::new()?;
        // The voice reporter tells the app; nothing may print to stdout.
        ui.on_mute(Arc::new(|_| {}));
        Some(ui)
    };
    let consent = Consent::App {
        events: events.clone(),
        allow: args.allow_control,
    };
    let (sharer, link) = Sharer::start(
        &inject,
        revoke.clone(),
        ui.as_ref().map(|ui| ui.overlay()),
        consent,
    )?;

    let left = Arc::new(AtomicBool::new(false));
    let leave = Arc::new(tokio::sync::Notify::new());
    if args.call.ipc {
        let handle = sharer.handle();
        let (events, voice, left, leave, revoke) = (
            events.clone(),
            voice.clone(),
            left.clone(),
            leave.clone(),
            revoke.clone(),
        );
        let on_close = {
            let handle = handle.clone();
            move || {
                // Take control back and release anything held before going.
                handle.stop();
                std::thread::sleep(Duration::from_millis(100));
            }
        };
        read_commands(
            events.clone(),
            move |command| match command {
                Command::Mute => voice.set_muted(true),
                Command::Unmute => voice.set_muted(false),
                Command::Leave => {
                    left.store(true, Ordering::Release);
                    leave.notify_one();
                }
                Command::AllowControl { request } => handle.answer(Answer::Allow { request }),
                Command::DenyControl { request } => handle.answer(Answer::Deny { request }),
                // The same switch the hotkey trips, so both take the same path.
                Command::RevokeControl => revoke.trip(),
                Command::RequestControl | Command::ReleaseControl => events(Event::Error {
                    message: "Only a guest can ask for control.".into(),
                }),
                Command::SetMic { device } => voice.choose_mic(device.map(device_ref)),
                Command::SetSpeaker { device } => voice.choose_speaker(device.map(device_ref)),
            },
            on_close,
        );
    }

    let mic = matches!(config.audio, AudioMode::Mic(_)).then(|| voice.clone());
    let network = {
        let (events, voice) = (events.clone(), voice.clone());
        move || -> Option<String> {
            let runtime = match tokio::runtime::Runtime::new() {
                Ok(runtime) => runtime,
                Err(e) => return Some(format!("The engine couldn't start: {e}.")),
            };
            runtime.block_on(async {
                let stats = Arc::new(ShareStats::default());
                let reporter = tokio::spawn(report(events.clone(), voice.clone(), stats.clone()));
                let shutdown = async {
                    tokio::select! {
                        _ = tokio::signal::ctrl_c() => {}
                        _ = leave.notified() => {}
                    }
                };
                events(Event::Connection {
                    state: ConnectionState::Connecting,
                });
                let result = publish::run(config, stats, voice, Some(link), shutdown).await;
                reporter.abort();
                result.err().map(|e| format!("Sharing failed: {e}."))
            })
        }
    };
    let error = match ui {
        None => network(),
        Some(ui) => {
            let revoke = args.allow_control.then_some(revoke);
            match ui.run(revoke, mic, network) {
                Ok(error) => error,
                Err(e) => Some(format!("The sharer's hotkeys couldn't start: {e}.")),
            }
        }
    };
    sharer.stop();
    let failed = error.is_some();
    ended(&events, left.load(Ordering::Acquire), error);
    if failed {
        std::process::exit(1);
    }
    Ok(())
}

fn device_ref(name: String) -> DeviceRef {
    // Infallible: anything that isn't a number is a name.
    name.parse().unwrap_or(DeviceRef::Name(name))
}

fn share_config(args: &ShareArgs) -> anyhow::Result<(ShareConfig, Arc<VoiceState>)> {
    anyhow::ensure!(
        !(args.allow_control && (args.window.is_some() || args.app.is_some())),
        "{}",
        select::CONTROL_NEEDS_DISPLAY
    );
    let (source, source_pixels) = if args.window.is_none() && args.app.is_none() {
        (Source::Display, None)
    } else {
        anyhow::ensure!(
            matches!(args.capture, CaptureSource::Screen),
            "--window and --app need screen capture"
        );
        let (windows, apps) = capture::shareable().context("listing windows")?;
        select::resolve(&windows, &apps, args.window.as_deref(), args.app.as_deref())
            .map_err(anyhow::Error::msg)?
    };
    let backend = match args.capture {
        CaptureSource::Pattern => Backend::Pattern,
        #[cfg(target_os = "macos")]
        CaptureSource::Screen => Backend::ScreenCaptureKit,
        #[cfg(not(target_os = "macos"))]
        CaptureSource::Screen => anyhow::bail!("screen capture is macOS only for now"),
    };
    let tier = match args.resolution {
        Resolution::P720 => Tier::P720,
        Resolution::P1080 => Tier::P1080,
        Resolution::P1440 => Tier::P1440,
        Resolution::P2160 => Tier::P2160,
        Resolution::Native => Tier::Native,
    };
    let screen = match backend {
        Backend::Pattern => Some(capture::PATTERN_DISPLAY),
        _ => source_pixels.or_else(capture::display_pixels),
    }
    .unwrap_or(capture::FALLBACK_DISPLAY);
    let (width, height) = tier.size(screen);
    let (_, token) = args.call.token(Role::Sharer)?;
    let config = ShareConfig {
        url: args.call.url(),
        token,
        backend,
        capture: CaptureConfig {
            source,
            width,
            height,
            max_fps: 60,
            // A guest in control needs to see where the pointer is.
            show_cursor: args.allow_control,
            exclude_app_pid: args.exclude_app_pid,
            leave_out_twincursor: true,
            // A shared window's stream follows it when it's resized.
            follow_tier: Some(tier),
        },
        codec: match args.codec {
            Codec::H264 => VideoCodec::H264,
            Codec::H265 => VideoCodec::H265,
        },
        max_bitrate: tier.default_bitrate_kbps() * 1000,
        is_screencast: true,
        stamp_markers: args.markers,
        net_stats: true,
        adapt: Some(AdaptConfig {
            max: tier,
            display: screen,
        }),
        audio: args.call.audio()?,
        voice: VoiceOptions::default(),
        snapshots: None,
    };
    tracing::info!(
        "display {}x{} → sending {width}x{height}, {:?}",
        screen.0,
        screen.1,
        config.codec
    );
    Ok((config, Arc::new(VoiceState::default())))
}

/// The voice when it changes, and the video sent about once a second.
async fn report(events: Events, voice: Arc<VoiceState>, stats: Arc<ShareStats>) {
    let mut interval = tokio::time::interval(Duration::from_millis(250));
    let mut last_voice = None;
    let mut ticks = 0u32;
    loop {
        interval.tick().await;
        let status = voice.status();
        let now = (status.sending, status.muted, status.mic, status.speaker);
        if last_voice.as_ref() != Some(&now) {
            last_voice = Some(now.clone());
            let (sending, muted, mic, speaker) = now;
            events(Event::Voice {
                sending,
                muted,
                mic,
                speaker,
            });
        }
        if let Some(message) = voice.take_device_error() {
            events(Event::Error { message });
        }
        ticks += 1;
        if !ticks.is_multiple_of(4) {
            continue;
        }
        let event = {
            let net = stats.net.lock().unwrap();
            net.as_ref().and_then(|net| {
                let send = net.send.as_ref()?;
                Some(Event::Stats {
                    width: send.width,
                    height: send.height,
                    fps: send.fps,
                    kbps: send.kbps,
                    rtt_ms: net.rtt_ms,
                    codec: app::codec_name(&send.codec),
                    shown_pct: None,
                })
            })
        };
        if let Some(event) = event {
            events(event);
        }
    }
}

fn watch(args: ViewArgs) -> anyhow::Result<()> {
    let events = events(args.call.ipc);
    hello(&events, Role::Guest);
    let setup = (|| -> anyhow::Result<_> {
        let (_, token) = args.call.token(Role::Guest)?;
        Ok(ViewConfig {
            url: args.call.url(),
            token,
            tick_interval: Duration::from_millis(247),
            net_stats: true,
            measure_quality: false,
            measure_input: false,
            measure_audio: false,
            snapshots: None,
            audio: args.call.audio()?,
            voice: VoiceOptions::default(),
        })
    })();
    let config = setup.map_err(|e| failed_early(&events, e))?;
    let (tx, rx) = mpsc::channel();
    if args.call.ipc {
        // The sender lives in the stdin thread; the viewer reads until it is gone.
        read_commands(
            events.clone(),
            move |command| {
                let _ = tx.send(command);
            },
            || {},
        );
    } else {
        drop(tx);
    }
    events(Event::Connection {
        state: ConnectionState::Connecting,
    });
    let result = view::run(view::ViewOptions {
        config,
        voice: Arc::new(VoiceState::default()),
        headless: args.headless,
        duration: None,
        label: String::new(),
        out: None,
        control_script: args.control_script,
        sent_log: args.sent_log,
        name: args.call.name.clone(),
        app: Some(view::ViewApp {
            events: events.clone(),
            commands: rx,
        }),
    });
    // The viewer has already told the app how the call ended.
    if result.is_err() {
        std::process::exit(1);
    }
    Ok(())
}
