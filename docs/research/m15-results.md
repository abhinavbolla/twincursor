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

## Auto-throttle (PR 5)
Smoke test on one Mac: pattern at 1440p with H.264 and the bitrate capped at 1 Mbps, below the
1440 floor of 1.6 Mbps. Viewer headless with `--measure-quality`.
- After 3 s the sharer logged `adapt: P1440 → P1080 (1920x1080): budget 0.9 Mbps < 1.6 Mbps
  needed` and held 1080p for the rest of the run. The encoder (VideoToolbox) accepted the new
  size mid-stream, without republishing.
- The viewer received 1920x1080 at 52 fps, SSIM p50 0.998. The HUD showed the reason.
- The first version also stepped down to 720p on "budget 1.0 Mbps < 1.0 Mbps", a budget sitting
  right at the floor. Trouble now means below 90% of the floor.
- Quality scoring after a step-down needs the original render size. The pattern's marker now
  carries it (2 bits), and the viewer renders at that size and scales it the same way the
  sharer did, which matches pixel for pixel.

Lab evaluation on shaped networks (bandwidth drop, hotel, 4G; adapt on vs off) comes with the
network lab (PR 4).

## Sharer frame hand-off: queue vs newest-frame mailbox
The sharer handed frames to the encoder thread through a 4-slot queue. When the encoder falls
behind, every queued frame adds ~16 ms. It's now a mailbox that keeps only the newest frame
and counts the frames it drops. A/B: headless pattern, H.264 1080, 4 alternating runs of 30 s.

| hand-off | RTT p50 by run | RTT p95 by run | one-way p50 |
|---|---|---|---|
| 4-slot queue | 18.1, 18.3, 10.2, 14.0 ms | 69, 66, 21, 33 ms | 11–14 ms |
| mailbox | 14.7, 9.0, 22.6, 20.6 ms | 47, 41, 28, 30 ms | 11–14 ms |

- p50 doesn't change, and p95 is a little lower and steadier (median 35 vs 49 ms), but noisy.
- **The run-to-run p50 swing (~10 vs ~20 ms) is not in the video path.** One-way latency
  (capture → decode) is steady at 11–14 ms in every run. The swing is in the tick's path:
  viewer → SFU → sharer over a LiveKit data packet.
- This matters for M2, because remote-control input will take the same path. Measure the
  data-channel one-way delay on its own (reliable vs lossy, ordered vs not) before building
  input on it.

## Mac → Windows over home Wi-Fi (PR 6)
Sharer: this Mac (Wi-Fi, AWDL active) running `livekit-server --dev --bind 0.0.0.0`. Viewer:
a Windows laptop (Intel Wi-Fi 6 AX201, 1920x1080 at 100% scaling, decoding with FFmpeg),
through the `windows testing` Claude session. 60 s per run, 2026-10-29.

| run | fps | size | RTT p50 | RTT p95 | RTT max | SSIM p5 | fast-scroll SSIM p5 | ticks | path | link RTT max |
|---|---|---|---|---|---|---|---|---|---|---|
| pattern H.264 1080, headless | 60 | 1920x1080 | 25 ms | 105 ms | 320 ms | 0.9996 | 0.9996 | 240/240 | host udp | 78 ms |
| pattern H.264 1440, headless | 59 | 2560x1440 | 44 ms | 127 ms | 2594 ms | 0.9996 | 0.9996 | 231/240 | host udp | 612 ms |
| pattern VP8 1080, headless | 59 | 1920x1080 | 23 ms | 222 ms | 2897 ms | 0.9871 | 0.9838 | 229/240 | host udp | 964 ms |
| pattern H.264 1440, `--no-adapt`, headless | 58 | 2560x1440 | 37 ms | 147 ms | 2984 ms | 0.9996 | 0.9996 | 231/240 | **prflx tcp** | 219 ms |
| real screen H.264 1080, headless | 47 | 1664x1080 | 20 ms | 117 ms | 324 ms | - | - | 240/241 | host udp | 87 ms |
| real screen H.264 1440, windowed (shown at 1466x950) | 37 | 2224x1440 | 37 ms | 533 ms | 2475 ms | - | - | 176/240 | host udp | **2457 ms** |

Findings:
1. **H.264 decodes on Windows** (FFmpeg). This resolves the open item in ADR 0002. Text quality
   survives the network unchanged: SSIM p5 0.9996 at 1080p and 1440p, the same as on one
   machine, at 0.76–0.81 Mbps. VP8 again lands at 0.987 (PSNR p5 34.8 dB) and uses 3.3 Mbps.
   The ranking from one machine holds across a real network.
2. **The Wi-Fi tail is the network, and the stats show it.** Every multi-second RTT max came
   with 0% loss while the link RTT spiked (612 ms, 964 ms, 2.5 s). In the windowed run the ICE
   For example, in the windowed run the ICE link RTT (STUN consent checks) itself reached 2.5 s. Those are Wi-Fi
   stalls (power save, scanning or AWDL), not encode or decode. 64 lossy ticks were lost in
   the stall.
3. **ICE sometimes settles on TCP.** One run connected over `prflx tcp`. Its p95 and max (147 ms
   and 3 s) show TCP's head-of-line blocking. The HUD shows the path, so a user can see when
   this happens. Worth investigating: why UDP lost that ICE race on a LAN.
4. **`freeze_ms` is always 0** on this libwebrtc build, even when `freezes` > 0. The adapter
   now counts freezes instead (fixed in PR 5).
5. The Windows viewer showed 1440p downscaled to fit its 1080p screen (2224x1440 shown at
   1466x950), as designed.
6. Real-screen runs send 35–48 fps because the screen doesn't change every frame. That's
   expected with ScreenCaptureKit.
7. On a good link the adapter correctly did nothing: no tier changes and no limit reported in
   any run.

Still to run, and each needs a hand from you: Mac on Ethernet instead of Wi-Fi; AWDL off
(`sudo ifconfig awdl0 down`, then `up` afterwards); and Network Link Conditioner profiles on
the Mac. These separate Wi-Fi stalls from everything else.

## Network lab (PR 4)
`bench/netlab/` (profiles and SLOs in ARCHITECTURE.md): pattern sharer, headless viewer and
LiveKit in Docker on this Mac, 60 s per run. **VP8 software**, because Linux in Docker has no
hardware encoder. That makes the lab harsher than the product, since the Mac's H.264 sends
about 4x fewer bits. Final adapter: frame rate first, jump on severe drops, bitrate cap.

| profile | fps | RTT p50 | RTT p95 | SSIM p5 | loss | freezes / min | longest stall | SLO |
|---|---|---|---|---|---|---|---|---|
| lan | 50 | 8 ms | 78 ms | 0.983 | 0% | 0 | 0 s | miss (p95) |
| wifi-good | 52 | 13 ms | 62 ms | 0.982 | 0.1% | 0 | 0 s | pass |
| wifi-bad | 52 | 33 ms | 143 ms | 0.986 | 0.3% | 0 | 0 s | pass |
| 4g | 51 | 78 ms | 244 ms | 0.984 | 0.5% | 51 | 0 s | miss (freezes) |
| hotel, adapt off | 0 | 594 ms | 594 ms | 0.977 | 47% | 0 | 59 s | (loss > 2%) |
| **hotel, adapt on** (2 runs) | **13, 19** | 113, 155 ms | 348, 457 ms | 0.979, 0.974 | 11%, 8% | 63, 52 | 15 s, 15 s | (loss > 2%) |
| intercontinental (150 ms) | 51 | 159 ms | 292 ms | 0.983 | 0% | 0 | 0 s | pass |
| sharer uplink 3 Mbit, adapt off | 28 | 116 ms | 1024 ms | 0.977 | 0% | 6 | 2 s | miss (freezes) |
| sharer uplink 3 Mbit, adapt on | 23 | 52 ms | 623 ms | 0.976 | 0.2% | 6 | 1 s | miss (freezes) |
| bandwidth drop, adapt off | 38 | 13 ms | 410 ms | 0.979 | 0.1% | 10 | **7 s** | **fail** |
| **bandwidth drop, adapt on** (2 runs) | 38, 37 | 17, 43 ms | 533, 247 ms | 0.988, 0.984 | 0.1–0.3% | 11, 2 | **0 s, 2 s** | pass |
| UDP blocked (ICE over TCP) | 49 | 8 ms | 78 ms | 0.982 | 0% | 0 | 0 s | pass |
| 5 s outage | 45 | 16 ms | 190 ms | 0.985 | 0% | 1 | 6 s | pass |

Findings:
1. **Recovery works.** After a 5 s outage, video is back about 1 s after the network returns.
   With UDP blocked, ICE falls back to TCP and the call still runs at 49 fps.
2. **The adapter turns a bandwidth drop from a 7–10 s stall into 0–2 s.** It jumps straight to
   720p 30 fps instead of walking down.
3. **A congested viewer downlink needs a bitrate cap, not just a lower level.** Into a 3 Mbit,
   2%-loss downlink, VP8 kept sending 4–5.6 Mbps at any frame rate or resolution, because the
   encoder spends whatever target the sharer's uplink estimate allows. The viewer decoded 0
   fps, and the SFU still reported "allocation: optimal".
   - Hand-set caps: 2.5 Mbps still lost 21%; 1.5 Mbps ran at 18 fps with no stall.
   - The adapter now halves the max bitrate to the viewer's goodput when it overflows. It does
     this by republishing, because the SDK has no live bitrate API. That brings hotel from
     0 fps to 13–19 fps.
   - The 15 s stall left is the start-up before the cap: detection, republish, and keyframe
     recovery.
   - H.264 (OpenH264 at 30 fps) on the same profile ran at 28 fps with no stall: its actual
     bitrate is low.
4. **Remaining misses are VP8-in-Linux effects, not adaptation:**
   - lan p95 is 78 ms: software VP8 tails. On the Mac with hardware H.264, p95 is 29–35 ms.
   - 4G has ~50 freezes a minute: 0.5% loss at VP8's packet rate, with a 50 ms RTT for each
     NACK. H.264's ~4x lower rate means ~4x fewer loss events. FEC would help more, but the
     SDK doesn't expose it.
5. **Lab pitfalls we hit, for anyone extending it:**
   - netem's `rate` plus jitter tail-drops far more than configured, so rates go through tbf.
   - The report must match labels exactly (`4g-vp8-1080` vs `4g-vp8-1080-noadapt`).
   - Linux builds need clang 21+ and an explicit GLib/GIO link.
