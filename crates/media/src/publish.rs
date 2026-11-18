//! Sharer: publishes captured frames as a screen-share track and stamps latency markers.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use livekit::options::{TrackPublishOptions, VideoCodec, VideoEncoding};
use livekit::prelude::*;
use livekit::webrtc::video_frame::{FrameMetadata, VideoBuffer, VideoFrame, VideoRotation};
use livekit::webrtc::video_source::native::NativeVideoSource;
use livekit::webrtc::video_source::{RtcVideoSource, VideoResolution};

use crate::adapt::{Adapter, BitrateCap};
use crate::audio::{self, AudioMode, TickSlot, VoiceError, VoiceOptions, VoiceState};
use crate::capture::{
    self, capped, wall_clock_us, Backend, CaptureConfig, Frame, FrameBuffer, Tier,
};
use crate::link::{self, ControlLink};
use crate::netstats::{NetSnapshot, Poller, Side};
use crate::ping::{self, Route};
use crate::quality::Snapshots;
use crate::viewport::{self, SizedBy, Viewports};
use crate::{marker, wire};

/// SCK only delivers frames when the screen changes. Re-sending the last frame keeps the
/// encoder refining quality and lets a tick be answered on a static screen.
const IDLE_RESEND: Duration = Duration::from_millis(250);
const NO_TICK: u32 = u32::MAX;
const STATS_EVERY: Duration = Duration::from_secs(1);
/// How often a viewport change held back by the rate limit is looked at again.
const VIEWPORT_CHECK_EVERY: Duration = Duration::from_millis(250);

#[derive(Debug, Clone)]
pub struct ShareConfig {
    pub url: String,
    pub token: String,
    pub backend: Backend,
    pub capture: CaptureConfig,
    /// Pixel size of what is captured (the display, or the shared window), which sizes are
    /// worked out from.
    pub source: (u32, u32),
    /// Send no more than the largest viewer's viewport allows (`crate::viewport`). Off, the
    /// size is the setting's (or the auto-throttle's) whatever viewers show.
    pub fit_viewport: bool,
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
    /// What to send as the sharer's voice.
    pub audio: AudioMode,
    pub voice: VoiceOptions,
}

#[derive(Debug, Clone, Copy)]
pub struct AdaptConfig {
    /// The tier being captured: the most the adapter will send.
    pub max: Tier,
}

#[derive(Debug, thiserror::Error)]
pub enum ShareError {
    #[error(transparent)]
    Room(#[from] RoomError),
    #[error(transparent)]
    Capture(#[from] capture::CaptureError),
    #[error("voice: {0}")]
    Voice(VoiceError),
    /// Capture stopped by itself, for example because the shared window was closed.
    #[error("{0}")]
    Ended(String),
}

/// Counters the sharer reports periodically.
#[derive(Debug, Default)]
pub struct ShareStats {
    pub captured: AtomicU64,
    pub sent: AtomicU64,
    pub resent: AtomicU64,
    pub ticks: AtomicU64,
    /// Captured frames not sent: replaced by a newer one before the encoder took them, or
    /// skipped to keep to the adapter's frame-rate cap.
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

/// Connects, publishes, and streams frames until `shutdown` resolves. With a `control` link,
/// viewers' remote-control messages reach the app through it.
pub async fn run(
    config: ShareConfig,
    stats: Arc<ShareStats>,
    voice_state: Arc<VoiceState>,
    control: Option<ControlLink>,
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
    let mut publication = room
        .local_participant()
        .publish_track(LocalTrack::Video(track.clone()), options.clone())
        .await?;
    tracing::info!(codec = ?config.codec, "screen track published");

    // The voice is published once and never republished with the video.
    let audio_ticks = Arc::new(TickSlot::default());
    let voice = audio::start(&room, &config.audio, config.voice, audio_ticks.clone())
        .await
        .map_err(ShareError::Voice)?;
    let mut voice = audio::Session::new(voice, voice_state.clone());

    let mailbox = Arc::new(Mailbox::default());
    let latest_tick = Arc::new(AtomicU32::new(NO_TICK));
    // Size and frame-rate cap frames are sent at; the adapter and the viewers' viewports
    // lower them, the send loop scales and paces to them.
    let send_size = Arc::new(Mutex::new((
        config.capture.width,
        config.capture.height,
        config.capture.max_fps,
    )));
    let codec_factor = codec_factor(config.codec);
    let mut adapter = config
        .adapt
        .map(|a| Adapter::new(a.max, config.capture.max_fps, config.source, codec_factor));
    let mut sizing = Sizing {
        source: config.source,
        fixed: (config.capture.width, config.capture.height),
        max_fps: config.capture.max_fps,
        fit_viewport: config.fit_viewport,
        viewports: Viewports::default(),
        sized_by: SizedBy::Setting,
    };
    for identity in room.remote_participants().keys() {
        sizing.viewports.joined(&identity.to_string(), 0.0);
    }
    let mut adapt_note: Option<String> = None;
    let mut cap = config
        .adapt
        .map(|_| BitrateCap::new(config.max_bitrate as f64 / 1000.0));
    let mut cap_note: Option<String> = None;
    let mut viewer_net: Option<NetSnapshot> = None;
    let started = Instant::now();

    let sender = {
        let latest_tick = latest_tick.clone();
        let stats = stats.clone();
        let stamp = config.stamp_markers;
        // Only the synthetic pattern carries a frame number to keep through scaling.
        let restamp = config.backend == Backend::Pattern;
        let snapshots = config.snapshots.clone();
        let send_size = send_size.clone();
        let mailbox = mailbox.clone();
        let capture_fps = config.capture.max_fps;
        std::thread::spawn(move || {
            send_loop(
                &mailbox,
                &source,
                &latest_tick,
                &stats,
                &send_size,
                capture_fps,
                stamp,
                restamp,
                snapshots.as_ref(),
            )
        })
    };
    let mut capture = {
        let mailbox = mailbox.clone();
        let stats = stats.clone();
        capture::start(config.backend, config.capture, move |frame| {
            stats.captured.fetch_add(1, Ordering::Relaxed);
            if mailbox.put_frame(frame) {
                stats.dropped.fetch_add(1, Ordering::Relaxed);
            }
        })?
    };

    let pings = tokio::spawn(ping::serve(room.local_participant(), room.subscribe()));
    let link_tasks = control.map(|c| link::start(&room, c));
    let main_echo_await_us = AtomicU32::new(0);

    let mut poller = Poller::new(Side::Publisher);
    let mut stats_timer = tokio::time::interval(STATS_EVERY);
    let mut viewport_timer = tokio::time::interval(VIEWPORT_CHECK_EVERY);
    let mut device_timer = tokio::time::interval(audio::DEVICE_CHECK_EVERY);
    let mut ended = None;
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            _ = &mut shutdown => break,
            reason = capture.ended.wait() => {
                ended = Some(reason);
                break;
            }
            resize = capture.resized.next() => {
                // A shared window changed size and capture already follows it: send the new
                // size, at the adapter's current tier if it runs and within the viewers'
                // viewports, through the same path the adapter's own steps take.
                tracing::info!("window now {}x{}", resize.window.0, resize.window.1);
                sizing.source = resize.window;
                sizing.fixed = resize.output;
                if let Some(adapter) = adapter.as_mut() {
                    adapter.set_display(resize.window);
                }
                let t = started.elapsed().as_secs_f64();
                sizing.viewports.update(sizing.source, t);
                sizing.apply(adapter.as_mut(), &send_size);
            }
            _ = viewport_timer.tick() => {
                // A change the rate limit held back, or a viewer whose grace ran out.
                if sizing.viewports.update(sizing.source, started.elapsed().as_secs_f64()) {
                    sizing.apply(adapter.as_mut(), &send_size);
                }
            }
            _ = voice_state.changed() => {
                voice.apply_mute();
                voice.apply_devices();
            }
            _ = device_timer.tick() => voice.check_devices(),
            _ = stats_timer.tick(), if config.net_stats => {
                if let Some(mut snap) = poller.poll(&room).await {
                    if let Some(a) = adapter.as_mut() {
                        let t = started.elapsed().as_secs_f64();
                        if let Some(change) = a.update(t, &snap, viewer_net.as_ref()) {
                            tracing::info!(
                                "adapt: {} → {}: {}",
                                change.from, change.to, change.reason
                            );
                            let reduced = change.to != a.top();
                            sizing.apply(adapter.as_mut(), &send_size);
                            let (w, h, _) = *send_size.lock().unwrap();
                            adapt_note = reduced
                                .then(|| format!("{} ({w}x{h}): {}", change.to, change.reason));
                        }
                    }
                    if let Some(cap) = cap.as_mut() {
                        let t = started.elapsed().as_secs_f64();
                        if let Some(kbps) = cap.update(t, &snap, viewer_net.as_ref()) {
                            options.video_encoding = Some(VideoEncoding {
                                max_bitrate: (kbps * 1000.0) as u64,
                                max_framerate: f64::from(config.capture.max_fps),
                            });
                            tracing::info!("adapt: max bitrate {:.0} kbit/s (republishing)", kbps);
                            match republish(&room, &publication, &track, options.clone()).await {
                                Ok(p) => publication = p,
                                Err(e) => tracing::warn!("republish failed: {e}"),
                            }
                            cap_note = cap
                                .cap_kbps()
                                .map(|k| format!("capped at {:.1} Mbps (viewer downlink)", k / 1000.0));
                        }
                    }
                    snap.adapt = match (&adapt_note, &cap_note) {
                        (Some(a), Some(c)) => Some(format!("{a}; {c}")),
                        (a, c) => a.clone().or_else(|| c.clone()),
                    };
                    snap.sized_by = Some(sizing.sized_by.to_string());
                    send_stats(&room, &snap).await;
                    *stats.net.lock().unwrap() = Some(snap);
                }
            }
            event = events.recv() => match event {
                Some(event @ (RoomEvent::TrackSubscribed { .. }
                | RoomEvent::TrackUnsubscribed { .. }
                | RoomEvent::TrackMuted { .. }
                | RoomEvent::TrackUnmuted { .. })) => {
                    if let Some(muted) = voice.on_event(&event) {
                        tracing::info!(muted, "the viewer's voice");
                        poller.remote_muted(muted);
                    }
                }
                Some(RoomEvent::ParticipantConnected(p)) => {
                    let t = started.elapsed().as_secs_f64();
                    sizing.viewports.joined(&p.identity().to_string(), t);
                    if sizing.viewports.update(sizing.source, t) {
                        sizing.apply(adapter.as_mut(), &send_size);
                    }
                }
                Some(RoomEvent::ParticipantDisconnected(p)) => {
                    sizing.viewports.left(&p.identity().to_string());
                    if sizing.viewports.update(sizing.source, started.elapsed().as_secs_f64()) {
                        sizing.apply(adapter.as_mut(), &send_size);
                    }
                }
                Some(RoomEvent::DataReceived { payload, topic, participant, .. }) => {
                    // Control messages are the control session's (`link`), except the
                    // viewport, which is about the video and needs no control. Input arrives
                    // here too, many times a second while someone is in control, and the link
                    // decodes it already: only packets that can be a viewport are decoded.
                    if topic.as_deref() == Some(protocol::TOPIC) {
                        let viewport = might_be_viewport(&payload)
                            .then(|| protocol::decode(&payload).ok())
                            .flatten();
                        if let (Some(protocol::Message::Viewport { width, height }), Some(from)) =
                            (viewport, participant.as_ref())
                        {
                            let t = started.elapsed().as_secs_f64();
                            tracing::debug!(from = %from.identity(), "viewport {width}x{height}");
                            sizing.viewports.reported(&from.identity().to_string(), (width, height), t);
                            if sizing.viewports.update(sizing.source, t) {
                                sizing.apply(adapter.as_mut(), &send_size);
                            }
                        }
                    } else if topic.as_deref() == Some(wire::STATS_TOPIC) {
                        viewer_net = wire::decode_stats(&payload);
                    } else if topic.as_deref() == Some(wire::TICK_TOPIC) {
                        if let Some(id) = wire::decode_tick(&payload) {
                            latest_tick.store(id, Ordering::Relaxed);
                            audio_ticks.offer(id);
                            stats.ticks.fetch_add(1, Ordering::Relaxed);
                            mailbox.tick();
                        }
                    } else if topic.as_deref() == Some(wire::PING_TOPIC) {
                        ping::answer(
                            &room.local_participant(),
                            Route::Main,
                            &payload,
                            participant.as_ref(),
                            &main_echo_await_us,
                        )
                        .await;
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

    pings.abort();
    link_tasks.iter().flatten().for_each(|t| t.abort());
    drop(capture);
    mailbox.close();
    let _ = sender.join();
    room.close().await?;
    match ended {
        Some(reason) => Err(ShareError::Ended(reason)),
        None => Ok(()),
    }
}

/// Whether a control-topic payload names the viewport message, as `protocol::encode` writes
/// it. Anything else (input, heartbeats) is left to the control session without decoding.
fn might_be_viewport(payload: &[u8]) -> bool {
    const NAME: &[u8] = b"\"viewport\"";
    payload.windows(NAME.len()).any(|w| w == NAME)
}

/// What the size sent is worked out from, and what decided it last.
struct Sizing {
    /// Pixel size of the display or shared window.
    source: (u32, u32),
    /// The size captured frames arrive at: the most there is to send.
    fixed: (u32, u32),
    max_fps: u32,
    fit_viewport: bool,
    viewports: Viewports,
    sized_by: SizedBy,
}

impl Sizing {
    /// Works out the size and frame rate to send from the adapter's level (or the captured
    /// size) and the viewers' viewports, and hands them to the send loop.
    fn apply(&mut self, adapter: Option<&mut Adapter>, send_size: &Mutex<(u32, u32, u32)>) {
        let (limit, viewport) = if self.fit_viewport {
            (self.viewports.limit(self.source), self.viewports.chosen())
        } else {
            (None, None)
        };
        let (level, fps, top) = match adapter {
            Some(a) => {
                a.set_limit(limit);
                let level = a.level();
                let size = capped(level.tier.size(self.source), Some(self.fixed));
                (size, level.fps, level.tier == a.top().tier)
            }
            None => (self.fixed, self.max_fps, true),
        };
        let (size, by) = viewport::send_size(level, top, limit, viewport);
        let new = (size.0, size.1, fps);
        let mut current = send_size.lock().unwrap();
        if *current != new || by != self.sized_by {
            tracing::info!(
                "sending {}x{} at up to {fps} fps, sized by {by}",
                size.0,
                size.1
            );
        }
        *current = new;
        self.sized_by = by;
    }
}

/// How much more bitrate than H.264 a codec needs for the same text sharpness, for the
/// auto-throttle's floors. H.264 and HEVC run in hardware (VideoToolbox on macOS); the others
/// are software encoders, where VP8 needs about twice.
fn codec_factor(codec: VideoCodec) -> f64 {
    match codec {
        VideoCodec::H264 | VideoCodec::H265 => 1.0,
        _ => 2.0,
    }
}

/// Publishes the track again with new options: the only way the SDK offers to change the max
/// bitrate mid-call. The track and its video source keep running; the viewer briefly sees the
/// track go and come back.
async fn republish(
    room: &Room,
    publication: &LocalTrackPublication,
    track: &LocalVideoTrack,
    options: TrackPublishOptions,
) -> Result<LocalTrackPublication, RoomError> {
    let participant = room.local_participant();
    participant.unpublish_track(&publication.sid()).await?;
    participant
        .publish_track(LocalTrack::Video(track.clone()), options)
        .await
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

// The send thread's whole context; a struct for it would only be unpacked again here.
#[allow(clippy::too_many_arguments)]
fn send_loop(
    mailbox: &Mailbox,
    source: &NativeVideoSource,
    latest_tick: &AtomicU32,
    stats: &ShareStats,
    send_size: &Mutex<(u32, u32, u32)>,
    capture_fps: u32,
    stamp: bool,
    restamp: bool,
    snapshots: Option<&Snapshots>,
) {
    let start = Instant::now();
    let mut last: Option<Frame> = None;
    let mut last_snapshot = NO_TICK;
    let mut pacer = Pacer::new(capture_fps);
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
        let (w, h, fps) = *send_size.lock().unwrap();
        // A skipped frame isn't lost: the newest one is kept and goes out with the next tick
        // or frame.
        if fresh && !pacer.admit(Instant::now(), fps) {
            stats.dropped.fetch_add(1, Ordering::Relaxed);
            continue;
        }
        let mut scaled;
        let buffer = if (frame.buffer.width(), frame.buffer.height()) == (w, h) {
            &mut frame.buffer
        } else {
            scaled = frame.buffer.scaled(w, h);
            if restamp {
                restamp_frame_id(&frame.buffer, &mut scaled);
            }
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

/// Paces new frames to the adapter's frame-rate cap.
///
/// It only acts when the cap is below the capture rate. It keeps a schedule rather than
/// measuring from the last frame sent, so ordinary capture jitter doesn't drop frames. (A first
/// version used "at least 0.9 / fps since the last send" and dropped ~18 of 60 frames a second
/// at full rate.)
struct Pacer {
    capture_fps: u32,
    next_due: Option<Instant>,
}

impl Pacer {
    fn new(capture_fps: u32) -> Self {
        Self {
            capture_fps,
            next_due: None,
        }
    }

    /// Whether a new frame arriving at `now` should be sent under a cap of `fps`.
    fn admit(&mut self, now: Instant, fps: u32) -> bool {
        if fps >= self.capture_fps {
            self.next_due = None;
            return true;
        }
        let interval = Duration::from_secs_f64(1.0 / f64::from(fps.max(1)));
        if let Some(due) = self.next_due {
            // Accept frames a little early, so jitter averages out. Less than half an
            // interval: at a 30 fps cap, 60 fps frames land exactly half an interval early.
            if now + interval / 5 < due {
                return false;
            }
            // Keep the schedule, unless we're more than an interval behind it (a pause):
            // then restart from now instead of bursting to catch up.
            let base = if due + interval > now { due } else { now };
            self.next_due = Some(base + interval);
        } else {
            self.next_due = Some(now + interval);
        }
        true
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

/// An 8-bit luma plane and its stride.
fn luma(buffer: &FrameBuffer) -> (&[u8], usize) {
    match buffer {
        FrameBuffer::Nv12(b) => (b.data().0, b.strides().0 as usize),
        FrameBuffer::I420(b) => (b.data().0, b.strides().0 as usize),
    }
}

/// Carries the synthetic pattern's frame number (second marker slot) from `source` into its
/// scaled copy, at the slot's normal size and place, so the viewer can still read it and score
/// the frame (`pattern::expected` knows the scaling). Scaling smears the blocks past reading.
/// Does nothing when `source` carries no readable number. The slot is inside
/// `marker::MARKED_AREA`, which the comparison leaves out.
fn restamp_frame_id(source: &FrameBuffer, scaled: &mut FrameBuffer) {
    let (y, stride) = luma(source);
    let (w, h) = (source.width() as usize, source.height() as usize);
    let Some(id) = marker::read_luma_at(y, stride, w, h, marker::FRAME_SLOT_X) else {
        return;
    };
    let (w, h) = (scaled.width() as usize, scaled.height() as usize);
    match scaled {
        FrameBuffer::Nv12(b) => {
            let stride = b.strides().0 as usize;
            marker::stamp_luma_at(b.data_mut().0, stride, w, h, marker::FRAME_SLOT_X, id);
        }
        FrameBuffer::I420(b) => {
            let stride = b.strides().0 as usize;
            marker::stamp_luma_at(b.data_mut().0, stride, w, h, marker::FRAME_SLOT_X, id);
        }
    }
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

    #[test]
    fn hardware_codecs_share_the_h264_floors() {
        assert_eq!(codec_factor(VideoCodec::H264), 1.0);
        assert_eq!(codec_factor(VideoCodec::H265), 1.0);
        assert_eq!(codec_factor(VideoCodec::VP8), 2.0);
        assert_eq!(codec_factor(VideoCodec::VP9), 2.0);
        assert_eq!(codec_factor(VideoCodec::AV1), 2.0);
    }

    fn frame(id: u64) -> Frame {
        Frame {
            buffer: FrameBuffer::I420(I420Buffer::new(16, 16)),
            captured_at_us: id,
        }
    }

    /// Admitted frames per second for 10 s of arrivals at `capture` fps with ±`jitter_ms`.
    fn paced_rate(capture: u32, cap: u32, jitter_ms: u64) -> f64 {
        let mut pacer = Pacer::new(capture);
        let t0 = Instant::now();
        let period_us = 1_000_000 / u64::from(capture);
        let mut seed = 0x2545_f491_u32;
        let admitted = (0..u64::from(capture) * 10)
            .filter(|i| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                let jitter = (u64::from(seed) % (2 * jitter_ms * 1000 + 1)) as i64
                    - (jitter_ms * 1000) as i64;
                let at = (i * period_us) as i64 + jitter + 10_000;
                pacer.admit(t0 + Duration::from_micros(at as u64), cap)
            })
            .count();
        admitted as f64 / 10.0
    }

    #[test]
    fn pacer_never_drops_at_the_capture_rate() {
        assert_eq!(paced_rate(60, 60, 3), 60.0);
    }

    #[test]
    fn pacer_holds_the_cap_despite_jitter() {
        let r30 = paced_rate(60, 30, 3);
        let r15 = paced_rate(60, 15, 3);
        assert!((29.0..=31.0).contains(&r30), "30 fps cap gave {r30}");
        assert!((14.5..=15.5).contains(&r15), "15 fps cap gave {r15}");
    }

    #[test]
    fn pacer_restarts_after_a_pause_instead_of_bursting() {
        let mut p = Pacer::new(60);
        let t0 = Instant::now();
        assert!(p.admit(t0, 30));
        // Nothing for 2 s (static screen), then frames at 60 fps again.
        let back = t0 + Duration::from_secs(2);
        let burst = (0..6)
            .filter(|i| p.admit(back + Duration::from_micros(i * 16_667), 30))
            .count();
        assert_eq!(burst, 3, "every other frame, not a catch-up burst");
    }

    #[test]
    fn scaled_pattern_frames_keep_a_readable_frame_number() {
        use crate::pattern::{self, Pattern};
        use crate::quality::{self, Plane};
        let n = 1234;
        let mut source = FrameBuffer::I420(Pattern::new(3840, 2160).render_i420(n));
        let id = pattern::frame_id(n, (3840, 2160));
        for (w, h) in [(1920, 1080), (2656, 1728), (1504, 980), (1280, 720)] {
            let mut scaled = source.scaled(w, h);
            restamp_frame_id(&source, &mut scaled);
            let FrameBuffer::I420(b) = &scaled else {
                unreachable!()
            };
            let (y, stride) = (b.data().0, b.strides().0 as usize);
            let (w, h) = (w as usize, h as usize);
            let read = marker::read_luma_at(y, stride, w, h, marker::FRAME_SLOT_X);
            assert_eq!(read, Some(id), "{w}x{h}");
            // What the viewer compares it with scores as the same picture.
            let expected = pattern::expected(id, (w as u32, h as u32)).expect("a downscale");
            let scores = quality::compare(
                Plane {
                    data: expected.data().0,
                    stride: expected.strides().0 as usize,
                },
                Plane { data: y, stride },
                w,
                h,
                marker::MARKED_AREA,
            )
            .unwrap();
            assert!(scores.ssim > 0.999, "{w}x{h}: {scores:?}");
        }
    }

    #[test]
    fn only_viewports_are_decoded_on_the_control_topic() {
        let viewport = protocol::encode(&protocol::Message::Viewport {
            width: 1920,
            height: 1080,
        });
        assert!(might_be_viewport(&viewport));
        let input = protocol::encode(&protocol::Message::Input {
            seq: 1,
            sent_at_us: 2,
            after: 0,
            event: protocol::InputEvent::Move { x: 0.5, y: 0.5 },
        });
        assert!(!might_be_viewport(&input));
        assert!(!might_be_viewport(&protocol::encode(
            &protocol::Message::Heartbeat { sent_at_us: 1 }
        )));
    }

    #[test]
    fn frames_without_a_frame_number_are_left_alone() {
        // A real screen: nothing readable in the slot, so nothing is stamped.
        let mut grey = I420Buffer::new(1920, 1080);
        grey.data_mut().0.fill(128);
        let source = FrameBuffer::I420(grey);
        let mut scaled = FrameBuffer::I420(I420Buffer::new(1280, 720));
        if let FrameBuffer::I420(b) = &mut scaled {
            b.data_mut().0.fill(128);
        }
        restamp_frame_id(&source, &mut scaled);
        let FrameBuffer::I420(b) = &scaled else {
            unreachable!()
        };
        assert!(b.data().0.iter().all(|&v| v == 128));
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
