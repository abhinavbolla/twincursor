//! TwinCursor media: screen capture, WebRTC transport via LiveKit, and latency measurement.
//!
//! All media lives here in Rust, never in the app's webview (see CLAUDE.md).

pub mod capture;
pub mod latency;
pub mod marker;
pub mod netstats;
pub mod publish;
pub mod stats;
pub mod subscribe;
pub mod token;
pub mod wire;
