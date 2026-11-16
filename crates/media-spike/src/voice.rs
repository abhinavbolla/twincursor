//! Voice flags shared by `share` and `view`, and the `devices` command.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use clap::{Args, ValueEnum};
use media::audio::{device, AudioMode, DeviceChoice, DeviceRef, VoiceOptions, VoiceState};

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum AudioArg {
    /// No voice.
    Off,
    /// Your mic, and the other side's voice on your speakers.
    Mic,
    /// Marker tones that `view --measure-audio` times (sharer only; no mic or permission).
    Pattern,
    /// The WAV given with --audio-file, over and over (sharer only): a far-end voice for
    /// bench/audio-echo.sh.
    File,
}

#[derive(Args, Clone)]
pub struct VoiceArgs {
    /// What to send as your voice. Given twice, the last one counts, so scripts can default to
    /// `off` and still take an override.
    #[arg(long, value_enum, default_value = "mic", overrides_with = "audio")]
    audio: AudioArg,
    /// The WAV for `--audio file`: 48 kHz, mono, 16-bit.
    #[arg(long)]
    audio_file: Option<PathBuf>,
    /// Microphone to use: its number or part of its name (see `tc-spike devices`). Default:
    /// the system's.
    #[arg(long)]
    mic: Option<DeviceRef>,
    /// Speakers or headphones to play the other side on, as for --mic.
    #[arg(long)]
    speaker: Option<DeviceRef>,
    /// Opus bitrate for the voice, in kbit/s.
    #[arg(long, default_value_t = VoiceOptions::default().bitrate / 1000)]
    voice_kbps: u64,
    /// Send each voice frame once, without the previous one as backup (RED).
    #[arg(long)]
    no_red: bool,
    /// Mute the voice from FROM to TO seconds after starting, as FROM:TO, for scripted checks
    /// (bench/audio-acceptance.sh).
    #[arg(long, value_name = "FROM:TO", value_parser = crate::echo::parse_window)]
    mute_for: Option<(f64, f64)>,
}

impl VoiceArgs {
    /// The voice to send, after checking the mic can be used. The synthetic voices (`pattern`
    /// and `file`) are for the sharer only: the pattern follows the viewer's ticks.
    pub fn mode(&self, allow_synthetic: bool) -> anyhow::Result<AudioMode> {
        if self.audio != AudioArg::Mic && (self.mic.is_some() || self.speaker.is_some()) {
            anyhow::bail!("--mic and --speaker need --audio mic");
        }
        if self.audio != AudioArg::File && self.audio_file.is_some() {
            anyhow::bail!("--audio-file needs --audio file");
        }
        Ok(match self.audio {
            AudioArg::Off => AudioMode::Off,
            AudioArg::Pattern | AudioArg::File if !allow_synthetic => {
                anyhow::bail!("--audio pattern and --audio file are for the sharer only")
            }
            AudioArg::Pattern => AudioMode::Pattern,
            AudioArg::File => match &self.audio_file {
                Some(path) => AudioMode::File(path.clone()),
                None => anyhow::bail!("--audio file needs --audio-file <wav>"),
            },
            AudioArg::Mic => {
                engine::permission::check_mic_permission()?;
                AudioMode::Mic(DeviceChoice {
                    mic: self.mic.clone(),
                    speaker: self.speaker.clone(),
                })
            }
        })
    }

    /// The mute state for this run, with `--mute-for`'s mute and unmute already on their way.
    pub fn state(&self) -> Arc<VoiceState> {
        let state = Arc::new(VoiceState::default());
        if let Some((from, to)) = self.mute_for {
            let state = state.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_secs_f64(from));
                state.set_muted(true);
                std::thread::sleep(Duration::from_secs_f64(to - from));
                state.set_muted(false);
            });
        }
        state
    }

    pub fn options(&self) -> VoiceOptions {
        VoiceOptions {
            bitrate: self.voice_kbps * 1000,
            red: !self.no_red,
        }
    }
}

/// `tc-spike devices`: lists mics and speakers with the numbers `--mic` and `--speaker` take.
pub fn list_devices() -> anyhow::Result<()> {
    let devices = device::list()?;
    for (title, names) in [
        ("Microphones", &devices.mics),
        ("Speakers", &devices.speakers),
    ] {
        println!("{title}:");
        if names.is_empty() {
            println!("  (none)");
        }
        for (i, name) in names.iter().enumerate() {
            println!("  {i}: {name}");
        }
    }
    Ok(())
}
