# Performance

Latency is the product, so we measure it and publish the results, including the parts that aren't
good yet. Everything here can be [reproduced](#reproduce-these-numbers).

Numbers are from 2026-10-31, on an Apple M4 MacBook (macOS 26.5.1) and a Windows laptop with Intel
Wi-Fi 6 AX201. LiveKit runs on the Mac. TwinCursor is pre-release, so expect these to change.

## Summary

| Measure | Result |
|---|---|
| 1080p between two machines on home Wi-Fi | 60 fps, round trip 29 ms typical, 105 ms at p95 |
| Same, on one Mac with no network | 59 fps, round trip 16 ms typical, 24 ms at p95 |
| Text quality, H.264 | SSIM 0.9996 at about 0.8 Mbps, even while scrolling fast |
| Sharer CPU, H.264 at 1080p | 14% of one core |
| Delay the input path adds | Under 1 ms on top of the network |

**Round trip** is the remote-control loop: the viewer sends a tick, the sharer stamps it into the
next frame, and that frame comes back and is decoded. It runs on the viewer's clock, so it's valid
between machines. It leaves out the app's own redraw and part of the capture delay, so **it isn't
glass-to-glass latency yet**. Results here are headless, so they stop at decode. A window adds about
9–11 ms with today's CPU renderer, which is the largest single cost we control.

**SSIM** compares each decoded frame with what was sent, and 1.0 means identical. It's measured on a
synthetic code editor with a bitmap font. Real anti-aliased text has more fine detail, so expect
lower absolute scores on a real screen. Rely on the ranking between codecs.

## Codecs

One Mac, synthetic code editor, 30 s each, H.264 through VideoToolbox:

| Config | fps | Round trip p50 | p95 | SSIM p5 | Fast-scroll SSIM p5 | Bitrate | Sharer CPU | Viewer CPU |
|---|---|---|---|---|---|---|---|---|
| H.264 1080p | 59 | 16 ms | 24 ms | 0.9996 | 0.9995 | 0.8 Mbps | 14% | 23% |
| H.264 1440p | 59 | 14 ms | 29 ms | 0.9996 | 0.9996 | 0.9 Mbps | 20% | 38% |
| VP8 1080p | 58 | 5 ms | 76 ms | 0.9816 | 0.9798 | 3.3 Mbps | 71% | 25% |
| VP8 1440p | 59 | 18 ms | 261 ms | 0.9891 | 0.9869 | 4.1 Mbps | 96% | 38% |

H.264 is sharper, steadier and about 4x lighter on the network, at under a quarter of the sharer's
CPU, which is why it's the default ([ADR 0002](adr/0002-default-video-codec.md)). Compare p95 here.
A single p50 is unreliable, for the reason under [things that fooled us](#things-that-fooled-us).
VP9 was worse than both on every measure in earlier runs.

## Real Wi-Fi

Mac sharer to Windows viewer on one 5 GHz access point, so packets take two wireless hops. H.264
1080p, three 120 s runs:

| Run | fps | Video round trip p50 | p95 | max | Loss | Freezes |
|---|---|---|---|---|---|---|
| 1 | 60 | 29 ms | 105 ms | 384 ms | 0% | 0 |
| 2 | 60 | 29 ms | 103 ms | 357 ms | 0% | 0 |
| 3 | 60 | 29 ms | 107 ms | 421 ms | 0% | 0 |

**The tail comes from the Mac's Wi-Fi.** Pings from the Mac to its router, with no video and one
hop, show p50 4 ms, p95 73 ms and max 95 ms on their own. Spikes start every 0.5 s, in bursts of 2–4
s. The laptop's link to the same router is clean, with p95 4 ms.

**AWDL is the likely cause, but it isn't proven.** [Public reports](https://www.theregister.com/2025/10/23/apple_airdrop_awdl_latency_research/)
say AWDL, which AirDrop and Handoff use, prefers channels 44 and 149, and that a Mac on another
channel keeps hopping away, with delays near 90 ms. Our router is on channel 100. Apple doesn't
document this.

Earlier sessions also saw real stalls of 0.6–2.5 s, with the link RTT spiking at 0% loss. They don't
happen every run, so the lab reproduces them.

## Input path

Remote-control input travels from the viewer, through LiveKit, to the sharer. These numbers come
from pings on both data channels with video running. One-way delay, except where noted:

| Network | Added delay | Reliable p50 | p95 | Lossy p50 | p95 | Lossy lost |
|---|---|---|---|---|---|---|
| lan | 0 | 0.7 ms | 3.8 ms | 0.5 ms | 0.9 ms | 0% |
| wifi-good | 2 ±2 ms | 2.9 ms | 7.1 ms | 2.7 ms | 5.2 ms | 0.3% |
| wifi-bad | 8 ±8 ms | 8.6 ms | 16 ms | 9.1 ms | 16 ms | 3.2% |
| 4g | 25 ±5 ms | 26 ms | 31 ms | 26 ms | 31 ms | 1.5% |
| hotel | 40 ±10 ms | 42 ms | 127 ms | 41 ms | 50 ms | 9.3% |
| intercontinental | 75 ms | 76 ms | 77 ms | 76 ms | 77 ms | 0.2% |
| udp-blocked (ICE over TCP) | 0 | 0.4 ms | 0.6 ms | 0.7 ms | 1.6 ms | 0% |

Round trip on one Mac is 1.2 ms p50 and 2 ms p95. Round trip on real Wi-Fi is 7 ms p50 and about
77 ms p95, the same tail as above.

- **The path adds under 1 ms.** Everything else in the table is the network.
- **Reliable has a tail under loss, and lossy doesn't.** See `hotel`. That's why keys and clicks
  use reliable while pointer moves use lossy ([ADR 0003](adr/0003-input-transport.md)).
- **Measure input one way on congested links.** The round trip's echo shares the viewer's downlink
  with the video. On `hotel`, reliable shows p95 127 ms one way and 3.9 s round trip, which input
  would never feel. The two-machine round trip above can't be split this way, because the clocks
  differ.

### Link stalls

The viewer is cut off both ways, three times a run, for 60 s. Each cell is the worst one-way delay
in one run:

| Stall | Runs | Reliable worst case | Lossy worst case | Lossy lost per run |
|---|---|---|---|---|
| 0.6 s | 2 | 1.2 s, 1.2 s | 0.8 s, 0.7 s | 15–16 |
| 1.2 s | 4 | 1.2 s, 2.8 s, 2.8 s, 2.8 s | 0.8 s, 2.3 s, 2.3 s, 2.3 s | 12–15 |
| 2.5 s | 2 | 2.8 s, 2.8 s | 2.3 s, 2.4 s | 12–15 |

A single 5 s outage delayed both channels by 5 s and lost 8 lossy pings per stream.

- **Calls survive,** and video was back within 3 s of the network returning in every run.
- **Reliable delivers everything in order, but recovery can outlast the stall.** A 1.2 s stall
  usually cost about 2.8 s, which fits retransmit backoff. We haven't confirmed that.
- **Lossy queues in a stall instead of dropping.** The number lost stays around 12–16 however long
  the stall is. The rest arrive late and in order, so a sequence number can't catch them. Without
  an age check, a guest's cursor would replay old movement.

## Network lab

Sharer, viewer and LiveKit run in Docker on one Mac, with conditions applied by `tc netem`. It uses
**software VP8**, because Linux in Docker has no hardware encoder, and it shares the laptop's CPU.
That makes it harsher than the real product: compare runs and profiles rather than treating these
as what users will see. 1080p, 60 s. Ranges are three runs, and other rows are one:

| Profile | fps | Round trip p50 | p95 | SSIM p5 | Loss | Freezes | Longest stall | Target |
|---|---|---|---|---|---|---|---|---|
| lan | 57–58 | 12–15 ms | 46–101 ms | 0.982–0.983 | 0% | 1–5 | 0 s | 1 of 3 |
| wifi-good | 53–57 | 13–21 ms | 85–174 ms | 0.981–0.984 | 0.1% | 2–4 | 0–3 s | 0 of 3 |
| wifi-bad | 58 | 27 ms | 69 ms | 0.981 | 0.4% | 0 | 0 s | ✅ |
| 4g | 58 | 61–71 ms | 134–192 ms | 0.981–0.987 | 0.3–0.4% | 1–4 | 0 s | 1 of 3 |
| hotel | 23 | 96 ms | 208 ms | 0.971 | 10.7% | 8 | 16 s | ✅ |
| intercontinental | 57 | 156 ms | 229 ms | 0.982 | 0% | 0 | 0 s | ✅ |
| sharer-uplink-3mbit | 24 | 48 ms | 494 ms | 0.976 | 0.3% | 8 | 1 s | ❌ |
| bandwidth-drop | 41 | 19 ms | 196 ms | 0.982 | 0% | 3 | 0 s | ✅ |
| udp-blocked | 59 | 15 ms | 61 ms | 0.985 | 0% | 1 | 0 s | ✅ |
| outage-5s | 52 | 15 ms | 56 ms | 0.982 | 0% | 2 | 6 s | ✅ |
| stall-0.6s | 56 | 15 ms | 138 ms | 0.982 | 1.7% | 4 | 0 s | ✅ |
| stall-1.2s | 49–53 | 6–15 ms | 145–208 ms | 0.981–0.989 | 0.6% | 3–4 | 2–3 s | ✅ |
| stall-2.5s | 48 | 12 ms | 209 ms | 0.982 | 0% | 4 | 2 s | ✅ |

Targets are in [ARCHITECTURE.md](ARCHITECTURE.md#network-lab-and-slos). `hotel` has no freeze target
because its loss is over 2%.

**Where we miss:**
- **`lan` and `wifi-good` miss their p95 target of about 60 ms in most runs.** The tail is in
  software VP8 under Docker, not the network. Hardware H.264 on the Mac stays at 24 ms in the
  table above.
- **`4g` and `sharer-uplink-3mbit` freeze too often.** VP8 sends about 4x more data than H.264, so
  loss hits it more often. FEC would help, but the SDK doesn't expose it.
- **`hotel` needs 16 s to settle,** because the bitrate cap needs detection, a republish and a
  keyframe. Live bitrate control would fix most of this, and it's on the
  [roadmap](ROADMAP.md).

**What works:** with UDP blocked, ICE falls back to TCP and the call runs at 59 fps. The
auto-throttle kept the longest stall at 0 s through a bandwidth drop, where it took 7 s in an
earlier run with it turned off.

## Things that fooled us

Measuring this is easy to get wrong, so these are worth knowing before you compare numbers.

- **Ticks must not line up with frames.** At 250 ms, or exactly 15 frames at 60 fps, every tick in a
  run hits the same point in the frame clock, so one run shows one point of the real 14–33 ms
  spread. We blamed the network for that swing, and we were wrong. `tc-spike view` now ticks every
  247 ms. p50 is still noisy because the spread has two humps, so also look at p95.
- **Machine load matters.** With other apps busy on battery, p95 rose to 50–190 ms in every config.
  Compare numbers only within one session.
- **A sleeping display looks like a broken build.** ScreenCaptureKit lists no displays while the
  screen is asleep or locked, so unattended runs need `--capture pattern`.
- **The lab has its own traps.** netem's `rate` combined with jitter drops far more than configured,
  so rates go through tbf. A `docker compose exec` also takes long enough to blur a sub-second
  stall, so timing runs inside the containers.
- **One lab run isn't a result.** The `lan` p95 has ranged from 46 to 134 ms on the same code.

## Reproduce these numbers

You'll need a build, `jq`, and `livekit-server --dev` running. No windows open, and nothing needs
Screen Recording permission.

```bash
cargo build --release -p twincursor-media-spike
DURATION=30 bench/run-matrix.sh q-h264-1080 q-vp8-1080 q-h264-1440 q-vp8-1440 input-h264
```

The network lab needs Docker, and runs its own LiveKit server:

```bash
VIEW_FLAGS=--measure-input bench/netlab/run.sh --build
bench/netlab/report.sh
```

Results land in `bench/results/`. Two machines take a few more steps: see [Try it](../README.md#try-it),
then run the viewer with `--headless --measure-input`.

## Not measured yet

- True glass-to-glass latency, including the app's redraw
- Mac to Mac, and either end on Ethernet
- With AWDL off, or the router on channel 44 or 149
- Text quality on real anti-aliased screens over a real network
- Linux

If you can run any of these, we'd love your numbers. See
[Contributing](../CONTRIBUTING.md#measurements-count-as-contributions).
