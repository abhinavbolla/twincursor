//! The `view` subcommand's flags, turned into the engine's [`ViewOptions`].

use std::path::PathBuf;
use std::time::Duration;

use clap::Args;
use engine::view::ViewOptions;
use media::quality::Snapshots;
use media::subscribe::ViewConfig;

use crate::Connection;

#[derive(Args)]
pub struct ViewArgs {
    #[command(flatten)]
    connection: Connection,
    /// Interval between latency ticks, in milliseconds. Not a multiple of the frame time:
    /// at 250 ms (15 frames at 60 fps) every tick lands at the same point of the frame clock.
    #[arg(long, default_value_t = 247)]
    tick_ms: u64,
    /// Stop automatically after this many seconds.
    #[arg(long)]
    duration: Option<u64>,
    /// Label stored in the results file, e.g. "sck-vp8-static".
    #[arg(long, default_value = "run")]
    label: String,
    /// Don't poll network stats or show the HUD.
    #[arg(long)]
    no_stats: bool,
    /// Decode and measure without opening a window (for benchmarks and the network lab).
    /// Latency is then measured at decode.
    #[arg(long)]
    headless: bool,
    /// Score frames against the synthetic pattern (needs `share --capture pattern`).
    #[arg(long)]
    measure_quality: bool,
    /// Measure the data-packet path remote-control input takes (reliable vs lossy).
    #[arg(long)]
    measure_input: bool,
    /// Time the markers in the sharer's voice (needs `share --audio pattern`).
    #[arg(long)]
    measure_audio: bool,
    #[command(flatten)]
    voice: crate::voice::VoiceArgs,
    /// Save the frame carrying every Nth latency tick as a PGM (see `tc-spike compare`).
    #[arg(long, default_value_t = 0)]
    snapshot_every: u32,
    #[arg(long, default_value = "bench/snapshots")]
    snapshot_dir: PathBuf,
    /// Directory for the JSON results file.
    #[arg(long, default_value = "bench/results")]
    out: PathBuf,
    /// Play a remote-control script (JSON steps; see `bench/control/`), then stop. Needs
    /// `--headless` and a sharer started with `--allow-control`.
    #[arg(long, requires = "headless")]
    control_script: Option<PathBuf>,
    /// Write every control message sent as a line of JSON (see `tc-spike replay`).
    #[arg(long)]
    sent_log: Option<PathBuf>,
    /// The name the sharer sees when you ask for control.
    #[arg(long, default_value = "guest")]
    name: String,
}

fn view_config(args: &ViewArgs) -> anyhow::Result<ViewConfig> {
    Ok(ViewConfig {
        url: args.connection.url.clone(),
        token: args.connection.token("viewer")?,
        tick_interval: Duration::from_millis(args.tick_ms),
        net_stats: !args.no_stats,
        measure_quality: args.measure_quality,
        measure_input: args.measure_input,
        measure_audio: args.measure_audio,
        audio: args.voice.mode(false)?,
        voice: args.voice.options(),
        snapshots: (args.snapshot_every > 0).then(|| Snapshots {
            every: args.snapshot_every,
            dir: args.snapshot_dir.clone(),
        }),
    })
}

/// `tc-spike view`.
pub fn run(args: ViewArgs) -> anyhow::Result<()> {
    let config = view_config(&args)?;
    engine::view::run(ViewOptions {
        config,
        voice: args.voice.state(),
        headless: args.headless,
        duration: args.duration,
        label: args.label,
        out: args.out,
        control_script: args.control_script,
        sent_log: args.sent_log,
        name: args.name,
    })?;
    Ok(())
}
