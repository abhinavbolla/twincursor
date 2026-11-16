//! The parts of a TwinCursor call that own the main thread: the viewer window, the sharer's global
//! hotkeys and on-screen overlay, and the threads that run remote control on both sides.
//!
//! `tc-spike` is a command line over this crate. Media itself (capture, WebRTC, voice) lives in
//! `twincursor-media`; the rules of remote control live in `twincursor-control`.

mod error;
mod guest;
pub mod hotkey;
mod hud;
pub mod input;
mod overlay;
pub mod permission;
pub mod sharer;
pub mod view;

pub use error::{Error, ScriptError};
pub use overlay::OverlayHandle;
