//! Voice: the mic track each side publishes, and how the other side's audio is received and
//! measured.
//!
//! A real call uses the mic and speakers ([`device`]). For measuring there's a synthetic voice
//! ([`AudioMode::Pattern`]): marker tones that need no device or permission, so latency can be
//! measured headless and in the network lab.

pub mod device;
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
pub use device::{DeviceChoice, DeviceRef, Devices};
pub use tone::{TickSlot, ToneSource};

/// What a side sends as its voice.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum AudioMode {
    #[default]
    Off,
    /// The mic, and remote voices on the speakers.
    Mic(DeviceChoice),
    /// Marker tones driven by the viewer's ticks (see [`marker`]); sharer only.
    Pattern,
}

/// The voice a side is sending; stops when dropped.
pub enum Voice {
    Mic {
        devices: Devices,
        publication: LocalTrackPublication,
    },
    Pattern {
        publication: LocalTrackPublication,
        tones: ToneSource,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum VoiceError {
    #[error(transparent)]
    Device(#[from] device::DeviceError),
    #[error(transparent)]
    Room(#[from] RoomError),
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

/// Starts sending `mode`'s voice. `ticks` drives the synthetic voice's markers.
pub async fn start(
    room: &Room,
    mode: &AudioMode,
    options: VoiceOptions,
    ticks: Arc<TickSlot>,
) -> Result<Option<Voice>, VoiceError> {
    Ok(match mode {
        AudioMode::Off => None,
        AudioMode::Mic(choice) => {
            let devices = Devices::open(choice)?;
            let track = LocalAudioTrack::create_audio_track("voice", devices.source());
            let publication = publish(room, track, options).await?;
            tracing::info!(
                mic = devices.mic.as_deref().unwrap_or("system default"),
                speaker = devices.speaker.as_deref().unwrap_or("system default"),
                ?options,
                "voice track published (mic)"
            );
            Some(Voice::Mic {
                devices,
                publication,
            })
        }
        AudioMode::Pattern => {
            let (publication, tones) = publish_pattern(room, ticks, options).await?;
            Some(Voice::Pattern { publication, tones })
        }
    })
}

async fn publish(
    room: &Room,
    track: LocalAudioTrack,
    voice: VoiceOptions,
) -> Result<LocalTrackPublication, RoomError> {
    let options = TrackPublishOptions {
        source: TrackSource::Microphone,
        audio_encoding: Some(AudioEncoding {
            max_bitrate: voice.bitrate,
        }),
        red: voice.red,
        ..Default::default()
    };
    room.local_participant()
        .publish_track(LocalTrack::Audio(track), options)
        .await
}

/// Publishes the synthetic voice track and starts its tones. Each burst's start time goes to
/// the viewers on [`wire::AUDIO_TOPIC`], for the one-way measurement.
async fn publish_pattern(
    room: &Room,
    ticks: Arc<TickSlot>,
    voice: VoiceOptions,
) -> Result<(LocalTrackPublication, ToneSource), RoomError> {
    // No queue: each 10 ms frame goes straight to the encoder, as from a mic callback.
    let source = NativeAudioSource::new(AudioSourceOptions::default(), marker::RATE, 1, 0);
    let track =
        LocalAudioTrack::create_audio_track("voice", RtcAudioSource::Native(source.clone()));
    let publication = publish(room, track, voice).await?;
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
