# TwinCursor: Roadmap

Each milestone ends with something you can run and demo. Work on one at a time.

## M0: Project setup ✅
Repo, license, CLAUDE.md, product, architecture, roadmap, Claude Code settings.

## M1: Media spike (macOS, CLI only)
Goal: prove we can hit good latency before building any UI.
- [ ] Cargo workspace, `crates/media`, a local LiveKit dev server (docker)
- [ ] Capture one display, publish it as a video track (VP8)
- [ ] A second process subscribes and renders it in a native window (wgpu + winit)
- [ ] Latency harness (tick → frame marker) printing p50 and p95
- [ ] ADR: capture API choice (ScreenCaptureKit vs libwebrtc DesktopCapturer)
Done when: 1080p at 30+ fps between two Macs, with measured latency written in the doc.

## M2: Remote control (macOS)
- [ ] `crates/protocol`: input event messages
- [ ] Viewer captures mouse, keyboard, and scroll and sends them over the data channel
- [ ] Sharer injects them with enigo; Accessibility permission check
- [ ] Grant and revoke control; global revoke hotkey
- [ ] Guest cursor overlay window on the sharer
Done when: the guest can edit a file in the sharer's editor comfortably.

## M3: Audio
- [ ] Mic capture and playback in Rust, echo cancellation and noise suppression
- [ ] Mute, device selection

## M4: Desktop app (Tauri)
- [ ] App shell: tray icon, lobby, call window, settings
- [ ] Onboarding for macOS permissions
- [ ] Connect the Rust core to the UI with Tauri commands and events

## M5: Server and self-hosting
- [ ] axum API: sign-in, teams, rooms, LiveKit tokens
- [ ] Join a call via link
- [ ] `deploy/docker-compose.yml` and a self-hosting guide

## M6: Polish and first public release (macOS)
- [ ] Tuning pass: codec parameters, sharpening, adaptive resolution
- [ ] Signed and notarized builds, auto-update, CI
- [ ] Landing page on twincursor.com

## Later
Linux (Wayland and X11), Windows, camera, multiple cursors with 3+ people, annotations,
end-to-end encryption, editor integrations.
