//! Mute, and what each side knows about the call's voices.
//!
//! [`VoiceState`] is shared between whatever the person presses (a key in the viewer, the
//! sharer's global hotkey) and the session loop that owns the voice track. Pressing only records
//! the wish and wakes the loop; the loop mutes the track and writes back what is true.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use tokio::sync::Notify;

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

#[derive(Debug, Default)]
pub struct VoiceState {
    want_muted: AtomicBool,
    changed: Notify,
    status: Mutex<VoiceStatus>,
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
