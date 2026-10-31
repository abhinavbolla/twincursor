# TwinCursor: Roadmap

Each milestone ends with something you can run and demo. Work on one at a time.

## M0: Project setup ✅
Repo, license, CLAUDE.md, product, architecture, roadmap, Claude Code settings.

## M1: Media spike (macOS, CLI only) ✅
Goal: prove we can hit good latency before building any UI.
- [x] Cargo workspace, `crates/media`, a local LiveKit dev server (brew or docker)
- [x] Capture one display, publish it as a video track (VP8, VP9, H.264 selectable)
- [x] A second process subscribes and renders it in a native window (winit + softbuffer)
- [x] Latency harness (tick → frame marker) printing p50 and p95, plus `bench/run-matrix.sh`
- [x] ADR 0001: capture API choice (ScreenCaptureKit)
- [x] Results: `docs/research/m1-results.md`. Same Mac: ~58 fps at 1660x1080, RTT p50 ~18 ms, p95 ~31–34 ms
- [x] Run across two machines over a real network: Mac → Windows on Wi-Fi, 1080p at 60 fps with
      H.264, RTT p50 ~25 ms (`research/m1-results.md`, `research/m15-results.md`)
- [x] Cause of the ~220 ms p95 tail on Wi-Fi: Wi-Fi link stalls. The ICE link RTT spikes to
      0.6–2.5 s at 0% loss, so it's the network, not our pipeline (`research/m15-results.md`)
- [x] Poor-network conditions: covered by the Docker network lab (M1.5) rather than Network
      Link Conditioner
- [x] Compare text quality: H.264 vs VP8, then pick the default codec (done in M1.5: H.264)

Done: 1080p at 30+ fps between two machines (60 fps measured Mac → Windows), with measured
latency in the docs. The original criterion said "two Macs". The second machine was a Windows
laptop, and a literal Mac-to-Mac run moved to M6's pre-release checks.

Follow-ups found in M1 (for later milestones): Metal/wgpu viewer that renders the decoder's
CVPixelBuffer (saves ~10 ms, and ~17 ms at 1440p, see `research/m15-results.md`); exclude our
own windows from capture. The ~11 s first connect only happened with both ends on one Mac;
across two machines it took 0.6 s.

## M1.5: High-fidelity media (Tuple-level quality) ✅
Goal: prove the picture is sharp and stays usable on real networks, and show the call's health.
- [x] Network stats on both ends (RTT, loss, jitter, bitrate, bandwidth estimate, limit
      reason, freezes, path) and an on-screen HUD in the viewer (`S` toggles it)
- [x] Resolution tiers up to 1440p, with bitrate presets (`--resolution`)
- [x] Measured quality: synthetic code-text source, SSIM/PSNR, H.264 vs VP8; H.264 is the
      default (ADR 0002)
- [x] Network lab: Docker + netem profiles (Wi-Fi, 4G, hotel, intercontinental, bandwidth drop,
      UDP blocked, outage) with pass/fail SLOs; misses are explained in `research/m15-results.md`
- [x] Auto-throttle, text first: frame rate before resolution, jump on severe drops, and a
      bitrate cap from the viewer's goodput (`adapt.rs`; evaluated in the lab)
- [x] Real-world runs on Wi-Fi: Mac → Windows, H.264 decodes on Windows, quality holds (SSIM
      0.9996), tails come from Wi-Fi link stalls (`research/m15-results.md`)
Done: the lab's SLO misses are explained (software VP8 in Linux; `research/m15-results.md`), and
the real-world Wi-Fi runs are documented. Open items moved to M6.

## M2: Remote control (macOS)
- [x] First: measure the data-channel delay viewer → sharer on its own. It isn't the source of
      the run-to-run swing: that came from the 250 ms tick interval (`research/m2-results.md`)
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
- [ ] Live bitrate control without republishing the track (an upstream LiveKit SDK API or a
      small patch), and FEC for lossy links (4G-style loss still freezes often with VP8)
- [ ] Pre-release real-world checks: Mac to Mac; Mac on Ethernet; AWDL off (to pin the Wi-Fi
      stalls on it or not); Network Link Conditioner profiles
- [ ] (From M1, stretch) `--marker screen` for true glass-to-glass latency
- [ ] Signed and notarized builds, auto-update, CI
- [ ] Landing page on twincursor.com

## Later
Linux (Wayland and X11), Windows, camera, multiple cursors with 3+ people, annotations,
end-to-end encryption, editor integrations.
