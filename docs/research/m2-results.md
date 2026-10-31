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
  in the M2 plan, with one guard for link stalls (below).
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

## Mac → Windows over real Wi-Fi

Sharer and LiveKit on this Mac (Wi-Fi, -47 dBm, AWDL active), viewer on the Windows laptop (signal
88%), both on the same 5 GHz access point on channel 100, so each packet takes two wireless hops.
H.264 1080p pattern, headless, 247 ms ticks, 3 runs of 120 s. Range is across the four streams:

| run | input RTT p50 | p95 | max | lost | out of order | video RTT p50 | p95 | fps |
|---|---|---|---|---|---|---|---|---|
| 1 | 6.9–7.0 ms | 76–80 ms | 101–112 ms | 0 | 0 | 29 ms | 105 ms | 60 |
| 2 | 6.8–6.9 ms | 75–78 ms | 101–113 ms | 0 | 0 | 29 ms | 103 ms | 60 |
| 3 | 6.8–7.1 ms | 75–78 ms | 101–103 ms | 0 | 0 | 29 ms | 107 ms | 60 |

Video loss was 0.0%, with no freezes and no reconnects. `reliable/task` shows one ping unanswered
per run, the one in flight when the run ended, so `lost` above counts none.

- **Reliable and lossy match on this link.** So do the task and main-loop routes. The choice in
  this doc only matters under loss.
- **The tail is this Mac's Wi-Fi, not our path.** Pings from the Mac to its router, with no video
  and one hop, gave p50 4.1 ms, p90 49 ms, p95 73 ms and max 95 ms: the same tail on their own. They
  aren't random. Spikes start every 0.5 s, in bursts of 2–4 s with 2–3 s of quiet between. The
  laptop's own link to the same router is clean: p50 1 ms, p95 4 ms, one spike over 20 ms in 600
  pings, so the tail is on the Mac's side.
- **AWDL is the likely cause, not yet proven.** Public reports say AWDL prefers channels 44 and 149
  on 5 GHz, and that a Mac on another channel keeps hopping away from it, with delays near 90 ms
  ([The Register](https://www.theregister.com/2025/10/23/apple_airdrop_awdl_latency_research/)).
  This router is on channel 100 and our spikes peak at 93–95 ms. Apple documents none of this. The
  proof is the same ping with AWDL off.
- **The tail falls on the direction input uses.** Clocks differ here, so one-way values are
  meaningless and the offset moved 23 ms between runs, but within a run the shape still holds.
  One-way p95 minus p50 is 60–66 ms against 68–73 ms for RTT, and max minus p50 matches RTT within
  3 ms. That is what a late-arriving ping looks like: it waits for the Mac's radio, and the echo
  then leaves at once. Expect 20–100 ms extra on about 1 input in 7 on this setup.
- **No link stall happened** in these 6 minutes, so the lab tests those instead (below).

## Link stalls

Real stalls of 0.6–2.5 s came and went in M1.5, so the lab now scripts them: the viewer is cut off
both ways three times per run. VP8 1080p, 60 s each, one run per length, one-way delay:

| stall | reliable max | lossy max | lossy lost (per stream) | out of order | longest video stall | freezes |
|---|---|---|---|---|---|---|
| 0.6 s | 1.2 s | 0.8 s | 15–16 of 600 | 0 | 1 s | 5 |
| 1.2 s | 2.8 s | 2.3 s | 13 of 600 | 0 | 3 s | 3 |
| 2.5 s | 2.8 s | 2.3 s | 13–15 of 600 | 0 | 2 s | 4 |

The calls survived every stall, and video came back within 3 s of the network returning.

- **Reliable keeps its promise, at a price.** Every ping arrived in order, apart from the last one
  of each run, still in flight. But recovery outlasts the stall: a 1.2 s stall cost up to 2.8 s.
  That fits retransmit backoff (a timer near 1 s, then doubling), though we haven't confirmed it.
- **Lossy doesn't drop stale data in a stall.** It queues it. A 2.5 s stall spans 75 lossy pings per
  stream over its three repeats, and only 13–15 were lost; the rest arrived up to 2.3 s late and
  in order. So a sequence number can't catch these moves, and without a guard the guest's cursor
  would replay seconds of old movement.
- **PR 2 has to handle both.** Moves carry the viewer's timestamp. The sharer keeps the smallest
  arrival-minus-timestamp as a baseline, which needs no shared clock, and drops any move more than
  150 ms over it. After the heartbeat deadman fires, late key-ups must be no-ops and a late
  button-down must not start a drag. Keys typed during a stall still arrive, in order.
- **Data tracks are worth checking.** Both channels share one SCTP connection, which may be why
  lossy waits behind reliable here. LiveKit's newer data tracks might avoid it for moves.

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
- **Prove the AWDL cause.** Repeat the router ping with AWDL off, then with the router on channel
  44 or 149. It may also explain the video tail in `m1-results.md`.
- `input-sck-h264` in the matrix produced nothing in the unattended runs, likely because the
  display was asleep, so the numbers above use the synthetic pattern. Pings don't depend on the
  capture source.
