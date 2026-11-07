//! The sharer's global revoke hotkey.
//!
//! The hotkey needs the app's event loop on the main thread, so with `--allow-control` the
//! sharer runs its network work on a background thread and the main thread waits for the key.
//! The handler only trips the [`RevokeSwitch`]: no lock, no channel, nothing async.

use control::RevokeSwitch;
use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::WindowId;

/// How the hotkey is shown to the sharer.
pub const REVOKE: &str = "⌃⌥⌘R";

/// Runs `work` on a background thread, with the revoke hotkey live until it returns.
pub fn run_with_hotkey<T: Send + 'static>(
    switch: RevokeSwitch,
    work: impl FnOnce() -> T + Send + 'static,
) -> anyhow::Result<T> {
    let mut builder = EventLoop::<()>::with_user_event();
    #[cfg(target_os = "macos")]
    {
        // No Dock icon and no menu bar: the sharer only needs the run loop.
        use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
        builder
            .with_activation_policy(ActivationPolicy::Accessory)
            .with_default_menu(false);
    }
    let event_loop = builder.build()?;
    event_loop.set_control_flow(ControlFlow::Wait);

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

    let proxy = event_loop.create_proxy();
    let worker = std::thread::spawn(move || {
        let result = work();
        let _ = proxy.send_event(());
        result
    });
    event_loop.run_app(&mut Waiter)?;
    let _ = manager.unregister(hotkey);
    worker
        .join()
        .map_err(|_| anyhow::anyhow!("the sharer's network thread panicked"))
}

struct Waiter;

impl ApplicationHandler for Waiter {
    fn resumed(&mut self, _: &ActiveEventLoop) {}

    fn user_event(&mut self, event_loop: &ActiveEventLoop, (): ()) {
        event_loop.exit();
    }

    fn window_event(&mut self, _: &ActiveEventLoop, _: WindowId, _: WindowEvent) {}
}
