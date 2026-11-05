# ADR 0004: Keyboard model for remote control

Status: accepted (2026-11-05)

## Context
The guest types on their own keyboard, with their own layout, into the sharer's Mac, which may
use a different layout. A key can be sent two ways:

- **As the physical key** (`KeyA`, named as `KeyboardEvent.code` does). Shortcuts work, but the
  sharer's layout decides the character. A German guest pressing the key labelled ö on a US Mac
  would type `;`.
- **As the text it produced** on the guest (`ö`). The guest gets what they see on their keys, but
  text can't express ⌘C, Enter or the arrow keys.

The guest may also be on Windows, where copy is Ctrl+C rather than ⌘C.

## Decision
The viewer sends both, the physical key and its text. The sharer (`crates/control/src/keyboard.rs`)
picks one per key press:

- **Typing goes as text.** If the key produced text without control characters, and no Ctrl or ⌘
  is held on the sharer, the text is typed. Its key-up is then a no-op, because nothing was
  pressed.
- **Everything else goes as the physical key:** modifiers, shortcuts (anything with Ctrl or ⌘
  held), and keys like Enter, Backspace, Tab, Escape and the arrows. These are tracked as held
  until their key-up.
- **Option doesn't count as a shortcut,** because on a Mac it types characters (⌥E then E is é).
- **A Windows or Linux guest's Ctrl becomes ⌘, and the reverse,** so Ctrl+C copies and Ctrl+S
  saves. `swap_ctrl_cmd` turns this off. A Mac guest's keys aren't swapped.
- **Key repeats press again.** A repeated text key types again.

## Consequences
- Characters come out as the guest expects, including accents and other scripts, whatever layout
  the sharer uses.
- Shortcuts follow the sharer's layout, because they're physical keys. On most layouts ⌘C is in
  the same place, but on some, for example AZERTY, a few shortcut keys move.
- Dead keys and input methods, for example for Chinese or Japanese, run on the guest: the sharer
  only sees the finished text.
- Shift and Option are pressed for real while text is typed, so Shift-click and Shift-arrow
  selection work. Whether a Mac app treats typed text differently while Option is held is
  untested. We'll check that with real injection.
- Keys the guest's own OS keeps never reach the viewer: ⌘Tab and ⌘Space on a Mac, and the Windows
  key and Alt+Tab on Windows.
