//! The sharer's main thread: the global hotkeys and, with `--allow-control`, the overlay.
//!
//! - ⌃⌥⌘R takes remote control back. Its handler only trips the [`RevokeSwitch`]: no lock, no
//!   channel, nothing async.
//! - ⌃⌥⌘M mutes and unmutes the sharer's mic. The sharer has no window of its own, so a key
//!   that works from any app is the only way to reach it.
//!
//! Both need the app's event loop on the main thread, so the network work runs on a background
//! thread.

use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use control::RevokeSwitch;
use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use media::audio::VoiceState;
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::WindowId;

use crate::overlay::{Overlay, OverlayHandle, OverlayState, UiEvent};
use crate::Error;

/// How the revoke hotkey is shown to the sharer in the terminal.
pub const REVOKE: &str = "⌃⌥⌘R";
/// How the mute hotkey is shown to the sharer in the terminal.
pub const MUTE: &str = "⌃⌥⌘M";

fn hotkey(key: Code) -> HotKey {
    HotKey::new(
        Some(Modifiers::CONTROL | Modifiers::ALT | Modifiers::META),
        key,
    )
}

/// The sharer's main-thread app: its event loop, hotkeys and overlay.
pub struct SharerUi {
    event_loop: EventLoop<UiEvent>,
    handle: OverlayHandle,
    state: Arc<Mutex<OverlayState>>,
}

impl SharerUi {
    /// Makes the event loop. Call it on the main thread.
    pub fn new() -> Result<Self, Error> {
        let mut builder = EventLoop::<UiEvent>::with_user_event();
        #[cfg(target_os = "macos")]
        {
            // No Dock icon and no menu bar: the sharer only needs the run loop and the overlay.
            use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
            builder
                .with_activation_policy(ActivationPolicy::Accessory)
                .with_default_menu(false);
        }
        let event_loop = builder.build()?;
        event_loop.set_control_flow(ControlFlow::Wait);
        let (handle, state) = OverlayHandle::new(event_loop.create_proxy());
        Ok(Self {
            event_loop,
            handle,
            state,
        })
    }

    /// For the control thread, to show who is asking or controlling.
    pub fn overlay(&self) -> OverlayHandle {
        self.handle.clone()
    }

    /// Runs `work` on a background thread, with the hotkeys live until it returns: the revoke
    /// hotkey when control can be granted (`revoke`), and the mute hotkey when the sharer's mic
    /// is on (`mute`).
    ///
    /// Control never runs without its revoke hotkey, so failing to register that one is an
    /// error. Without the mute hotkey the call still works, so that one only warns.
    pub fn run<T: Send + 'static>(
        self,
        revoke: Option<RevokeSwitch>,
        mute: Option<Arc<VoiceState>>,
        work: impl FnOnce() -> T + Send + 'static,
    ) -> Result<T, Error> {
        let manager = match GlobalHotKeyManager::new() {
            Ok(manager) => Some(manager),
            Err(e) if revoke.is_some() => return Err(e.into()),
            Err(e) => {
                tracing::warn!("no global hotkeys, so {MUTE} won't mute: {e}");
                None
            }
        };
        let mut registered = Vec::new();
        let mut revoke_id = None;
        let mut mute_id = None;
        if let Some(manager) = &manager {
            if revoke.is_some() {
                let key = hotkey(Code::KeyR);
                manager.register(key)?;
                revoke_id = Some(key.id());
                registered.push(key);
            }
            if mute.is_some() {
                let key = hotkey(Code::KeyM);
                match manager.register(key) {
                    Ok(()) => {
                        mute_id = Some(key.id());
                        registered.push(key);
                        println!("{MUTE} mutes and unmutes your microphone.");
                    }
                    Err(e) => tracing::warn!("{MUTE} is taken, so it won't mute: {e}"),
                }
            }
        }
        GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
            if event.state != HotKeyState::Pressed {
                return;
            }
            if Some(event.id) == revoke_id {
                if let Some(switch) = &revoke {
                    switch.trip();
                    tracing::info!("revoke hotkey pressed");
                }
            } else if Some(event.id) == mute_id {
                if let Some(voice) = &mute {
                    if voice.toggle_mute() {
                        println!("Your microphone is muted. {MUTE} unmutes it.");
                    } else {
                        println!("Your microphone is on.");
                    }
                }
            }
        }));

        let proxy = self.event_loop.create_proxy();
        let start = move || {
            std::thread::spawn(move || {
                let result = work();
                let _ = proxy.send_event(UiEvent::Done);
                result
            })
        };
        let mut app = App {
            overlay: Overlay::new(self.state),
            start: Some(Box::new(start)),
            worker: None,
        };
        self.event_loop.run_app(&mut app)?;
        if let Some(manager) = &manager {
            for key in registered {
                let _ = manager.unregister(key);
            }
        }
        app.worker
            .ok_or(Error::NotStarted)?
            .join()
            .map_err(|_| Error::WorkerPanicked)
    }
}

type Start<T> = Box<dyn FnOnce() -> JoinHandle<T>>;

struct App<T> {
    overlay: Overlay,
    /// Starts the network thread, which starts screen capture. It waits for `resumed`, so the
    /// overlay's windows exist first and capture can leave them out.
    start: Option<Start<T>>,
    worker: Option<JoinHandle<T>>,
}

impl<T> ApplicationHandler<UiEvent> for App<T> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        self.overlay.prepare(event_loop);
        if let Some(start) = self.start.take() {
            self.worker = Some(start());
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UiEvent) {
        match event {
            UiEvent::Redraw => self.overlay.update(event_loop),
            UiEvent::Done => event_loop.exit(),
        }
    }

    fn window_event(&mut self, _: &ActiveEventLoop, _: WindowId, _: WindowEvent) {}
}
