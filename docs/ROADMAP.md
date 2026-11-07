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
- [x] Input event messages (`crates/protocol`) and the control session (`crates/control`), with
  the keyboard model (ADR 0004)
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

## Then: your own devices

Connect to your own computers when nobody is at them, from a laptop or a phone, with the same
latency and sharpness as pairing. The consent model is in
[ADR 0005](adr/0005-unattended-access.md) (proposed).

- [ ] Enroll devices and clients to your account, with a second factor
- [ ] A background agent that accepts your enrolled clients while you're signed in to the computer
- [ ] An on-screen label, a start sound and a local log for every unattended session
- [ ] Direct peer-to-peer connections, with the server as a relay only when needed
- [ ] Multiple monitors, clipboard sync, file drop and sound from the remote computer
- [ ] Mobile viewer for iOS and Android: native decoding, a trackpad mode alongside direct touch,
      pinch to zoom that follows the cursor, and a keyboard with a modifier bar

## Later

Linux (Wayland and X11), a Windows sharer, camera, multiple cursors with three or more people,
annotations, end-to-end encryption, and editor integrations.
