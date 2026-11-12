//! The real mic and speakers, through WebRTC's own device layer (`PlatformAudio`: CoreAudio on
//! macOS, WASAPI on Windows). It plays every remote voice track by itself, and runs echo
//! cancellation, noise suppression and gain control on the mic, with the speaker output as the
//! echo reference. This SDK version always turns all three on for a device track.
//!
//! The rest of the code goes through [`Devices`] and never touches the SDK type.
//!
//! Devices come and go mid-call (headphones unplugged, AirPods back in their case). The SDK
//! doesn't tell us, so [`Devices::recheck`] looks at the list every couple of seconds: when the
//! device in use is gone, or the system default has moved, it switches to the system default.

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

/// WebRTC lists the system default first, as "default (<name>)", and again under its own name.
const DEFAULT_PREFIX: &str = "default (";

/// The system-default entry in a device list, and the device behind it.
fn default_entry(names: &[String]) -> Option<(usize, &str)> {
    names.iter().enumerate().find_map(|(i, name)| {
        let device = name.strip_prefix(DEFAULT_PREFIX)?.strip_suffix(')')?;
        Some((i, device))
    })
}

/// Which device one direction (mic or speakers) is on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InUse {
    /// The system default, which was this device when we last looked.
    Default(Option<String>),
    /// A device picked by name.
    Named(String),
}

impl InUse {
    fn default_of(names: &[String]) -> Self {
        Self::Default(default_entry(names).map(|(_, device)| device.to_owned()))
    }

    /// The device's name, for the HUD and the log.
    pub fn name(&self) -> &str {
        match self {
            Self::Named(name) | Self::Default(Some(name)) => name,
            Self::Default(None) => "system default",
        }
    }
}

/// Decides what to do when the device list may have changed: the entry to switch to and what
/// is then in use, or `None` to stay put.
///
/// - A named device that is gone falls back to the system default.
/// - The system default follows the system: when it moves to another device, so do we.
pub fn fallback(in_use: &InUse, names: &[String]) -> Option<(usize, InUse)> {
    match in_use {
        InUse::Named(name) => {
            if names.iter().any(|n| n == name) || names.is_empty() {
                return None;
            }
            let i = default_entry(names).map_or(0, |(i, _)| i);
            Some((i, InUse::default_of(names)))
        }
        InUse::Default(was) => {
            let (i, now) = default_entry(names)?;
            (was.as_deref() != Some(now)).then(|| (i, InUse::Default(Some(now.to_owned()))))
        }
    }
}

/// The open mic and speakers. Remote voices play as long as this lives.
pub struct Devices {
    audio: PlatformAudio,
    mic_in_use: InUse,
    speaker_in_use: InUse,
}

impl std::fmt::Debug for Devices {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Devices")
            .field("mic", &self.mic_in_use)
            .field("speaker", &self.speaker_in_use)
            .finish()
    }
}

impl Devices {
    /// Opens the platform's audio devices and selects the chosen ones. Recording starts when a
    /// track from [`Self::source`] is published.
    ///
    /// Selection goes through the SDK's live switch: if the other side is already talking when
    /// we join, playout has started, and a plain "set" is refused once a device is running.
    pub fn open(choice: &DeviceChoice) -> Result<Self, DeviceError> {
        let audio = PlatformAudio::new().map_err(|e| DeviceError::Unavailable(e.to_string()))?;
        let devices: Vec<_> = audio.recording_devices().collect();
        let names: Vec<_> = devices.iter().map(|d| d.name.clone()).collect();
        let mut mic_in_use = InUse::default_of(&names);
        if let Some(wanted) = &choice.mic {
            let i = pick("mic", &names, wanted)?;
            audio
                .switch_recording_device(&devices[i].id)
                .map_err(|e| DeviceError::Select("mic", e.to_string()))?;
            mic_in_use = in_use(&names, i);
        }
        let devices: Vec<_> = audio.playout_devices().collect();
        let names: Vec<_> = devices.iter().map(|d| d.name.clone()).collect();
        let mut speaker_in_use = InUse::default_of(&names);
        if let Some(wanted) = &choice.speaker {
            let i = pick("speaker", &names, wanted)?;
            audio
                .switch_playout_device(&devices[i].id)
                .map_err(|e| DeviceError::Select("speaker", e.to_string()))?;
            speaker_in_use = in_use(&names, i);
        }
        Ok(Self {
            audio,
            mic_in_use,
            speaker_in_use,
        })
    }

    /// The mic and the speakers in use, by name.
    pub fn in_use(&self) -> (&str, &str) {
        (self.mic_in_use.name(), self.speaker_in_use.name())
    }

    /// Looks at the devices again and moves to the system default if the mic or the speakers
    /// in use are gone, or if the system default has moved. Returns whether anything changed.
    pub fn recheck(&mut self) -> bool {
        let mut changed = false;
        let devices: Vec<_> = self.audio.recording_devices().collect();
        let names: Vec<_> = devices.iter().map(|d| d.name.clone()).collect();
        if let Some((i, next)) = fallback(&self.mic_in_use, &names) {
            match self.audio.switch_recording_device(&devices[i].id) {
                Ok(()) => {
                    tracing::info!(
                        from = self.mic_in_use.name(),
                        to = next.name(),
                        "mic changed"
                    );
                    self.mic_in_use = next;
                    changed = true;
                }
                Err(e) => tracing::warn!(to = next.name(), "couldn't switch the mic: {e}"),
            }
        }
        let devices: Vec<_> = self.audio.playout_devices().collect();
        let names: Vec<_> = devices.iter().map(|d| d.name.clone()).collect();
        if let Some((i, next)) = fallback(&self.speaker_in_use, &names) {
            match self.audio.switch_playout_device(&devices[i].id) {
                Ok(()) => {
                    tracing::info!(
                        from = self.speaker_in_use.name(),
                        to = next.name(),
                        "speakers changed"
                    );
                    self.speaker_in_use = next;
                    changed = true;
                }
                Err(e) => tracing::warn!(to = next.name(), "couldn't switch the speakers: {e}"),
            }
        }
        changed
    }

    /// The source for a mic track.
    pub fn source(&self) -> RtcAudioSource {
        self.audio.rtc_source()
    }
}

/// What entry `i` of a device list is: the system default, or a device of its own.
fn in_use(names: &[String], i: usize) -> InUse {
    match default_entry(names) {
        Some((d, device)) if d == i => InUse::Default(Some(device.to_owned())),
        _ => InUse::Named(names[i].clone()),
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

    fn list(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| (*n).to_owned()).collect()
    }

    #[test]
    fn a_named_device_that_is_unplugged_falls_back_to_the_default() {
        let with = list(&[
            "default (AirPods Pro)",
            "MacBook Pro Microphone",
            "AirPods Pro",
        ]);
        let airpods = InUse::Named("AirPods Pro".into());
        assert_eq!(fallback(&airpods, &with), None);
        // The AirPods go back in their case: macOS moves its default to the built-in mic.
        let without = list(&["default (MacBook Pro Microphone)", "MacBook Pro Microphone"]);
        let (i, next) = fallback(&airpods, &without).unwrap();
        assert_eq!(i, 0);
        assert_eq!(next, InUse::Default(Some("MacBook Pro Microphone".into())));
        assert_eq!(next.name(), "MacBook Pro Microphone");
        // Settled: nothing more to do, even when the AirPods come back as a plain entry.
        assert_eq!(fallback(&next, &without), None);
        let back = list(&[
            "default (MacBook Pro Microphone)",
            "MacBook Pro Microphone",
            "AirPods Pro",
        ]);
        assert_eq!(fallback(&next, &back), None);
    }

    #[test]
    fn the_system_default_is_followed_when_it_moves() {
        let before = list(&["default (MacBook Pro Speakers)", "MacBook Pro Speakers"]);
        let in_use = InUse::default_of(&before);
        assert_eq!(in_use, InUse::Default(Some("MacBook Pro Speakers".into())));
        assert_eq!(fallback(&in_use, &before), None);
        // Headphones plugged in: macOS makes them the default.
        let after = list(&[
            "default (External Headphones)",
            "MacBook Pro Speakers",
            "External Headphones",
        ]);
        let (i, next) = fallback(&in_use, &after).unwrap();
        assert_eq!((i, next.name()), (0, "External Headphones"));
    }

    #[test]
    fn lists_without_a_default_entry_or_without_devices() {
        // No "default (…)" entry: a missing named device falls back to the first one, and the
        // default itself can't be followed.
        let plain = list(&["USB Mic", "Line In"]);
        let (i, next) = fallback(&InUse::Named("Webcam".into()), &plain).unwrap();
        assert_eq!((i, next.name()), (0, "system default"));
        assert_eq!(fallback(&InUse::Default(None), &plain), None);
        // Nothing to fall back to.
        assert_eq!(fallback(&InUse::Named("Webcam".into()), &[]), None);
        assert_eq!(fallback(&InUse::Default(Some("Webcam".into())), &[]), None);
    }

    #[test]
    fn picking_the_default_entry_follows_the_default() {
        let names = list(&["default (USB Mic)", "MacBook Pro Microphone", "USB Mic"]);
        assert_eq!(in_use(&names, 0), InUse::Default(Some("USB Mic".into())));
        assert_eq!(in_use(&names, 2), InUse::Named("USB Mic".into()));
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
