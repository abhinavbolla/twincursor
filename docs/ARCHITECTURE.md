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
- The shared screen is shown in a **native window** we render ourselves (wgpu + winit, to be
  confirmed in M1). That also lets us apply render-side sharpening and upscaling.

### 2. WebRTC through LiveKit (self-hostable SFU)
- LiveKit's Rust SDK (`livekit` crate, built on libwebrtc) gives us congestion control,
  NAT traversal, simulcast, and data channels without building our own.
- Self-hosting is a single LiveKit server next to our API.
- Peer-to-peer without an SFU is an option for 1:1 calls later. Not v1.
- One LiveKit participant per person (screen, camera, and audio are tracks, not separate participants).

### 3. Encoding: tune for latency, choose per platform
Start with VP8 on macOS and H.264 (hardware) on Windows. Make the codec and its parameters
configurable so we can measure. See `research/hopp-lessons.md` for the rate-control and
buffer settings that cut Hopp's latency from ~330 ms to ~90 ms. Disable simulcast for screen
share on stable links. Check libwebrtc's screencast frame-rate cap.

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

Build this in M1, before any tuning:
1. The guest sends a "tick" over the data channel.
2. On receipt, the sharer stamps a marker into the next captured frame (for example a bright
   pattern in the top rows of the Y plane) together with the tick ID.
3. The guest detects the marker after decoding. Latency is the time from tick to marker seen.
4. Log p50 and p95 per run. Use Network Link Conditioner (macOS) to simulate poor networks.

Every media-path PR reports before and after numbers.

## Platform plan

macOS first (M1–M5), Linux next (Wayland via PipeWire capture and libei input, plus X11), Windows after that.
