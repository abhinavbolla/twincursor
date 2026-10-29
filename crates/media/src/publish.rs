//! Sharer: publishes captured frames as a screen-share track and stamps latency markers.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use livekit::options::{TrackPublishOptions, VideoCodec, VideoEncoding};
use livekit::prelude::*;
use livekit::webrtc::video_frame::{FrameMetadata, VideoBuffer, VideoFrame, VideoRotation};
use livekit::webrtc::video_source::native::NativeVideoSource;
use livekit::webrtc::video_source::{RtcVideoSource, VideoResolution};

use crate::capture::{self, wall_clock_us, Backend, CaptureConfig, Frame, FrameBuffer};
use crate::netstats::{NetSnapshot, Poller, Side};
use crate::{marker, wire};

/// SCK only delivers frames when the screen changes. Re-sending the last frame keeps the
/// encoder refining quality and lets a tick be answered on a static screen.
const IDLE_RESEND: Duration = Duration::from_millis(250);
const NO_TICK: u32 = u32::MAX;
const STATS_EVERY: Duration = Duration::from_secs(1);

#[derive(Debug, Clone)]
pub struct ShareConfig {
    pub url: String,
    pub token: String,
    pub backend: Backend,
    pub capture: CaptureConfig,
    pub codec: VideoCodec,
    pub max_bitrate: u64,
    pub is_screencast: bool,
    pub stamp_markers: bool,
    /// Poll WebRTC stats and send them to the viewer.
    pub net_stats: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum ShareError {
    #[error(transparent)]
    Room(#[from] RoomError),
    #[error(transparent)]
    Capture(#[from] capture::CaptureError),
}

/// Counters the sharer reports periodically.
#[derive(Debug, Default)]
pub struct ShareStats {
    pub captured: AtomicU64,
    pub sent: AtomicU64,
    pub resent: AtomicU64,
    pub ticks: AtomicU64,
    /// Latest network stats of the publishing connection.
    pub net: Mutex<Option<NetSnapshot>>,
}

enum Msg {
    Frame(Frame),
    Tick,
}

/// Connects, publishes, and streams frames until `shutdown` resolves.
pub async fn run(
    config: ShareConfig,
    stats: Arc<ShareStats>,
    shutdown: impl std::future::Future<Output = ()>,
) -> Result<(), ShareError> {
    let (room, mut events) =
        Room::connect(&config.url, &config.token, RoomOptions::default()).await?;
    tracing::info!(room = %room.name(), "connected as sharer");

    let resolution = VideoResolution {
        width: config.capture.width,
        height: config.capture.height,
    };
    let source = NativeVideoSource::new(resolution, config.is_screencast);
    let track =
        LocalVideoTrack::create_video_track("screen", RtcVideoSource::Native(source.clone()));
    let mut options = TrackPublishOptions {
        source: TrackSource::Screenshare,
        video_codec: config.codec,
        simulcast: false,
        video_encoding: Some(VideoEncoding {
            max_bitrate: config.max_bitrate,
            max_framerate: f64::from(config.capture.max_fps),
        }),
        ..Default::default()
    };
    options.frame_metadata_features.user_timestamp = true;
    room.local_participant()
        .publish_track(LocalTrack::Video(track), options)
        .await?;
    tracing::info!(codec = ?config.codec, "screen track published");

    let (tx, rx) = mpsc::sync_channel::<Msg>(4);
    let latest_tick = Arc::new(AtomicU32::new(NO_TICK));

    let sender = {
        let latest_tick = latest_tick.clone();
        let stats = stats.clone();
        let stamp = config.stamp_markers;
        std::thread::spawn(move || send_loop(&rx, &source, &latest_tick, &stats, stamp))
    };
    let _capture = {
        let tx = tx.clone();
        let stats = stats.clone();
        capture::start(config.backend, config.capture, move |frame| {
            stats.captured.fetch_add(1, Ordering::Relaxed);
            // Drop the frame rather than queue it if the sender falls behind.
            let _ = tx.try_send(Msg::Frame(frame));
        })?
    };

    let mut poller = Poller::new(Side::Publisher);
    let mut stats_timer = tokio::time::interval(STATS_EVERY);
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            _ = &mut shutdown => break,
            _ = stats_timer.tick(), if config.net_stats => {
                if let Some(snap) = poller.poll(&room).await {
                    send_stats(&room, &snap).await;
                    *stats.net.lock().unwrap() = Some(snap);
                }
            }
            event = events.recv() => match event {
                Some(RoomEvent::DataReceived { payload, topic, .. }) => {
                    if topic.as_deref() == Some(wire::TICK_TOPIC) {
                        if let Some(id) = wire::decode_tick(&payload) {
                            latest_tick.store(id, Ordering::Relaxed);
                            stats.ticks.fetch_add(1, Ordering::Relaxed);
                            notify(&tx, Msg::Tick);
                        }
                    }
                }
                Some(RoomEvent::Disconnected { reason }) => {
                    tracing::warn!(?reason, "disconnected");
                    break;
                }
                Some(_) => {}
                None => break,
            }
        }
    }

    drop(_capture);
    drop(tx);
    let _ = sender.join();
    room.close().await?;
    Ok(())
}

/// Shares our stats with the viewer so its HUD can show both ends of the call.
async fn send_stats(room: &Room, snap: &NetSnapshot) {
    let packet = DataPacket {
        payload: wire::encode_stats(snap),
        topic: Some(wire::STATS_TOPIC.into()),
        reliable: true,
        ..Default::default()
    };
    if let Err(e) = room.local_participant().publish_data(packet).await {
        tracing::debug!("stats not sent: {e}");
    }
}

fn notify(tx: &SyncSender<Msg>, msg: Msg) {
    let _ = tx.try_send(msg);
}

fn send_loop(
    rx: &mpsc::Receiver<Msg>,
    source: &NativeVideoSource,
    latest_tick: &AtomicU32,
    stats: &ShareStats,
    stamp: bool,
) {
    let start = Instant::now();
    let mut last: Option<Frame> = None;
    loop {
        let fresh = match rx.recv_timeout(IDLE_RESEND) {
            Ok(Msg::Frame(frame)) => {
                last = Some(frame);
                true
            }
            Ok(Msg::Tick) | Err(RecvTimeoutError::Timeout) => false,
            Err(RecvTimeoutError::Disconnected) => return,
        };
        let Some(frame) = last.as_mut() else { continue };
        let tick = latest_tick.load(Ordering::Relaxed);
        if stamp && tick != NO_TICK {
            stamp_marker(&mut frame.buffer, tick);
        }
        // A re-sent frame shows content as of now, so that's its capture time.
        let captured_at_us = if fresh {
            frame.captured_at_us
        } else {
            wall_clock_us()
        };
        let timestamp_us = start.elapsed().as_micros() as i64;
        match &frame.buffer {
            FrameBuffer::Nv12(b) => send(source, b, timestamp_us, captured_at_us),
            FrameBuffer::I420(b) => send(source, b, timestamp_us, captured_at_us),
        }
        let counter = if fresh { &stats.sent } else { &stats.resent };
        counter.fetch_add(1, Ordering::Relaxed);
    }
}

fn send<T: AsRef<dyn VideoBuffer>>(
    source: &NativeVideoSource,
    buffer: T,
    timestamp_us: i64,
    captured_at_us: u64,
) {
    source.capture_frame(&VideoFrame {
        rotation: VideoRotation::VideoRotation0,
        timestamp_us,
        frame_metadata: Some(FrameMetadata {
            user_timestamp: Some(captured_at_us),
            frame_id: None,
            user_data: None,
        }),
        buffer,
    });
}

fn stamp_marker(buffer: &mut FrameBuffer, id: u32) {
    let (w, h) = (buffer.width() as usize, buffer.height() as usize);
    match buffer {
        FrameBuffer::Nv12(b) => {
            let (sy, suv) = b.strides();
            let (y, uv) = b.data_mut();
            if marker::stamp_luma(y, sy as usize, w, h, id) {
                marker::neutral_chroma_nv12(uv, suv as usize);
            }
        }
        FrameBuffer::I420(b) => {
            let (sy, su, sv) = b.strides();
            let (y, u, v) = b.data_mut();
            if marker::stamp_luma(y, sy as usize, w, h, id) {
                marker::neutral_chroma_planar(u, su as usize);
                marker::neutral_chroma_planar(v, sv as usize);
            }
        }
    }
}
