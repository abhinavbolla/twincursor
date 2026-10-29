# M1.5 results: high-fidelity media

Measurements for milestone M1.5 (see `ROADMAP.md`). Same Mac as `m1-results.md` (Apple M4,
3024x1964 panel), sharer and viewer on one machine with `livekit-server --dev`, busy "mirror"
screen, `bench/run-matrix.sh`, 3 rounds × 30 s. Medians of the rounds unless noted.

**Machine load matters.** On 2026-10-29 the first matrix ran on battery with load average ~6.4
(other apps at 20–65% CPU each): p95 rose to 50–190 ms in every config, including ones identical
to M1. Later runs on a quieter machine were back to M1's ~31–35 ms. Compare numbers only within
one run, and prefer the network lab (PR 4) for controlled conditions.

## Stats polling cost (PR 1)
Stats polled once a second plus the HUD, vs `--no-stats`, on the loaded machine:

| config | RTT p50 on / off | sharer CPU on / off |
|---|---|---|
| sck-vp8 | 19.6 / 22.4 ms | 47% / 46–63% |
| sck-h264 | 20.9 / 22.6 ms | 9–13% / 10–15% |

No measurable cost. p95 was noise-dominated in both variants.

## 1080 vs 1440 (PR 2)

| config | size | fps | RTT p50 | RTT p95 | dec p50 | sharer CPU | viewer CPU |
|---|---|---|---|---|---|---|---|
| H.264 1080 | 1664x1080 | 56 | 16 ms | 35 ms | 8 ms | 7–9% | 48–51% |
| H.264 1440 | 2224x1440 | 54 | 29 ms | 94 ms (46–123) | 12 ms | 9–12% | 72–82% |
| VP8 1080 | 1664x1080 | 56 | 17 ms | 32 ms | 6 ms | 37–39% | 52–55% |
| VP8 1440 | 2224x1440 | 48 | 34 ms | 416 ms | 13 ms | 120–170% | 81–87% |

Findings:
1. **1440p needs hardware H.264.** Software VP8 at 2224x1440 takes more than a full core on the
   sharer and its p95 goes to ~400 ms. H.264 (VideoToolbox) stays at ~10% CPU.
2. **1440p costs ~12 ms of RTT, mostly on the viewer.** Decode-to-screen grows from ~8 ms to
   ~17 ms because the viewer converts I420 to RGB on the CPU and softbuffer copies 2.4x more
   pixels. The Metal/wgpu viewer (an M1 follow-up) is now the main lever for 1440p latency.
3. The bandwidth estimate on localhost settled around 5 Mbps, so the target bitrate stayed
   near 5 Mbps at both tiers, below the 8 Mbps cap for 1440. Actual bitrate on this screen was
   0.3–0.8 Mbps (H.264) and 0.6–2.3 Mbps (VP8).
4. Widths are now multiples of 16; the encoder no longer crops (1664 in, 1664 out).

Default stays 1080. Offer 1440 with H.264 once PR 3 shows whether the extra pixels make text
measurably sharper.

## Text quality: H.264 vs VP8 (PR 3)
Synthetic code editor (`share --capture pattern`, 16:9 reference: 1920x1080 and 2560x1440),
headless viewer with `--measure-quality`, one 40 s run each (two gentle-scroll and two
fast-scroll phases), no windows and no screen capture. RTT here is measured at decode, and
includes rendering the pattern on the sharer (about 10 ms more at 1440p).

| config | fps | RTT p50 | RTT p95 | SSIM p50 | SSIM p5 | fast-scroll SSIM p5 | PSNR p5 | actual rate | sharer CPU |
|---|---|---|---|---|---|---|---|---|---|
| H.264 1080 (cap 5 Mbps) | 58 | 11 ms | 29 ms | 0.9996 | 0.9996 | 0.9996 | 48.4 dB | 0.8 Mbps | 19% |
| H.264 1080, cap 1.5 Mbps | 58 | 14 ms | 34 ms | 0.9994 | 0.9993 | 0.9993 | 45.2 dB | 0.7 Mbps | 19% |
| H.264 1440 (cap 8 Mbps) | 59 | 29 ms | 33 ms | 0.9996 | 0.9996 | 0.9996 | 48.4 dB | 0.9 Mbps | 31% |
| H.264 1440, cap 2.5 Mbps | 59 | 28 ms | 32 ms | 0.9995 | 0.9995 | 0.9995 | 47.2 dB | 0.8 Mbps | 31% |
| VP8 1080 (cap 5 Mbps) | 59 | 17 ms | 84 ms | 0.9978 | 0.9845 | 0.9817 | 33.4 dB | 3.5 Mbps | 75% |
| VP8 1080, cap 1.5 Mbps | 37 | 14 ms | 247 ms | 0.9916 | 0.9774 | 0.9745 | 31.3 dB | 1.5 Mbps | 54% |
| VP8 1440 (cap 8 Mbps) | 59 | 10 ms | 76 ms | 0.9980 | 0.9871 | 0.9857 | 34.0 dB | 3.6 Mbps | 112% |
| VP8 1440, cap 2.5 Mbps | 37 | 28 ms | 164 ms | 0.9953 | 0.9818 | 0.9812 | 31.9 dB | 2.4 Mbps | 78% |

Findings:
1. **H.264 is sharper, cheaper and steadier.** Its text quality barely moves with bitrate cap,
   resolution or fast scrolling. It sends ~4x less data than VP8, at ~1/4 of the CPU.
   Decision: H.264 is the default (ADR 0002).
2. **VP8 trades frame rate for quality when capped.** It drops to ~37 fps and its p95 RTT
   grows to 160–250 ms.
3. **1440p H.264 costs ~17 ms more RTT than 1080p here,** about 10 ms of which is rendering
   the pattern itself. With a real screen and window (PR 2) it was ~12 ms.
4. A first run of this matrix measured some "1440" configs at 1920x1080. While the Mac's
   screen was asleep or locked, ScreenCaptureKit listed no displays, and the size fell back to
   the default. The pattern source now uses a fixed 16:9 reference display, so results don't
   depend on the machine or its screen.

Caveat: the pattern uses a binary bitmap font. Anti-aliased text on a real screen has more
fine texture, so expect lower absolute SSIM there. Compare real screens with
`--snapshot-every` and `tc-spike compare`.
