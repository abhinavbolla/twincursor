# Threat model

TwinCursor shows your screen to someone else and can let them use your computer. That's the
product, so it needs to be clear what protects you, and what doesn't.

This describes the app as it is now. Where something isn't built, it says so.

## What we're protecting

- **Control of the sharer's computer.** Only a person the sharer approved may inject input, and
  only until the sharer revokes it.
- **What's on the sharer's screen.** Only people in the call should see it.
- **What the guest types.** While in control, the guest's keystrokes go to the sharer's machine.
  They may include passwords.
- **What's said.** Only people in the call should hear it, and nothing should be sent while
  you're muted.
- **Who can join a call.** Only people the host let in, or who answered the host's ring, get a
  call pass.

## Who we consider

| Who | What they might try | Our position |
|---|---|---|
| Someone outside the call | Join, watch or inject input | Must be stopped |
| Someone on your network | Read or change traffic | Must be stopped, using TLS in real deployments |
| A guest you granted control | Anything you can do on that computer | Allowed by design, so consent must be obvious and revocable |
| A guest you didn't grant control | Inject input anyway | Must be stopped |
| A buggy or stalled network | Leave a key or button stuck down | Must be handled |
| Whoever runs the LiveKit server | See video, hear voice and read keystrokes | **Not protected today**: see below |
| Whoever runs the backend | See who has an account, who is whose contact, and who called whom and when | Allowed by design: they hold that data ([ADR 0011](adr/0011-accounts-and-call-passes.md#consequences)). They can't see or hear a call, or grant control |
| Malware on either computer | Anything | Out of scope |

## How control works

These rules are a [hard rule of the project](../CONTRIBUTING.md#ground-rules), and changes that
weaken them won't be merged.

- **Control starts off.** The sharer's app doesn't inject anything until the sharer accepts a
  request from a named person. Sharing a screen doesn't imply control. In the desktop app, only
  an explicit allow for that very request grants it ([IPC](ipc.md#consent)), or a trust the
  sharer gave that guest earlier in the call.
- **Trust is the sharer's, for one call.** "Let in with control" and "Give control" let one guest
  take control without a prompt until the sharer revokes it or the guest leaves
  ([ADR 0010](adr/0010-trusted-control.md)). Only the sharer's own app can give it (over the
  engine's stdin); no message from a guest or the server can, and it is never stored. The guest
  still has to ask each time, one controller at a time, and the label shows every grant. Any
  revoke, even with nobody in control, takes every trust back.
- **A request can't be answered by a stray click.** A knock or a control request can appear
  while you're typing or about to click something else. The app's Rust side refuses to let
  someone in until a second after their knock was first on screen, and refuses an Allow until a
  second after that request was ("Too soon. Try again."), whatever the window does. The call bar
  that shows them never takes the keyboard when it appears.
  An Allow names its request and reaches only the engine that asked. Turning away and denying
  are never delayed. "Let in with control" trusts that one identity if they join within a
  minute, a promise kept in the engine, so any revoke (the hotkey included, even before they
  join), the end of the call or a restart of the engine cancels it ([calls](calls.md#arming-why-let-in-and-allow-wait-a-second)).
- **Names come from the token.** The name in "Sam wants control", in "Sam is controlling" and in
  the list of trusted guests is the one in that person's token, not the one their own app sends.
  With a call pass, that is the account's name, signed by the account server. Without one, it is
  what the person typed: the server signs it into their token but nothing checks it, so anyone in
  the call could type any name, and you should grant control because you recognise the person on
  the call, not because of the name. Each participant also has an identity the server vouches
  for, unique in the call, which starts with their role (`sharer-…` or `guest-…`); input is only
  accepted from the identity that was granted, and trust is given to an identity, not a name.
- **The sharer can always see it.** A label stays on screen for as long as control lasts, naming
  the guest and showing the revoke hotkey.
- **Revoking is instant and doesn't depend on the network.** A global hotkey flips a flag that the
  injection thread checks before every event, so a stuck network task can't delay it.
- **Input is checked on the sharer's machine.** Each event must come from the granted participant,
  during a grant, with a known version and values in range. The viewer isn't trusted.
- **Nothing stays held down.** Every held key and button is released on revoke, on disconnect, when
  the guest's window loses focus, and after 1 s without a heartbeat.
- **Input lands on the display the guest sees, and nowhere else.** A share is about one display
  (the main one unless the sharer picks another), and the capture, the injector and the label
  all get that same display. If it goes away mid-call, control is taken back and stays off for
  the rest of the share; input never moves to another display. While one window or app is
  shared, nobody can be given control, since a click on the window's picture would land
  elsewhere on the screen: every request is turned down with "Control isn't available while
  one window is shared."
- **Nothing is injected while the sharer's screen is locked.** Typed keys would land in the lock
  screen's password field, so a guest could try to unlock the computer. Input pauses until the
  sharer unlocks it; releases still go through.

The session logic lives in `crates/control` with no network or platform code, so these rules can
be tested with scripted traces. *Status: the session rules are built and tested, and run over the
network with a consent prompt in the desktop app's window (or `tc-spike`'s terminal), the revoke
hotkey, the on-screen label, and real input on macOS.*

## Voice

- **Your mic is on from the moment you join,** unless you start with `--audio off`. In the
  desktop app, voice is on in every call, and the call bar's mic button mutes it.
- **Mute stops the sound at its source.** A muted track sends silence, not your voice at a lower
  level, and the other side is shown that you're muted. Mute is yours alone: the other side
  can't unmute you.
- **You can see that you're muted.** The viewer says so in its window title and its HUD. The
  desktop app's window says so too; `tc-spike` on the sharer prints a line in its terminal.
- **Only voices are sent,** not the computer's own sound.

## Known limits

- **The server can see everything that passes through it.** WebRTC encrypts traffic between each
  person and the LiveKit server, but the server decrypts it to forward it. So whoever runs the
  server can read video, audio and keystrokes: you, if you run `livekit-server` yourself, or
  LiveKit if you use LiveKit Cloud. End-to-end encryption is on the [roadmap](ROADMAP.md).
- **A guest in control can do anything you can.** That includes opening a terminal. Only grant
  control to people you'd hand your unlocked laptop to, and stay at the computer.
- **Screen sharing shows what's on your screen,** including notifications. Excluding windows or
  hiding notifications isn't built.
- **macOS can block input on some protected prompts** and possibly secure text fields. We'll
  document what we find.
- **Guests shouldn't type passwords during control.** They go to the sharer's machine, and through
  the server, like every other keystroke.
- **A call link lets anyone signed in knock.** The link's code is 128 random bits; knowing it
  lets someone ask to join, not join: the host lets each person in, and the backend signs a pass
  only for those. Share the link only with the person you're calling. The app puts it on your
  clipboard when you share, where other apps you run can read it, as they can anything you copy.
  Paste link reads the clipboard only when you click it, in the app's Rust side; what it finds
  goes to the link parser and nowhere else (never to the window or a log), and anything over 256
  characters isn't looked at.
- **Anyone with a GitHub account can sign up** on a backend that allows sign-ups. An account
  alone can knock only with a link and ring only mutual contacts. Whoever runs the backend can
  turn sign-ups off ([self-hosting](self-hosting.md#7-what-you-are-responsible-for)).
- **A room code is the only key to a room-code call.** The terminal tools and the Windows viewer
  join rooms by name on a server whose key they hold; anyone with that key and the code can
  join. The menu bar app doesn't start such calls.
- **A build with a baked-in LiveKit server carries its secret.** `tc-engine` can be built with a
  server's URL, API key and secret (`crates/engine-cli/build.rs`), for test rounds with the
  Windows viewer (`package.sh --legacy-keys`). `package.sh` refuses to combine that with a
  backend, and checks that an app built for a backend has no server key in its engine.
  Anyone who has that build can read the secret out of the binary and mint tokens for any room on
  that server. Give such builds only to people you'd give the key to, and rotate the key when a
  test round ends. `tc-engine config` never prints the secret. A Windows viewer zip with a
  `server.txt` (`packaging/windows/assemble.sh`) carries the secret in plain text, with the same
  rules. In pass mode no secret ships in the app at all: the account server signs a call pass
  for each person and call, and the engine only reads who the pass is for (it can't verify the
  signature, and doesn't need to: LiveKit does on connect). `tc-engine dev-pass`, which signs
  passes for testing, refuses to run in a build that carries a key.
- **A call pass is a bearer credential.** Whoever holds it can join that one room, as that
  person, until it expires. The engine takes it only from its environment (never a flag, which
  other users can read), holds it there for the life of the call, and never logs it or puts it
  in an event or error. Keep passes short-lived.
- **Server settings in the desktop app.** Overrides typed into Settings are saved in the app's
  config folder, readable only by your user, and passed to the engine in its environment rather
  than its flags (other users can read a process's flags). The window never gets the saved
  secret back.
- **Your account session is a bearer credential.** Signing in leaves a Supabase session (an
  access token that lasts an hour and a refresh token that renews it) in `session.json` in the
  app's config folder, readable and writable by your user only and replaced atomically. Anyone who
  can read that file can act as you on the backend until you sign out, which ends the session on
  the server and deletes the file (even when the server can't be reached; an access token already
  issued then still works until it expires, within the hour). Other users of the Mac can't read
  it; software running as you can. Once builds are signed, the session moves to the Keychain; an
  unsigned build would make macOS ask for Keychain access on every update. The window never
  receives a token: it learns only whether you're signed in and your name, handle and picture.
- **The sign-in callback listens on your machine for a few minutes.** During a sign-in, and only
  then, the app listens on `127.0.0.1:53682` (never another interface) for the browser's return
  from GitHub, for at most five minutes. Another program on your Mac could connect to it: it
  could end the sign-in early by sending an error or a made-up code (you'd see "Sign-in didn't
  finish" and try again), or take the port first so sign-in can't start (the app says the port is
  taken). It can't sign you in as someone else or take your session: the code alone is useless,
  because exchanging it needs the PKCE verifier, a random secret that never leaves the app's
  memory and that the backend checks against the challenge sent at the start. The page the
  listener answers with is fixed text, nothing from the request is echoed, and a provider's error
  description is never shown.
- **The backend's public key is not a secret.** A build can carry the Supabase project's URL and
  its public (anon or publishable) key (`apps/desktop/src-tauri/build.rs`). Both are public by
  design: they identify the project, and the database's row-level rules decide what each
  signed-in person may do. No service key, LiveKit secret or GitHub secret is ever in the app.
- **`tc-spike` isn't safe on untrusted networks.** It uses LiveKit's well-known development keys and
  an unencrypted `ws://` connection, so anyone who can reach the server can join. Real deployments
  need the server's own keys and TLS ([self-hosting](self-hosting.md)).

## macOS permissions

TwinCursor needs Screen Recording to capture, Accessibility to inject input, and Microphone to
talk. Accessibility is first needed when control is given, not when sharing starts: without it,
nobody is given control (or trusted with it), and the sharer is told what to allow. The desktop app holds these itself: its engine runs inside the app bundle, so macOS asks
for TwinCursor and grants nothing to anything else (ADR 0007). Run from a terminal, `tc-spike`
and `tc-engine` get the terminal's permissions instead, and so does everything else running from
it.

## Supply chain

- `Cargo.lock` is committed, and CI builds with `--locked`.
- `cargo deny` checks licenses and only allows crates from crates.io. Advisories are reported in CI.
- Dependabot proposes updates, and people review them.
- The WebRTC library is a prebuilt binary from LiveKit's GitHub releases.
- Release builds aren't signed yet. Signing and notarization are required before the first public
  release.

## Reporting a problem

See [SECURITY.md](../SECURITY.md). Please report privately.
