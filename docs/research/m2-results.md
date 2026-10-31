# M2 results: remote control

Measured with `tc-spike view --measure-input`, 2026-10-31.

## The input path (PR 1)

Remote-control input travels viewer → SFU → sharer as LiveKit data packets. The viewer pings on
both data channels, and the sharer echoes at once.

**On one Mac** (H.264 1080p pattern, 3 runs of 30 s), round trip p50 is 1.0–1.5 ms and p95 under
2.5 ms, on both channels. Latency budget for input is not a concern on a clean link.

**In the network lab** (60 s each, video running, one-way delay in ms):

| profile | network delay | reliable p50 | p95 | max | lossy p50 | p95 | max | lossy lost |
|---|---|---|---|---|---|---|---|---|
| lan | 0 | 0.5 | 0.7 | 2 | 0.6 | 1.1 | 6 | 0% |
| wifi-bad | 8 ±8 | 8.7 | 16.3 | 92 | 9.1 | 16.2 | 19 | 3% |
| 4g | 25 ±5 | 25.9 | 30.8 | 155 | 25.8 | 30.7 | 131 | 1.7% |
| hotel | 40 ±10 | 42.3 | 132 | 297 | 41.8 | 50 | 52 | 6.5% |

`udp-blocked` (ICE over TCP, 30 s) round trip: 1 ms p50, 1.3 ms p95 on the reliable channel,
1.7 ms on lossy, with no packets lost. `intercontinental` (30 s) round trip: 151.5 ms p50, which
is the network itself.

What this means:
- **The path adds under 1 ms.** Everything above that in the table is the network.
- **Keys and clicks go on the reliable channel.** At most 1 ping in 600 went unanswered, which
  fits one still in flight when the run ended. The price is a tail under loss: p95 132 ms and max
  ~300 ms on `hotel`, from retransmissions.
- **Pointer moves go on the lossy channel.** It has no such tail (p95 50 ms on `hotel`), and a
  lost move doesn't matter because the next one replaces it. That settles the transport question
  in the M2 plan.
- **Use one-way delay, not round trip, on congested links.** On `hotel` the reliable round trip
  has p95 600 ms against 132 ms one way, because the echo returns over the viewer's downlink,
  which the video fills. Input only uses the viewer's uplink. The first 30 s hotel run, before the
  auto-throttle settled, showed a round-trip p95 of 3 s for the same reason.
- **The sharer's main loop costs nothing measurable.** Pings answered from the loop that also
  awaits stats polls match pings answered by a task of their own, at p50 and p95, in every
  profile. Republishing wasn't exercised; it's rare and input will use its own subscription
  anyway.
- Nothing arrived out of order on either channel in the 60 s runs. The 30 s `hotel` run had 8–11
  out-of-order pings on the lossy channel; moves must keep a sequence number and drop stale ones.

## The run-to-run swing was our measurement

M1.5 blamed a 10–20 ms swing in RTT p50 on the data-packet path. That was wrong: the path takes
about 1 ms.

The cause is the tick interval. 250 ms is exactly 15 frames at 60 fps, so all of a run's ticks land
at the same phase of the frame clock. A run samples one point of the real spread, set by its
start timing:

| ticks | stats | RTT p50 (3 runs) | RTT p95 |
|---|---|---|---|
| 250 ms | on | 13.7, 13.8, 14.0, 14.8, 15.8, 13.9 | 16–26 |
| 250 ms | off | 25.6, 25.9, 27.7 | 30–33 |
| 247 ms | on | 14.8, 17.3, 20.5 | 31–33 |
| 247 ms | off | 14.7, 20.9, 14.7 | 32–34 |

- With 250 ms ticks, stats on and off looked 12 ms apart. At 247 ms they match, so stats polling
  isn't a latency cost. We don't know why the 250 ms runs sorted so neatly by that flag; start
  timing is the likely reason. The lab showed the same split at 250 ms with one setup: 12.8 and
  25.4 ms on `lan`.
- The honest picture is a spread of about 14–33 ms from "input arrives" to "frame showing it
  decoded", depending on where in the 16.7 ms frame cycle it lands. p95 at 250 ms understated
  this: 16 ms against the real ~32 ms.
- `tc-spike view` now ticks every 247 ms by default. Compare new RTT numbers only with each other:
  p50 remains noisy because the spread has two humps, so also look at p95.

## Latency effect of PR 1

A/B against `main`, headless pattern, H.264 1080p, 247 ms ticks on both, 3 alternating runs of 30 s:

| build | fps | RTT p50 | RTT p95 | one-way p50 |
|---|---|---|---|---|
| main | 58, 59, 59 | 20.6, 15.9, 20.8 ms | 33.8, 32.8, 32.9 ms | 17.0, 17.9, 17.7 ms |
| this PR | 59, 59, 59 | 21.3, 21.8, 21.7 ms | 31.3, 32.5, 34.8 ms | 17.9, 17.8, 17.9 ms |

No measurable change. Pings only flow with `--measure-input`, and the lab's `lan` video numbers
are the same with and without them.

## Open items

- **`lan` now misses its RTT p95 SLO in the lab** (74–134 ms against 60 ms), with or without pings
  and at either tick interval. The tail is in the video's one-way delay (p95 60–123 ms, p50 8 ms),
  from software VP8 in Docker. The final M1.5 lab sweep ran before the pacing fix, at ~50 fps
  instead of 59, which likely hid it. Worth a look on hardware H.264 in M6, not an M2 blocker.
- **Mac → Windows over real Wi-Fi isn't measured yet.** Link stalls of 0.6–2.5 s are the case to
  check, since the lab doesn't reproduce them.
- `input-sck-h264` in the matrix produced nothing in the unattended runs, likely because the
  display was asleep, so the numbers above use the synthetic pattern. Pings don't depend on the
  capture source.
