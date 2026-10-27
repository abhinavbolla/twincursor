# ADR 0001: Screen capture API on macOS

Status: accepted (2026-10-27)

## Context
The sharer has to capture the display at about 1080p and up to 60 fps, with as little delay
and CPU as possible, and hand frames to the LiveKit/libwebrtc encoder. Two options were
built and measured in M1 (`crates/media/src/capture/`, results in
`docs/research/m1-results.md`):

- **ScreenCaptureKit** through the `screencapturekit` crate (MIT/Apache-2.0). SCK scales and
  converts to NV12 on the GPU, and pushes frames only when the screen changes.
- **libwebrtc's `DesktopCapturer`**, shipped with the LiveKit SDK. It is cross-platform. On
  macOS it uses SCK internally, but it returns full-resolution BGRA and must be polled. That
  leaves conversion and scaling to the CPU. Hopp forked LiveKit to get access to it; upstream
  has exposed it since late 2025.

## Decision
Use **ScreenCaptureKit directly** on macOS, behind the `capture::Backend` abstraction.
Keep the `DesktopCapturer` backend as a portable fallback and a starting point for Linux and Windows.

## Consequences
- At 1660x1080 VP8: sharer CPU was 43–46% with SCK vs 59–66% with DesktopCapturer, and p95
  latency was steady with SCK but had heavy tails in 2 of 4 DesktopCapturer rounds.
- SCK gives us a capture timestamp (presentation time on the host clock), which is how we
  measure capture-to-screen latency.
- SCK sends nothing on a static screen, so the sender re-sends the last frame (every 250 ms,
  and immediately when a tick arrives).
- We depend on a fast-moving crate (`screencapturekit` ships major versions often). It is
  pinned (`=11.1.1`), and upgrades are deliberate.
- Binaries need `-Wl,-rpath,/usr/lib/swift` (set in `.cargo/config.toml`), because the crate's
  Swift bridge links the Swift runtime.
- Future: pass SCK's CVPixelBuffer to the encoder without copying
  (`NativeBuffer::from_cv_pixel_buffer`), and exclude our own windows from capture with an SCK content filter.
