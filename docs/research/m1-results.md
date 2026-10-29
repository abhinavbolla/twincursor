# M1 results: macOS media spike

Measured 2026-10-27 with `bench/run-matrix.sh` (4 rounds × 5 configs × 30 s).

## Setup
- **Machine:** Apple M4 MacBook, macOS 26.5.1, 3024x1964 Retina panel.
- **Sharer and viewer:** both on the same Mac, talking to a local `livekit-server --dev`
  1.13.11. Network delay is about zero, so this is the floor of our pipeline, not a real-world number.
- **LiveKit Rust SDK:** `livekit` 0.9.7 with the prebuilt libwebrtc `webrtc-a545de4`.
- **Capture size:** 1662x1080 (keeps the panel's aspect ratio). Every encoder delivered
  **1660x1080**, because the width gets aligned. Use widths that are multiples of 4 (or 16) from now on.
- **Settings:** 4 Mbps max bitrate, 60 fps max, simulcast off, zero playout delay, a 1-frame
  receive queue, and ticks every 250 ms.
- **Screen content:** the viewer window sits on the captured screen. That creates a
  "hall of mirrors", so the screen changes on every frame, which is a busy-screen scenario.
  One sck-vp8 round ran while the viewer wasn't visible, so the screen was static; it is
  listed separately below.

## What the numbers mean
- **RTT** = viewer sends a tick (data channel) → sharer stamps the tick id into the next frame
  → that frame is decoded and **presented** on the viewer. This is the remote-control loop
  without the app redraw. Only the viewer's clock is used.
- **one-way** = sharer's ScreenCaptureKit capture timestamp (carried in the RTP packet
  trailer) → frame presented on the viewer. It needs a shared clock, so it's only valid on one machine.
- **dec** = RTT measured at decode instead of present. RTT minus dec is the viewer's render cost.

## Results (median of 4 rounds; worst round in brackets)

| config | fps | RTT p50 | RTT p95 | dec p50 | one-way p50 | one-way p95 | ticks seen | sharer CPU | viewer CPU |
|---|---|---|---|---|---|---|---|---|---|
| **sck + VP8** (busy, 3 rounds) | 58 | 18 ms | 31 ms (81) | 7 ms | 17 ms | 21 ms (29) | 100% | 43–46% | 56–57% |
| **sck + H.264** | 57 | 18 ms | 34 ms (43) | 8 ms | 19 ms | 25 ms (26) | 100% | **8–9%** | 48–50% |
| sck + VP8, `is_screencast=false` | 58 | 18 ms | 40 ms (80) | 8 ms | 18 ms | 23 ms (55) | 100% | 44–52% | 58–62% |
| sck + VP9 | 54 | 26 ms | 105 ms (4 s stall) | 18 ms | 17 ms | 25 ms (44) | **80–93%** | 60–69% | 53–59% |
| lk capturer + VP8 | 52 | 16 ms | 126 ms (293) | 7 ms | 18 ms | 70 ms (269) | 100% | 59–66% | 52–54% |
| sck + VP8, static screen | 4* | 21 ms | 24 ms | 10 ms | 20 ms | 23 ms | 97% | 6% | 6% |

\* On a static screen, frames are only re-sent when a tick arrives (4/s).
"CPU" is a percentage of one core, sampled every 2 s with `ps`.

## Findings
1. **The stock SDK is already fast on localhost.** Interaction RTT is about 18 ms p50 and
   about 31–34 ms p95 with VP8 or H.264, and capture to on-screen is about 17–19 ms p50. Hopp
   reported about 330 ms with default libwebrtc settings, but that was over a real network at
   1080p with a 4 Mbps uplink, so these numbers **are not comparable** yet. The next step is
   to measure with Network Link Conditioner and Mac-to-Mac.
2. **H.264 uses about 5x less sharer CPU** (8–9% vs 43–46% for VP8) with the same latency.
   That is hardware encoding through VideoToolbox. It ran at the full 1660x1080, so the 720p
   limit Hopp described for the Rust SDK's H.264 doesn't apply to this SDK version.
   Text quality at equal bitrate is **not measured yet**.
3. **VP9 is worse on every measure:** it drops ticks (frames are skipped), has higher decode
   time, costs the most CPU, and one round stalled for 4 s.
4. **`is_screencast` made no measurable difference, and there was no 5 fps cap.** Screencast
   mode ran at 58 fps. The cap Hopp patched doesn't show up in this libwebrtc build.
5. **ScreenCaptureKit beats the LiveKit DesktopCapturer.** The DesktopCapturer captures at
   full resolution in BGRA and converts and scales on the CPU, which costs about 15–20 points
   more sharer CPU. It also had heavy tails in 2 of 4 rounds (p95 up to 293 ms). See ADR 0001.
6. **The viewer's rendering costs about 9–11 ms** (RTT minus dec), in every config. That's
   the main-thread wake-up, I420→ARGB on the CPU (libyuv), and softbuffer creating a CGImage
   per frame. It's the largest single cost we control: a Metal/wgpu renderer that takes the
   decoder's CVPixelBuffer directly should remove most of it.
7. The viewer uses about 50–60% CPU at 58 fps, mostly from the same rendering path.
8. The first connect took about 11 s (ICE over many IPv6 candidates). Across two machines it
   took 0.6 s (see below), so this looks specific to running both ends on one Mac.

## Mac → Windows over a home LAN (2026-10-29)

First run across two machines. Sharer and `livekit-server --dev --bind 0.0.0.0` on the Mac
above (on **Wi-Fi**, with AWDL active). Viewer on a Windows PC (Rust 1.93 MSVC) with
`tc-spike view --url ws://192.168.50.19:7880`. sck + VP8 defaults, one 64 s run.

| fps | resolution | RTT p50 | RTT p95 | RTT max | dec p50 | dec p95 | ticks seen | connect |
|---|---|---|---|---|---|---|---|---|
| 52 | 1660x1080 | 22 ms | 223 ms | 331 ms | 20 ms | 221 ms | 254/254 | 0.6 s |

Each 10 s window looked the same (p50 22–30 ms, p95 222–243 ms), so the tail is steady, not a
start-up effect. One-way isn't valid here (clocks not synced). The window wasn't watched, so
image correctness wasn't checked by eye; 3310 frames were presented.

Findings:
1. **The typical case is barely slower than on one machine:** p50 22 ms vs 18 ms.
2. **About 5–10% of round trips take 220–330 ms.** The tail is already there at decode, so it
   isn't the Windows renderer; it's the network, the sharer, or the decoder waiting for data.
   Not diagnosed yet. Main suspect: the Mac's Wi-Fi. AWDL (AirDrop/Continuity) is known to cause
   periodic latency spikes on macOS Wi-Fi. Next: ping the Mac from the viewer during a run, and
   repeat on Ethernet or with AWDL off.
3. **Connecting took 0.6 s,** not the ~11 s seen with both ends on one Mac. That delay was
   specific to the same-machine setup.
4. **Windows needs the static C runtime.** The prebuilt libwebrtc is built with `/MT`; without
   `-C target-feature=+crt-static` the link fails with LNK2038 `RuntimeLibrary` mismatches.
   `.cargo/config.toml` now sets it for `windows` + `msvc`. Also: clone to a short path
   (`C:\tc`); a deep folder hits git's "Filename too long".

## Not covered yet
- Mac-to-Mac, a wired network, and Network Link Conditioner. The marker RTT metric works across
  machines; use `tc-spike view --url ws://<sharer-ip>:7880` with `livekit-server --dev --bind 0.0.0.0`.
- The cause of the ~220 ms p95 tail on Wi-Fi (see above).
- Capture delay from the app redraw: stamping happens after capture. The planned
  `--marker screen` mode (a marker drawn on screen and captured) would close this gap.
- Text-quality comparison between H.264 and VP8 at equal bitrate.
- Encoder tuning (rate control, buffer sizes). The stock SDK doesn't expose it; it would
  need a custom libwebrtc build (`LK_CUSTOM_WEBRTC`). Revisit only if real-network numbers demand it.
