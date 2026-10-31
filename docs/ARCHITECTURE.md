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
- **H.264 (hardware) is the default** (ADR 0002). It matches VP8's latency on macOS at about
  1/5 of the sharer's CPU. Its text is measurably sharper (SSIM p5 0.9995 vs 0.98), and it
  uses ~4x less bandwidth.
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
  to the shared surface (0..1). It sends them over LiveKit data channels using
  `crates/protocol` messages:
  - clicks, keys and scrolls on the reliable, ordered channel: they must never drop or reorder;
  - pointer moves on the lossy channel: a lost move is replaced by the next one, and it avoids
    retransmission tails (`research/m2-results.md`). Each click carries its own position, so it
    can't land in the wrong place by overtaking a move.
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
1. The viewer sends a tick (lossy data packet) every 247 ms. The interval must not be a multiple
   of the frame time. At 250 ms (15 frames at 60 fps) every tick lands at the same point of the
   frame clock, so one run samples a single point of the real 14–33 ms spread and p50 jumps
   between runs.
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

`tc-spike view --measure-input` measures the data path input takes, with pings on both data
channels that the sharer echoes. It reports the round trip and, where both ends share a clock
(one machine, the network lab), the one-way delay. Use one-way for input: the round trip's
return leg shares the viewer's downlink with the video.

**Input SLO:** one-way p50 ≤ network delay + 15 ms and p95 ≤ network delay + 40 ms on clean links.

## Measuring quality

Latency alone doesn't tell us whether text is readable, so we measure picture quality too:
- `tc-spike share --capture pattern` streams a synthetic dark-theme code editor
  (`crates/media/src/pattern.rs`), drawn from the frame number alone:
  - tiny 8 px code on the left, 16 px code on the right, coloured syntax;
  - the left pane alternates 10 s of gentle scrolling with 10 s of fast scrolling (one line
    per frame, the hardest case for screen codecs);
  - a line being typed and a blinking cursor.
  The frame number travels in a second marker slot next to the latency marker.
- `tc-spike view --measure-quality` regenerates the expected frame and compares the decoded
  luma with it on a background thread (`crates/media/src/quality.rs`, frames are skipped
  while it's busy):
  - **SSIM** (8x8 blocks; drops with blur and blocking, which is what makes text unreadable);
  - **PSNR**.
  Both skip the marker area. Results report p50, p5 and min, plus SSIM during fast scrolling
  only.
- `--headless` runs the viewer without a window. Latency is then measured at decode. Use it
  for benchmarks and the network lab.
- For a real screen, `--snapshot-every N` on both ends saves the frame carrying every Nth tick
  as a PGM in `bench/snapshots/` (git-ignored: it holds real screen content), and
  `tc-spike compare sharer-N.pgm viewer-N.pgm` prints SSIM and PSNR.

## Network lab and SLOs

`bench/netlab/` runs a pattern sharer, a headless viewer and the LiveKit SFU in Docker, each
with a fixed IP on a private bridge. Each profile's conditions are applied with `tc netem`:
- the sharer's uplink is shaped on its own interface;
- the viewer's uplink is shaped on its own interface;
- the viewer's downlink is shaped on the SFU's interface, filtered to the viewer's IP.

Run it with `bench/netlab/run.sh [--build] [profile ...]` (`CODEC`, `RES`, `DURATION`,
`ADAPT=off`, `SHARE_FLAGS`, `VIEW_FLAGS`, `TAG`), and summarise with `bench/netlab/report.sh`.

Lab notes:
- Rate limits use a token bucket (tbf) under netem. netem's own `rate` combined with jitter
  tail-drops far more than configured.
- Building tc-spike for Linux needs clang 21+ (libwebrtc's hermetic libc++) and an explicit
  link to GLib/GIO (see the lab's Dockerfile). Linux in Docker has no hardware
encoder, so the lab uses VP8 (H.264 there is OpenH264 at ~35 fps). Mac hardware H.264 is
checked on real machines instead.

| profile | conditions (one way) |
|---|---|
| lan | none |
| wifi-good | 2 ms ±2 ms, 0.1% loss, both ways |
| wifi-bad | 8 ms ±8 ms, 1% loss with 25% correlation, both ways |
| 4g | 25 ms ±5 ms, 0.5% loss; 12 Mbit down, 4 Mbit up |
| hotel | 40 ms ±10 ms, 2% loss; 3 Mbit down, 1 Mbit up |
| intercontinental | 75 ms each way (150 ms RTT) |
| sharer-uplink-3mbit | sharer uplink 3 Mbit, 10 ms |
| bandwidth-drop | sharer uplink 10 → 1.5 → 10 Mbit, 20 s each |
| udp-blocked | viewer drops all outgoing UDP (ICE must use TCP) |
| outage-5s | viewer cut off completely for 5 s at t = 25 s |

**SLOs** (checked by `report.sh`; "network RTT" is the RTT the profile adds):
- lan, wifi-good: RTT p50 ≤ network RTT + 25 ms; p95 ≤ network RTT + 60 ms; SSIM p5 ≥ 0.97;
  no stall (a second under 5 fps).
- ≤ 2% average loss: at most one freeze a minute.
- bandwidth-drop: no stall longer than 3 s.
- outage-5s: video back within 3 s of the network returning (longest stall ≤ 8 s).
- udp-blocked: the call connects and measures latency.

## Auto-throttle (adaptive frame rate and resolution)

libwebrtc adapts the bitrate to the bandwidth estimate, and for screen share it keeps the
resolution and drops the frame rate when squeezed. That isn't enough on its own:
- When the budget is too small for the resolution, every frame is blurry.
- When the *viewer's* downlink is the bottleneck, the sharer's estimate doesn't see it.
  Without simulcast the SFU can only drop packets, and the LiveKit SDK has no public API to
  change the max bitrate mid-call.

`crates/media/src/adapt.rs` walks a ladder that halves the frame rate before giving up
resolution, because text needs pixels more than 60 fps:

1440p 60 → 1440p 30 → 1080p 30 → 720p 30 → 720p 15

The ladder starts at what the user picked.
- **Step down one level** after 3 s of trouble:
  - the budget (encoder target, capped by the bandwidth estimate) is under 90% of the
    level's floor. Floors are 0.5 / 1.0 / 1.6 / 2.5 Mbps for 720 / 1080 / 1440 / native at
    60 fps with H.264, ~2x for VP8, and scale with (fps/60)^0.75;
  - or the encoder is CPU-limited;
  - or it's bandwidth-limited below half the level's frame rate;
  - or the viewer reports ≥ 5% loss or ≥ 2 freezes a second.
- **Step up** after 10 s of calm with 1.5x the next level's floor available. That wait
  doubles (up to 80 s) when it flaps.
- The viewer sends its stats back on `tc.stats`, so the sharer sees its downlink.
- The sharer captures at the chosen tier. The send loop scales frames to the level's size
  (libyuv) and paces them to its frame rate; libwebrtc reconfigures the encoder by itself. The
  viewer's window keeps its size, and lower tiers are scaled up to it.
- On a big drop it jumps straight to the first level the budget can carry, instead of
  walking down one level every few seconds while the picture stalls.
- **Bitrate cap for the viewer's downlink** (`BitrateCap`). Frame rate and resolution don't
  lower the bitrate on their own: the encoder spends whatever its target allows. So when the
  viewer loses ≥ 5% for 4 s while we send more than it receives, the sharer caps its max
  bitrate at 75% of the viewer's goodput. It raises the cap 1.5x after 30 calm seconds.
  - The SDK has no public way to change the max bitrate mid-call, so this republishes the
    track: the viewer sees it drop and come back.
  - In the lab, VP8 into a 3 Mbit, 2%-loss downlink decoded 0 fps uncapped, and 18 fps with
    no stall at a 1.5 Mbps cap.
- The HUD shows "adapted to 1080p 30 fps: <reason>; capped at 1.5 Mbps (viewer downlink)".
  `share --no-adapt` turns both off.

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
