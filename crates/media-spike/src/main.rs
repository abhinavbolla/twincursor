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
use media::capture::{Backend, CaptureConfig};
use media::publish::{self, ShareConfig, ShareStats};
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
}

#[derive(Clone, Copy, ValueEnum)]
enum Codec {
    Vp8,
    Vp9,
    H264,
    Av1,
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
    #[arg(long, value_enum, default_value = "vp8")]
    codec: Codec,
    /// Maximum video bitrate in kbit/s.
    #[arg(long, default_value_t = 4000)]
    bitrate_kbps: u64,
    #[arg(long, default_value_t = 60)]
    fps: u32,
    /// Output width. The default keeps a 3024x1964 Retina panel's aspect ratio at 1080 lines.
    #[arg(long, default_value_t = 1662)]
    width: u32,
    #[arg(long, default_value_t = 1080)]
    height: u32,
    /// Publish as a regular video source instead of a screencast.
    #[arg(long)]
    no_screencast: bool,
    /// Don't stamp latency markers into frames.
    #[arg(long)]
    no_markers: bool,
    /// Don't poll network stats or send them to the viewer.
    #[arg(long)]
    no_stats: bool,
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
    }
}

fn share(args: ShareArgs) -> anyhow::Result<()> {
    let backend = match args.capture {
        #[cfg(target_os = "macos")]
        CaptureBackend::Sck => Backend::ScreenCaptureKit,
        #[cfg(not(target_os = "macos"))]
        CaptureBackend::Sck => anyhow::bail!("ScreenCaptureKit is macOS only"),
        CaptureBackend::Lk => Backend::LiveKit,
    };
    let config = ShareConfig {
        url: args.connection.url.clone(),
        token: args.connection.token("sharer")?,
        backend,
        capture: CaptureConfig {
            width: args.width,
            height: args.height,
            max_fps: args.fps,
            show_cursor: args.cursor,
        },
        codec: args.codec.into(),
        max_bitrate: args.bitrate_kbps * 1000,
        is_screencast: !args.no_screencast,
        stamp_markers: !args.no_markers,
        net_stats: !args.no_stats,
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
