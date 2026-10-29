# ADR 0002: Default video codec for screen sharing

Status: accepted (2026-10-29)

## Context
The sharer can publish VP8, VP9, H.264 or AV1 through the LiveKit SDK. M1 measured latency
and CPU (`research/m1-results.md`): VP9 was worse on every measure, and H.264 (VideoToolbox,
hardware) matched VP8's latency at about 1/5 of the CPU. Text quality was not measured then.

M1.5 added an objective quality measurement (`research/m15-results.md`, "Text quality"):
- a synthetic dark-theme code editor with 8 px and 16 px text and alternating gentle and fast
  scrolling;
- SSIM and PSNR of every decoded frame against what was sent;
- runs at 1080p and 1440p, with default and constrained bitrate caps.

Results, H.264 vs VP8:
- **Sharpness:** H.264 SSIM p5 is 0.9993–0.9996, PSNR p5 45–48 dB. VP8 is 0.977–0.987 and
  31–34 dB.
- **Fast scrolling:** H.264 holds 0.9993+. VP8 drops to 0.975–0.986.
- **Bandwidth:** H.264 sends ~0.8 Mbps. VP8 sends 2.4–3.5 Mbps, and when capped it drops to
  ~37 fps.
- **CPU:** the sharer used 19–31% with H.264 and 54–112% with VP8. Software VP8 can't sustain
  1440p (M1.5 PR 2).
- **Latency tails:** H.264 p95 is 29–34 ms (headless, one Mac). VP8 is 76–247 ms.

Hopp reported problems with H.264 and H.265 on macOS (`research/hopp-lessons.md`): an H.264
profile limited to 720p in the Rust SDK, and H.265 breaking on M1 Macs. With `livekit` 0.9.7,
H.264 runs at 2560x1440 at 60 fps, so the 720p limit doesn't apply to this SDK version. We
don't use H.265.

## Decision
**H.264 is the default codec** on macOS sharers (`tc-spike share --codec h264`). VP8 stays as
the fallback: for sharers without a hardware H.264 encoder, and for any viewer that can't
decode H.264.

## Consequences
- Lower bandwidth leaves headroom on poor networks. That matters most for the auto-throttle
  (M1.5 PR 5) and for sharers on thin uplinks.
- 1440p becomes practical. It needs a hardware encoder, which every supported Mac has.
- **To verify:** H.264 decoding on the Windows viewer, and on Linux later. M1.5 PR 6 runs
  Mac → Windows with H.264. If a viewer can't decode it, we need codec negotiation or a VP8
  backup track.
- Caveats on the measurement:
  - The pattern uses a binary bitmap font. Real anti-aliased text has more fine texture, so
    absolute SSIM on a real screen will be lower. The ranking between codecs is what we rely on.
  - Hardware encoder behaviour can change with macOS updates. Re-run the quality configs in
    `bench/run-matrix.sh` (`q-*`) after major OS or SDK upgrades.
