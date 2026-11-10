//! The real mic and speakers, through WebRTC's own device layer (`PlatformAudio`: CoreAudio on
//! macOS, WASAPI on Windows). It plays every remote voice track by itself, and runs echo
//! cancellation, noise suppression and gain control on the mic, with the speaker output as the
//! echo reference. This SDK version always turns all three on for a device track.
//!
//! The rest of the code goes through [`Devices`] and never touches the SDK type.

use livekit::webrtc::audio_source::RtcAudioSource;
use livekit::PlatformAudio;

/// A device chosen on the command line: its index in [`list`], or part of its name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceRef {
    Index(usize),
    Name(String),
}

impl std::str::FromStr for DeviceRef {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s.parse() {
            Ok(index) => Self::Index(index),
            Err(_) => Self::Name(s.to_owned()),
        })
    }
}

impl std::fmt::Display for DeviceRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Index(i) => write!(f, "#{i}"),
            Self::Name(name) => write!(f, "\"{name}\""),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum DeviceError {
    #[error("no audio devices: {0}")]
    Unavailable(String),
    #[error("no {kind} matches {wanted}; there are: {}", .names.join(", "))]
    NotFound {
        kind: &'static str,
        wanted: DeviceRef,
        names: Vec<String>,
    },
    #[error("{wanted} matches more than one {kind}: {}", .names.join(", "))]
    Ambiguous {
        kind: &'static str,
        wanted: DeviceRef,
        names: Vec<String>,
    },
    #[error("couldn't select the {0}: {1}")]
    Select(&'static str, String),
}

/// Which devices to use. `None` is the system default.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeviceChoice {
    pub mic: Option<DeviceRef>,
    pub speaker: Option<DeviceRef>,
}

#[derive(Debug, Clone, Default)]
pub struct DeviceList {
    pub mics: Vec<String>,
    pub speakers: Vec<String>,
}

/// Lists the mics and speakers, in index order.
pub fn list() -> Result<DeviceList, DeviceError> {
    let audio = PlatformAudio::new().map_err(|e| DeviceError::Unavailable(e.to_string()))?;
    Ok(DeviceList {
        mics: audio.recording_devices().map(|d| d.name).collect(),
        speakers: audio.playout_devices().map(|d| d.name).collect(),
    })
}

/// The open mic and speakers. Remote voices play as long as this lives.
pub struct Devices {
    audio: PlatformAudio,
    pub mic: Option<String>,
    pub speaker: Option<String>,
}

impl std::fmt::Debug for Devices {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Devices")
            .field("mic", &self.mic)
            .field("speaker", &self.speaker)
            .finish()
    }
}

impl Devices {
    /// Opens the platform's audio devices and selects the chosen ones. Recording starts when a
    /// track from [`Self::source`] is published.
    pub fn open(choice: &DeviceChoice) -> Result<Self, DeviceError> {
        let audio = PlatformAudio::new().map_err(|e| DeviceError::Unavailable(e.to_string()))?;
        let mut mic = None;
        if let Some(wanted) = &choice.mic {
            let devices: Vec<_> = audio.recording_devices().collect();
            let names: Vec<_> = devices.iter().map(|d| d.name.clone()).collect();
            let i = pick("mic", &names, wanted)?;
            audio
                .set_recording_device(&devices[i].id)
                .map_err(|e| DeviceError::Select("mic", e.to_string()))?;
            mic = Some(names[i].clone());
        }
        let mut speaker = None;
        if let Some(wanted) = &choice.speaker {
            let devices: Vec<_> = audio.playout_devices().collect();
            let names: Vec<_> = devices.iter().map(|d| d.name.clone()).collect();
            let i = pick("speaker", &names, wanted)?;
            audio
                .set_playout_device(&devices[i].id)
                .map_err(|e| DeviceError::Select("speaker", e.to_string()))?;
            speaker = Some(names[i].clone());
        }
        Ok(Self {
            audio,
            mic,
            speaker,
        })
    }

    /// The source for a mic track.
    pub fn source(&self) -> RtcAudioSource {
        self.audio.rtc_source()
    }
}

/// Finds `wanted` in `names`: by index, else by name, where a whole-name match (ignoring case)
/// beats a partial one.
pub fn pick(
    kind: &'static str,
    names: &[String],
    wanted: &DeviceRef,
) -> Result<usize, DeviceError> {
    let not_found = || DeviceError::NotFound {
        kind,
        wanted: wanted.clone(),
        names: names.to_vec(),
    };
    match wanted {
        DeviceRef::Index(i) => (*i < names.len()).then_some(*i).ok_or_else(not_found),
        DeviceRef::Name(name) => {
            let name = name.to_lowercase();
            if let Some(i) = names.iter().position(|n| n.to_lowercase() == name) {
                return Ok(i);
            }
            let mut hits: Vec<usize> = (0..names.len())
                .filter(|&i| names[i].to_lowercase().contains(&name))
                .collect();
            // WebRTC lists the system default twice: as "default (<name>)" and under its own
            // name. Matching both isn't a real ambiguity.
            if hits.len() > 1 {
                hits.retain(|&i| !names[i].starts_with("default ("));
            }
            match hits[..] {
                [i] => Ok(i),
                [] => Err(not_found()),
                _ => Err(DeviceError::Ambiguous {
                    kind,
                    wanted: wanted.clone(),
                    names: hits.iter().map(|&i| names[i].clone()).collect(),
                }),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names() -> Vec<String> {
        [
            "MacBook Pro Microphone",
            "AirPods Pro",
            "AirPods Pro Max",
            "USB Mic",
        ]
        .map(String::from)
        .to_vec()
    }

    fn r(s: &str) -> DeviceRef {
        s.parse().unwrap()
    }

    #[test]
    fn by_index_or_name() {
        assert_eq!(pick("mic", &names(), &r("3")).unwrap(), 3);
        assert_eq!(pick("mic", &names(), &r("macbook")).unwrap(), 0);
        assert_eq!(pick("mic", &names(), &r("usb mic")).unwrap(), 3);
    }

    #[test]
    fn default_alias_isnt_ambiguous() {
        let names = [
            "default (MacBook Pro Microphone)",
            "MacBook Pro Microphone",
            "USB Mic",
        ]
        .map(String::from);
        assert_eq!(pick("mic", &names, &r("macbook")).unwrap(), 1);
        assert_eq!(pick("mic", &names, &r("default")).unwrap(), 0);
    }

    #[test]
    fn whole_name_beats_partial() {
        // "AirPods Pro" is also part of "AirPods Pro Max".
        assert_eq!(pick("mic", &names(), &r("airpods pro")).unwrap(), 1);
    }

    #[test]
    fn ambiguous_and_missing() {
        let err = pick("mic", &names(), &r("airpods")).unwrap_err();
        assert!(matches!(err, DeviceError::Ambiguous { ref names, .. } if names.len() == 2));
        assert!(err.to_string().contains("AirPods Pro Max"));
        let err = pick("speaker", &names(), &r("hdmi")).unwrap_err();
        assert!(matches!(err, DeviceError::NotFound { .. }));
        assert!(err
            .to_string()
            .contains("there are: MacBook Pro Microphone"));
        assert!(matches!(
            pick("mic", &names(), &r("4")),
            Err(DeviceError::NotFound { .. })
        ));
    }
}
