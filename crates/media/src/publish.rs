//! Sharer: publishes captured frames as a screen-share track and stamps latency markers.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use livekit::options::{TrackPublishOptions, VideoCodec, VideoEncoding};
use livekit::prelude::*;
use livekit::webrtc::video_frame::{FrameMetadata, VideoBuffer, VideoFrame, VideoRotation};
use livekit::webrtc::video_source::native::NativeVideoSource;
use livekit::webrtc::video_source::{RtcVideoSource, VideoResolution};

use crate::adapt::Adapter;
use crate::capture::{self, wall_clock_us, Backend, CaptureConfig, Frame, FrameBuffer, Tier};
use crate::netstats::{NetSnapshot, Poller, Side};
use crate::quality::Snapshots;
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
    pub snapshots: Option<Snapshots>,
    /// Auto-throttle: step the resolution down from the captured tier when the call can't
    /// sustain it. `None` keeps the captured size.
    pub adapt: Option<AdaptConfig>,
}

#[derive(Debug, Clone, Copy)]
pub struct AdaptConfig {
    /// The tier being captured: the most the adapter will send.
    pub max: Tier,
    /// Display size the tiers are computed from.
    pub display: (u32, u32),
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
    /// Captured frames replaced by a newer one before the encoder took them.
    pub dropped: AtomicU64,
    /// Latest network stats of the publishing connection.
    pub net: Mutex<Option<NetSnapshot>>,
}

/// Hands frames and tick wake-ups to the send loop. It holds only the newest frame: when the
/// encoder falls behind, older frames are dropped rather than queued, because every queued
/// frame adds a frame time (16 ms at 60 fps) of latency.
#[derive(Default)]
struct Mailbox {
    state: Mutex<MailState>,
    ready: Condvar,
}

#[derive(Default)]
struct MailState {
    frame: Option<Frame>,
    tick: bool,
    closed: bool,
}

enum Mail {
    Frame(Frame),
    /// A tick arrived, or nothing did for a while: re-send the last frame.
    Resend,
    Closed,
}

impl Mailbox {
    /// Leaves `frame` for the send loop. Returns `true` if it replaced one never sent.
    fn put_frame(&self, frame: Frame) -> bool {
        let replaced = self.state.lock().unwrap().frame.replace(frame).is_some();
        self.ready.notify_one();
        replaced
    }

    fn tick(&self) {
        self.state.lock().unwrap().tick = true;
        self.ready.notify_one();
    }

    fn close(&self) {
        self.state.lock().unwrap().closed = true;
        self.ready.notify_one();
    }

    /// Waits up to `timeout` for a frame or a tick. A new frame wins over a tick, since it
    /// gets the latest marker stamped anyway.
    fn wait(&self, timeout: std::time::Duration) -> Mail {
        let state = self.state.lock().unwrap();
        let (mut state, _) = self
            .ready
            .wait_timeout_while(state, timeout, |s| {
                s.frame.is_none() && !s.tick && !s.closed
            })
            .unwrap();
        if state.closed {
            return Mail::Closed;
        }
        state.tick = false;
        state.frame.take().map_or(Mail::Resend, Mail::Frame)
    }
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

    let mailbox = Arc::new(Mailbox::default());
    let latest_tick = Arc::new(AtomicU32::new(NO_TICK));
    // Size frames are sent at; the adapter lowers it, the send loop scales to it.
    let send_size = Arc::new(Mutex::new((config.capture.width, config.capture.height)));
    let codec_factor = if config.codec == VideoCodec::H264 {
        1.0
    } else {
        2.0
    };
    let mut adapter = config.adapt.map(|a| (Adapter::new(a.max, codec_factor), a));
    let mut adapt_note: Option<String> = None;
    let mut viewer_net: Option<NetSnapshot> = None;
    let started = Instant::now();

    let sender = {
        let latest_tick = latest_tick.clone();
        let stats = stats.clone();
        let stamp = config.stamp_markers;
        let snapshots = config.snapshots.clone();
        let send_size = send_size.clone();
        let mailbox = mailbox.clone();
        std::thread::spawn(move || {
            send_loop(
                &mailbox,
                &source,
                &latest_tick,
                &stats,
                &send_size,
                stamp,
                snapshots.as_ref(),
            )
        })
    };
    let _capture = {
        let mailbox = mailbox.clone();
        let stats = stats.clone();
        capture::start(config.backend, config.capture, move |frame| {
            stats.captured.fetch_add(1, Ordering::Relaxed);
            if mailbox.put_frame(frame) {
                stats.dropped.fetch_add(1, Ordering::Relaxed);
            }
        })?
    };

    let mut poller = Poller::new(Side::Publisher);
    let mut stats_timer = tokio::time::interval(STATS_EVERY);
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            _ = &mut shutdown => break,
            _ = stats_timer.tick(), if config.net_stats => {
                if let Some(mut snap) = poller.poll(&room).await {
                    if let Some((adapter, cfg)) = adapter.as_mut() {
                        let t = started.elapsed().as_secs_f64();
                        if let Some(change) = adapter.update(t, &snap, viewer_net.as_ref()) {
                            let size = change.to.size(cfg.display);
                            *send_size.lock().unwrap() = size;
                            tracing::info!(
                                "adapt: {:?} → {:?} ({}x{}): {}",
                                change.from, change.to, size.0, size.1, change.reason
                            );
                            adapt_note = Some(format!(
                                "{}p, from {}p: {}",
                                size.1,
                                cfg.max.size(cfg.display).1,
                                change.reason
                            ));
                            if change.to == cfg.max {
                                adapt_note = None;
                            }
                        }
                    }
                    snap.adapt = adapt_note.clone();
                    send_stats(&room, &snap).await;
                    *stats.net.lock().unwrap() = Some(snap);
                }
            }
            event = events.recv() => match event {
                Some(RoomEvent::DataReceived { payload, topic, .. }) => {
                    if topic.as_deref() == Some(wire::STATS_TOPIC) {
                        viewer_net = wire::decode_stats(&payload);
                    } else if topic.as_deref() == Some(wire::TICK_TOPIC) {
                        if let Some(id) = wire::decode_tick(&payload) {
                            latest_tick.store(id, Ordering::Relaxed);
                            stats.ticks.fetch_add(1, Ordering::Relaxed);
                            mailbox.tick();
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
    mailbox.close();
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

fn send_loop(
    mailbox: &Mailbox,
    source: &NativeVideoSource,
    latest_tick: &AtomicU32,
    stats: &ShareStats,
    send_size: &Mutex<(u32, u32)>,
    stamp: bool,
    snapshots: Option<&Snapshots>,
) {
    let start = Instant::now();
    let mut last: Option<Frame> = None;
    let mut last_snapshot = NO_TICK;
    loop {
        let fresh = match mailbox.wait(IDLE_RESEND) {
            Mail::Frame(frame) => {
                last = Some(frame);
                true
            }
            Mail::Resend => false,
            Mail::Closed => return,
        };
        let Some(frame) = last.as_mut() else { continue };
        let (w, h) = *send_size.lock().unwrap();
        let mut scaled;
        let buffer = if (frame.buffer.width(), frame.buffer.height()) == (w, h) {
            &mut frame.buffer
        } else {
            scaled = frame.buffer.scaled(w, h);
            &mut scaled
        };
        let tick = latest_tick.load(Ordering::Relaxed);
        if stamp && tick != NO_TICK {
            stamp_marker(buffer, tick);
            if let Some(snaps) = snapshots.filter(|s| tick != last_snapshot && s.wants(tick)) {
                save_snapshot(snaps, tick, buffer);
                last_snapshot = tick;
            }
        }
        // A re-sent frame shows content as of now, so that's its capture time.
        let captured_at_us = if fresh {
            frame.captured_at_us
        } else {
            wall_clock_us()
        };
        let timestamp_us = start.elapsed().as_micros() as i64;
        match buffer {
            FrameBuffer::Nv12(b) => send(source, &*b, timestamp_us, captured_at_us),
            FrameBuffer::I420(b) => send(source, &*b, timestamp_us, captured_at_us),
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

fn save_snapshot(snaps: &Snapshots, tick: u32, buffer: &FrameBuffer) {
    let (w, h) = (buffer.width() as usize, buffer.height() as usize);
    match buffer {
        FrameBuffer::Nv12(b) => {
            let (sy, _) = b.strides();
            snaps.save("sharer", tick, b.data().0, sy as usize, w, h);
        }
        FrameBuffer::I420(b) => {
            let (sy, _, _) = b.strides();
            snaps.save("sharer", tick, b.data().0, sy as usize, w, h);
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use livekit::webrtc::video_frame::I420Buffer;
    use std::time::Duration;

    fn frame(id: u64) -> Frame {
        Frame {
            buffer: FrameBuffer::I420(I420Buffer::new(16, 16)),
            captured_at_us: id,
        }
    }

    #[test]
    fn mailbox_keeps_only_the_newest_frame() {
        let m = Mailbox::default();
        assert!(!m.put_frame(frame(1)));
        assert!(m.put_frame(frame(2)), "replaced an unsent frame");
        match m.wait(Duration::from_millis(1)) {
            Mail::Frame(f) => assert_eq!(f.captured_at_us, 2),
            _ => panic!("expected the newest frame"),
        }
        assert!(
            matches!(m.wait(Duration::from_millis(1)), Mail::Resend),
            "timeout"
        );
    }

    #[test]
    fn mailbox_tick_wakes_and_frame_wins() {
        let m = Mailbox::default();
        m.tick();
        assert!(matches!(m.wait(Duration::from_secs(5)), Mail::Resend));
        m.tick();
        m.put_frame(frame(3));
        assert!(matches!(m.wait(Duration::from_secs(5)), Mail::Frame(_)));
        assert!(
            matches!(m.wait(Duration::from_millis(1)), Mail::Resend),
            "tick consumed"
        );
    }

    #[test]
    fn mailbox_wakes_a_waiting_thread() {
        let m = Arc::new(Mailbox::default());
        let waiter = {
            let m = m.clone();
            std::thread::spawn(move || matches!(m.wait(Duration::from_secs(5)), Mail::Frame(_)))
        };
        std::thread::sleep(Duration::from_millis(20));
        m.put_frame(frame(4));
        assert!(waiter.join().unwrap());
        m.close();
        assert!(matches!(m.wait(Duration::from_secs(5)), Mail::Closed));
    }
}
