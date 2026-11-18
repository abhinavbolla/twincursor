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
    /// Join as this participant. Two viewers in one room need different ones.
    #[arg(long, default_value = "viewer")]
    identity: String,
    /// Headless: report this viewport (physical pixels the picture could take, as WxH) to the
    /// sharer, as a window reports its own. Without it, a headless viewer reports none and
    /// receives the sharer's own size.
    #[arg(long, value_name = "WxH", value_parser = parse_size, requires = "headless")]
    viewport: Option<(u32, u32)>,
    /// Headless: report another viewport SECS seconds after starting, as SECS:WxH, for
    /// scripted checks (bench/viewport-e2e.sh). Repeatable.
    #[arg(long, value_name = "SECS:WxH", value_parser = parse_timed_size, requires = "headless")]
    viewport_at: Vec<(f64, (u32, u32))>,
}

/// `1920x1080` as (1920, 1080).
fn parse_size(s: &str) -> Result<(u32, u32), String> {
    let (w, h) = s
        .split_once(['x', 'X'])
        .ok_or_else(|| format!("expected WxH, such as 1920x1080, not {s:?}"))?;
    let side = |v: &str| {
        v.trim()
            .parse::<u32>()
            .ok()
            .filter(|v| (protocol::MIN_VIEWPORT..=protocol::MAX_VIEWPORT).contains(v))
            .ok_or_else(|| {
                format!(
                    "{v:?} isn't a size from {} to {} pixels",
                    protocol::MIN_VIEWPORT,
                    protocol::MAX_VIEWPORT
                )
            })
    };
    Ok((side(w)?, side(h)?))
}

/// `12.5:1920x1080` as (12.5, (1920, 1080)).
fn parse_timed_size(s: &str) -> Result<(f64, (u32, u32)), String> {
    let (secs, size) = s
        .split_once(':')
        .ok_or_else(|| format!("expected SECS:WxH, such as 10:1920x1080, not {s:?}"))?;
    let secs = secs
        .parse::<f64>()
        .ok()
        .filter(|t| t.is_finite() && *t >= 0.0)
        .ok_or_else(|| format!("{secs:?} isn't a number of seconds"))?;
    Ok((secs, parse_size(size)?))
}

/// The viewports to report and when: `--viewport` at the start, then each `--viewport-at`.
fn viewports(args: &ViewArgs) -> Vec<(Duration, (u32, u32))> {
    let mut out: Vec<(Duration, (u32, u32))> = args
        .viewport
        .map(|v| (Duration::ZERO, v))
        .into_iter()
        .chain(
            args.viewport_at
                .iter()
                .map(|&(t, v)| (Duration::from_secs_f64(t), v)),
        )
        .collect();
    out.sort_by_key(|&(t, _)| t);
    out
}

fn view_config(args: &ViewArgs) -> anyhow::Result<ViewConfig> {
    Ok(ViewConfig {
        url: args.connection.url.clone(),
        token: args.connection.token(&args.identity)?,
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
    let viewports = viewports(&args);
    engine::view::run(ViewOptions {
        config,
        voice: args.voice.state(),
        headless: args.headless,
        duration: args.duration,
        label: args.label,
        out: Some(args.out),
        control_script: args.control_script,
        sent_log: args.sent_log,
        name: args.name,
        app: None,
        viewports,
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_viewports() {
        assert_eq!(parse_size("1920x1080"), Ok((1920, 1080)));
        assert_eq!(parse_size("2560X1440"), Ok((2560, 1440)));
        assert!(parse_size("1920").is_err());
        assert!(parse_size("0x1080").is_err());
        assert!(parse_size("99999x1080").is_err());
        assert_eq!(parse_timed_size("12.5:1280x720"), Ok((12.5, (1280, 720))));
        assert!(parse_timed_size("-1:1280x720").is_err());
        assert!(parse_timed_size("1280x720").is_err());
    }
}
