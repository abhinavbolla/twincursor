//! `tc-spike`: the media development tool. `share` publishes this Mac's screen, `view` shows it in a
//! native window and measures latency. Start `livekit-server --dev` first.

mod echo;
mod listen;
mod permissions;
mod probe;
mod probe_gate;
mod replay;
mod view_args;
mod voice;

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use clap::{Args, Parser, Subcommand, ValueEnum};
use engine::{hotkey, sharer};
use livekit::options::VideoCodec;
use media::audio::AudioMode;
use media::capture::select;
use media::capture::{self, Backend, CaptureConfig, Source, Tier};
use media::publish::{self, AdaptConfig, ShareConfig, ShareStats};
use media::token;

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
    /// Exit when stdin reaches end of file: for a desktop app that starts this tool and keeps
    /// its stdin open, so the tool never outlives the app.
    #[arg(long, global = true)]
    exit_on_stdin_close: bool,
}

#[derive(Subcommand)]
enum Command {
    /// Capture the main display (or `--display`, or one window or app) and publish it.
    Share(ShareArgs),
    /// List the windows `share --window` can capture: id, size in pixels, app and title.
    Windows {
        /// Print JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// List the displays `share --display` can capture: id, main, place in points, pixels.
    Displays {
        /// Print JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Subscribe, render in a window, and measure latency.
    View(view_args::ViewArgs),
    /// Compare two snapshots (PGM) and print SSIM and PSNR, skipping the marker area.
    Compare {
        sharer: std::path::PathBuf,
        viewer: std::path::PathBuf,
    },
    /// List the microphones and speakers `--mic` and `--speaker` can pick.
    Devices,
    /// Print this process's Screen Recording, Accessibility and Microphone access as JSON.
    Permissions(permissions::PermissionsArgs),
    /// Hear the sharer's synthetic voice through a real mic (stdin, from
    /// bench/audio/record-mic.swift) and time it against when the sharer started each burst.
    /// With --echo, check the viewer's echo cancellation instead (bench/audio-echo.sh).
    Listen(listen::ListenArgs),
    /// A full-screen window that records the input it receives, to check real injection.
    Probe(probe::ProbeArgs),
    /// Wait until a `probe` window's records show a step of a gated control script arrived
    /// (exit 0), went wrong (1) or didn't come in time (2). For bench/control-inject.sh.
    ProbeGate(probe_gate::ProbeGateArgs),
    /// Check that the actions a sharer recorded match the messages a viewer sent.
    Replay {
        /// The viewer's `--sent-log`.
        #[arg(long)]
        sent: std::path::PathBuf,
        /// The sharer's `--inject record:PATH` file.
        #[arg(long, required_unless_present = "probe")]
        actions: Option<std::path::PathBuf>,
        /// Instead, what a `tc-spike probe` window received from real injection.
        #[arg(long, conflicts_with = "actions")]
        probe: Option<std::path::PathBuf>,
        /// The sharer took control back partway: expect the start of the run, then releases.
        #[arg(long)]
        revoked: bool,
    },
}

#[derive(Args, Clone)]
pub struct Connection {
    /// LiveKit server URL.
    #[arg(long, default_value = token::DEV_URL)]
    url: String,
    /// Room to join.
    #[arg(long, default_value = "m1")]
    room: String,
}

impl Connection {
    fn token(&self, identity: &str) -> anyhow::Result<String> {
        token::dev_token(&self.room, identity).context("minting dev token")
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum CaptureBackend {
    /// ScreenCaptureKit (GPU-scaled NV12).
    Sck,
    /// libwebrtc DesktopCapturer from the LiveKit SDK (BGRA, CPU-scaled).
    Lk,
    /// Synthetic code-editor pattern; the viewer measures its quality (SSIM/PSNR).
    Pattern,
}

#[derive(Clone, Copy, ValueEnum)]
enum Codec {
    Vp8,
    Vp9,
    H264,
    /// HEVC: hardware on macOS, opt-in (see docs/adr/0008).
    H265,
    Av1,
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

impl From<Resolution> for Tier {
    fn from(r: Resolution) -> Self {
        match r {
            Resolution::P720 => Tier::P720,
            Resolution::P1080 => Tier::P1080,
            Resolution::P1440 => Tier::P1440,
            Resolution::P2160 => Tier::P2160,
            Resolution::Native => Tier::Native,
        }
    }
}

impl From<Codec> for VideoCodec {
    fn from(codec: Codec) -> Self {
        match codec {
            Codec::Vp8 => VideoCodec::VP8,
            Codec::Vp9 => VideoCodec::VP9,
            Codec::H264 => VideoCodec::H264,
            Codec::H265 => VideoCodec::H265,
            Codec::Av1 => VideoCodec::AV1,
        }
    }
}

#[derive(Args)]
struct ShareArgs {
    #[command(flatten)]
    connection: Connection,
    #[arg(long, value_enum, default_value = "sck")]
    capture: CaptureBackend,
    /// Share one window: its id, or text its title or app name contains (ignoring case; see
    /// `tc-spike windows`). Sent at the window's own pixel size, within --resolution.
    #[arg(long, conflicts_with = "app")]
    window: Option<String>,
    /// Share one app: the display with only this app's windows on it (name or bundle id).
    #[arg(long)]
    app: Option<String>,
    /// The display to share, by id (see `tc-spike displays`). Default: the main display, the
    /// one with the menu bar. Remote control goes to the same display.
    #[arg(long)]
    display: Option<u32>,
    /// Video codec. H.264 is the default (hardware on macOS; see docs/adr/0002). H.265 (HEVC)
    /// is opt-in, for 4K (docs/adr/0008).
    #[arg(long, value_enum, default_value = "h264")]
    codec: Codec,
    /// Output size in lines (the width follows the display's aspect ratio), or the
    /// display's own pixel size. Never larger than the display.
    #[arg(long, value_enum, default_value = "1080")]
    resolution: Resolution,
    /// Maximum video bitrate in kbit/s. Defaults per resolution: 2500, 5000, 8000, 15000 for
    /// 2160, and 12000 for native.
    #[arg(long)]
    bitrate_kbps: Option<u64>,
    #[arg(long, default_value_t = 60)]
    fps: u32,
    /// Exact output width, overriding --resolution (needs --height too).
    #[arg(long, requires = "height")]
    width: Option<u32>,
    /// Exact output height, overriding --resolution (needs --width too).
    #[arg(long, requires = "width")]
    height: Option<u32>,
    /// Publish as a regular video source instead of a screencast.
    #[arg(long)]
    no_screencast: bool,
    /// Don't stamp latency markers into frames.
    #[arg(long)]
    no_markers: bool,
    /// Don't poll network stats or send them to the viewer (also turns off --adapt).
    #[arg(long)]
    no_stats: bool,
    /// Keep the resolution fixed instead of stepping it down when the call can't sustain it.
    #[arg(long)]
    no_adapt: bool,
    /// Send the --resolution size whatever the viewers show, instead of no more than the
    /// largest viewer's viewport allows (for A/B runs).
    #[arg(long)]
    ignore_viewport: bool,
    /// Save the frame stamped with every Nth latency tick as a PGM (see `tc-spike compare`).
    #[arg(long, default_value_t = 0)]
    snapshot_every: u32,
    #[arg(long, default_value = "bench/snapshots")]
    snapshot_dir: std::path::PathBuf,
    /// Include the mouse cursor in the capture.
    #[arg(long)]
    cursor: bool,
    #[command(flatten)]
    voice: voice::VoiceArgs,
    /// Let viewers ask for remote control. You're asked in this terminal before anyone gets it.
    /// With --window or --app every request is turned down, with a reason: input can't follow
    /// one window yet.
    #[arg(long)]
    allow_control: bool,
    /// What granted input does: `real` input (the default; macOS, needs Accessibility), `log`
    /// it, or `record:PATH` it as JSON lines (see `replay`).
    #[arg(long, default_value = "real", requires = "allow_control")]
    inject: sharer::Inject,
    /// Run without the revoke hotkey and the on-screen label, for test runs on machines without
    /// a display (the network lab). Only with `--inject log` or `record`: real control always
    /// shows the sharer who is in control.
    #[arg(long, requires = "allow_control")]
    headless: bool,
    /// Also leave this app's windows out of the capture, by process id (the desktop app that
    /// started this one).
    #[arg(long)]
    exclude_app_pid: Option<u32>,
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,livekit=warn,libwebrtc=warn".into()),
        )
        .init();
    let cli = Cli::parse();
    if cli.exit_on_stdin_close {
        // The control prompt reads its answers from stdin, so the two can't share it.
        if matches!(&cli.command, Command::Share(args) if args.allow_control) {
            anyhow::bail!("--exit-on-stdin-close doesn't work with --allow-control yet");
        }
        permissions::exit_on_stdin_close();
    }
    match cli.command {
        Command::Share(args) => share(args),
        Command::Windows { json } => list_windows(json),
        Command::Displays { json } => {
            let displays = engine::screen::list();
            if json {
                println!("{}", serde_json::to_string_pretty(&displays)?);
            } else {
                print!("{}", engine::screen::table(&displays));
            }
            Ok(())
        }
        Command::View(args) => view_args::run(args),
        Command::Compare { sharer, viewer } => compare(&sharer, &viewer),
        Command::Devices => voice::list_devices(),
        Command::Permissions(args) => permissions::run(args),
        Command::Listen(args) => listen::run(args),
        Command::Probe(args) => probe::run(args),
        Command::ProbeGate(args) => probe_gate::run(args),
        Command::Replay {
            sent,
            actions,
            probe,
            revoked,
        } => match (actions, probe) {
            (_, Some(probe)) => replay::run_probe(&sent, &probe),
            (Some(actions), None) => replay::run(&sent, &actions, revoked),
            (None, None) => anyhow::bail!("give --actions or --probe"),
        },
    }
}

fn list_windows(json: bool) -> anyhow::Result<()> {
    let (windows, _) = capture::shareable().context("listing windows")?;
    if json {
        println!("{}", serde_json::to_string_pretty(&windows)?);
    } else {
        println!("{}", select::window_table(&windows));
    }
    Ok(())
}

/// What `--window` or `--app` asks for, and the pixel size the tiers are computed from
/// (`None`: the display's).
fn pick_source(args: &ShareArgs) -> anyhow::Result<(Source, Option<(u32, u32)>)> {
    if args.window.is_none() && args.app.is_none() {
        return Ok((Source::Display, None));
    }
    anyhow::ensure!(
        matches!(args.capture, CaptureBackend::Sck),
        "--window and --app need --capture sck"
    );
    let (windows, apps) = capture::shareable().context("listing windows")?;
    select::resolve(&windows, &apps, args.window.as_deref(), args.app.as_deref())
        .map_err(anyhow::Error::msg)
}

fn share(args: ShareArgs) -> anyhow::Result<()> {
    let (source, source_pixels) = pick_source(&args)?;
    let backend = match args.capture {
        #[cfg(target_os = "macos")]
        CaptureBackend::Sck => Backend::ScreenCaptureKit,
        #[cfg(not(target_os = "macos"))]
        CaptureBackend::Sck => anyhow::bail!("ScreenCaptureKit is macOS only"),
        CaptureBackend::Lk => Backend::LiveKit,
        CaptureBackend::Pattern => Backend::Pattern,
    };
    // The one display this share is about: captured, and the only one control input goes to.
    let display = engine::screen::choose(backend, args.display).map_err(anyhow::Error::msg)?;
    let surface = if source.is_whole_display() {
        sharer::Surface::Display(display.id)
    } else {
        sharer::Surface::Window
    };
    let tier = Tier::from(args.resolution);
    let detected = match backend {
        Backend::Pattern => Some(capture::PATTERN_DISPLAY),
        _ => source_pixels.or_else(|| capture::display_pixels(display.id)),
    };
    let screen = detected.unwrap_or_else(|| {
        tracing::warn!(
            "display size unknown; assuming {:?}",
            capture::FALLBACK_DISPLAY
        );
        capture::FALLBACK_DISPLAY
    });
    let (width, height) = match (args.width, args.height) {
        (Some(w), Some(h)) => (w, h),
        _ => tier.size(screen),
    };
    let bitrate_kbps = args
        .bitrate_kbps
        .unwrap_or_else(|| tier.default_bitrate_kbps());
    tracing::info!(
        "{} {}x{} → sending {width}x{height}, up to {bitrate_kbps} kbit/s",
        if source_pixels.is_some() {
            "window"
        } else {
            "display"
        },
        screen.0,
        screen.1
    );
    let config = ShareConfig {
        url: args.connection.url.clone(),
        token: args.connection.token("sharer")?,
        backend,
        source: screen,
        // An exact --width/--height stays exact.
        fit_viewport: !args.ignore_viewport && args.width.is_none(),
        capture: CaptureConfig {
            source,
            display: Some(display.id),
            width,
            height,
            max_fps: args.fps,
            // A guest in control needs to see where the pointer is.
            show_cursor: args.cursor || args.allow_control,
            exclude_app_pid: args.exclude_app_pid,
            leave_out_twincursor: true,
            // A shared window's stream follows it when it's resized, unless the size is fixed.
            follow_tier: args.width.is_none().then_some(tier),
        },
        codec: args.codec.into(),
        max_bitrate: bitrate_kbps * 1000,
        is_screencast: !args.no_screencast,
        stamp_markers: !args.no_markers,
        net_stats: !args.no_stats,
        // The adapter needs stats, and works in tiers (not with an explicit --width/--height).
        adapt: (!args.no_adapt && !args.no_stats && args.width.is_none())
            .then_some(AdaptConfig { max: tier }),
        audio: args.voice.mode(true)?,
        voice: args.voice.options(),
        snapshots: (args.snapshot_every > 0).then(|| media::quality::Snapshots {
            every: args.snapshot_every,
            dir: args.snapshot_dir.clone(),
        }),
    };

    // A real mic gets the mute hotkey, which needs the app's event loop on the main thread.
    let voice = args.voice.state();
    let mute = matches!(config.audio, AudioMode::Mic(_)).then(|| voice.clone());
    let network = move |link| -> anyhow::Result<()> {
        let runtime = tokio::runtime::Runtime::new()?;
        runtime.block_on(async {
            let stats = Arc::new(ShareStats::default());
            let reporter = tokio::spawn(report_share_stats(stats.clone()));
            let shutdown = async {
                let _ = tokio::signal::ctrl_c().await;
            };
            let result = publish::run(config, stats, voice, link, shutdown).await;
            reporter.abort();
            result.context("sharing failed")
        })
    };
    if !args.allow_control {
        if mute.is_none() {
            return network(None);
        }
        return match hotkey::SharerUi::new(display.id) {
            Ok(ui) => ui.run(None, mute, move || network(None))?,
            Err(e) => {
                tracing::warn!("no {} to mute with: {e}", hotkey::MUTE);
                network(None)
            }
        };
    }
    let revoke = control::RevokeSwitch::default();
    if args.headless {
        anyhow::ensure!(
            !matches!(args.inject, sharer::Inject::Real),
            "--headless only works with --inject log or record: real control needs the label"
        );
        let (sharer, link) = sharer::Sharer::start(
            &args.inject,
            surface,
            revoke,
            None,
            sharer::Consent::Terminal,
        )?;
        let result = network(Some(link));
        sharer.stop();
        return result;
    }
    let ui = hotkey::SharerUi::new(display.id)?;
    let (sharer, link) = sharer::Sharer::start(
        &args.inject,
        surface,
        revoke.clone(),
        Some(ui.overlay()),
        sharer::Consent::Terminal,
    )?;
    let result = ui.run(Some(revoke), mute, move || network(Some(link)));
    // Release anything a guest still holds before exiting.
    sharer.stop();
    result?
}

fn compare(a: &std::path::Path, b: &std::path::Path) -> anyhow::Result<()> {
    use media::quality::{self, Plane};
    let (wa, ha, pa) = quality::read_pgm(a).with_context(|| format!("reading {}", a.display()))?;
    let (wb, hb, pb) = quality::read_pgm(b).with_context(|| format!("reading {}", b.display()))?;
    anyhow::ensure!((wa, ha) == (wb, hb), "sizes differ: {wa}x{ha} vs {wb}x{hb}");
    let scores = quality::compare(
        Plane {
            data: &pa,
            stride: wa,
        },
        Plane {
            data: &pb,
            stride: wb,
        },
        wa,
        ha,
        media::marker::MARKED_AREA,
    )
    .context("images too small to compare")?;
    println!(
        "{wa}x{ha}  SSIM {:.4}  PSNR {:.1} dB",
        scores.ssim, scores.psnr_db
    );
    Ok(())
}

async fn report_share_stats(stats: Arc<ShareStats>) {
    const EVERY: Duration = Duration::from_secs(10);
    let mut interval = tokio::time::interval(EVERY);
    interval.tick().await;
    loop {
        interval.tick().await;
        let take = |c: &std::sync::atomic::AtomicU64| c.swap(0, Ordering::Relaxed) as f64;
        let secs = EVERY.as_secs_f64();
        tracing::info!(
            "captured {:.1} fps, sent {:.1} fps (+{:.1} re-sends/s for ticks or idle), dropped {:.1}/s (encoder behind or paced by the adapter), ticks {:.1}/s",
            take(&stats.captured) / secs,
            take(&stats.sent) / secs,
            take(&stats.resent) / secs,
            take(&stats.dropped) / secs,
            take(&stats.ticks) / secs,
        );
        if let Some(net) = stats.net.lock().unwrap().as_ref() {
            tracing::info!("network {}", serde_json::to_string(net).unwrap_or_default());
        }
    }
}
