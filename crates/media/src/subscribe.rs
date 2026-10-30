//! Viewer: subscribes to the screen track, decodes frames, and sends latency ticks.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures::StreamExt;
use livekit::prelude::*;
use livekit::webrtc::video_frame::{I420Buffer, VideoBuffer};
use livekit::webrtc::video_stream::native::NativeVideoStream;

use crate::latency::Recorder;
use crate::netstats::{Board, Poller, Side};
use crate::{marker, wire};

const STATS_EVERY: Duration = Duration::from_secs(1);

#[derive(Debug, Clone)]
pub struct ViewConfig {
    pub url: String,
    pub token: String,
    pub tick_interval: Duration,
    /// Poll WebRTC stats for the HUD.
    pub net_stats: bool,
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
        recorder,
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
                    tokio::spawn(receive_frames(stream, on_frame.clone()));
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

async fn receive_frames(mut stream: NativeVideoStream, on_frame: Arc<impl Fn(DecodedFrame)>) {
    while let Some(frame) = stream.next().await {
        let i420 = frame.buffer.to_i420();
        let (w, h) = (i420.width() as usize, i420.height() as usize);
        let (stride_y, _, _) = i420.strides();
        let marker = marker::read_luma(i420.data().0, stride_y as usize, w, h);
        on_frame(DecodedFrame {
            i420,
            marker,
            decoded_at: Instant::now(),
            captured_at_us: frame.frame_metadata.and_then(|m| m.user_timestamp),
        });
    }
}
