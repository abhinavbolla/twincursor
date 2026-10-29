//! Viewer: subscribes to the screen track, decodes frames, and sends latency ticks.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures::StreamExt;
use livekit::prelude::*;
use livekit::webrtc::video_frame::{I420Buffer, VideoBuffer};
use livekit::webrtc::video_stream::native::NativeVideoStream;

use crate::latency::Recorder;
use crate::netstats::{Board, Poller, Side};
use crate::pattern::{fast_scrolling, Pattern};
use crate::quality::{self, Plane, Snapshots};
use crate::{marker, wire};

const STATS_EVERY: Duration = Duration::from_secs(1);

#[derive(Debug, Clone)]
pub struct ViewConfig {
    pub url: String,
    pub token: String,
    pub tick_interval: Duration,
    /// Poll WebRTC stats for the HUD.
    pub net_stats: bool,
    /// Score decoded frames against the synthetic pattern (`share --capture pattern`).
    pub measure_quality: bool,
    pub snapshots: Option<Snapshots>,
}

/// A decoded frame ready to render.
pub struct DecodedFrame {
    pub i420: I420Buffer,
    pub marker: Option<u32>,
    pub decoded_at: Instant,
    /// Sharer's wall-clock capture time, carried in the RTP packet trailer.
    pub captured_at_us: Option<u64>,
}

#[derive(Debug, thiserror::Error)]
pub enum ViewError {
    #[error(transparent)]
    Room(#[from] RoomError),
}

/// Connects and delivers decoded frames to `on_frame` until `shutdown` resolves.
pub async fn run(
    config: ViewConfig,
    recorder: Arc<Mutex<Recorder>>,
    board: Arc<Mutex<Board>>,
    on_frame: impl Fn(DecodedFrame) + Send + Sync + 'static,
    shutdown: impl std::future::Future<Output = ()>,
) -> Result<(), ViewError> {
    if let Err(e) = livekit::webrtc::enable_zero_playout_delay() {
        tracing::warn!("zero playout delay not enabled: {e}");
    }
    let (room, mut events) =
        Room::connect(&config.url, &config.token, RoomOptions::default()).await?;
    tracing::info!(room = %room.name(), "connected as viewer");

    let on_frame = Arc::new(on_frame);
    let ticker = tokio::spawn(send_ticks(
        room.local_participant(),
        recorder.clone(),
        config.tick_interval,
    ));

    let started = Instant::now();
    let mut poller = Poller::new(Side::Subscriber);
    let mut stats_timer = tokio::time::interval(STATS_EVERY);
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            _ = &mut shutdown => break,
            _ = stats_timer.tick(), if config.net_stats => {
                if let Some(snap) = poller.poll(&room).await {
                    board.lock().unwrap().local_update(started.elapsed().as_secs_f64(), snap);
                }
            }
            event = events.recv() => match event {
                Some(RoomEvent::TrackSubscribed { track: RemoteTrack::Video(track), participant, .. }) => {
                    tracing::info!(from = %participant.identity(), "video track subscribed");
                    // Creating the timing stream installs the packet-trailer handler, which
                    // is what delivers the sharer's capture timestamp with each frame.
                    let _timing = track.subscribe_timing_events();
                    let stream = NativeVideoStream::new(track.rtc_track());
                    let probe = config
                        .measure_quality
                        .then(|| QualityProbe::start(recorder.clone()));
                    tokio::spawn(receive_frames(
                        stream,
                        on_frame.clone(),
                        probe,
                        config.snapshots.clone(),
                    ));
                }
                Some(RoomEvent::DataReceived { payload, topic, .. })
                    if topic.as_deref() == Some(wire::STATS_TOPIC) =>
                {
                    if let Some(snap) = wire::decode_stats(&payload) {
                        board.lock().unwrap().remote = Some(snap);
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

    ticker.abort();
    room.close().await?;
    Ok(())
}

async fn send_ticks(
    participant: LocalParticipant,
    recorder: Arc<Mutex<Recorder>>,
    every: Duration,
) {
    let mut interval = tokio::time::interval(every);
    let mut id: u32 = 0;
    loop {
        interval.tick().await;
        id = (id + 1) & marker::MAX_ID;
        let packet = DataPacket {
            payload: wire::encode_tick(id),
            topic: Some(wire::TICK_TOPIC.into()),
            reliable: false,
            ..Default::default()
        };
        let sent_at = Instant::now();
        if participant.publish_data(packet).await.is_ok() {
            recorder.lock().unwrap().tick_sent(id, sent_at);
        }
    }
}

async fn receive_frames(
    mut stream: NativeVideoStream,
    on_frame: Arc<impl Fn(DecodedFrame)>,
    probe: Option<QualityProbe>,
    snapshots: Option<Snapshots>,
) {
    let mut last_marker = None;
    while let Some(frame) = stream.next().await {
        let i420 = frame.buffer.to_i420();
        let (w, h) = (i420.width() as usize, i420.height() as usize);
        let (stride_y, _, _) = i420.strides();
        let (y, stride) = (i420.data().0, stride_y as usize);
        let marker = marker::read_luma(y, stride, w, h);
        if let Some(probe) = &probe {
            if let Some(n) = marker::read_luma_at(y, stride, w, h, marker::FRAME_SLOT_X) {
                probe.offer(n, y, stride, w, h);
            }
        }
        if let (Some(snaps), Some(id)) = (&snapshots, marker) {
            if last_marker != Some(id) && snaps.wants(id) {
                snaps.save("viewer", id, y, stride, w, h);
            }
        }
        last_marker = marker;
        on_frame(DecodedFrame {
            i420,
            marker,
            decoded_at: Instant::now(),
            captured_at_us: frame.frame_metadata.and_then(|m| m.user_timestamp),
        });
    }
}

/// Scores decoded pattern frames against what the sharer rendered, on its own thread. Frames
/// that arrive while it's busy are skipped, so it never holds up the frame path.
struct QualityProbe {
    tx: SyncSender<(u32, usize, usize, Vec<u8>)>,
    busy: Arc<AtomicBool>,
}

impl QualityProbe {
    fn start(recorder: Arc<Mutex<Recorder>>) -> Self {
        let (tx, rx) = mpsc::sync_channel::<(u32, usize, usize, Vec<u8>)>(1);
        let busy = Arc::new(AtomicBool::new(false));
        let done = busy.clone();
        std::thread::spawn(move || {
            for (n, w, h, luma) in rx {
                let expected = Pattern::new(w as u32, h as u32).render_i420(n);
                let (sy, _, _) = expected.strides();
                let scores = quality::compare(
                    Plane {
                        data: expected.data().0,
                        stride: sy as usize,
                    },
                    Plane {
                        data: &luma,
                        stride: w,
                    },
                    w,
                    h,
                    marker::MARKED_AREA,
                );
                if let Some(scores) = scores {
                    let mut recorder = recorder.lock().unwrap();
                    recorder.quality.push(scores);
                    if fast_scrolling(n) {
                        recorder.quality_fast.push(scores);
                    }
                }
                done.store(false, Ordering::Release);
            }
        });
        Self { tx, busy }
    }

    fn offer(&self, n: u32, y: &[u8], stride: usize, w: usize, h: usize) {
        if self.busy.swap(true, Ordering::AcqRel) {
            return;
        }
        let mut luma = Vec::with_capacity(w * h);
        for row in 0..h {
            luma.extend_from_slice(&y[row * stride..row * stride + w]);
        }
        if self.tx.try_send((n, w, h, luma)).is_err() {
            self.busy.store(false, Ordering::Release);
        }
    }
}
