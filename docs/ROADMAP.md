# Roadmap

TwinCursor is built in stages. Each one ends with something you can run and demo, and we work on
one at a time. The first public release comes after the desktop app and self-hosting exist.

## Done

**Sharp, low-latency screen sharing.** A native pipeline from ScreenCaptureKit through LiveKit to a
native viewer, measured on real networks.

- 1080p at 60 fps from Mac to Windows over home Wi-Fi, with sharp text
- H.264 by default, VP8 as a fallback, and resolutions up to 1440p and native
- Auto-throttle that keeps text readable when bandwidth drops
- Call health stats and an on-screen HUD
- Latency and text-quality measurement, and a Docker network lab with pass/fail targets

The results are in [performance.md](performance.md).

## Now: remote control on macOS

The guest uses the sharer's Mac with their own mouse and keyboard, with consent built in. It's
done when a guest can comfortably edit a file in the sharer's editor.

- [x] Measure the path input takes, and choose its transport (ADR 0003)
- [ ] Input event messages (`crates/protocol`) and the control session (`crates/control`)
- [ ] Viewer captures mouse, keyboard and scroll
- [ ] Grant and revoke control, with a global revoke hotkey
- [ ] Real input injection, with an Accessibility permission check
- [ ] Guest cursor overlay on the sharer's screen

## Next

**Audio.** Mic capture and playback in Rust, echo cancellation, noise suppression, mute and device
selection.

**Desktop app.** A Tauri shell with a tray icon, lobby, call window and settings, plus onboarding
for macOS permissions.

**Self-hosting.** An axum API for sign-in, teams, rooms and LiveKit tokens. Join calls by link, and
run it all with one `docker compose up`.

**First public release on macOS.**

- [ ] Signed and notarized builds, and auto-update
- [ ] Live bitrate control without republishing the track, and FEC for lossy links
- [ ] Encoder tuning, sharpening and a faster Metal or wgpu viewer
- [ ] Real-world checks: Mac to Mac, Mac on Ethernet, Network Link Conditioner profiles
- [ ] True glass-to-glass latency with an on-screen marker (`--marker screen`)
- [ ] Exclude our own windows from capture
- [ ] Landing page

## Later

Linux (Wayland and X11), a Windows sharer, camera, multiple cursors with three or more people,
annotations, end-to-end encryption, and editor integrations.
