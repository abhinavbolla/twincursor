# Threat model

TwinCursor shows your screen to someone else and can let them use your computer. That's the
product, so it needs to be clear what protects you, and what doesn't.

This describes the design for remote control and the first release. Remote control isn't finished
yet. Where something isn't built, it says so.

## What we're protecting

- **Control of the sharer's computer.** Only a person the sharer approved may inject input, and
  only until the sharer revokes it.
- **What's on the sharer's screen.** Only people in the call should see it.
- **What the guest types.** While in control, the guest's keystrokes go to the sharer's machine.
  They may include passwords.
- **Voice** (when it exists), and accounts and room access on the server (when it exists).

## Who we consider

| Who | What they might try | Our position |
|---|---|---|
| Someone outside the call | Join, watch or inject input | Must be stopped |
| Someone on your network | Read or change traffic | Must be stopped, using TLS in real deployments |
| A guest you granted control | Anything you can do on that computer | Allowed by design, so consent must be obvious and revocable |
| A guest you didn't grant control | Inject input anyway | Must be stopped |
| A buggy or stalled network | Leave a key or button stuck down | Must be handled |
| Whoever runs the server | See video and keystrokes | **Not protected today**: see below |
| Malware on either computer | Anything | Out of scope |

## How control works

These rules are a [hard rule of the project](../CONTRIBUTING.md#ground-rules), and changes that
weaken them won't be merged.

- **Control starts off.** The sharer's app doesn't inject anything until the sharer accepts a
  request from a named person. Sharing a screen doesn't imply control.
- **The sharer can always see it.** A label stays on screen for as long as control lasts, naming
  the guest and showing the revoke hotkey.
- **Revoking is instant and doesn't depend on the network.** A global hotkey flips a flag that the
  injection thread checks before every event, so a stuck network task can't delay it.
- **Input is checked on the sharer's machine.** Each event must come from the granted participant,
  during a grant, with a known version and values in range. The viewer isn't trusted.
- **Nothing stays held down.** Every held key and button is released on revoke, on disconnect, when
  the guest's window loses focus, and after 1 s without a heartbeat.

The session logic lives in `crates/control` with no network or platform code, so these rules can
be tested with scripted traces. *Status: being built.*

## Known limits

- **The server can see everything that passes through it.** WebRTC encrypts traffic between each
  person and the LiveKit server, but the server decrypts it to forward it. So whoever runs the
  server can read video, audio and keystrokes. Self-hosting means that's you. End-to-end encryption
  is on the [roadmap](ROADMAP.md) under "Later".
- **A guest in control can do anything you can.** That includes opening a terminal. Only grant
  control to people you'd hand your unlocked laptop to, and stay at the computer.
- **Screen sharing shows what's on your screen,** including notifications. Excluding windows or
  hiding notifications isn't built.
- **macOS can block input on some protected prompts** and possibly secure text fields. We'll
  document what we find.
- **Guests shouldn't type passwords during control.** They go to the sharer's machine, and through
  the server, like every other keystroke.
- **`tc-spike` isn't safe on untrusted networks.** It uses LiveKit's well-known development keys and
  an unencrypted `ws://` connection, so anyone who can reach the server can join. Real deployments
  need the server's own keys and TLS, which come with the self-hosting work.

## macOS permissions

TwinCursor needs Screen Recording to capture and Accessibility to inject input. macOS grants these
to the app you launch it from, which today is your terminal. So everything else running from that
terminal gets them too. The desktop app will hold these permissions itself.

## Supply chain

- `Cargo.lock` is committed, and CI builds with `--locked`.
- `cargo deny` checks licenses and only allows crates from crates.io. Advisories are reported in CI.
- Dependabot proposes updates, and people review them.
- The WebRTC library is a prebuilt binary from LiveKit's GitHub releases.
- Release builds aren't signed yet. Signing and notarization are required before the first public
  release.

## Reporting a problem

See [SECURITY.md](../SECURITY.md). Please report privately.
