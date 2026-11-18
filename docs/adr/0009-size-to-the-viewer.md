# ADR 0009: Send the size the viewer shows

Status: proposed (2026-11-18)

## Context
The sharer picked its send size from its own setting alone (`--resolution`, a tier). The viewer
then scaled the picture into its window:
- **Larger window than stream**: a 1664x1080 stream in a maximised window on a 3024x1964 Retina
  screen was drawn at 170%, and text was visibly blurry.
- **Smaller window than stream**: the sharer captured, encoded and sent pixels the viewer
  decoded and threw away, costing CPU on both ends, bandwidth, and latency.

Only the viewer knows how large the picture can be drawn there. Without simulcast the SFU can't
pick a layer per viewer, so the sharer has to choose one size for everyone.

## Decision
- **Viewers report a viewport**: the largest area, in physical pixels, the picture can take on
  their screen. It is a new message in `crates/protocol` (`Message::Viewport`), validated like the
  others, so the protocol version stays 1: a sharer from before it turns it away as an unknown
  message, and a viewer from before it never sends one.
- **The viewport doesn't depend on the picture.** While the viewer's window follows the
  picture's size, it reports the monitor's usable area; once the guest sizes the window by hand,
  the window's size. The sharer's answer can't change what the viewer reports, so there is no
  feedback loop.
- **The sharer sends the source fitted into the largest viewer's viewport**, rounded as the
  tiers are and never upscaled, then limited by the tier and the auto-throttle. The tier keeps
  meaning "the most this sharer sends", and the default (1080p, H.264) is unchanged.
- **The media loop reads viewports itself**, on the control topic, rather than through the
  remote-control session: sizing matters in every call, and the control session only runs when
  the sharer allows control.
- **Conservative with mixed calls**: a viewer that hasn't reported within 2 s of joining gets
  the tier's size, as before. The size follows a new viewport at most once a second.
- **The viewer draws a near miss 1:1.** Rounding leaves the frame a few pixels smaller than the
  window; it is drawn centred with thin bars instead of stretched.

## Consequences
- A smaller viewer costs the sharer less: fewer pixels to encode and send, and the
  auto-throttle's bitrate floors follow the size actually sent.
- A larger viewer on a high tier gets a 1:1 picture instead of an upscaled one, at the cost of
  sending more pixels than before, up to the tier.
- Capture stays at the tier's size and the send loop scales on the CPU (libyuv), as the
  auto-throttle's lower levels already do. Capturing at the send size (ScreenCaptureKit scales
  on the GPU) would save that, and is left for later.
- With several viewers, smaller ones receive more than they show and scale it down, as all
  viewers did before.
- A joining viewer's first data packets are dropped until the sharer's room has heard it
  joined (2 to 3 s on a local server). The viewport goes with each offer of the viewer's hello,
  which repeats every 250 ms until the sharer acknowledges it, and every 2 s after that; a
  joining viewer gets its own size as soon as the sharer can attribute its packets.
