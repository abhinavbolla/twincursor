# TwinCursor: Architecture

Status: draft. Validate each decision with a spike before building on it.

## Overview

```
 Sharer (desktop app)                                   Guest (desktop app)
 ┌──────────────────────────────┐                      ┌──────────────────────────────┐
 │ Tauri UI (webview)           │                      │ Tauri UI (webview)           │
 │  lobby · settings · controls │                      │  lobby · settings · controls │
 ├──────────────────────────────┤                      ├──────────────────────────────┤
 │ Rust core                    │   video/audio (RTP)  │ Rust core                    │
 │  capture → encode ──────────────────► LiveKit SFU ──────► decode → native viewer   │
 │  inject input ◄────────────────── data channel ◄───────── capture local input      │
 │  cursor overlay window       │                      │  (mouse, keys, scroll)       │
 └──────────────────────────────┘                      └──────────────────────────────┘
                 ▲                                                    ▲
                 └────────── HTTPS: auth, rooms, LiveKit tokens ──────┘
                                     server/ (axum + Postgres)
```

## Key decisions

### 1. Tauri 2 for the app shell, Rust for all media
- Tauri keeps the app small and gives us a Rust process for free.
- The webview **only** renders app UI. Hopp ran WebRTC inside Tauri's WebKit webview and ran
  into audio glitches, browser codec limits, no access to frame buffers, and no WebRTC in the
  default WebKitGTK build (which blocks Linux). They are now migrating media to Rust. We start there.
- The shared screen is shown in a **native window** we render ourselves. That also lets us
  apply render-side sharpening and upscaling. M1 uses winit + softbuffer (CPU conversion),
  which costs about 9–11 ms per frame. The real viewer should render the decoder's
  CVPixelBuffer with Metal/wgpu (see `research/m1-results.md`).
- Capture on macOS uses ScreenCaptureKit directly (ADR 0001).

### 2. WebRTC through LiveKit (self-hostable SFU)
- LiveKit's Rust SDK (`livekit` crate, built on libwebrtc) gives us congestion control,
  NAT traversal, simulcast, and data channels without building our own.
- Self-hosting is a single LiveKit server next to our API.
- Peer-to-peer without an SFU is an option for 1:1 calls later. Not v1.
- One LiveKit participant per person (screen, camera, and audio are tracks, not separate participants).

### 3. Encoding: tune for latency, choose per platform
Codecs are configurable (`tc-spike share --codec`). M1 measurements (`research/m1-results.md`):
- **H.264 (hardware)** matches VP8's latency on macOS at about 1/5 of the sharer's CPU, at
  full 1080p. It's the leading candidate for the default, pending a text-quality comparison.
- **VP8** is the software fallback.
- **VP9** was worse on every measure.
- Simulcast is off for screen share.
- **Resolution tiers** (`--resolution`): 720, 1080 (default), 1440 or native lines; the width
  follows the display's aspect ratio, rounded to a multiple of 16 so encoders don't crop
  (1662 became 1660). On a 3024x1964 MacBook panel: 1104x720, 1664x1080, 2224x1440.
  ScreenCaptureKit scales on the GPU. Default max bitrates are 2.5 / 5 / 8 / 12 Mbps; the
  bandwidth estimate usually sets the actual target lower.
- The viewer shows frames 1:1 (sharpest text) and only downscales when a frame doesn't fit
  on its monitor (e.g. 1440p onto a 1080p screen).
- `is_screencast` showed no frame-rate cap and no latency difference.

Encoder tuning (Hopp's rate-control and buffer settings, `research/hopp-lessons.md`) needs a
custom libwebrtc build. Do it only if real-network numbers call for it.

### 4. Remote control over a data channel
- The guest's viewer captures mouse, keyboard and scroll events and normalizes coordinates
  to the shared surface (0..1). It sends them over a reliable, ordered LiveKit data channel
  using `crates/protocol` messages.
- The sharer validates the events (control granted? within the shared display?) and injects
  them with `enigo` (macOS CGEvent, Windows SendInput, Linux X11/libei).
- The sharer gets a transparent, click-through, always-on-top overlay window that draws the
  guest's cursor and name.
- macOS needs Screen Recording and Accessibility permissions. Onboarding must guide the user
  through both.
- Keyboard layouts: send key *codes* plus the produced *text*, and decide per platform which
  to inject. This is a known hard part, so it gets its own ADR.

### 5. Server
- Rust + axum + Postgres. Handles accounts, teams, rooms, and issuing LiveKit access tokens.
  It never touches media.
- `deploy/` ships a docker compose file: server, LiveKit, Postgres, and Caddy for TLS.

## Measuring latency

Built in M1 (`crates/media/src/{marker,latency}.rs`, `tc-spike`, `bench/run-matrix.sh`):
1. The viewer sends a tick (lossy data packet) every 250 ms.
2. The sharer stamps the latest tick id into outgoing frames as a black/white block marker in
   the top-left of the luma plane (4 guard bits + a 20-bit id). It re-sends the last frame
   immediately when a tick arrives.
3. The viewer reads the marker after decoding. **RTT** = tick sent → marker frame presented
   (viewer clock only, so it's valid across machines).
4. Each frame also carries the sharer's capture timestamp in the RTP packet trailer
   (`PTF_USER_TIMESTAMP`). **One-way** = capture → presented. Valid on one machine or with
   synced clocks.
5. Every run logs p50/p95/max and writes JSON to `bench/results/`. Use Network Link
   Conditioner (macOS) to simulate poor networks.

Limitation: the marker is stamped after capture, so the app redraw and part of the capture
delay aren't in RTT. A planned `--marker screen` mode fixes that.

Every media-path PR reports before and after numbers (`bench/run-matrix.sh`).

## Call health stats

`crates/media/src/netstats.rs` polls WebRTC stats once a second on both ends (`Room::get_stats`)
and turns the cumulative counters into per-second `NetSnapshot`s:
- link RTT and path (ICE candidate type and protocol, e.g. `host udp`, `relay tcp`);
- receive: bitrate, fps, resolution, loss %, jitter, freezes, dropped frames, NACK/PLI, decoder;
- send: bitrate vs target, fps, resolution, loss reported by the SFU, retransmit %, bandwidth
  estimate, what limits quality (`cpu`/`bandwidth`), encoder.

The sharer sends its snapshot to the viewer on data topic `tc.stats`, so the viewer sees both
ends. `connection_quality` grades the call Good/Fair/Poor; the worst signal wins:

| signal | good below | fair below |
|---|---|---|
| link RTT | 80 ms | 200 ms |
| loss | 1% | 5% |
| jitter | 30 ms | 80 ms |
| freezes in the last second | none | under 500 ms in total |

The viewer draws this as a HUD in the top-right corner (away from the latency marker) and
writes the per-second history into the results JSON (`net`). `--no-stats` turns it all off,
for A/B latency runs.

SDK quirk: with current LiveKit servers the SDK uses one peer connection for both directions
and reports it as `publisher_stats`; `subscriber_stats` is then empty. The poller handles both.

## Platform plan

macOS first (M1–M5), Linux next (Wayland via PipeWire capture and libei input, plus X11), Windows after that.
