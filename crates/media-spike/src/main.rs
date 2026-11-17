//! `tc-spike`: the media development tool. `share` publishes this Mac's screen, `view` shows it in a
//! native window and measures latency. Start `livekit-server --dev` first.

mod echo;
mod listen;
mod probe;
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
use media::capture::select::{self, Pick, WindowInfo};
use media::capture::{self, Backend, CaptureConfig, Source, Tier};
use media::publish::{self, AdaptConfig, ShareConfig, ShareStats};
use media::token;

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Capture the main display (or one window or app) and publish it.
    Share(ShareArgs),
    /// List the windows `share --window` can capture: id, size in pixels, app and title.
    Windows {
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
    /// Hear the sharer's synthetic voice through a real mic (stdin, from
    /// bench/audio/record-mic.swift) and time it against when the sharer started each burst.
    /// With --echo, check the viewer's echo cancellation instead (bench/audio-echo.sh).
    Listen(listen::ListenArgs),
    /// A full-screen window that records the input it receives, to check real injection.
    Probe(probe::ProbeArgs),
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
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,livekit=warn,libwebrtc=warn".into()),
        )
        .init();
    match Cli::parse().command {
        Command::Share(args) => share(args),
        Command::Windows { json } => list_windows(json),
        Command::View(args) => view_args::run(args),
        Command::Compare { sharer, viewer } => compare(&sharer, &viewer),
        Command::Devices => voice::list_devices(),
        Command::Listen(args) => listen::run(args),
        Command::Probe(args) => probe::run(args),
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

/// One line of the window table.
fn window_row(w: &WindowInfo) -> String {
    format!(
        "{:>7}  {:>11}  {:<20}  {}",
        w.id,
        format!("{}x{}", w.pixels.0, w.pixels.1),
        w.app,
        w.title
    )
}

fn window_table<'a>(windows: impl IntoIterator<Item = &'a WindowInfo>) -> String {
    let mut out = format!("{:>7}  {:>11}  {:<20}  {}", "id", "pixels", "app", "title");
    for w in windows {
        out.push('\n');
        out += &window_row(w);
    }
    out
}

fn list_windows(json: bool) -> anyhow::Result<()> {
    let (windows, _) = capture::shareable().context("listing windows")?;
    if json {
        println!("{}", serde_json::to_string_pretty(&windows)?);
    } else {
        println!("{}", window_table(&windows));
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
    if let Some(query) = &args.window {
        return match select::pick_window(&windows, query) {
            Pick::One(w) => {
                tracing::info!("sharing window {} ({}: {:?})", w.id, w.app, w.title);
                Ok((Source::Window(w.id), Some(w.pixels)))
            }
            Pick::None => anyhow::bail!(
                "no window matches {query:?}. Shareable windows:\n{}",
                window_table(&windows)
            ),
            Pick::Many(found) => anyhow::bail!(
                "{} windows match {query:?}; give more of the title, or the id:\n{}",
                found.len(),
                window_table(found)
            ),
        };
    }
    let query = args.app.as_deref().unwrap_or_default();
    let names = |apps: &[&select::AppInfo]| {
        apps.iter()
            .map(|a| format!("  {} ({})", a.name, a.bundle_id))
            .collect::<Vec<_>>()
            .join("\n")
    };
    match select::pick_app(&apps, query) {
        Pick::One(a) => {
            tracing::info!("sharing the windows of {} ({})", a.name, a.bundle_id);
            Ok((Source::App(a.pid), None))
        }
        Pick::None => anyhow::bail!(
            "no app with windows matches {query:?}. Apps with windows:\n{}",
            names(&apps.iter().collect::<Vec<_>>())
        ),
        Pick::Many(found) => anyhow::bail!(
            "{} apps match {query:?}; give the full name or the bundle id:\n{}",
            found.len(),
            names(&found)
        ),
    }
}

fn share(args: ShareArgs) -> anyhow::Result<()> {
    anyhow::ensure!(
        !(args.allow_control && (args.window.is_some() || args.app.is_some())),
        "--allow-control can't be combined with --window or --app: remote control maps the guest's pointer onto the whole display, so share the whole display to give control"
    );
    let (source, source_pixels) = pick_source(&args)?;
    let backend = match args.capture {
        #[cfg(target_os = "macos")]
        CaptureBackend::Sck => Backend::ScreenCaptureKit,
        #[cfg(not(target_os = "macos"))]
        CaptureBackend::Sck => anyhow::bail!("ScreenCaptureKit is macOS only"),
        CaptureBackend::Lk => Backend::LiveKit,
        CaptureBackend::Pattern => Backend::Pattern,
    };
    let tier = Tier::from(args.resolution);
    let detected = match backend {
        Backend::Pattern => Some(capture::PATTERN_DISPLAY),
        _ => source_pixels.or_else(capture::display_pixels),
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
        capture: CaptureConfig {
            source,
            width,
            height,
            max_fps: args.fps,
            // A guest in control needs to see where the pointer is.
            show_cursor: args.cursor || args.allow_control,
        },
        codec: args.codec.into(),
        max_bitrate: bitrate_kbps * 1000,
        is_screencast: !args.no_screencast,
        stamp_markers: !args.no_markers,
        net_stats: !args.no_stats,
        // The adapter needs stats, and works in tiers (not with an explicit --width/--height).
        adapt: (!args.no_adapt && !args.no_stats && args.width.is_none()).then_some(AdaptConfig {
            max: tier,
            display: screen,
        }),
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
        return match hotkey::SharerUi::new() {
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
        let (sharer, link) = sharer::Sharer::start(&args.inject, revoke, None)?;
        let result = network(Some(link));
        sharer.stop();
        return result;
    }
    let ui = hotkey::SharerUi::new()?;
    let (sharer, link) = sharer::Sharer::start(&args.inject, revoke.clone(), Some(ui.overlay()))?;
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
