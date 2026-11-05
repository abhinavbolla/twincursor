//! How a guest's key press becomes input on the sharer's Mac. ADR 0004 has the reasoning.

use crate::held::Held;

/// Held on the sharer, these turn a key into a shortcut, so it's pressed rather than typed.
/// Option isn't one: on a Mac it types characters.
const SHORTCUT_MODIFIERS: [&str; 4] = ["ControlLeft", "ControlRight", "MetaLeft", "MetaRight"];

#[derive(Debug, Clone, Copy)]
pub(crate) struct Keyboard {
    /// A Windows or Linux guest's Ctrl becomes ⌘, and the reverse, so Ctrl+C copies.
    pub swap_ctrl_cmd: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Press {
    /// Type this text, whatever the sharer's layout.
    Text(String),
    /// Press this physical key.
    Key(String),
}

impl Keyboard {
    /// The key to press on the sharer for the guest's physical key.
    pub fn sharer_code(&self, code: &str) -> String {
        let swapped = match code {
            "ControlLeft" if self.swap_ctrl_cmd => "MetaLeft",
            "ControlRight" if self.swap_ctrl_cmd => "MetaRight",
            "MetaLeft" if self.swap_ctrl_cmd => "ControlLeft",
            "MetaRight" if self.swap_ctrl_cmd => "ControlRight",
            other => other,
        };
        swapped.to_owned()
    }

    /// Types the key's text when it has some and no shortcut modifier is held, so the guest's
    /// layout is respected. Otherwise presses the key, so shortcuts and keys like Enter work.
    pub fn press(&self, sharer_code: String, text: Option<String>, held: &Held) -> Press {
        let shortcut = SHORTCUT_MODIFIERS.iter().any(|m| held.has_key(m));
        match text {
            Some(text) if !shortcut && !text.chars().any(char::is_control) => Press::Text(text),
            _ => Press::Key(sharer_code),
        }
    }
}
