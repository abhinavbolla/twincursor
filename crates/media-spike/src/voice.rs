//! Voice flags shared by `share` and `view`, the `devices` command, and the macOS microphone
//! permission check.

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
                check_mic_permission()?;
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

/// Microphone access, as macOS records it for this app (the terminal, when run from one).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
enum MicPermission {
    NotAsked,
    Restricted,
    Denied,
    Granted,
}

impl MicPermission {
    /// From `AVAuthorizationStatus`.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    fn from_status(status: isize) -> Self {
        match status {
            0 => Self::NotAsked,
            1 => Self::Restricted,
            2 => Self::Denied,
            _ => Self::Granted,
        }
    }
}

/// Stops with a clear message when macOS won't let us record; without access, CoreAudio quietly
/// records silence instead of failing.
fn check_mic_permission() -> anyhow::Result<()> {
    match mic_permission() {
        Some(MicPermission::Denied) => anyhow::bail!(
            "microphone access is off for this app. Turn it on in System Settings > Privacy & \
             Security > Microphone, or run with --audio off"
        ),
        Some(MicPermission::Restricted) => anyhow::bail!(
            "microphone access is blocked on this Mac (device management). Run with --audio off"
        ),
        Some(MicPermission::NotAsked) => {
            tracing::info!("macOS will ask whether this app may use the microphone")
        }
        Some(MicPermission::Granted) | None => {}
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn mic_permission() -> Option<MicPermission> {
    use objc2::runtime::{AnyClass, AnyObject};
    use objc2::{class, msg_send};

    #[link(name = "AVFoundation", kind = "framework")]
    extern "C" {
        static AVMediaTypeAudio: &'static AnyObject;
    }
    // Make sure the class is loaded before asking for it by name.
    let _: Option<&AnyClass> = AnyClass::get(c"AVCaptureDevice");
    // SAFETY: `authorizationStatusForMediaType:` is a class method that takes an
    // AVMediaType (an NSString constant AVFoundation exports) and returns an NSInteger.
    let status: isize = unsafe {
        msg_send![class!(AVCaptureDevice), authorizationStatusForMediaType: AVMediaTypeAudio]
    };
    Some(MicPermission::from_status(status))
}

#[cfg(not(target_os = "macos"))]
fn mic_permission() -> Option<MicPermission> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_status_values() {
        assert_eq!(MicPermission::from_status(0), MicPermission::NotAsked);
        assert_eq!(MicPermission::from_status(1), MicPermission::Restricted);
        assert_eq!(MicPermission::from_status(2), MicPermission::Denied);
        assert_eq!(MicPermission::from_status(3), MicPermission::Granted);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn permission_can_be_read() {
        // Reading the status never prompts; it only has to come back as a known value.
        assert!(mic_permission().is_some());
    }
}
