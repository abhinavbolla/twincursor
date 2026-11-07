//! What the guest is holding down on the sharer's machine, so it can always be let go.

use crate::Action;
use protocol::Button;

#[derive(Debug, Default)]
pub(crate) struct Held {
    /// In press order.
    keys: Vec<String>,
    buttons: Vec<Button>,
}

impl Held {
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty() && self.buttons.is_empty()
    }

    pub fn has_key(&self, code: &str) -> bool {
        self.keys.iter().any(|k| k == code)
    }

    /// Returns false if it was already held: a key repeat.
    pub fn press_key(&mut self, code: &str) -> bool {
        if self.has_key(code) {
            return false;
        }
        self.keys.push(code.to_owned());
        true
    }

    /// Returns whether it was held. Releasing something that isn't is a no-op for the caller.
    pub fn release_key(&mut self, code: &str) -> bool {
        let before = self.keys.len();
        self.keys.retain(|k| k != code);
        self.keys.len() != before
    }

    pub fn press_button(&mut self, button: Button) -> bool {
        if self.buttons.contains(&button) {
            return false;
        }
        self.buttons.push(button);
        true
    }

    pub fn release_button(&mut self, button: Button) -> bool {
        let before = self.buttons.len();
        self.buttons.retain(|b| *b != button);
        self.buttons.len() != before
    }

    /// Releases everything: buttons first, then keys in reverse press order, so modifiers go last.
    pub fn release_all(&mut self) -> Vec<Action> {
        let buttons = self.buttons.drain(..).map(|button| Action::Button {
            button,
            pressed: false,
        });
        let keys = self.keys.drain(..).rev().map(|code| Action::Key {
            code,
            pressed: false,
        });
        buttons.chain(keys).collect()
    }
}
