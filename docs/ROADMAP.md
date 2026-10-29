# TwinCursor: Roadmap

Each milestone ends with something you can run and demo. Work on one at a time.

## M0: Project setup ✅
Repo, license, CLAUDE.md, product, architecture, roadmap, Claude Code settings.

## M1: Media spike (macOS, CLI only), mostly done
Goal: prove we can hit good latency before building any UI.
- [x] Cargo workspace, `crates/media`, a local LiveKit dev server (brew or docker)
- [x] Capture one display, publish it as a video track (VP8, VP9, H.264 selectable)
- [x] A second process subscribes and renders it in a native window (winit + softbuffer)
- [x] Latency harness (tick → frame marker) printing p50 and p95, plus `bench/run-matrix.sh`
- [x] ADR 0001: capture API choice (ScreenCaptureKit)
- [x] Results: `docs/research/m1-results.md`. Same Mac: ~58 fps at 1660x1080, RTT p50 ~18 ms, p95 ~31–34 ms
- [x] Run across two machines: Mac → Windows on Wi-Fi LAN, RTT p50 22 ms, p95 223 ms (see results)
- [ ] Find the cause of the ~220 ms p95 tail on Wi-Fi (ping test, Ethernet, AWDL off)
- [ ] Run between **two Macs** and under Network Link Conditioner (needs a second Mac)
- [x] Compare text quality: H.264 vs VP8, then pick the default codec (done in M1.5: H.264)
- [ ] (Stretch) `--marker screen` for true glass-to-glass
Done when: 1080p at 30+ fps between two Macs, with measured latency written in the doc.

Follow-ups found in M1 (for later milestones): Metal/wgpu viewer that renders the decoder's
CVPixelBuffer (saves ~10 ms, and ~17 ms at 1440p, see `research/m15-results.md`); exclude our
own windows from capture. The ~11 s first connect
only happened with both ends on one Mac; across two machines it took 0.6 s.

## M1.5: High-fidelity media (Tuple-level quality)
Goal: prove the picture is sharp and stays usable on real networks, and show the call's health.
- [x] Network stats on both ends (RTT, loss, jitter, bitrate, bandwidth estimate, limit
      reason, freezes, path) and an on-screen HUD in the viewer (`S` toggles it)
- [x] Resolution tiers up to 1440p, with bitrate presets (`--resolution`)
- [x] Measured quality: synthetic code-text source, SSIM/PSNR, H.264 vs VP8; H.264 is the
      default (ADR 0002)
- [ ] Network lab: Docker + netem profiles (Wi-Fi, 4G, hotel, intercontinental, bandwidth drop,
      UDP blocked, outage) with pass/fail SLOs
- [x] Auto-throttle: step resolution with the bandwidth estimate and viewer health, text first
      (`adapt.rs`; lab evaluation pending)
- [ ] Real-world runs: Mac → Windows on Wi-Fi vs Ethernet, AWDL, Network Link Conditioner
Done when: the lab passes its SLOs (or misses are explained) and the real-world runs are documented.

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
