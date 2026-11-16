# ADR 0008: HEVC as an opt-in codec for 4K

Status: proposed (2026-11-17)

## Context
Pairing on a 4K or 5K display at 60 fps needs a tier above 1440p. A 3840x2160 frame has 2.25x the pixels of 1440p and 4x those of 1080p. H.264 through VideoToolbox
is the default codec ([ADR 0002](0002-default-video-codec.md)) and runs in hardware on every
supported Mac, but it spends more bits than HEVC for the same detail, and fine text at 4K is
mostly fine detail.

`livekit` 0.9.7 can publish HEVC (`VideoCodec::H265`), and on macOS libwebrtc encodes and decodes
it with VideoToolbox, in hardware like H.264.

Hopp reported that H.265 broke screen sharing on M1 Macs and that they went back to H.264
(`docs/research/hopp-lessons.md`). We don't know the cause, and we haven't tested on an M1.

Not every viewer can decode HEVC. Browsers and Windows machines without the HEVC extension may
not.

## Decision
- **The sharer offers a 2160-line tier** (`share --resolution 2160`). Like the other tiers it
  follows the display's aspect ratio, never upscales, and steps down to 1440p when the call can't
  sustain it.
- **HEVC is opt-in** (`share --codec h265`). **H.264 stays the default** at every resolution,
  including 2160.
- The auto-throttle treats HEVC like H.264: a hardware codec with the same bitrate floors.
- The benchmark matrix runs 4K and HEVC rows next to the H.264 ones (`q-h264-2160`,
  `q-h265-2160`, `q-h265-1440`, `sck-h264-native`, `sck-h265-native`), so each change to the
  media path shows its effect on both codecs.

## Consequences
- Anyone can compare H.264 and HEVC at 4K on their own Mac with one flag, and the results go to
  the same files as the other benchmarks.
- **Known risk:** HEVC may fail on M1 Macs, as Hopp saw. HEVC can't become the default without
  runs on M1 and on other Apple silicon generations.
- **Viewers that can't decode HEVC see nothing.** The app has to fall back to H.264 for them,
  through codec negotiation or a backup track. That fallback is future work in the app, and it is
  required before HEVC can be offered outside development builds.
- HEVC is patent-encumbered. We use the operating system's encoder and ship no HEVC code, but
  distributions that bundle their own codecs need to check their position.
