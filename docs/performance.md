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
| Voice on one Mac, sound sent → played | 45–54 ms typical, screen latency unchanged |
| Voice on one Mac, out of real speakers into a mic | 155–159 ms typical, 178–204 ms at p95 |
| Echo removed, a MacBook's own speakers and mic | 46–53 dB once the canceller has settled |

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

### Remote control end to end

`bench/control-e2e.sh` sends a 168-step script (pointer sweeps, clicks, a drag, both kinds of
scroll, 46 characters of code with accents and symbols, ⌘A ⌘C, and Shift-arrow selection) from a
headless guest to a sharer on one Mac, then compares what the sharer would inject with what the
guest sent.

- **5 runs out of 5 matched exactly:** 72 keys, clicks, scrolls and characters each time, in order,
  and every pointer move arrived. A refused request injected nothing.
- **Packets sent just after joining can be lost.** In one run, the guest's first four requests,
  over about 4 s, never reached the sharer, though both sides were connected and the video was
  flowing. Once one got through, nothing else was lost. The guest now repeats an unanswered
  request every second, and the sharer treats a repeat as the same request.
- **2,000 characters typed as fast as the guest can send arrive exactly:** 2,019 actions (with
  the release), 5 runs out of 5. The first try lost about 1,600 of them, and it wasn't the
  network. A burst like that queues up on the guest's side, and the viewer left the room before
  the queue was sent. The guest now waits for everything to go out before it leaves.
- **Through the network lab, too.** The same scripts, with the sharer in a container
  (`bench/netlab/control.sh`), pass on `lan`, `wifi-bad`, `4g` and `hotel`: the smoke script
  and 2,000 characters, every key and click exactly once and in order. The first lab run found
  two bugs, fixed since ([ADR 0003](adr/0003-input-transport.md#update-2026-11-07)): a move
  that overtook a resent button release, which dragged with the button down, and the end of
  fast typing cut off when the guest left.
- **A revoke stops typing at once.** Taking control back 1.5 s into typing 368 characters, with
  Shift and the mouse button held: 5 runs out of 5 stopped mid-text, released both, and let
  nothing but releases through afterwards. In one run, 3 keys the guest sent before it heard of
  the revoke were turned away.
- **Carrying control costs no measurable latency.** Two interleaved 30 s runs each, 1080p H.264,
  synthetic pattern:

| Build | Capture → decode p50 | p95 | Input one-way p50, reliable |
|---|---|---|---|
| Before (main) | 15.8–15.9 ms | 17.2–17.6 ms | 0.8–1.0 ms |
| After, control off | 15.7–15.9 ms | 17.4–17.6 ms | 0.7 ms |
| After, `--allow-control` | 15.7–15.8 ms | 17.6 ms | 0.8 ms |

The viewer window's new letterboxed drawing costs nothing either: capture → presented p50 19.7–19.8
ms against 19.8–20.0 ms before (two interleaved 25 s runs each, 1080p, windowed).

## Voice

Voice is measured with synthetic audio: marker tones that spell each tick's id, timed as they
play out on the viewer ([how](ARCHITECTURE.md#measuring-latency)). That covers the codec, the
network and the receiver's jitter buffer, but not a mic, speakers or echo cancellation. Numbers
are from 2026-11-08 on the same Mac, headless, with 1080p H.264 video running alongside:

| | Voice off | Voice on |
|---|---|---|
| Screen round trip p50 | 9.8–10.0 ms | 8.9–9.2 ms |
| Screen round trip p95 | 25.4–25.5 ms | 24.4–25.2 ms |
| Voice one way p50 | | 45–54 ms |
| Voice one way p95 | | 72–85 ms |
| Audio behind the screen, p50 | | 43–50 ms |
| Audio the receiver made up (concealment) | | 0.04–0.18% |
| Viewer CPU (% of one core) | 4–5% | 7–8% |

All 243 bursts were heard in each run. On a quiet machine, one way holds at about 48 ms p50 and
49 ms p95.

- **Voice doesn't slow the screen.** WebRTC can hold video back to line it up with audio, which
  would add the voice's 45–50 ms lag to every frame. It doesn't here: the screen's round trip is
  the same with voice on. Audio trailing the screen by 50 ms is fine for pairing, since nobody
  lip-reads a code editor.
- **The jitter buffer is most of the delay.** It holds about 30 ms even on one machine, about 60%
  of the one-way time, and grows for a while after any late packets. That's the first thing to
  tune.
- **Voice costs about 66 kbit/s on the wire,** twice the 32 kbit/s Opus bitrate: LiveKit's
  redundant audio (RED) repeats each packet in the next one, so one lost packet costs nothing.

### The real mic

The sharer sends its real mic through WebRTC's device layer, with echo cancellation, noise
suppression and gain control on. In this run the viewer plays nothing, so the two ends on one
Mac can't feed back. 1080p H.264, headless, 60 s, two quiet rounds on 2026-11-08:

| | Voice off | Synthetic voice | Real mic |
|---|---|---|---|
| fps | 59 | 59 | 59 |
| Screen round trip p50 | 14.8–16.2 ms | 15.3–15.9 ms | 15.4–17.2 ms |
| Screen round trip p95 | 29.6–30.4 ms | 30.8–31.0 ms | 30.9–32.6 ms |
| Voice on the wire | | 66 kbit/s | 50–51 kbit/s |

The mic doesn't slow the screen. It sends less than the synthetic voice because Opus sends
almost nothing while the room is quiet (1–2 kbit/s), and the tones never stop. A first round,
run while the Mac was busy, is left out: its voice-off run fell to 50 fps.

### Through real speakers

`bench/audio-loopback.sh` plays the sharer's tones on the viewer's speakers and times them with a
raw mic recording, on one MacBook Pro with its built-in speakers and mic. Two 30 s runs on
2026-11-09:

| | p50 | p95 |
|---|---|---|
| Sharer started → heard in the room | 155–159 ms | 178–204 ms |
| Sharer started → left the viewer's jitter buffer | 53–54 ms | 76–103 ms |
| The rest: WebRTC's playout, the speakers, the air and the mic | 101–106 ms | |

106 and 121 of 121 bursts were heard; the first run was louder (peak −7 dBFS against −24).

- **Most of the delay is after the network.** Played and recorded by macOS directly
  (AVAudioEngine, a beep scheduled for a known time), the built-in speakers to the built-in mic
  take 48 ms, steady to 1 ms. The speakers and mic do their own signal processing, which is
  where that goes. That leaves about 55 ms between WebRTC taking a frame from the jitter buffer
  and the sound reaching macOS's output. That's most likely WebRTC's own macOS playout buffer,
  which the SDK doesn't let us size.
- **It's already past the 150 ms comfort limit**, with no network at all. On top of this, the
  talker's mic adds WebRTC's recording buffer, which the loopback leaves out (it records with its
  own mic path, standing in for the talker's). The jitter buffer, the playout buffer and the
  built-in speakers are the three places left to look; headphones skip the last.
- **The recorder's clock is checked.** Its timestamps are when each buffer's first sample was
  captured, not when it arrived: they are 100–112 ms old when the 100 ms buffer is delivered.

### Echo and noise

A mic track runs WebRTC's echo canceller (AEC3) and noise suppressor. `crates/media/tests/voice_processing.rs`
drives the same modules offline, in CI, with synthetic speech: harmonic syllables with pauses,
played through a simulated room (direct sound plus four reflections) into the mic, with a second
talker on top.

| Case | Result |
|---|---|
| Echo removed, first 2 s | 31–39 dB |
| Echo removed, after that | 37–47 dB |
| The local voice, alone | within 0.2 dB |
| The local voice, while both talk | 0.5–4.6 dB quieter |
| Echo delay jumps mid-call (60 → 120, 250 or 400 ms) | 1–16 dB for the first 1–1.5 s, 40–49 dB from 2 s on |
| Steady noise removed (−40 and −30 dBFS under speech at −18) | 15 dB, speech within 0.5 dB |
| Everything on, as on a mic track | echo 25 dB down, voice 0.4 dB off |

The echo paths are 40, 80 and 120 ms, with the echo a third as loud as the local voice or as
loud. The tests fail at 20 dB (first 2 s), 30 dB (after) and 10 dB (noise), well under these
numbers. With the modules off they measure 0.3 dB and 3.1 dB.

- **The canceller finds the delay by itself, but it takes a moment.** Switching to speakers with
  a different delay mid-call lets the other side hear itself for a second or two. Overdriven
  laptop speakers (soft clipping) cost it 2–4 dB, nothing more.

`bench/audio-echo.sh` checks the same on a MacBook's own speakers and mic: it compares the raw
mic with the track the other side hears back, with speech as the far-end voice. Two runs at half
volume in a quiet room (−51 and −53 dBFS in the pauses):

| | Run 1 | Run 2 |
|---|---|---|
| The far end's speech, at the mic | −14.4 dBFS | −15.0 dBFS |
| What it heard back, after the first 5 s | −66.9 dBFS | −60.7 dBFS |
| Echo removed | 52.5 dB | 45.7 dB |
| Echo removed, first 5 s | 20.0 dB | 18.9 dB |
| A local voice alone, as sent on | 4.4 dB louder | 4.2 dB louder |
| The same voice while both talk, loudest parts | 2.9 dB quieter | 1.7 dB quieter |
| The same voice while both talk, median 10 ms | −42 dBFS (alone: −17) | −29 dBFS (alone: −17) |

Levels are the 95th percentile of 10 ms slices unless they say otherwise. The check fails under
30 dB of echo removed, or when the local voice alone comes back more than 10 dB quieter.

- **The first words of a call come back.** For the first 2–3 s that the far end talks it hears
  itself at about −30 to −35 dBFS, while the canceller learns the room. After that the echo is
  under −80 dBFS most of the time.
- **Talking over each other clips words.** The loud parts of the local voice get through 2–3 dB
  down, but 35–49% of its 10 ms slices came back under −40 dBFS, against 9–11% when it spoke
  alone (the gaps between words). The local voice here came from the same speakers and reached
  the mic as loud as the echo; a person at the keyboard may fare differently.
- **Speech cancels far better than test tones.** At full volume, with speech as the far-end
  voice, nothing above −45 dBFS came back in 20 s while the raw mic heard it at −18 dBFS:
  42–60 dB removed. Marker bursts (110 ms of tones once a second) never came back whole, but in
  a quiet room their first 30–60 ms sometimes did, 6–36 dB down, and in the best run every burst
  was at least 37 dB down after the first 5 s. A canceller adapts while the far end makes sound,
  and sparse tones give it little to adapt on, so the check uses speech.
- **A voice in the room changes the result.** While someone talks near the Mac the canceller
  holds back so it doesn't cut their voice, and the bursts came back only 0–15 dB down. Mixing
  that room recording into a quiet run reproduces it offline (4–18 dB instead of 22–37 dB), so it
  is the canceller's double-talk behaviour, not the device path: the same recordings replayed
  through the offline module match what the device track did. The check listens to the raw mic
  in the far end's pauses, and calls a failed run in a room that wasn't quiet inconclusive.
- **Gain control hides part of the cancellation.** It starts about 14 dB up and applies to
  whatever is left, so leftover echo and room noise both come back louder than the canceller left
  them. Compare levels with it in mind.
- **The echo reaches the mic about 100 ms after the canceller sees the sound** (device buffers,
  air, mic), well inside what it handles.

### Mute and devices

`bench/audio-acceptance.sh` checks mute both ways, headless. The sharer mutes its synthetic voice
for 5 s of a 20 s call:

| | Result |
|---|---|
| Time the viewer saw the sharer as muted | 5.0 s |
| Markers heard while muted | 0 |
| Markers heard before and after | 28 and 33 |
| The screen, capture to decode p50, voice off | 11.9 ms |
| The same with voice on | 12.5 ms |

The other way round, the viewer mutes its real mic and the sharer is told of the mute and of the
unmute.

- **A muted voice would read as a broken link.** A muted track sends nothing, and WebRTC counts
  the silence the jitter buffer fills in as concealed: 100% for each muted second, and 21% for
  that 20 s call. Concealment therefore leaves out the seconds in which the other side was
  muted. With that, the same call reports 0.0%.
- **Watching for device changes costs about 5 ms every 2 s**, off the media path: listing the
  mics or the speakers takes 1.5–3.8 ms each on a MacBook.
- **Switching devices mid-call doesn't drop the voice.** With the named mic and speakers taken
  out of the list the call sees, as if unplugged, it moved both to the system default and the mic
  kept sending: 59 kbit/s in the second of the switch, 63–68 kbit/s in the others.
- **Picking speakers by name used to fail when the other side was already talking**: playout had
  started, and WebRTC refuses to set a device that is running. Selection now uses the SDK's live
  switch, which stops, changes and restarts it.

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

### Voice in the lab

The synthetic voice runs alongside the screen in every profile (32 kbit/s Opus with RED). One run
each, 60 s, 243 bursts sent:

| Profile | One way p50 | p95 | Behind the screen, p50 | Concealed | Jitter buffer | Heard | Target |
|---|---|---|---|---|---|---|---|
| lan | 186 ms | 621 ms | 170 ms | 1.34% | 228 ms | 237 | ❌ |
| wifi-good | 111 ms | 131 ms | 88 ms | 0.11% | 84 ms | 239 | ❌ |
| wifi-bad | 164 ms | 289 ms | 113 ms | 0.46% | 128 ms | 240 | ❌ |
| 4g | 114 ms | 116 ms | 69 ms | 0% | 68 ms | 241 | ✅ |
| hotel | 212 ms | 329 ms | 145 ms | 0.69% | 155 ms | 227 | ❌ |
| intercontinental | 190 ms | 209 ms | 100 ms | 0.13% | 85 ms | 238 | ✅ |
| sharer-uplink-3mbit | 150 ms | 165 ms | 77 ms | 0.19% | 106 ms | 242 | ❌ |
| bandwidth-drop | 135 ms | 223 ms | 75 ms | 0.2% | 102 ms | 243 | |
| udp-blocked | 75 ms | 173 ms | 69 ms | 0.32% | 66 ms | 241 | |
| outage-5s | 93 ms | 134 ms | 76 ms | 9.45% | 78 ms | 217 | |
| stall-0.6s | 77 ms | 149 ms | 68 ms | 3.29% | 65 ms | 234 | |
| stall-1.2s | 76 ms | 96 ms | 59 ms | 6.09% | 52 ms | 228 | |
| stall-2.5s | 66 ms | 81 ms | 49 ms | 12.53% | 44 ms | 207 | |

The jitter buffer is the average over the run. Targets are in
[ARCHITECTURE.md](ARCHITECTURE.md#network-lab-and-slos), and the dynamic profiles have none.

On the two lossy profiles, with each encoding:

| | Opus | On the wire | wifi-bad one way p50 | Concealed | hotel one way p50 | Concealed | Heard |
|---|---|---|---|---|---|---|---|
| RED (default) | 32 kbit/s | 67 kbit/s | 164 ms | 0.46% | 212 ms | 0.69% | 227 |
| RED | 24 kbit/s | 51 kbit/s | 87 ms | 0.06% | 207 ms | 0.33% | 235 |
| no RED | 32 kbit/s | 32 kbit/s | 80 ms | 0.12% | 174 ms | 3.81% | 190 |
| no RED | 24 kbit/s | 24 kbit/s | 104 ms | 0.05% | 209 ms | 2.69% | 205 |

"Heard" is on hotel, out of 243.

- **RED stays on.** At hotel's 2% loss it keeps concealment under 1%, against 2.7–3.8% without,
  and loses far fewer bursts. It costs 35 kbit/s, which is little next to the screen's 2–5 Mbit/s in the lab.
  At wifi-bad's 1% loss nothing needed it.
- **The bitrate stays at 32 kbit/s.** 24 kbit/s saved 16 kbit/s on the wire and made no difference
  we could measure to delay or concealment. Tones can't judge how speech sounds, though, so we
  keep the margin until real voices are tested.
- **The jitter buffer holds on to a delay spike.** When the lab stalls, late packets arrive in a
  burst and the receiver raises its buffer to cover them. On `lan` it reached 667 ms, took about
  10 s to come back to 160 ms, and was still at 90 ms 40 s later. When packets are lost instead of late, as
  in the stall and outage profiles, it stays at 44–78 ms. On real Wi-Fi, where delay spikes are
  common, this would leave voice lagging long after the spike. It's the first thing to tune.
- **Concealment counts outages exactly.** The 2.5 s stalls cut 7.5 s out of 60 s, or 12.5%, which
  is what was concealed.
- **The lab misses the voice latency targets on clean links**, and the lab is the cause. The
  Docker VM stalls for a second now and then. The screen drops to 10–17 fps in those same
  seconds, and dips like that with voice off too. Quieter runs of `lan` got 78–125 ms one way. On
  the Mac, without Docker, it's 45–54 ms. Use the lab for concealment and for comparing settings,
  and the Mac for absolute delay.
- **The screen is the same with voice off,** within the lab's run-to-run noise. `lan` ran at 56 fps
  and 20 ms p50 without voice, and 53 fps and 21 ms with it. The A/B on the Mac (Voice, above) is
  the cleaner answer.

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
- **The latency marker hides the app menus.** While a viewer sends ticks, the sharer paints a
  384x64 px grid of black and white blocks into the top-left corner of every frame
  (`crates/media/src/marker.rs`). On a Mac that covers the front app's name and first menus, so a
  real-screen snapshot looks like it lost part of the menu bar. That is the marker, not capture
  or the codec: the sharer's own snapshot already has it, and ScreenCaptureKit delivers the menu
  bar intact. `share --no-markers` turns it off, along with RTT and snapshots.
- **A test source needs a sound card's priority.** The synthetic voice first ran on an ordinary
  thread, and macOS sometimes woke it up to 98 ms late under normal desktop load, even at the
  user-interactive QoS class. The receiver took that for network jitter and grew its buffer to
  130 ms. The thread now asks for the real-time policy Core Audio gives a sound card's thread,
  and hasn't been late since. The lab's Linux containers needed the same: at normal priority,
  their frames left up to 70 ms late.
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
DURATION=60 bench/run-matrix.sh av-off av-on av-mic   # the screen without voice, with it, with the mic
```

Voice through the speakers into a mic plays tones out loud for 30 s, so turn the volume up and
keep the room quiet:

```bash
bench/audio-loopback.sh
```

Remote control end to end, with nothing injected for real:

```bash
bench/control-e2e.sh                # every script in bench/control/
ANSWER=n bench/control-e2e.sh       # the sharer says no: nothing may be injected
REVOKE=r bench/control-e2e.sh       # the sharer takes control back mid-typing
REVOKE=hotkey bench/control-e2e.sh  # the same with ⌃⌥⌘R, pressed for real (unlocked screen)
```

The network lab needs Docker, and runs its own LiveKit server:

```bash
VIEW_FLAGS=--measure-input bench/netlab/run.sh --build
bench/netlab/report.sh
```

The voice runs in every profile. To compare its encodings, or the screen without it:

```bash
TAG=24k-nored SHARE_FLAGS="--voice-kbps 24 --no-red" bench/netlab/run.sh wifi-bad hotel
AUDIO=off bench/netlab/run.sh lan wifi-bad hotel
```

Mute both ways, and the screen's latency with voice off and on, with no sound and nobody at the
Mac (the other steps play sound out loud or press the mute hotkey for real):

```bash
STEPS="3 4" bench/audio-acceptance.sh
```

Results land in `bench/results/`. Two machines take a few more steps: see [Try it](../README.md#try-it),
then run the viewer with `--headless --measure-input`.

## Not measured yet

- True glass-to-glass latency, including the app's redraw
- Mac to Mac, and either end on Ethernet
- With AWDL off, or the router on channel 44 or 149
- Text quality on real anti-aliased screens over a real network
- Voice mouth to ear between two machines, and through headphones
- A device really unplugged mid-call (headphones, AirPods), on macOS and on Windows
- Linux

If you can run any of these, we'd love your numbers. See
[Contributing](../CONTRIBUTING.md#measurements-count-as-contributions).
