//! `tc-spike`: the M1 media spike. `share` publishes this Mac's screen, `view` shows it in a
//! native window and measures latency. Start `livekit-server --dev` first.

mod hud;
mod view;

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use clap::{Args, Parser, Subcommand, ValueEnum};
use livekit::options::VideoCodec;
use media::capture::{self, Backend, CaptureConfig, Tier};
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
    /// Capture the main display and publish it.
    Share(ShareArgs),
    /// Subscribe, render in a window, and measure latency.
    View(view::ViewArgs),
    /// Compare two snapshots (PGM) and print SSIM and PSNR, skipping the marker area.
    Compare {
        sharer: std::path::PathBuf,
        viewer: std::path::PathBuf,
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
    Native,
}

impl From<Resolution> for Tier {
    fn from(r: Resolution) -> Self {
        match r {
            Resolution::P720 => Tier::P720,
            Resolution::P1080 => Tier::P1080,
            Resolution::P1440 => Tier::P1440,
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
    /// Video codec. H.264 is the default (hardware on macOS; see docs/adr/0002).
    #[arg(long, value_enum, default_value = "h264")]
    codec: Codec,
    /// Output size in lines (the width follows the display's aspect ratio), or the
    /// display's own pixel size.
    #[arg(long, value_enum, default_value = "1080")]
    resolution: Resolution,
    /// Maximum video bitrate in kbit/s. Defaults per resolution: 2500, 5000, 8000, 12000.
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
        Command::View(args) => view::run(args),
        Command::Compare { sharer, viewer } => compare(&sharer, &viewer),
    }
}

fn share(args: ShareArgs) -> anyhow::Result<()> {
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
        _ => capture::display_pixels(),
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
        "display {}x{} → sending {width}x{height}, up to {bitrate_kbps} kbit/s",
        screen.0,
        screen.1
    );
    let config = ShareConfig {
        url: args.connection.url.clone(),
        token: args.connection.token("sharer")?,
        backend,
        capture: CaptureConfig {
            width,
            height,
            max_fps: args.fps,
            show_cursor: args.cursor,
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
        snapshots: (args.snapshot_every > 0).then(|| media::quality::Snapshots {
            every: args.snapshot_every,
            dir: args.snapshot_dir.clone(),
        }),
    };

    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async {
        let stats = Arc::new(ShareStats::default());
        let reporter = tokio::spawn(report_share_stats(stats.clone()));
        let shutdown = async {
            let _ = tokio::signal::ctrl_c().await;
        };
        let result = publish::run(config, stats, shutdown).await;
        reporter.abort();
        result.context("sharing failed")
    })
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
            "captured {:.1} fps, sent {:.1} fps (+{:.1} re-sends/s for ticks or idle), ticks {:.1}/s",
            take(&stats.captured) / secs,
            take(&stats.sent) / secs,
            take(&stats.resent) / secs,
            take(&stats.ticks) / secs,
        );
        if let Some(net) = stats.net.lock().unwrap().as_ref() {
            tracing::info!("network {}", serde_json::to_string(net).unwrap_or_default());
        }
    }
}
