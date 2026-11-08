//! Voice: the mic track each side publishes, and how the other side's audio is received and
//! measured.
//!
//! Only synthetic audio so far ([`AudioMode::Pattern`]): marker tones that need no device or
//! permission, so latency can be measured headless and in the network lab.

pub mod marker;
pub mod measure;
mod tone;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use futures::StreamExt;
use livekit::options::{AudioEncoding, TrackPublishOptions};
use livekit::prelude::*;
use livekit::webrtc::audio_source::native::NativeAudioSource;
use livekit::webrtc::audio_source::{AudioSourceOptions, RtcAudioSource};
use livekit::webrtc::audio_stream::native::NativeAudioStream;

use crate::capture::wall_clock_us;
use crate::latency::Recorder;
use crate::wire;
pub use tone::{TickSlot, ToneSource};

/// What the sharer sends as its voice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AudioMode {
    #[default]
    Off,
    /// Marker tones driven by the viewer's ticks (see [`marker`]).
    Pattern,
}

/// How the voice track is encoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VoiceOptions {
    /// Opus bitrate in bit/s.
    pub bitrate: u64,
    /// RED: each packet also carries the previous frame, so a single lost packet costs nothing,
    /// at about twice the bitrate on the wire.
    pub red: bool,
}

impl Default for VoiceOptions {
    /// The network lab chose these (see docs/performance.md, Voice).
    fn default() -> Self {
        Self {
            bitrate: 32_000,
            red: true,
        }
    }
}

/// Publishes the synthetic voice track and starts its tones. Each burst's start time goes to
/// the viewers on [`wire::AUDIO_TOPIC`], for the one-way measurement.
pub async fn publish_pattern(
    room: &Room,
    ticks: Arc<TickSlot>,
    voice: VoiceOptions,
) -> Result<(LocalTrackPublication, ToneSource), RoomError> {
    // No queue: each 10 ms frame goes straight to the encoder, as from a mic callback.
    let source = NativeAudioSource::new(AudioSourceOptions::default(), marker::RATE, 1, 0);
    let track =
        LocalAudioTrack::create_audio_track("voice", RtcAudioSource::Native(source.clone()));
    let options = TrackPublishOptions {
        source: TrackSource::Microphone,
        audio_encoding: Some(AudioEncoding {
            max_bitrate: voice.bitrate,
        }),
        red: voice.red,
        ..Default::default()
    };
    let publication = room
        .local_participant()
        .publish_track(LocalTrack::Audio(track), options)
        .await?;
    tracing::info!(?voice, "voice track published (synthetic tones)");

    let (marks, mut rx) = tokio::sync::mpsc::unbounded_channel::<(u32, u64)>();
    let participant = room.local_participant();
    tokio::spawn(async move {
        while let Some((id, at_us)) = rx.recv().await {
            let packet = DataPacket {
                payload: wire::encode_audio_mark(id, at_us),
                topic: Some(wire::AUDIO_TOPIC.into()),
                reliable: true,
                ..Default::default()
            };
            if let Err(e) = participant.publish_data(packet).await {
                tracing::debug!("audio mark not sent: {e}");
            }
        }
    });
    let tones = tone::start(source, ticks, move |id, at_us| {
        let _ = marks.send((id, at_us));
    });
    Ok((publication, tones))
}

/// Listens to a remote voice track for markers until it ends.
pub async fn measure(track: RemoteAudioTrack, recorder: Arc<Mutex<Recorder>>) {
    let mut stream = NativeAudioStream::new(track.rtc_track(), marker::RATE as i32, 1);
    let mut detector = marker::Detector::default();
    // Where each recent frame starts in the sample count, and when it arrived. Frames arrive
    // as the receiver plays them out, so a sample's arrival is when it would be heard.
    let mut frames: VecDeque<(u64, Instant, u64)> = VecDeque::new();
    let mut next = 0u64;
    while let Some(frame) = stream.next().await {
        let (now, wall) = (Instant::now(), wall_clock_us());
        frames.push_back((next, now, wall));
        if frames.len() > 300 {
            frames.pop_front();
        }
        next += frame.data.len() as u64;
        let heard = detector.push(&frame.data);
        let mut recorder = recorder.lock().unwrap();
        recorder.audio.rejected = detector.rejected;
        for h in heard {
            let Some(&(start, at, at_wall)) = frames.iter().rev().find(|(s, _, _)| *s <= h.at)
            else {
                continue;
            };
            let offset_us = (h.at - start) * 1_000_000 / u64::from(marker::RATE);
            recorder.audio.heard(
                h.id,
                at + std::time::Duration::from_micros(offset_us),
                at_wall + offset_us,
            );
        }
    }
}
