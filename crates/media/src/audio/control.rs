//! Mute, and what each side knows about the call's voices.
//!
//! [`VoiceState`] is shared between whatever the person presses (a key in the viewer, the
//! sharer's global hotkey) and the session loop that owns the voice track. Pressing only records
//! the wish and wakes the loop; the loop mutes the track and writes back what is true.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use tokio::sync::Notify;

use super::DeviceRef;

/// What is true about the voices right now, for the HUD and the terminal.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VoiceStatus {
    /// We publish a voice track.
    pub sending: bool,
    /// Our voice track is muted.
    pub muted: bool,
    /// Whether the other side's voice is muted; `None` until it sends one.
    pub remote_muted: Option<bool>,
    /// The mic and speakers in use, when they're real devices.
    pub mic: Option<String>,
    pub speaker: Option<String>,
}

/// Devices asked for mid-call. The outer `Option` is whether to change that direction at all,
/// the inner one the device (`None`: the system default).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeviceWish {
    pub mic: Option<Option<DeviceRef>>,
    pub speaker: Option<Option<DeviceRef>>,
}

#[derive(Debug, Default)]
pub struct VoiceState {
    want_muted: AtomicBool,
    changed: Notify,
    status: Mutex<VoiceStatus>,
    want_devices: Mutex<DeviceWish>,
    device_error: Mutex<Option<String>>,
}

impl VoiceState {
    /// Asks for the mic to be muted or not. Takes effect when the session loop next runs.
    pub fn set_muted(&self, muted: bool) {
        self.want_muted.store(muted, Ordering::Release);
        self.changed.notify_one();
    }

    /// Flips mute and returns what was asked for.
    pub fn toggle_mute(&self) -> bool {
        let muted = !self.want_muted.fetch_xor(true, Ordering::AcqRel);
        self.changed.notify_one();
        muted
    }

    /// Asks for another mic mid-call (`None`: the system default). Takes effect when the session
    /// loop next runs; a failure shows in [`VoiceState::take_device_error`].
    pub fn choose_mic(&self, mic: Option<DeviceRef>) {
        self.want_devices.lock().unwrap().mic = Some(mic);
        self.changed.notify_one();
    }

    /// Asks for other speakers mid-call, as for [`VoiceState::choose_mic`].
    pub fn choose_speaker(&self, speaker: Option<DeviceRef>) {
        self.want_devices.lock().unwrap().speaker = Some(speaker);
        self.changed.notify_one();
    }

    /// Why the last device change didn't happen, once.
    pub fn take_device_error(&self) -> Option<String> {
        self.device_error.lock().unwrap().take()
    }

    pub(crate) fn take_device_wish(&self) -> DeviceWish {
        std::mem::take(&mut *self.want_devices.lock().unwrap())
    }

    pub(crate) fn device_failed(&self, why: String) {
        *self.device_error.lock().unwrap() = Some(why);
    }

    pub fn wants_muted(&self) -> bool {
        self.want_muted.load(Ordering::Acquire)
    }

    /// Resolves when mute has been asked for or taken back since the last time this resolved.
    pub async fn changed(&self) {
        self.changed.notified().await;
    }

    pub fn status(&self) -> VoiceStatus {
        self.status.lock().unwrap().clone()
    }

    pub(crate) fn update(&self, change: impl FnOnce(&mut VoiceStatus)) {
        change(&mut self.status.lock().unwrap());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggling_flips_what_is_wanted() {
        let state = VoiceState::default();
        assert!(!state.wants_muted());
        assert!(state.toggle_mute());
        assert!(state.wants_muted());
        assert!(!state.toggle_mute());
        assert!(!state.wants_muted());
        state.set_muted(true);
        assert!(state.wants_muted());
    }

    #[tokio::test]
    async fn a_press_before_the_loop_waits_is_not_lost() {
        let state = VoiceState::default();
        state.toggle_mute();
        // The loop wasn't waiting yet; it still hears about it.
        tokio::time::timeout(std::time::Duration::from_secs(1), state.changed())
            .await
            .expect("the change was remembered");
    }

    #[test]
    fn device_wishes_are_taken_once() {
        let state = VoiceState::default();
        assert_eq!(state.take_device_wish(), DeviceWish::default());
        state.choose_mic(Some(DeviceRef::Name("USB".into())));
        state.choose_speaker(None);
        let wish = state.take_device_wish();
        assert_eq!(wish.mic, Some(Some(DeviceRef::Name("USB".into()))));
        assert_eq!(wish.speaker, Some(None));
        assert_eq!(state.take_device_wish(), DeviceWish::default());
        state.device_failed("no".into());
        assert_eq!(state.take_device_error().as_deref(), Some("no"));
        assert_eq!(state.take_device_error(), None);
    }

    #[test]
    fn status_starts_empty_and_takes_updates() {
        let state = VoiceState::default();
        assert_eq!(state.status(), VoiceStatus::default());
        state.update(|s| {
            s.sending = true;
            s.remote_muted = Some(true);
        });
        let status = state.status();
        assert!(status.sending && !status.muted);
        assert_eq!(status.remote_muted, Some(true));
    }
}
