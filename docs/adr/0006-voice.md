# ADR 0006: Voice

Status: accepted (2026-11-12)

## Context
People pairing need to talk, and today they open a second app for it. Voice in TwinCursor has to
work over speakers, without headphones, while one person controls the other's computer, and it
must not slow the screen. Both sides talk, so each one sends a mic and plays the other's.

Three things shaped the design:

- **The SDK we already use has a device layer.** LiveKit's `PlatformAudio` wraps WebRTC's own
  audio devices (CoreAudio on macOS, WASAPI on Windows): it lists and switches devices, plays
  remote voices, and runs WebRTC's echo cancellation, noise suppression and gain control with
  the speaker output as the echo reference.
- **WebRTC can hold video back to keep it in sync with audio.** For a call that's right. For a
  shared screen it would add the voice's delay to every frame.
- **Audio carries no per-frame metadata,** unlike video, where each frame carries its capture
  time. Whatever measures voice latency has to be in the sound.

## Decision
- **Real calls use WebRTC's device layer; we write no device or echo-cancellation code.** It's
  wrapped in `crates/media/src/audio/device.rs`, and nothing else touches the SDK type. No new
  dependency.
- **Voice is its own track and the screen never waits for it.** Measured with voice off and on,
  the screen's latency is the same
  ([results](../performance.md#voice)).
- **Measurement uses synthetic voices that need no device or permission.**
  - Marker tones: when the sharer gets the viewer's latency tick, it plays that tick's id as a
    short chirp and four tones. The viewer decodes them as they play out, which gives the same
    round trip the screen uses, the one-way delay, and how far the voice trails the screen.
    This runs headless and in the network lab.
  - A recording: the sharer can send a WAV file as its voice, so the other side's echo
    cancellation can be checked with speech.
- **Opus at 32 kbit/s, mono, with RED.** Each packet repeats the previous frame, so one lost
  packet costs nothing. The network lab chose it: at 2% loss it keeps made-up audio under 1%,
  against 2.7–3.8% without.
- **Echo cancellation, noise suppression and gain control are always on.** This SDK version
  gives a device track no switch for them. They're checked two ways: offline in CI, driving
  WebRTC's processing module with synthetic speech through a simulated room, and out loud on one
  Mac, comparing the raw mic with what the other side hears back
  ([results](../performance.md#echo-and-noise)).
- **Mute is LiveKit's track mute,** so the other side is told. The viewer uses a key in its
  window; the sharer has no window, so it uses a global hotkey, ⌃⌥⌘M.
- **Devices are picked by name and followed when they change.** A device that disappears
  mid-call falls back to the system default, and the system default is followed when it moves.
  The SDK doesn't report changes, so each side checks every 2 s.

## Consequences
- **Mouth to ear is past the 150 ms comfort limit on built-in speakers, with no network at all:**
  155–159 ms on one MacBook. About 100 ms of it comes after the jitter buffer: WebRTC's playout
  buffer, which the SDK doesn't let us size, and the speakers' and mic's own processing. The
  jitter buffer and the playout buffer are the places to tune; headphones skip the speakers'
  share.
- **The first words of a call come back as echo** for 2–3 s, while the canceller learns the
  room. After that it removes 46–53 dB on a MacBook's own speakers and mic.
- **Talking over each other clips words.** The canceller ducks the local voice while the far end
  talks: a third to a half of its 10 ms slices came back nearly silent in the out-loud check. A
  talk test between two machines is the judge of how that sounds.
- **The server can hear the call.** Voice passes through the LiveKit server, which decrypts it
  to forward it, so whoever runs the server can listen. End-to-end encryption is on the roadmap
  under "Later", and the [threat model](../threat-model.md) says so.
- **Only voices are sent,** not the computer's own sound.
- **Bluetooth headsets drop to their low-quality call profile while the mic is open.** That's the
  operating system's behaviour with any calling app.
- **Linux has synthetic voice only,** in the network lab, until its milestone.
- **The viewer keeps building and running on Windows,** and its voice path there is the same
  code over WASAPI. It has been built in CI but not yet measured.
- A muted track sends nothing, and WebRTC counts the silence the receiver fills in as made-up
  audio. Our concealment number leaves out the time the other side was muted, so mute doesn't
  read as a broken link.
