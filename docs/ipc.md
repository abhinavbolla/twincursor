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
```

`--window` and `--app` share one window or one app instead of the display, and are refused with
`--allow-control`: control maps the guest's pointer onto the whole display.

The server comes from `--url`, `--api-key` and `--api-secret`, or from `TWINCURSOR_SERVER_URL`,
`TWINCURSOR_API_KEY` and `TWINCURSOR_API_SECRET`; without either, the local dev server. Prefer
the environment for the secret: other users of the Mac can read a process's flags. The secret is
only used to sign the access token and is never logged.

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
| `{"cmd":"revoke_control"}` | sharer | Take control back at once, as the revoke hotkey does |
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
| `control` | `state`: `none`, `asked` or `controlling`, and `by`: who (on a guest, this guest) |
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
never grant another. No answer means no control. Without `--allow-control`, requests are turned
down without reaching the app.

The rest of hard rule 4 stays in the engine, where it doesn't depend on the app: the "is
controlling" label on the sharer's screen, and the revoke hotkey (⌃⌥⌘R), which works with the
app hung or gone and the network down. `revoke_control` trips the same switch as the hotkey.

**Names are typed by the person and are not proof of who they are.** The name in
`control_asked` is what the guest typed. The app should show it as such ("Sam (unverified)"
until accounts exist), and the sharer should grant control because they recognise the person on
the call, not because of the name.

## Who is who

A participant's **role travels in their LiveKit identity**: `sharer-<random>` or
`guest-<random>` (`tc-spike` keeps its plain `sharer` and `viewer`). The server signs the identity
into the token, and every data packet carries its sender's identity, so the engine knows whose
message it holds without a lookup. The **display name travels in the token's name field**
(`media::token::mint`). Identities are unique per call; names need not be.

Guests **announce the codecs they decode** in the `hello` they already send the sharer on the
control channel (`crates/protocol`, a `decoders` list that older peers ignore). The sharer
shows them in `participants`, so a sharer using HEVC can see a guest that can't decode it.
Falling back to H.264 for such a guest isn't built yet.

## Checking it

`bench/engine-e2e.sh` plays the app's part with nobody at the Mac: a sharer on the synthetic
pattern and a headless guest, a request, the check that nothing is granted without an answer, an
allow, typing (recorded, not injected), a revoke, a stale allow, both leaving, and the engine
exiting when its stdin closes.
