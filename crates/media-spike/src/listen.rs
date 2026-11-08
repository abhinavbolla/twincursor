//! `tc-spike listen`: hears the sharer's synthetic voice through a real microphone, for the
//! acoustic loopback (`bench/audio-loopback.sh`).
//!
//! The mic arrives on stdin from `bench/audio/record-mic.swift`, raw and stamped with its capture
//! time. The listener joins the room only to receive the sharer's burst start times
//! ([`wire::AUDIO_TOPIC`]), so `heard − started` is the whole path: codec, network, the viewer's
//! jitter buffer and speakers, the air, and a mic's input buffer. On one machine both ends share a
//! clock.

use std::collections::VecDeque;
use std::io::Read;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::Context;
use clap::Args;
use livekit::prelude::*;
use media::audio::marker::{Detector, RATE};
use media::audio::measure::AudioLog;
use media::capture::wall_clock_us;
use media::wire;
use serde_json::json;

use crate::Connection;

#[derive(Args)]
pub struct ListenArgs {
    #[command(flatten)]
    connection: Connection,
    /// Stop after this many seconds (also stops when stdin ends).
    #[arg(long)]
    duration: Option<u64>,
    /// Label stored in the results file.
    #[arg(long, default_value = "listen")]
    label: String,
    /// Directory for the JSON results file.
    #[arg(long, default_value = "bench/results")]
    out: PathBuf,
}

/// One chunk from `record-mic`: its first sample's wall-clock time, and 48 kHz mono samples.
#[derive(Debug, PartialEq)]
struct Chunk {
    wall_us: u64,
    samples: Vec<i16>,
}

/// Reads the next chunk; `None` at the end of the stream.
fn read_chunk(input: &mut impl Read) -> std::io::Result<Option<Chunk>> {
    let mut header = [0u8; 12];
    match input.read_exact(&mut header) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let wall_us = u64::from_le_bytes(header[..8].try_into().unwrap());
    let n = u32::from_le_bytes(header[8..].try_into().unwrap()) as usize;
    if n > RATE as usize {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("chunk of {n} samples: not record-mic's format"),
        ));
    }
    let mut bytes = vec![0u8; n * 2];
    input.read_exact(&mut bytes)?;
    let samples = bytes
        .chunks_exact(2)
        .map(|b| i16::from_le_bytes([b[0], b[1]]))
        .collect();
    Ok(Some(Chunk { wall_us, samples }))
}

/// Finds markers in the mic and pairs them with the sharer's start times.
#[derive(Default)]
struct Listener {
    detector: Detector,
    /// Where recent chunks start in the sample count, and their wall-clock time.
    chunks: VecDeque<(u64, u64)>,
    next: u64,
    log: AudioLog,
    /// The loudest 10 ms seen, in dBFS, to tell a quiet room from a dead mic.
    loudest_db: f64,
}

impl Listener {
    fn new() -> Self {
        Self {
            loudest_db: f64::NEG_INFINITY,
            ..Default::default()
        }
    }

    fn started(&mut self, id: u32, wall_us: u64) {
        self.log.emitted(id, wall_us);
    }

    fn push(&mut self, chunk: &Chunk) {
        for frame in chunk.samples.chunks(480) {
            self.loudest_db = self.loudest_db.max(rms_db(frame));
        }
        self.chunks.push_back((self.next, chunk.wall_us));
        if self.chunks.len() > 2000 {
            self.chunks.pop_front();
        }
        self.next += chunk.samples.len() as u64;
        for heard in self.detector.push(&chunk.samples) {
            let Some(&(start, wall)) = self.chunks.iter().rev().find(|(s, _)| *s <= heard.at)
            else {
                continue;
            };
            let at_us = wall + (heard.at - start) * 1_000_000 / u64::from(RATE);
            // The log wants an Instant too; only the wall clock matters here.
            let ago = Duration::from_micros(wall_clock_us().saturating_sub(at_us));
            let at = Instant::now().checked_sub(ago).unwrap_or_else(Instant::now);
            self.log.heard(heard.id, at, at_us);
        }
        self.log.rejected = self.detector.rejected;
    }
}

fn rms_db(samples: &[i16]) -> f64 {
    if samples.is_empty() {
        return f64::NEG_INFINITY;
    }
    let sum: f64 = samples.iter().map(|&s| f64::from(s).powi(2)).sum();
    20.0 * ((sum / samples.len() as f64).sqrt() / 32768.0).log10()
}

pub fn run(args: ListenArgs) -> anyhow::Result<()> {
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(listen(args))
}

async fn listen(args: ListenArgs) -> anyhow::Result<()> {
    let token = args.connection.token("listener")?;
    // Only the data packets matter; don't receive the screen or the voice.
    let mut options = RoomOptions::default();
    options.auto_subscribe = false;
    let (room, mut events) = Room::connect(&args.connection.url, &token, options).await?;
    tracing::info!(room = %room.name(), "listening (mic on stdin)");

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    std::thread::spawn(move || {
        let mut stdin = std::io::stdin().lock();
        loop {
            match read_chunk(&mut stdin) {
                Ok(Some(chunk)) => {
                    if tx.send(chunk).is_err() {
                        break;
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    tracing::error!("mic stream: {e}");
                    break;
                }
            }
        }
    });

    let started = Instant::now();
    let deadline = tokio::time::sleep(Duration::from_secs(args.duration.unwrap_or(u64::MAX / 4)));
    tokio::pin!(deadline);
    let ctrl_c = tokio::signal::ctrl_c();
    tokio::pin!(ctrl_c);
    let mut listener = Listener::new();
    loop {
        tokio::select! {
            _ = &mut deadline => break,
            _ = &mut ctrl_c => break,
            chunk = rx.recv() => match chunk {
                Some(chunk) => listener.push(&chunk),
                None => break,
            },
            event = events.recv() => match event {
                Some(RoomEvent::DataReceived { payload, topic, .. })
                    if topic.as_deref() == Some(wire::AUDIO_TOPIC) =>
                {
                    if let Some((id, at_us)) = wire::decode_audio_mark(&payload) {
                        listener.started(id, at_us);
                    }
                }
                Some(_) => {}
                None => break,
            },
        }
    }
    room.close().await.ok();

    let report = listener.log.report();
    if listener.loudest_db < -70.0 {
        tracing::warn!(
            loudest_db = listener.loudest_db,
            "the mic heard nothing: check its permission and that it isn't muted"
        );
    }
    let results = json!({
        "label": args.label,
        "finished_at_unix_ms": wall_clock_us() / 1000,
        "duration_s": started.elapsed().as_secs_f64(),
        "loudest_dbfs": listener.loudest_db.is_finite().then_some(listener.loudest_db),
        "audio": report,
    });
    std::fs::create_dir_all(&args.out)?;
    let path = args.out.join(format!(
        "{}-{}.json",
        args.label,
        wall_clock_us() / 1_000_000
    ));
    std::fs::write(&path, serde_json::to_string_pretty(&results)?)
        .with_context(|| format!("writing {}", path.display()))?;
    tracing::info!("results written to {}", path.display());
    println!("{}", serde_json::to_string_pretty(&results)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use media::audio::marker::{Generator, FRAME};

    fn encode(chunk: &Chunk) -> Vec<u8> {
        let mut out = chunk.wall_us.to_le_bytes().to_vec();
        out.extend_from_slice(&(chunk.samples.len() as u32).to_le_bytes());
        for s in &chunk.samples {
            out.extend_from_slice(&s.to_le_bytes());
        }
        out
    }

    #[test]
    fn chunks_round_trip_and_end_cleanly() {
        let a = Chunk {
            wall_us: 1_700_000_000_000_000,
            samples: vec![1, -2, 3],
        };
        let b = Chunk {
            wall_us: 1_700_000_000_010_000,
            samples: vec![i16::MIN, i16::MAX],
        };
        let bytes = [encode(&a), encode(&b)].concat();
        let mut input = &bytes[..];
        assert_eq!(read_chunk(&mut input).unwrap(), Some(a));
        assert_eq!(read_chunk(&mut input).unwrap(), Some(b));
        assert_eq!(read_chunk(&mut input).unwrap(), None);
    }

    #[test]
    fn rejects_a_stream_that_isnt_record_mic() {
        let mut bytes = 0u64.to_le_bytes().to_vec();
        bytes.extend_from_slice(&u32::MAX.to_le_bytes());
        assert!(read_chunk(&mut &bytes[..]).is_err());
    }

    #[test]
    fn times_bursts_from_the_chunk_clock() {
        // A burst for id 7 starts 300 ms into the recording; the mic's clock starts at t0.
        let t0 = 1_700_000_000_000_000u64;
        let mut generator = Generator::default();
        let mut listener = Listener::new();
        listener.started(7, t0 + 250_000);
        let mut frame = vec![0i16; FRAME];
        for n in 0..100u64 {
            if n == 30 {
                assert!(generator.start(7));
            }
            generator.fill(&mut frame);
            listener.push(&Chunk {
                wall_us: t0 + n * 10_000,
                samples: frame.clone(),
            });
        }
        // Heard 300 ms in, started at 250 ms: 50 ms one way, give or take the detector's hop.
        assert_eq!(listener.log.one_way.len(), 1);
        assert!(
            (listener.log.one_way[0] - 50.0).abs() <= 2.0,
            "{:?}",
            listener.log.one_way
        );
        assert!(listener.loudest_db > -40.0);
    }
}
