//! `tc-engine`: the call engine the desktop app starts. `share` and `view` speak the
//! `twincursor-ipc` protocol with `--ipc`: commands as JSON lines on stdin, events as JSON lines
//! on stdout, logs on stderr. The engine exits when its stdin closes, so it never outlives the
//! app. `permissions` and `devices` print one JSON object.
//!
//! The server's URL comes from `--url`, else the environment, else what the build baked in
//! (`build.rs`), else the local dev server. A call gets into its room one of two ways:
//!
//! - **With a call pass** in `TWINCURSOR_PASS`: an access token the account server signed for one
//!   person in one room (see `pass.rs`). The pass names the room, the identity and the name, so
//!   `--room` and `--name` aren't needed, and no API key or secret is read.
//! - **With an API key and secret** (flags, else the environment, else the build's, else the dev
//!   server's): the engine mints its own token for `--room` and `--name`.
//!
//! `config` prints which server and mode a call would use, never the secret or the pass.
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
use engine::sharer::{Answer, Consent, Inject, Sharer, Surface};
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
/// A call pass. Only ever from the environment: other users of this Mac can read a process's
/// flags, and the pass lets whoever holds it into the call.
const PASS_ENV: &str = "TWINCURSOR_PASS";

mod pass;

/// The server the build environment named, if any (see `build.rs`).
mod baked {
    include!(concat!(env!("OUT_DIR"), "/baked.rs"));
}

/// Where a setting came from, strongest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Origin {
    Flag,
    Env,
    Build,
    Dev,
}

impl Origin {
    fn name(self) -> &'static str {
        match self {
            Origin::Flag => "flag",
            Origin::Env => "env",
            Origin::Build => "build",
            Origin::Dev => "dev",
        }
    }
}

/// A flag beats the environment, which beats what the build baked in, which beats the local dev
/// server. Empty values count as unset.
fn resolve(
    flag: Option<&str>,
    env: Option<String>,
    baked: Option<&str>,
    dev: &str,
) -> (String, Origin) {
    let set = |v: &str| !v.trim().is_empty();
    if let Some(v) = flag.filter(|v| set(v)) {
        return (v.to_owned(), Origin::Flag);
    }
    if let Some(v) = env.filter(|v| set(v)) {
        return (v, Origin::Env);
    }
    if let Some(v) = baked.filter(|v| set(v)) {
        return (v.to_owned(), Origin::Build);
    }
    (dev.to_owned(), Origin::Dev)
}

fn env_var(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

/// The call pass in the environment, if there is one. Empty counts as unset.
fn pass_from_env() -> Option<String> {
    env_var(PASS_ENV).filter(|v| !v.trim().is_empty())
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// The server's URL, API key and API secret, each from wherever it comes.
struct Server {
    url: (String, Origin),
    api_key: (String, Origin),
    api_secret: (String, Origin),
}

impl Server {
    fn resolve(url: Option<&str>, api_key: Option<&str>, api_secret: Option<&str>) -> Self {
        Server {
            url: resolve(url, env_var(URL_ENV), baked::URL, token::DEV_URL),
            api_key: resolve(
                api_key,
                env_var(KEY_ENV),
                baked::API_KEY,
                token::DEV_API_KEY,
            ),
            api_secret: resolve(
                api_secret,
                env_var(SECRET_ENV),
                baked::API_SECRET,
                token::DEV_API_SECRET,
            ),
        }
    }

    /// What `config` prints. Never the secret: only whether one other than the dev server's is
    /// in effect. Never the pass: only whether one is, in which case a call uses it and not the
    /// key and secret.
    fn describe(&self, has_pass: bool) -> serde_json::Value {
        serde_json::json!({
            "url": self.url.0,
            "api_key": self.api_key.0,
            "has_secret": self.api_secret.1 != Origin::Dev,
            "has_pass": has_pass,
            "source": self.url.1.name(),
        })
    }
}

#[derive(Parser)]
#[command(
    version,
    about,
    after_help = "A call joins with the call pass in TWINCURSOR_PASS if there is one (then --room and \
--name come from the pass), else with an API key and secret, minting its own token."
)]
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
        /// Ask macOS for the access not yet given: all of it, or only the one named.
        #[arg(long, value_name = "PERMISSION", num_args = 0..=1)]
        request: Option<Option<Permission>>,
    },
    /// Print the microphones and speakers as JSON.
    Devices,
    /// Print the windows `share --window` can share as JSON: id, app, pid, title, pixels.
    Windows,
    /// Print the displays as JSON: id (what `share --display` takes), main, capturable, their
    /// place in points and their pixels. `share` shares the main one unless told otherwise.
    Displays,
    /// Print the server a call would use, as JSON: url, api_key, has_secret, has_pass and
    /// source (`env`, `build` or `dev`). Never prints the secret or the pass.
    Config,
    /// Print a call pass minted with this engine's key and secret, for testing pass mode against
    /// a development server. Refused by builds that carry a server's key.
    #[command(hide = true)]
    DevPass(DevPassArgs),
}

#[derive(Args)]
struct DevPassArgs {
    #[arg(long)]
    room: String,
    #[arg(long, value_enum)]
    role: PassRole,
    #[arg(long)]
    name: String,
    /// The identity after the role's prefix. Default: a random UUID-like value.
    #[arg(long)]
    id: Option<String>,
    /// How long the pass is valid, in seconds.
    #[arg(long, default_value_t = 3600)]
    ttl: u64,
}

/// One permission for `permissions --request`, named as `permissions` prints it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Permission {
    #[value(name = "screen_recording")]
    ScreenRecording,
    #[value(name = "accessibility")]
    Accessibility,
    #[value(name = "microphone")]
    Microphone,
}

impl Permission {
    fn which(self) -> engine::permission::Which {
        use engine::permission::Which;
        match self {
            Permission::ScreenRecording => Which::ScreenRecording,
            Permission::Accessibility => Which::Accessibility,
            Permission::Microphone => Which::Microphone,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum PassRole {
    Sharer,
    Guest,
}

/// Where the call is, and who this person is.
#[derive(Args)]
struct Call {
    /// The server's URL [env: TWINCURSOR_SERVER_URL; default: the build's, else the local dev
    /// server].
    #[arg(long)]
    url: Option<String>,
    /// The server's API key [env: TWINCURSOR_API_KEY; default: the build's, else the dev
    /// server's]. Not with a call pass.
    #[arg(long)]
    api_key: Option<String>,
    /// The server's API secret [env: TWINCURSOR_API_SECRET; default: the build's, else the dev
    /// server's]. Prefer the environment: other users of this Mac can read a process's flags.
    /// Not with a call pass.
    #[arg(long)]
    api_secret: Option<String>,
    /// The room to join. Required without a call pass; with one, it must be the pass's room.
    #[arg(long)]
    room: Option<String>,
    /// The name the others see, without a call pass (required then). They can't check it, and
    /// neither can we. A call pass carries its account's name, which wins.
    #[arg(long)]
    name: Option<String>,
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
    fn server(&self) -> Server {
        Server::resolve(
            self.url.as_deref(),
            self.api_key.as_deref(),
            self.api_secret.as_deref(),
        )
    }

    fn url(&self) -> String {
        self.server().url.0
    }

    /// How `role` gets into the room: with the call pass in the environment, else with a token
    /// minted for `--room` and `--name`.
    fn join(&self, role: Role) -> anyhow::Result<Joining> {
        self.join_with(role, pass_from_env(), now_secs())
    }

    fn join_with(&self, role: Role, pass: Option<String>, now: u64) -> anyhow::Result<Joining> {
        let Some(jwt) = pass else {
            return self.mint(role);
        };
        // Only flags: a key and secret in the environment or the build are simply not used.
        anyhow::ensure!(
            self.api_key.is_none() && self.api_secret.is_none(),
            "--api-key and --api-secret can't be used with a call pass ({PASS_ENV})"
        );
        let pass = pass::Pass::decode(&jwt, now)?;
        pass.check_role(role)?;
        if let Some(room) = &self.room {
            anyhow::ensure!(
                *room == pass.room,
                "This call pass is for room `{}`, not `{room}`",
                pass.room
            );
        }
        if self.name.as_ref().is_some_and(|name| *name != pass.name) {
            tracing::info!("using the call pass's name, not --name");
        }
        Ok(Joining {
            identity: pass.identity,
            name: pass.name,
            token: jwt.trim().to_owned(),
        })
    }

    /// A token for a fresh identity with `role`'s prefix. The secret goes into the token's
    /// signature and nowhere else.
    fn mint(&self, role: Role) -> anyhow::Result<Joining> {
        // `main` has made clap require both without a pass; this is only a backstop.
        let (Some(room), Some(name)) = (&self.room, &self.name) else {
            anyhow::bail!("--room and --name are needed without a call pass ({PASS_ENV})");
        };
        let identity = app::new_identity(role);
        let server = self.server();
        let token = token::mint(
            &server.api_key.0,
            &server.api_secret.0,
            room,
            &identity,
            name,
        )
        .context("couldn't make an access token")?;
        Ok(Joining {
            identity,
            name: name.clone(),
            token,
        })
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

/// Who joins, as whom, with which token. The token is a credential: `Debug` leaves it out.
struct Joining {
    identity: String,
    name: String,
    token: String,
}

impl std::fmt::Debug for Joining {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Joining")
            .field("identity", &self.identity)
            .field("name", &self.name)
            .finish_non_exhaustive()
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
    /// A display (ScreenCaptureKit): `--display`, else the main one.
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
    /// The display to share, by id (see `displays`). Default: the main display, the one with
    /// the menu bar. Remote control goes to the same display.
    #[arg(long)]
    display: Option<u32>,
    /// Let guests ask for control. Each request goes to the app as `control_asked`, and nothing
    /// is granted until it answers `allow_control`. Without this, requests are turned down.
    /// With --window or --app every request is turned down, with a reason: input can't follow
    /// one window yet.
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
    /// Announce only these decoders to the sharer (comma-separated, such as `h264,vp8`), as a
    /// guest that can't decode the others would. For tests.
    #[arg(long, hide = true, value_delimiter = ',')]
    decoders: Option<Vec<String>>,
    /// Claim this name in the control hello instead of the one the call joins with, as a
    /// guest's own app could. The sharer must show the token's name anyway. For tests.
    #[arg(long, hide = true)]
    hello_name: Option<String>,
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
    match parse().command {
        Cmd::Share(args) => share(args),
        Cmd::View(args) => watch(args),
        Cmd::Permissions { request } => {
            let before = engine::permission::status();
            match request {
                None => {}
                Some(None) => engine::permission::request_missing(&before),
                Some(Some(one)) => engine::permission::request_one(one.which(), &before),
            }
            print_line(&serde_json::to_string(&engine::permission::status())?);
            Ok(())
        }
        Cmd::Windows => {
            let (windows, _) = capture::shareable().context("listing windows")?;
            print_line(&serde_json::to_string(&windows)?);
            Ok(())
        }
        Cmd::Displays => {
            print_line(&serde_json::to_string(&engine::screen::list())?);
            Ok(())
        }
        Cmd::Config => {
            let server = Server::resolve(None, None, None);
            print_line(&server.describe(pass_from_env().is_some()).to_string());
            Ok(())
        }
        Cmd::DevPass(args) => {
            print_line(&dev_pass(&Server::resolve(None, None, None), &args)?);
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

/// Parses the command line. Without a call pass, `share` and `view` need `--room` and `--name`,
/// and clap says so as it would for any required flag.
fn parse() -> Cli {
    use clap::FromArgMatches;
    let command = cli_command(pass_from_env().is_some());
    let mut matches = command.get_matches();
    Cli::from_arg_matches_mut(&mut matches).unwrap_or_else(|e| e.exit())
}

fn cli_command(has_pass: bool) -> clap::Command {
    use clap::CommandFactory;
    let command = Cli::command();
    if has_pass {
        return command;
    }
    let required = |c: clap::Command| {
        c.mut_arg("room", |a| a.required(true))
            .mut_arg("name", |a| a.required(true))
    };
    command
        .mut_subcommand("share", required)
        .mut_subcommand("view", required)
}

/// A call pass for `args`, signed with `server`'s key and secret, for testing pass mode. A build
/// that carries a server's key must not hand out passes for it, so this needs a key and secret
/// from the environment or the dev server's.
fn dev_pass(server: &Server, args: &DevPassArgs) -> anyhow::Result<String> {
    anyhow::ensure!(
        server.api_key.1 != Origin::Build && server.api_secret.1 != Origin::Build,
        "dev-pass only signs with a key and secret from the environment or the dev server's, \
         not ones built into this engine"
    );
    let id = args.id.clone().unwrap_or_else(random_id);
    anyhow::ensure!(
        !id.trim().is_empty() && !id.chars().any(char::is_control),
        "--id can't be empty or hold control characters"
    );
    let (prefix, role) = match args.role {
        PassRole::Sharer => ("sharer", Role::Sharer),
        PassRole::Guest => ("guest", Role::Guest),
    };
    let jwt = token::mint_with_ttl(
        &server.api_key.0,
        &server.api_secret.0,
        &args.room,
        &format!("{prefix}-{id}"),
        &args.name,
        Duration::from_secs(args.ttl),
    )
    .context("couldn't make a call pass")?;
    // What a call would make of it, so a pass this prints is one `share` or `view` takes.
    pass::Pass::decode(&jwt, now_secs())?.check_role(role)?;
    Ok(jwt)
}

/// A random value shaped like a UUID (8-4-4-4-12 hex digits), as account ids are.
fn random_id() -> String {
    use std::hash::{BuildHasher, Hasher};
    let half = |salt: u64| {
        let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
        hasher.write_u64(salt);
        hasher.write_u32(std::process::id());
        hasher.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos()),
        );
        hasher.finish()
    };
    let hex = format!("{:016x}{:016x}", half(1), half(2));
    format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    )
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
    events(Event::Connection {
        state: ConnectionState::Connecting,
    });
    let setup = share_config(&args).map_err(|e| failed_early(&events, e))?;
    let (config, voice, display) = setup;
    let surface = if config.capture.source.is_whole_display() {
        Surface::Display(display)
    } else {
        Surface::Window
    };
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
        let mut ui = hotkey::SharerUi::new(display)?;
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
        surface,
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
                Command::TrustControl { identity } => handle.trust(identity),
                Command::TrustOnArrival { identity } => handle.trust_on_arrival(identity),
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

/// The share's settings, its voice, and its display ([`engine::screen::choose`]): the one the
/// capture shows and the one control input goes to.
fn share_config(args: &ShareArgs) -> anyhow::Result<(ShareConfig, Arc<VoiceState>, u32)> {
    // First, so a pass that doesn't fit fails before anything else is set up.
    let token = args.call.join(Role::Sharer)?.token;
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
    let display = engine::screen::choose(backend, args.display).map_err(anyhow::Error::msg)?;
    let screen = match backend {
        Backend::Pattern => Some(capture::PATTERN_DISPLAY),
        _ => source_pixels.or_else(|| capture::display_pixels(display.id)),
    }
    .unwrap_or(capture::FALLBACK_DISPLAY);
    let (width, height) = tier.size(screen);
    let config = ShareConfig {
        url: args.call.url(),
        token,
        backend,
        source: screen,
        fit_viewport: true,
        capture: CaptureConfig {
            source,
            display: Some(display.id),
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
        adapt: Some(AdaptConfig { max: tier }),
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
    Ok((config, Arc::new(VoiceState::default()), display.id))
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
    if let Some(decoders) = &args.decoders {
        app::announce_only(decoders).map_err(|e| failed_early(&events, anyhow::anyhow!(e)))?;
    }
    let setup = (|| -> anyhow::Result<_> {
        let joining = args.call.join(Role::Guest)?;
        let config = ViewConfig {
            url: args.call.url(),
            token: joining.token,
            tick_interval: Duration::from_millis(247),
            net_stats: true,
            measure_quality: false,
            measure_input: false,
            measure_audio: false,
            snapshots: None,
            audio: args.call.audio()?,
            voice: VoiceOptions::default(),
        };
        Ok((config, joining.name))
    })();
    let (config, name) = setup.map_err(|e| failed_early(&events, e))?;
    let name = args.hello_name.clone().unwrap_or(name);
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
        name,
        app: Some(view::ViewApp {
            events: events.clone(),
            commands: rx,
        }),
        viewports: Vec::new(),
    });
    // The viewer has already told the app how the call ended.
    if result.is_err() {
        std::process::exit(1);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_come_from_the_strongest_source() {
        let dev = "ws://localhost:7880";
        assert_eq!(
            resolve(Some("f"), Some("e".into()), Some("b"), dev),
            ("f".into(), Origin::Flag)
        );
        assert_eq!(
            resolve(None, Some("e".into()), Some("b"), dev),
            ("e".into(), Origin::Env)
        );
        assert_eq!(
            resolve(None, None, Some("b"), dev),
            ("b".into(), Origin::Build)
        );
        assert_eq!(resolve(None, None, None, dev), (dev.into(), Origin::Dev));
        // Empty values are unset, at every level.
        assert_eq!(
            resolve(Some(""), Some(" ".into()), Some(""), dev),
            (dev.into(), Origin::Dev)
        );
    }

    #[test]
    fn permissions_request_all_or_one() {
        let request = |args: &[&str]| match Cli::try_parse_from(args).unwrap().command {
            Cmd::Permissions { request } => request,
            _ => panic!("not permissions"),
        };
        assert_eq!(request(&["tc-engine", "permissions"]), None);
        assert_eq!(
            request(&["tc-engine", "permissions", "--request"]),
            Some(None)
        );
        for (word, one) in [
            ("screen_recording", Permission::ScreenRecording),
            ("accessibility", Permission::Accessibility),
            ("microphone", Permission::Microphone),
        ] {
            assert_eq!(
                request(&["tc-engine", "permissions", "--request", word]),
                Some(Some(one))
            );
        }
        assert!(Cli::try_parse_from(["tc-engine", "permissions", "--request", "camera"]).is_err());
    }

    #[test]
    fn config_never_prints_the_secret() {
        let server = Server {
            url: ("wss://example.test".into(), Origin::Build),
            api_key: ("key".into(), Origin::Build),
            api_secret: ("very-secret-value".into(), Origin::Build),
        };
        let text = server.describe(false).to_string();
        assert!(!text.contains("very-secret-value"), "{text}");
        assert_eq!(server.describe(false)["has_secret"], true);
        assert_eq!(server.describe(false)["has_pass"], false);
        assert_eq!(server.describe(true)["has_pass"], true);
        assert_eq!(server.describe(false)["source"], "build");
        let dev = dev_server();
        assert_eq!(dev.describe(false)["has_secret"], false);
        assert_eq!(dev.describe(false)["source"], "dev");
    }

    fn dev_server() -> Server {
        Server {
            url: (token::DEV_URL.into(), Origin::Dev),
            api_key: (token::DEV_API_KEY.into(), Origin::Dev),
            api_secret: (token::DEV_API_SECRET.into(), Origin::Dev),
        }
    }

    const UUID: &str = "3f2c8a8e-5b7d-4e8f-9a51-0d6c1e2b7f40";

    /// A pass the dev server would take, for `role`-`UUID`.
    fn a_pass(role: PassRole, room: &str, name: &str) -> String {
        dev_pass(
            &dev_server(),
            &DevPassArgs {
                room: room.into(),
                role,
                name: name.into(),
                id: Some(UUID.into()),
                ttl: 600,
            },
        )
        .unwrap()
    }

    fn call(has_pass: bool, args: &[&str]) -> Result<Call, clap::Error> {
        use clap::FromArgMatches;
        let argv = ["tc-engine"].iter().chain(args);
        let mut matches = cli_command(has_pass).try_get_matches_from(argv)?;
        Ok(match Cli::from_arg_matches_mut(&mut matches)?.command {
            Cmd::Share(a) => a.call,
            Cmd::View(a) => a.call,
            _ => panic!("not a call"),
        })
    }

    fn join(call: &Call, role: Role, pass: &str) -> anyhow::Result<Joining> {
        call.join_with(role, Some(pass.into()), now_secs())
    }

    #[test]
    fn room_and_name_are_required_without_a_pass() {
        for sub in ["share", "view"] {
            let e = call(false, &[sub]).err().expect("needs --room and --name");
            assert_eq!(e.kind(), clap::error::ErrorKind::MissingRequiredArgument);
            let text = e.to_string();
            assert!(text.contains("--room") && text.contains("--name"), "{text}");
            assert!(call(false, &[sub, "--room", "r", "--name", "Sam"]).is_ok());
            // With a pass, neither is needed.
            assert!(call(true, &[sub]).is_ok());
        }
    }

    #[test]
    fn a_pass_says_who_joins_where() {
        let pass = a_pass(PassRole::Sharer, "r1", "Alex");
        let joining = join(&call(true, &["share"]).unwrap(), Role::Sharer, &pass).unwrap();
        assert_eq!(joining.identity, format!("sharer-{UUID}"));
        assert_eq!(app::role_of(&joining.identity), Role::Sharer);
        assert_eq!(joining.name, "Alex");
        assert_eq!(joining.token, pass);

        // The pass's name wins; a --room that agrees is fine.
        let pass = a_pass(PassRole::Guest, "r1", "Sam");
        let given = call(true, &["view", "--room", "r1", "--name", "Typed"]).unwrap();
        let joining = join(&given, Role::Guest, &pass).unwrap();
        assert_eq!(
            (
                joining.name.as_str(),
                joining.identity.starts_with("guest-")
            ),
            ("Sam", true)
        );
    }

    #[test]
    fn a_pass_that_doesnt_fit_the_call_is_refused() {
        let guest = a_pass(PassRole::Guest, "r1", "Sam");
        let sharer = a_pass(PassRole::Sharer, "r1", "Alex");
        let e = join(&call(true, &["share"]).unwrap(), Role::Sharer, &guest).unwrap_err();
        assert_eq!(
            e.to_string(),
            "This pass is for a guest; it can't be used to share"
        );
        let e = join(&call(true, &["view"]).unwrap(), Role::Guest, &sharer).unwrap_err();
        assert_eq!(
            e.to_string(),
            "This pass is for the sharer; it can't be used to view"
        );
        let other_room = call(true, &["view", "--room", "r2"]).unwrap();
        let e = join(&other_room, Role::Guest, &guest).unwrap_err();
        assert_eq!(e.to_string(), "This call pass is for room `r1`, not `r2`");
        for flag in ["--api-key", "--api-secret"] {
            let with_key = call(true, &["view", flag, "x"]).unwrap();
            let e = join(&with_key, Role::Guest, &guest).unwrap_err();
            assert_eq!(
                e.to_string(),
                "--api-key and --api-secret can't be used with a call pass (TWINCURSOR_PASS)"
            );
        }
    }

    #[test]
    fn the_pass_is_never_a_flag_nor_printed() {
        use clap::CommandFactory;
        let pass = a_pass(PassRole::Guest, "r1", "Sam");
        // No flag takes it. (clap is built without its `env` feature, so no flag reads the
        // environment either: only `pass_from_env` does.)
        let command = Cli::command();
        for sub in ["share", "view"] {
            let sub = command.find_subcommand(sub).unwrap();
            for arg in sub.get_arguments() {
                let long = arg.get_long().unwrap_or_default();
                assert!(!long.contains("pass"), "--{long}");
            }
            assert!(call(true, &[sub.get_name(), "--pass", &pass]).is_err());
        }
        // Not in what a call keeps, and not in any error a pass can cause.
        let joining = join(&call(true, &["view"]).unwrap(), Role::Guest, &pass).unwrap();
        let debug = format!("{joining:?}");
        assert!(!debug.contains(&pass) && debug.contains("Sam"), "{debug}");
        let failures = [
            join(&call(true, &["share"]).unwrap(), Role::Sharer, &pass),
            join(
                &call(true, &["view", "--room", "r2"]).unwrap(),
                Role::Guest,
                &pass,
            ),
            join(
                &call(true, &["view", "--api-key", "k"]).unwrap(),
                Role::Guest,
                &pass,
            ),
            join(
                &call(true, &["view"]).unwrap(),
                Role::Guest,
                &format!("{pass}.x"),
            ),
        ];
        for failure in failures {
            let e = failure.unwrap_err();
            for text in [format!("{e:#}"), format!("{e:?}")] {
                for part in pass.split('.') {
                    assert!(!text.contains(part), "{text}");
                }
            }
        }
    }

    #[test]
    fn without_a_pass_the_engine_mints_its_own_token() {
        let given = call(false, &["share", "--room", "r1", "--name", "Alex"]).unwrap();
        let joining = given.join_with(Role::Sharer, None, now_secs()).unwrap();
        assert_eq!(joining.name, "Alex");
        assert_eq!(app::role_of(&joining.identity), Role::Sharer);
        let back = pass::Pass::decode(&joining.token, now_secs()).unwrap();
        assert_eq!(
            (back.room.as_str(), back.identity),
            ("r1", joining.identity)
        );
    }

    #[test]
    fn dev_pass_never_signs_with_a_built_in_key() {
        let args = DevPassArgs {
            room: "r1".into(),
            role: PassRole::Guest,
            name: "Sam".into(),
            id: None,
            ttl: 60,
        };
        for (key, secret) in [
            (Origin::Build, Origin::Build),
            (Origin::Env, Origin::Build),
            (Origin::Build, Origin::Env),
        ] {
            let server = Server {
                url: ("wss://example.test".into(), Origin::Env),
                api_key: ("key".into(), key),
                api_secret: ("a-secret-long-enough".into(), secret),
            };
            let e = dev_pass(&server, &args).unwrap_err().to_string();
            assert!(e.contains("not ones built into this engine"), "{e}");
        }
        let server = Server {
            url: ("wss://example.test".into(), Origin::Build),
            api_key: ("key".into(), Origin::Env),
            api_secret: ("a-secret-long-enough".into(), Origin::Env),
        };
        let pass = pass::Pass::decode(&dev_pass(&server, &args).unwrap(), now_secs()).unwrap();
        assert_eq!(pass.role, Role::Guest);
        let id = pass.identity.strip_prefix("guest-").unwrap();
        let groups: Vec<usize> = id.split('-').map(str::len).collect();
        assert_eq!(groups, [8, 4, 4, 4, 12], "{id}");
        assert!(
            id.chars().all(|c| c == '-' || c.is_ascii_hexdigit()),
            "{id}"
        );
        assert_ne!(random_id(), random_id());
    }
}
