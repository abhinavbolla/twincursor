# TwinCursor

Open-source (AGPL-3.0) remote pair-programming app, an alternative to Tuple: low-latency
screen sharing, voice, and full remote control of the other person's machine.

Read before larger work:
- `docs/PRODUCT.md`: what we're building and for whom
- `docs/ARCHITECTURE.md`: how the pieces fit together, and why
- `docs/ROADMAP.md`: milestones; work on the current one only
- `docs/performance.md`: current numbers and how to reproduce them
- `docs/calls.md` and `docs/accounts.md`: calls with accounts, and the backend they use
  (`supabase/README.md`); `docs/self-hosting.md` sets one up
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

## Repo layout

```
apps/desktop/        Tauri 2 menu bar app (Rust in src-tauri/, TypeScript/React UI in src/),
                     packaging scripts in scripts/
crates/media/        screen capture, encoding, LiveKit/WebRTC session, voice
crates/control/      remote input: consent rules, injection (enigo)
crates/engine/       what owns the main thread: native viewer, sharer hotkeys and overlay,
                     the control threads on both sides
crates/engine-cli/   tc-engine, the engine process the app starts (JSON lines over stdin/stdout)
crates/ipc/          the commands and events between the app and tc-engine
crates/media-spike/  tc-spike, the development CLI over engine and media
crates/protocol/     shared message types between the two ends of a call (serde), versioned
crates/account/      accounts: GitHub sign-in via Supabase Auth, session, backend calls; tc-account
supabase/            the backend: migration, call-pass Edge Function, tests
bench/               latency, quality, control and end-to-end scripts; netlab/ (Docker)
deploy/dev/          docker compose for a local LiveKit dev server
packaging/windows/   the Windows app zip (package-app.sh) and the terminal viewer zip
docs/                product, architecture, roadmap, research, ADRs
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
- Media spike: `target/release/tc-spike share [--resolution 720|1080|1440|2160|native]
  [--codec h264|h265|vp8|vp9|av1]` and `target/release/tc-spike view` (see `--help`; `S` toggles
  the stats HUD). H.264 is the default; `h265` (HEVC) is opt-in, see `docs/adr/0008-hevc-for-4k.md`.
  `--capture pattern` pretends to be a 3840x2160 display, so every tier up to 2160 is 16:9
- One window or app: `tc-spike windows [--json]` lists shareable windows (id, pixels, app, title);
  `tc-spike share --window <id or text> [--resolution native]` sends that window at its own pixel
  size, `--app <name>` the display with only that app's windows. Not with `--allow-control`.
  A whole-display share leaves out every TwinCursor window, so share and view work on one Mac.
  To judge sharpness on one Mac: `tc-spike share --window <text> --resolution native --no-adapt
  --audio off` and `tc-spike view --audio off` (the HUD says "shown 1:1" or "scaled to N%")
- Quality without windows or screen capture: `tc-spike share --capture pattern` +
  `tc-spike view --headless --measure-quality` (SSIM/PSNR vs the synthetic pattern)
- Sizing to the viewer: `--resolution` is a ceiling; each viewer reports its viewport and the
  sharer sends no more (`share --ignore-viewport` sends the tier's size anyway, for A/B runs).
  Headless: `tc-spike view --headless --viewport 1920x1080 [--viewport-at SECS:WxH]
  [--identity NAME]` (two viewers in one room need different identities). End to end, no
  windows: `bench/viewport-e2e.sh` (sizes for one and two viewers, a change mid-call, SSIM)
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
- Engine for the desktop app: `target/release/tc-engine share|view --ipc --room R --name N`
  (JSON lines on stdin and stdout, logs on stderr, exits when stdin closes; see `docs/ipc.md`),
  `tc-engine permissions`, `tc-engine devices`, `tc-engine windows`, `tc-engine config` (the
  server it would use and whether a call pass is set, never the secret or the pass). Build:
  `cargo build --release -p twincursor-engine-cli`.
  Server: flag > `TWINCURSOR_SERVER_URL`/`_API_KEY`/`_API_SECRET` > baked in at build time from
  `TWINCURSOR_BUILD_SERVER_URL`/`_API_KEY`/`_API_SECRET` (`.env.testbuild.example`) > local dev
  server
- IPC end to end: `bench/engine-e2e.sh` (a scripted app drives a sharer and a headless guest:
  request, allow, typing recorded not injected, revoke, leave, and the engine exiting when its
  stdin closes; pattern capture and `--audio off`, so no permissions or unlocked screen needed)
- Call passes: `TWINCURSOR_PASS=$(target/release/tc-engine dev-pass --room R --role sharer
  --name Alex) tc-engine share --ipc ...` joins with a pass instead of a key and secret (no
  `--room`/`--name`); `bench/engine-e2e.sh` runs a call both ways
- Accounts end to end (local Supabase + LiveKit, no GitHub): `bench/account-e2e.sh` (needs
  `supabase start`); by hand: `tc-account --session FILE sign-in|whoami|start-call|knock|pass …`
  with `TWINCURSOR_SUPABASE_URL`/`_KEY` set (docs/accounts.md)
- Calls with accounts through the real app: `bench/app-accounts-e2e.sh` (needs `supabase start`;
  starts `livekit-server --dev` if none runs; builds: `npm run build` in apps/desktop, then
  `cargo build --release -p twincursor-desktop -p twincursor-engine-cli -p twincursor-media-spike
  -p twincursor-account`). The app runs in script mode with `TWINCURSOR_SESSION_FILE`
- One permission at a time: `tc-engine permissions --request screen_recording|accessibility|microphone`
  (plain `--request` asks for everything missing)
- Releases: `docs/RELEASING.md`; a `v*` tag builds archives and a draft release
- Desktop app (`apps/desktop`, Node 20.19+): `npm ci`, then `npm run build` (UI into `dist`,
  which the app crate embeds: build it before `cargo clippy/test --workspace`), `npm run
  typecheck`, `npm test`. Run it: `cargo build --release -p twincursor-desktop -p
  twincursor-engine-cli`, then `target/release/twincursor-desktop` (finds `tc-engine` next to
  it). Bundle: `apps/desktop/scripts/bundle.sh` → `target/release/bundle/macos/TwinCursor.app`
  (ad-hoc signed, `tc-engine` inside)
- Packaging (`docs/RELEASING.md`): `apps/desktop/scripts/package.sh [BACKEND_FILE]` →
  `target/release/packages/TwinCursor-<version>-macos-arm64.dmg`. The backend file holds the
  Supabase project URL and public key (`apps/desktop/backend.example.conf`; keep the real one
  outside the repo, e.g. `~/.config/twincursor/backend.conf`); without it the app has no backend.
  It clears every LiveKit variable from the build and fails if the bundled `tc-engine config`
  reports a key source other than `dev`. `--legacy-keys testbuild.env` bakes a LiveKit key and
  secret into `tc-engine` instead (no backend; only for the Windows terminal viewer's test server;
  rerun without it afterwards). Windows terminal viewer zip: built by `release.yml`, then
  `packaging/windows/assemble.sh VIEWER.zip testbuild.env` adds `server.txt`. Testers read
  `docs/testing-guide.md`
- Windows app (a guest: joins, watches, talks, takes control; never shares): on Windows in Git
  Bash, `packaging/windows/package-app.sh` → `dist/TwinCursor-<version>-windows-x64.zip`
  (backend from `TWINCURSOR_BUILD_SUPABASE_URL`/`_KEY`, else none). By hand: `npm ci && npm run
  build` in `apps/desktop`, then `cargo build --release -p twincursor-engine-cli` and `cargo build
  --release -p twincursor-desktop --features tauri/custom-protocol` (without the feature the page
  comes from a dev server and the windows are blank); set `CARGO_PROFILE_RELEASE_DEBUG=0` there.
  Its log: `%LOCALAPPDATA%\dev.twincursor.desktop\logs\twincursor.log` (`TWINCURSOR_LOG_FILE`
  anywhere). Platform code: `src-tauri/src/platform.rs` and `native.rs`. Dev page: add
  `&platform=windows`. Cross-check from the Mac is possible with `cargo check --target
  x86_64-pc-windows-msvc` only with stand-in C headers and tools; CI is the real check
- Windows smoke test: `bench/windows-smoke.sh` (CI's `windows-app` job; `LIVEKIT=…exe` adds a
  guest joining an empty room)
- Desktop dev in a browser, no engine: `npm run dev` in `apps/desktop`, then
  `http://localhost:5173/?view=panel&state=home` or `?view=bar&state=knock` (every state is
  listed in `docs/ui.md`; `?view=gallery&of=bar` shows them all; add `&theme=light|dark`,
  `&window=1`, `&frame=desktop`, `&menu=1`). `?view=panel` or `?view=bar` alone runs the live
  mock engine. Development builds only; the bundled app has no mock
- Desktop app against a backend: build the engine once (`cargo build --release -p
  twincursor-engine-cli`), then in `apps/desktop`
  `TWINCURSOR_SUPABASE_URL=… TWINCURSOR_SUPABASE_KEY=… npm run tauri dev` (a debug build of the
  app with the Vite dev server; it finds `target/release/tc-engine`). For the local stack:
  `http://127.0.0.1:54421` and the anon key from `supabase status`
- App end to end: `bench/app-e2e.sh` (the real app in its test-only `--script`/`--report` mode
  shares the pattern to a headless guest: deny, allow, recorded typing, revoke, leave; then a 4K
  HEVC share switching to H.264 for a guest without HEVC, and the engine dying with the app.
  Needs `livekit-server --dev`; no permissions, sound or unlocked screen)
- CI: `.github/workflows/ci.yml` runs fmt, clippy and tests on `macos-26` for every PR and
  push to `main`, plus `windows-viewer` (the engine builds on Windows) and `windows-app` (the
  Windows app: UI tests, the zip, Windows-applicable Rust tests, the smoke test; the zip is an
  artifact). Keep it green; run the same three commands locally before pushing.
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
