# TwinCursor

Open-source (AGPL-3.0) remote pair-programming app, an alternative to Tuple: low-latency
screen sharing, voice, and full remote control of the other person's machine.

Read before larger work:
- `docs/PRODUCT.md`: what we're building and for whom
- `docs/ARCHITECTURE.md`: how the pieces fit together, and why
- `docs/ROADMAP.md`: milestones; work on the current one only
- `docs/performance.md`: current numbers and how to reproduce them
- `docs/research/hopp-lessons.md`: what we learned from Hopp's public engineering blog
- `docs/research/rustdesk-lessons.md`: what RustDesk's design teaches us, and what not to take

## Hard rules

1. **Clean room: never copy code from Hopp (github.com/gethopp/hopp) or Tuple.** Do not open,
   fetch, paste, or paraphrase their source files. Learning from their *public blog posts* and
   docs is fine; implement ideas in our own code. If a task seems to need their code, stop and ask.
   RustDesk (AGPL-3.0, like us) is different: its source and docs may be read to learn from, but
   write our own code. Copying a file needs asking first, its notices kept, and a third-party
   notice entry; never copy from its unlicensed repositories (`hbb_common`, `hwcodec`).
2. **Media never runs in the webview.** Capture, encode, WebRTC, decode and rendering of the
   shared screen live in Rust (`crates/`). The Tauri webview is only for app UI (lobby, settings,
   call controls). This is the main lesson from Hopp; see the research note.
3. **Latency is the product.** Any change to the media path must report its effect on
   glass-to-glass latency (see "Measuring latency" in ARCHITECTURE.md). No unmeasured "optimizations".
4. **Remote control is consent-based.** The sharer must explicitly grant control, see who is
   controlling, and be able to revoke it instantly with a global hotkey. Never weaken this.
5. New dependencies need a one-line justification in the PR description. Licenses must be
   AGPL-compatible (MIT, Apache-2.0, BSD, ISC, Zlib, Unicode-3.0, MPL-2.0, LGPL, GPL-3.0, AGPL-3.0).
   `cargo deny` enforces this in CI (`deny.toml`).

## Repo layout (planned; create as milestones need them)

```
apps/desktop/     Tauri 2 shell (Rust + TypeScript/React UI)
crates/media/     screen capture, encoding, LiveKit/WebRTC session, native viewer
crates/control/   remote input: event protocol, injection (enigo), cursor overlay
crates/protocol/  shared message types (serde), versioned
server/           Rust (axum) API: auth, rooms, LiveKit token issuing
deploy/           docker compose for self-hosting (server + LiveKit + Postgres)
docs/             product, architecture, roadmap, research, ADRs
```

## Commands

Fill these in as code lands; keep them accurate.

- Build all Rust: `cargo build --release --workspace` (release is the default profile we use;
  libwebrtc is a ~277 MB prebuilt download on the first build of each profile)
- Test all Rust: `cargo test --release --workspace`
- Lint: `cargo clippy --release --workspace --all-targets -- -D warnings`
- Format: `cargo fmt --all`
- Local LiveKit: `livekit-server --dev` (`brew install livekit`), or
  `docker compose -f deploy/dev/docker-compose.yml up`
- Media spike: `target/release/tc-spike share [--resolution 720|1080|1440|native] [--codec vp8]`
  and `target/release/tc-spike view` (see `--help`; `S` toggles the stats HUD)
- Quality without windows or screen capture: `tc-spike share --capture pattern` +
  `tc-spike view --headless --measure-quality` (SSIM/PSNR vs the synthetic pattern)
- Input path: `tc-spike view --headless --measure-input` (reliable vs lossy data channels)
- Voice latency without a mic: `tc-spike share --audio pattern` + `tc-spike view --headless
  --measure-audio` (marker tones; `bench/run-matrix.sh av-off av-on` compares the video with and
  without voice)
- Voice: `share` and `view` send the mic and play the other side by default (`--audio off` to
  skip; bench scripts pass it). `tc-spike devices` lists devices for `--mic`/`--speaker`
- Mute: `M` in the viewer (F8 while in control), ⌃⌥⌘M on the sharer from any app.
  `--mute-for FROM:TO` mutes on a schedule, for scripts
- Voice checks in one go: `bench/audio-acceptance.sh` (`STEPS="3 4"` for the two that make no
  sound and need nobody at the Mac: mute both ways, and the screen's latency with voice off and
  on; 1 and 2 play sound out loud, 5 presses ⌃⌥⌘M for real on an unlocked screen)
- Voice through real speakers and mic, one Mac: `bench/audio-loopback.sh` (plays tones out loud
  for ~30 s: volume up, quiet room; mic access for the terminal)
- Echo cancellation through real speakers and mic, one Mac: `bench/audio-echo.sh` (talks out
  loud for about a minute, same needs; exit 2 means the room wasn't quiet enough to judge).
  `share --audio file --audio-file speech.wav` sends a WAV as the sharer's voice. Both out-loud
  scripts refuse to run while another app is using the mic (a call); `FORCE=1` overrides
- Remote control end to end: `bench/control-e2e.sh` (headless scripted guest, sharer records what
  it would inject, `tc-spike replay` checks it, and the "is controlling" label must be on screen
  during control and gone after); `ANSWER=n` checks a refusal; `REVOKE=r` takes
  control back mid-typing; `REVOKE=hotkey` presses ⌃⌥⌘R for real (unlocked screen only: macOS
  doesn't deliver synthetic key presses to apps while the screen is locked)
- Remote control by hand: `tc-spike share --allow-control` and `tc-spike view`; F9 in the viewer
  asks for control and gives it back, F10 toggles the HUD while in control. The sharer injects
  real input by default (`--inject log` to only log); its terminal needs Accessibility
- Real injection, read back: `bench/control-inject.sh` (screen unlocked, hands off for ~15 s; a
  full-screen `tc-spike probe` window records what arrives). Nothing is injected while the screen
  is locked
- Remote-control checks that need someone at the Mac, in one go: `bench/control-acceptance.sh` (`STEPS="2 3"` for some)
  (hotkey, injection readback, overlay screenshot vs stream frame, capture latency)
- Remote control under bad networks: `bench/netlab/control.sh [profile ...]` (Docker; the
  sharer runs `share --allow-control --headless --inject record:…`)
- Latency matrix: `bench/run-matrix.sh [config ...]` → `bench/results/*.json` (git-ignored)
- Network lab (Docker): `bench/netlab/run.sh [--build] [profile ...]`, then
  `bench/netlab/report.sh` (profiles and SLOs in ARCHITECTURE.md). Synthetic voice runs
  alongside unless `AUDIO=off`; `SHARE_FLAGS="--voice-kbps 24 --no-red"` changes its encoding
- Releases: `docs/RELEASING.md`; a `v*` tag builds archives and a draft release
- Desktop dev: _TBD_
- CI: `.github/workflows/ci.yml` runs fmt, clippy and tests on `macos-26` for every PR and
  push to `main`. Keep it green; run the same three commands locally before pushing.
- Building needs **Xcode 26** (macOS 26 SDK): `screencapturekit`'s `apple-metal` Swift bridge
  uses macOS 26 Metal APIs and fails to compile with Xcode 16 (this broke CI on `macos-15`).

If the libwebrtc download fails mid-way (connection reset), download
`https://github.com/livekit/rust-sdks/releases/download/<WEBRTC_TAG>/webrtc-mac-arm64-release.zip`
with `curl -C -`, unzip it anywhere, and point `LK_CUSTOM_WEBRTC` at the unpacked
`mac-arm64-release/` folder (CI does exactly this). The tag is `WEBRTC_TAG` in the
`webrtc-sys-build` crate.

## Conventions

- Rust edition 2021+, `cargo fmt` and clippy-clean. Errors: `thiserror` in libraries, `anyhow` in binaries.
- Prefer small, focused crates with clear boundaries over one large crate.
- TypeScript: strict mode, functional React components, no `any`.
- Commits: Conventional Commits (`feat:`, `fix:`, `perf:`, `docs:`, `chore:`).
- Architecture decisions go in `docs/adr/NNNN-title.md` (short: context, decision, consequences).
- Platform-specific code goes behind `cfg(target_os = ...)` modules with a shared trait.

## Working agreement for Claude

- Before a milestone or any multi-file change: plan first (plan mode), and confirm the plan.
- One feature per session/branch. Keep PRs small enough to review in ~15 minutes.
- Write or update tests alongside code; run build, test, clippy before saying something works.
- When you learn something non-obvious (a platform quirk, a latency finding), add it to the
  relevant doc rather than leaving it only in chat.
- macOS is the first platform. Keep code portable, but don't build Windows or Linux paths before their milestone.
