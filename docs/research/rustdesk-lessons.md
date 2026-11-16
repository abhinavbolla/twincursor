# Lessons from RustDesk

RustDesk is an open-source remote desktop written in Rust: an ID server, a relay, and clients
for every desktop and mobile OS. It isn't a pairing tool, but it has met many problems we're
about to face: macOS permissions, a background service, keyboard layouts, self-hosting,
signing, updates, and abuse.

Read on 2026-11-14: `rustdesk/rustdesk` at `fc9713c` (the 1.5.3 line), `rustdesk/rustdesk-server`
at `680b603`, `rustdesk/hbb_common` at `bb416bc`, and their public docs, blog, wiki, releases
and issues.

RustDesk and its server are AGPL-3.0, like us, so its source may be read to understand a design.
Nothing here is copied; file paths point to where an idea lives. See [Reuse](#reuse) for what
copying would require. The clean-room rule for Hopp and Tuple is unchanged.

## Decisions for TwinCursor

**Desktop app**
- **The engine leaves the call when the shell dies.** RustDesk lets sessions outlive its UI;
  "only accept while the window is open" is an opt-in setting. Ours is the safer default.
- **Keep stdin and stdout between the shell and the engine.** RustDesk's processes talk over
  local sockets, which need a check of who is on the other end on each OS
  (`src/ipc.rs`, `src/ipc/auth.rs`), and 1.5.3 shipped a fix to that check on Windows. A pipe
  only the parent holds has nothing to authenticate.
- **Sign with one stable identity.** macOS ties a permission grant to the code signature.
  RustDesk has a long-running issue about Screen Recording not taking effect after an update
  (#3261). An ad-hoc signed build is a new identity on every rebuild, so release builds need a
  Developer ID that never changes.
- **After a grant, restart the engine, not the app.** A grant usually applies only to a process
  started after it. RustDesk's docs tell users to quit, remove and re-add the entry, or reboot.
  Our engine is a child process, so the shell can start it again.
- **Ask for Screen Recording, Accessibility and Microphone. Not Input Monitoring.** RustDesk
  needs Input Monitoring because it hooks the keyboard globally. Our viewer reads keys from its
  own window.
- **The control prompt names the person and warns about strangers.** A code read out over the
  phone is how RustDesk gets abused, and a room code has the same shape. Never add auto-accept.
- **Keep ScreenCaptureKit and the keyboard model of ADR 0004.** RustDesk's macOS capture uses
  the older `CGDisplayStream`, and its "translate" keyboard mode arrives at nearly the same
  model as ADR 0004.

**Self-hosting**
- **Measure the reliable channel's retransmit floor.** RustDesk patched its SCTP because a
  minimum retransmit timeout of one second made a single lost packet cost seconds (comments in
  its `Cargo.toml`). That may be the tail ADR 0003 measured on our reliable channel.
- **SQLite first, Postgres optional, few ports.** RustDesk's server is two binaries and a SQLite
  file, started from one compose file. That simplicity is much of why people run it.
- **Authorize every relay and token path.** RustDesk's relay accepts anyone unless it is started
  with a key. Keep LiveKit tokens short-lived and per room, and TURN credentials temporary.

**First release**
- **Sign the update metadata separately from the app.** RustDesk's Windows code-signing
  certificate was revoked by its issuer (discussion #13757, December 2025), and an earlier
  Windows installer added a test certificate to the trusted roots (CVE-2024-25140). Its updater
  now checks a signed manifest and the download's hash (`src/update_metadata.rs`). The Tauri
  updater's own signatures give us the same independence. Never install a certificate.
- **Notarize in CI with `rcodesign`.** RustDesk signs with Developer ID and the hardened
  runtime, then notarizes without Xcode's GUI (`res/osx-dist.sh`).

**Your own devices**
- **ADR 0005 stands: no permanent passwords.** A device ID plus a password is what scammers
  reuse. RustDesk keeps adding layers on top: 2FA, allow-lists by IP and ID, sign-in for the
  controlling side on its public server, scam warnings.
- **Never build privacy mode, input blocking or hidden indicators.** RustDesk can black out the
  controlled screen and block local input. Both are what a scam needs, and both break our
  consent rule.
- **End-to-end encryption is a requirement there.** RustDesk peers agree on a session key
  between themselves, so its relay sees only bytes. People who self-host remote access expect
  that.

## What RustDesk is

- **Processes.** One binary started with different flags (`src/core_main.rs`): the controlled
  side, a "connection manager" window that shows who is connected, a tray icon, and an installed
  service. On macOS the installer adds a root LaunchDaemon and a LaunchAgent that also runs at
  the login window, which is how it reaches the login screen.
- **Servers.** `hbbs` registers IDs and brokers hole punching; `hbbr` relays bytes when a direct
  path fails. Accounts, the web console, single sign-on and audit logs are in the closed Server
  Pro.
- **Transport.** Its own protobuf messages, video frames included, over a reliable ordered
  stream: TCP, KCP over UDP, or a WebRTC data channel. It doesn't use WebRTC media tracks.
- **Video.** Software VP8, VP9 (the default) and AV1; hardware H.264 and H.265 through an FFmpeg
  wrapper. The sharer waits for viewers to confirm a frame before sending the next, and adapts
  frame rate and bitrate to measured delay (`src/server/video_qos.rs`). 30 fps is the default.
- **Input.** A fork of `enigo` posting CGEvents on macOS, with three keyboard modes: map
  (physical keys), translate (characters, falling back to keys for shortcuts) and legacy.
- **UI and packaging.** Flutter on every platform. A signed, notarized DMG on macOS, with silent
  auto-update since 1.5.0.

## Where it differs from what we're building

- **Latency under loss.** Video on a reliable, ordered stream means one lost packet holds up
  every frame behind it. A WebRTC media track retransmits only while that still helps, and has
  congestion control and a jitter buffer. RustDesk publishes no latency figures, and the
  third-party numbers we found give no method, so there is nothing to compare ours with
  ([performance.md](../performance.md)) short of measuring it ourselves.
- **Calls.** One controlling and one controlled computer per session. There is audio forwarding
  and chat, but no two-way voice tuned for speakers, and no SFU.
- **Pairing.** No named second cursor and no "either person drives". The controlled side has no
  always-on-top label and no take-back hotkey.
- **What it does better today.** Every OS including Linux and Android, unattended access, file
  transfer, a clipboard that carries files, multiple monitors, and a tiny server.

## Reuse

Ideas, written in our own code, are the default. Nothing below is worth depending on today.

| Candidate | License seen | Verdict |
|---|---|---|
| `libs/scrap` (capture) | MIT | No: its macOS backend is `CGDisplayStream`; we use ScreenCaptureKit |
| `libs/enigo` (input fork) | MIT | No: we use maintained upstream `enigo` |
| `rustdesk-org/rdev` (key tables) | Not read; upstream is MIT | Maybe for a Windows sharer; it is a git dependency, which `cargo deny` refuses |
| `hbb_common`, `rustdesk-org/hwcodec` | None found | No: unlicensed code can't be used |
| `libs/clipboard` (files over the network) | AGPL-3.0 (the repo's) | Study for file drop later, then write our own |
| macOS permission checks | AGPL-3.0 (the repo's) | Write our own: a few calls to public Apple APIs |

Copying a RustDesk file into this repository is allowed by its license but carries duties:
keep its copyright and license notices, mark the file as changed with the date (AGPL section
5a), and list it in a third-party notice. Ask before doing so.

## Sources

Read 2026-11-14.

- Source: [rustdesk](https://github.com/rustdesk/rustdesk),
  [rustdesk-server](https://github.com/rustdesk/rustdesk-server),
  [hbb_common](https://github.com/rustdesk/hbb_common)
- [Self-hosting docs](https://rustdesk.com/docs/en/self-host/) and
  [macOS client docs](https://rustdesk.com/docs/en/client/mac/)
- [Screen Recording permission after an update, issue #3261](https://github.com/rustdesk/rustdesk/issues/3261)
- [RustDesk and remote access scams](https://rustdesk.com/blog/rustdesk-and-remote-access-scams/)
  and [sign-in on the public server](https://github.com/rustdesk/rustdesk/wiki/Login-required-for-public-server)
- [Code-signing certificate revoked, discussion #13757](https://github.com/rustdesk/rustdesk/discussions/13757)
- [CVE-2024-25140](https://github.com/advisories/GHSA-xc8c-3cmg-p9qr)
- [Releases](https://github.com/rustdesk/rustdesk/releases) 1.5.0 to 1.5.3
