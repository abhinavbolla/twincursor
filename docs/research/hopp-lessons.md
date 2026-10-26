# Lessons from Hopp's public engineering blog

Hopp (AGPL-3.0) is the main open-source Tuple alternative. This note summarizes what they
wrote **publicly in blog posts** (read 2026-10-26). We do **not** read or copy their source
code. See the clean-room rule in CLAUDE.md.

## What they backed away from
- **WebRTC inside Tauri's WebKit webview.** Problems: blurry rendering of some effects, audio
  glitches, an outdated user agent breaking third-party detection (Krisp), browser-limited
  codec choice, and the default WebKitGTK build has no WebRTC, which blocks Linux. They chose
  to move WebRTC into the Rust backend, with the call windows built in Rust using `iced`.
  They are still on WebRTC and LiveKit, just not in the webview.
  https://gethopp.app/blog/hate-webkit
- **One LiveKit participant per window** (audio, screen, and camera windows each joined
  separately). They are moving to one participant per person.
  https://gethopp.app/blog/hate-webkit
- **Default libwebrtc settings** gave about 330 ms median latency at 1080p.
  https://gethopp.app/blog/latency-exploration

## Latency techniques that worked (verify before relying on them)
- VP9 rate control: tighter quantizer range (min 4, max 36), undershoot 100%, overshoot 15%.
  This brought latency down to about 296 ms and also saved bandwidth.
- Encoder buffer sizes: total 300 ms, initial 0 ms, optimal 200 ms. This gave a gain of more
  than 100 ms on poor networks.
- Limit alt-ref frames (ARNR max frames 5, strength 5) and enable parallel decoding. Result:
  about 86 ms median at 1080p and 120 ms at 1440p on a good network. (The post uses `VP8E_*`
  names for a VP9 encoder. Check the real libvpx names.)
- libwebrtc's `is_screencast` mode felt better in practice but caps output at 5 fps. They patched that cap.
- Zero playout delay on the receiver.
  https://gethopp.app/blog/latency-exploration

## Codec choice
- macOS: VP8 had the best latency and frame rate. Windows: H.264 (hardware) was best.
  VP9/AV1 give better quality per bit but cost CPU. AV1 has no simulcast in libwebrtc.
- LiveKit's Rust SDK limited H.264 on macOS to profile 42e01f (max 720p) at the time.
- H.265 broke sharing on M1 Macs. They reverted it.
  https://gethopp.app/blog/screensharing-encoders-compared

## Quality tricks
- Render-side sharpening (unsharp mask with halo clamping) improves text readability at no bandwidth cost.
  https://gethopp.app/blog/sharpening-screen-sharing
- Camera: simulcast layers (720p, 360p, 216p) so the SFU can serve slower receivers.
  https://gethopp.app/blog/high-quality-camera-in-meetings

## Measuring
- They stamp a marker into frames after a data-channel tick and detect it on the receiver.
  They simulate networks with Network Link Conditioner (macOS) and clumsy (Windows). We use
  the same approach in M1.

## Platform reality (as of their posts)
- macOS-first. Windows support was paused to "alpha" to focus on macOS. Linux alpha was planned for summer 2026.
- Opportunity for us: first-class Linux, made possible because our media isn't in the webview.

Other posts worth reading later: /blog/tauri-window-state-sync, /blog/rust-app-start-on-login,
/blog/tauri-vs-electron, /blog/15-months-of-building-a-pair-programming-app.
