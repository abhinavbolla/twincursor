# The app and the engine: IPC

The desktop app and the engine are two processes ([ADR 0007](adr/0007-shell-and-engine.md)). The
app starts the engine, `tc-engine`, and talks to it over the engine's stdin and stdout: **one
JSON object per line**, commands in and events out. Logs go to stderr, so stdout carries nothing
but events. The types live in `crates/ipc`; `crates/ipc/fixtures/*.jsonl` has one example of
every command and event, which the Rust tests read back and the app's tests can read too.

## Starting the engine

```
tc-engine share --ipc --room ROOM --name "Alex" [--resolution 720|1080|1440|2160|native]
                [--codec h264|h265] [--audio mic|off] [--mic DEVICE] [--speaker DEVICE]
                [--window ID|TEXT | --app NAME] [--allow-control] [--exclude-app-pid PID]
tc-engine view  --ipc --room ROOM --name "Sam" [--audio mic|off] [--mic DEVICE] [--speaker DEVICE]
tc-engine permissions [--request]     # one JSON object: Screen Recording, Accessibility, Microphone
tc-engine devices                     # {"mics": [...], "speakers": [...]}
tc-engine windows                     # [{"id", "app", "pid", "title", "pixels"}, ...]
tc-engine config                      # {"url", "api_key", "has_secret", "has_pass", "source"}
```

With a call pass in `TWINCURSOR_PASS`, `share` and `view` need neither `--room` nor `--name`
(see [Getting into the room](#getting-into-the-room)).

`--window` and `--app` share one window or one app instead of the display, and are refused with
`--allow-control`: control maps the guest's pointer onto the whole display.

### Getting into the room

The server's URL comes from `--url`, else `TWINCURSOR_SERVER_URL`, else the build's, else the
local dev server. The engine then joins its room in one of two ways.

**With a call pass.** If `TWINCURSOR_PASS` holds one, the engine uses it and nothing else. A
call pass is an ordinary LiveKit access token that the account server signed for one person in
one room: its `sub` is the identity (`sharer-<account id>` for the call's host, `guest-<account
id>` for an admitted guest), its `name` is the account's display name, and `video.room` is the
room. The engine reads those claims, without checking the signature, to refuse a pass that can't
work before it connects:

- the pass must be a well-formed token whose `video.roomJoin` is true, with a non-empty `sub` and
  room, not past its `exp`; a name over 64 characters or with control characters is refused, and
  a missing name shows as "Unnamed";
- `share` needs a sharer's pass and `view` a guest's ("This pass is for a guest; it can't be
  used to share.");
- `--room`, if given, must be the pass's room, and `--api-key`/`--api-secret` can't be given at
  all. `--name` is ignored: the pass's name wins. A key and secret in the environment or the
  build are simply not used.

Not verifying the signature is deliberate: the engine has no secret to verify it with, and it
doesn't need one. The pass grants nothing until LiveKit checks it on connect, so a forged or
altered pass fails there. The pass is only ever read from the environment, never a flag (other
users of the Mac can read a process's flags), and is never logged or put in an event or error.

For development and the benches, `tc-engine dev-pass --room R --role sharer|guest --name N [--id
ID] [--ttl SECONDS]` prints a pass signed with the engine's own key and secret (the dev server's
by default; `--id` defaults to a random UUID-like value, `--ttl` to an hour). It is hidden from
`--help` and refuses to run in a build that carries a server's key.

**With an API key and secret.** Without a pass, `--room` and `--name` are required, and the
engine mints its own token for a fresh identity. Each of the server's URL, API key and API secret
comes from the first of:

1. the flag: `--url`, `--api-key`, `--api-secret`;
2. the environment: `TWINCURSOR_SERVER_URL`, `TWINCURSOR_API_KEY`, `TWINCURSOR_API_SECRET`;
3. what the build baked in: `TWINCURSOR_BUILD_SERVER_URL`, `TWINCURSOR_BUILD_API_KEY` and
   `TWINCURSOR_BUILD_API_SECRET` in the environment of `cargo build` (`crates/engine-cli/build.rs`;
   see `.env.testbuild.example`);
4. the local dev server (`livekit-server --dev`).

Prefer the environment for the secret: other users of the Mac can read a process's flags. The
desktop app passes its saved overrides that way. The secret is only used to sign the access
token and is never logged. `tc-engine config` prints the URL and key a call would use, whether a
secret other than the dev server's is in effect (`has_secret`), whether a call pass is in its
environment (`has_pass`: then a call would use the pass), and where the URL came from (`source`:
`env`, `build` or `dev`), never the secret or the pass itself.

**The engine exits when its stdin closes.** The app holds the engine's stdin open for as long as
it runs. When the app quits or dies, the kernel closes the pipe; a sharer first takes control back
and releases anything a guest holds, then the process exits (about 150 ms in `bench/engine-e2e.sh`).
A screen is never shared with no app showing it.

The latency markers `tc-spike` stamps into the top left of the picture are off in `tc-engine`.

## Commands (app to engine)

| Command | Who | What |
|---|---|---|
| `{"cmd":"mute"}`, `{"cmd":"unmute"}` | both | This side's microphone |
| `{"cmd":"leave"}` | both | Leave the call; the engine sends `ended` and exits |
| `{"cmd":"allow_control","request":N}` | sharer | Grant request `N` from `control_asked` |
| `{"cmd":"deny_control","request":N}` | sharer | Turn request `N` down |
| `{"cmd":"revoke_control"}` | sharer | Take control back at once, as the revoke hotkey does, and every trust with it |
| `{"cmd":"trust_control","identity":"…"}` | sharer | Let this guest take control during this call without a prompt (see [Trusted control](#trusted-control)) |
| `{"cmd":"request_control"}` | guest | Ask the sharer for control |
| `{"cmd":"release_control"}` | guest | Give control back, or withdraw the request |
| `{"cmd":"set_mic","device":"…"}`, `{"cmd":"set_speaker","device":null}` | both | Switch devices mid-call, by name as `devices` lists them; `null` is the system default |

A command that can't be read, or doesn't fit the role, gets an `error` event; the call goes on.

## Events (engine to app)

| Event | What |
|---|---|
| `hello` | Always first: `version` (the protocol, now 1), `role` (`sharer` or `guest`), `engine` |
| `connection` | `state`: `connecting`, `connected`, `reconnecting`, `disconnected` |
| `participants` | Everyone else in the call, after any change: `identity`, `name`, `role`, and for guests the `decoders` they announced (`h264`, `h265`, `vp8`, `vp9`, `av1`) |
| `control_asked` | Sharer: `request` (counts from 1), the guest's `identity`, `name` and `platform` |
| `control` | `state`: `none`, `asked` or `controlling`; `by`: who (on a guest, this guest); `trusted`: on the sharer, the guests trusted with control right now (`identity`, `name`), always `[]` on a guest. Sent again whenever any of it changes |
| `voice` | `sending`, `muted`, and the `mic` and `speaker` in use |
| `stats` | About once a second: `width`, `height`, `fps`, `kbps`, `rtt_ms`, `codec`, and on a guest with a window `shown_pct` (100 is 1:1) |
| `error` | `message`: a sentence for a person. The call may go on |
| `ended` | Last: `reason` is `left`, `disconnected` or `failed` |

Every field is always present, `null` when there is nothing to say.

## Consent

`tc-engine share` asks the app, never the terminal. A guest's request becomes a `control_asked`
event with a request number, and **only an `allow_control` naming that number, while that guest is
still asking, grants control**. An allow for any other number, for a request that was withdrawn,
or one sent after control ended is refused with an `error`, so a click meant for one request can
never grant another. No answer means no control. The one other way to get control is a trust
the sharer gave that guest beforehand, in this call ([below](#trusted-control)). Without
`--allow-control`, requests are turned down without reaching the app.

The rest of hard rule 4 stays in the engine, where it doesn't depend on the app: the "is
controlling" label on the sharer's screen, and the revoke hotkey (⌃⌥⌘R), which works with the
app hung or gone and the network down. `revoke_control` trips the same switch as the hotkey.

### Trusted control

`trust_control` is the sharer's standing grant for one guest, for one call ("Let in with
control", "Give control"; [ADR 0010](adr/0010-trusted-control.md)). After it, that guest's
requests are granted at once: no `control_asked`, no request number, straight to a `control`
event with `state` `controlling`. Everything else about the grant is the same as after an allow:
the label, the input checks, the revoke hotkey. If the guest is asking when the trust arrives,
they get control then, and that request's number is closed, so a later `allow_control` for it is
refused as no longer open. Trust grants nothing by itself: the guest still has to ask.

The trust lasts until any revoke (the hotkey, `revoke_control`, "Take back"), which clears every
trust whether or not anyone is in control, or until that guest leaves the call. Giving control
back keeps it. It is never stored anywhere, and nothing a guest or the server sends can create it.
A revoke that happens after the app sent `trust_control` wins, even if the engine carries it out
first.

`trust_control` is refused with an `error` event, and trusts nobody, when:

| When | `message` |
|---|---|
| The share has no `--allow-control` | "This share doesn't allow remote control." |
| The identity isn't a guest's (the sharer's own included) | "Only a guest can be given control." |
| Nobody with that identity is in the call | "That person isn't in the call." |
| Another guest has control | "Someone else is in control. Take control back first." |
| A revoke came after the app sent it | "Control was just taken back, so nobody was given it." |

### Names

The name in `control_asked`, in `control` (`by` and `trusted`) and on the sharer's "… is
controlling" label is the one in the guest's token, as `participants` shows it, and not the one
the guest's own app puts in its control `hello` (the hello's name is only used for a guest whose
token has none). With a call pass, the token's name is the guest's account name, signed by the
account server. Without one, it is still whatever the guest typed: there, a name is not proof of
who someone is, and the sharer should grant control because they recognise the person on the
call.

## Who is who

A participant's **role travels in their LiveKit identity**: `sharer-…` or `guest-…`, followed by
the account's id with a call pass, or by a random value the engine picked without one (`tc-spike`
keeps its plain `sharer` and `viewer`). Only the part before the first dash is the role. The
server signs the identity into the token, and every data packet carries its sender's identity, so
the engine knows whose message it holds without a lookup. The **display name travels in the
token's name field**: the account's name in a call pass, the typed `--name` otherwise
(`media::token::mint`). Identities are unique per call; names need not be.

Guests **announce the codecs they decode** in the `hello` they already send the sharer on the
control channel (`crates/protocol`, a `decoders` list that older peers ignore). The sharer
shows them in `participants`, so a sharer using HEVC can see a guest that can't decode it. The
desktop app then starts its engine again with `--codec h264` for the rest of the call (and does
the same, once, if an engine sharing with HEVC fails before it sends any video). For tests,
`tc-engine view --decoders h264,vp8` announces only the codecs listed.

## Checking it

`bench/engine-e2e.sh` plays the app's part with nobody at the Mac: a sharer on the synthetic
pattern and a headless guest, a request, the check that nothing is granted without an answer, an
allow, typing (recorded, not injected), a revoke, a stale allow, both leaving, and the engine
exiting when its stdin closes. It then runs a call again with passes from `dev-pass` and no
`--room` or `--name`, then trusted control with passes (a trusted request granted with no prompt,
again after giving control back, a revoke before and after the guest asks, refused trusts, a guest
who leaves and rejoins with the same pass, a share without `--allow-control`, and a guest whose
hello claims another name), and checks that a pass for the other role is refused before
connecting.

`bench/app-e2e.sh` runs the real desktop app instead, in its test-only script mode, against a
headless guest: a deny, an allow with recorded typing, a revoke and a leave, then the switch from
HEVC to H.264 and the engine exiting when the app is killed.
