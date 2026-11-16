//! What can go wrong in the engine. Messages read the same as `tc-spike` has always printed them.

use std::path::PathBuf;

/// An engine error.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A file, thread or runtime couldn't be made.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// A control script or results file wasn't valid JSON.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    /// The window system's event loop failed.
    #[error(transparent)]
    EventLoop(#[from] winit::error::EventLoopError),
    /// A global hotkey couldn't be set up.
    #[error(transparent)]
    Hotkey(#[from] global_hotkey::Error),
    /// Real input injection couldn't start (macOS).
    #[cfg(target_os = "macos")]
    #[error(transparent)]
    Inject(#[from] control::macos::MacError),
    /// The viewer's call failed.
    #[error(transparent)]
    View(#[from] media::subscribe::ViewError),
    /// The results file couldn't be written.
    #[error("writing {}", path.display())]
    Write {
        /// The results file.
        path: PathBuf,
        /// Why.
        source: std::io::Error,
    },
    /// A headless guest's control script stopped early.
    #[error("control script failed")]
    Script(#[source] ScriptError),
    /// A headless guest's control script panicked.
    #[error("control script panicked")]
    ScriptPanicked,
    /// The sharer's event loop exited before starting the network thread.
    #[error("the event loop ended before it started")]
    NotStarted,
    /// The sharer's network thread panicked.
    #[error("the sharer's network thread panicked")]
    WorkerPanicked,
    /// `Inject::Real` on a platform without an injector.
    #[error("real input injection is macOS only for now")]
    NoRealInjection,
    /// The user turned microphone access off for this app.
    #[error(
        "microphone access is off for this app. Turn it on in System Settings > Privacy & \
         Security > Microphone, or run with --audio off"
    )]
    MicDenied,
    /// Device management blocks the microphone.
    #[error("microphone access is blocked on this Mac (device management). Run with --audio off")]
    MicRestricted,
}

/// Why a control script stopped before its end.
#[derive(Debug, thiserror::Error)]
pub enum ScriptError {
    /// The sharer said no to a request.
    #[error("the sharer turned the request down")]
    Denied,
    /// The sharer didn't answer a request in time.
    #[error("no answer from the sharer")]
    NoAnswer,
    /// Control ended (a revoke, say) before the script had sent everything.
    #[error("control ended while the script was still running")]
    Ended,
}
