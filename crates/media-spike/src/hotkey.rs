//! The sharer's main thread with `--allow-control`: the global revoke hotkey and the overlay.
//!
//! Both need the app's event loop on the main thread, so the network work runs on a background
//! thread. The hotkey handler only trips the [`RevokeSwitch`]: no lock, no channel, nothing
//! async.

use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use control::RevokeSwitch;
use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::WindowId;

use crate::overlay::{Overlay, OverlayHandle, OverlayState, UiEvent};

/// How the hotkey is shown to the sharer in the terminal.
pub const REVOKE: &str = "⌃⌥⌘R";

pub struct SharerUi {
    event_loop: EventLoop<UiEvent>,
    handle: OverlayHandle,
    state: Arc<Mutex<OverlayState>>,
}

impl SharerUi {
    pub fn new() -> anyhow::Result<Self> {
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

    /// Runs `work` on a background thread, with the revoke hotkey live until it returns.
    pub fn run<T: Send + 'static>(
        self,
        switch: RevokeSwitch,
        work: impl FnOnce() -> T + Send + 'static,
    ) -> anyhow::Result<T> {
        let manager = GlobalHotKeyManager::new()?;
        let hotkey = HotKey::new(
            Some(Modifiers::CONTROL | Modifiers::ALT | Modifiers::META),
            Code::KeyR,
        );
        manager.register(hotkey)?;
        let id = hotkey.id();
        GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
            if event.id == id && event.state == HotKeyState::Pressed {
                switch.trip();
                tracing::info!("revoke hotkey pressed");
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
        let _ = manager.unregister(hotkey);
        app.worker
            .ok_or_else(|| anyhow::anyhow!("the event loop ended before it started"))?
            .join()
            .map_err(|_| anyhow::anyhow!("the sharer's network thread panicked"))
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
