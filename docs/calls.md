# Calls with accounts

How the desktop app shares, joins, lets people in and rings, once you're signed in
([accounts](accounts.md)). The backend ([`supabase/README.md`](../supabase/README.md)) decides
who may join a call and signs their call pass. The app decides everything that happens on your
Mac: when its engine starts and stops, and whom it gives control to.

The flows live in the app's Rust side (`apps/desktop/src-tauri/src/calls.rs`). The window asks
for them with commands and is told the result; it never sees a token, a pass or what's on your
clipboard.

## The flows

**Share my screen.** Needs you signed in and not in a call. The app checks Screen Recording
first (below). It starts a call on the backend, fetches its pass, and starts the engine as the
sharer with the pass in its environment (`TWINCURSOR_PASS`, and the LiveKit server from the
pass in `TWINCURSOR_SERVER_URL`), at the best quality: 4K with HEVC, falling back to H.264 when
needed. The engine always gets `--allow-control` and `--exclude-app-pid`. The call link goes on
your clipboard. While you share, every poll names the call: that is the host's heartbeat, which
keeps the call live.

**The call link** is `twincursor://call/<code>`, where the code is the 32 lowercase hex
characters the backend gave the call. It names no server: the app knows its own backend. Pasting
takes the whole link or the bare code, with whitespace around it, and nothing else.

**Paste link.** The app reads the clipboard itself, checks it holds a call link, and knocks. You
see whose call it is while you wait. The app repeats the knock every two seconds; once the host
lets you in, it fetches your pass and starts the engine as a guest. If the host turns you away,
it says "… didn't let you in."; a call that has ended, or a wrong link, says so. Cancel stops
waiting.

**Someone knocks on your call.** You see them, and can let them in (with or without control) or
turn them away. Letting them in "with control" means: once the backend has let them in, the app
asks its engine to trust them when they arrive (`trust_on_arrival`): if they join within a
minute, they may take control for this call without a prompt
([trusted control](adr/0010-trusted-control.md)). The engine keeps that promise, not the app, so
any revoke cancels it, the hotkey included, even before they join. They still have to ask each
time, and the "is controlling" label shows every grant.

**Give control** to someone already in the call does the same at once. **Take back** (or the
revoke hotkey, ⌃⌥⌘R) takes control and every trust back, including one promised to someone
who hasn't arrived yet. These two are the only ways to trust anyone: the window can't send
`trust_control` or `trust_on_arrival` to the engine itself ("Control is given with Let in with
control or Give control."), so the Accessibility check always comes first.

**Ring a contact.** Shares first if you aren't sharing, then rings them. You see whether it's
ringing, missed, declined or answered.

**Being rung.** You see who is calling. Accepting leaves any call you're in, then joins; declining
tells the backend.

**Stop or Leave.** The engine leaves first, then the backend is told: the call ends for the host,
or you leave it as a guest. If the engine ends on its own (its window was closed, or the
connection failed), the app tells the backend the same way. A guest whose call has ended on the
server leaves by itself and says "The call ended."

**A call left open by a crash.** If you're signed in and not sharing, but the backend still has a
call of yours open, the app ends it.

## Polling

While you're signed in, the app asks the backend for your contacts, rings, knocks and your call
every two seconds, one request at a time. After three network failures in a row it says "Can't
reach the server." and slows to every ten seconds until the server answers again. Signed out, it
asks nothing.

While it holds a call open (sharing, knocking or in a call), the app tells macOS it is doing
something the person started, so App Nap doesn't stretch the poll: the heartbeat and the knock
must reach the backend within 45 seconds.

## Arming: why Let in and Allow wait a second

A knock appears, or a control request does, while you're typing or about to click something
else. A click or a key press already on its way could land on Let in or Allow. So the app
refuses to let someone in until a second after their knock could first be seen, and refuses an
Allow until a second after that request could: "Too soon. Try again." Could be seen means
delivered to the call bar with the call bar on screen; one delivered while the bar was hidden
counts from when the bar came back. The bar never takes the keyboard when it appears, so typing
goes on into the app you were in. The window keeps the buttons off for that second too, but the
rule is enforced in the app's Rust side, so it holds whatever the window does. Turning someone away and denying a request are never delayed.

An Allow also names the request it answers, and goes only to the engine that asked. Request
numbers start again when the engine restarts, so an Allow for a request from before a restart is
refused.

## When the engine restarts

When a guest's computer can't decode HEVC, or sharing with HEVC fails before any video goes out,
the app replaces its engine with one on H.264. A pass is only good for joining for ten minutes,
so the new engine gets a fresh one. Control is not carried over: nobody is in control and nobody
is trusted or expected in the new engine, and the app says "Control was reset: anyone you gave it to has to be given it again." A command meant
for the old engine never reaches the new one.

## Permissions, asked when first needed

Sharing needs Screen Recording. The first time you share without it, the app asks macOS
(`tc-engine permissions --request screen_recording`), which shows its prompt; if it still isn't
granted, the share doesn't start and the app says Screen Recording is needed, so the window can
offer to open System Settings. Giving control needs Accessibility, asked the same way. Letting
someone in "with control" without it still lets them in, without control, and says why. The
microphone is asked for by the engine when a call starts.

## What the window gets

Every change to accounts and calls arrives on the `home` event as one view, which `home_state`
also returns, so a window that opens later starts out right:

```json
{
  "account": { "configured": true, "signed_in": true, "me": { "id": "…", "name": "…", "handle": "…", "avatar_url": null }, "signing_in": false },
  "stage": { "kind": "hosting", "call_id": "…", "link": "twincursor://call/…" },
  "contacts": [{ "id": "…", "name": "…", "handle": "…", "avatar_url": null }],
  "rings": [{ "call_id": "…", "host": { "id": "…", "name": "…", "handle": null, "avatar_url": null } }],
  "knocks": [{ "user_id": "…", "name": "…", "handle": null, "avatar_url": null, "armed": false }],
  "members": [{ "user_id": "…", "name": "…", "handle": null, "avatar_url": null, "state": "admitted" }],
  "notice": "The call link is on your clipboard.",
  "error": null,
  "needs": null,
  "server_problem": null
}
```

`stage.kind` is `idle`, `starting`, `hosting` (with `call_id` and `link`), `knocking` or `joined`
(with the `host`), or `ending`. `needs` is `screen_recording`, `accessibility` or `null`.

| Command | Arguments | Does |
|---|---|---|
| `home_state` | | The view above |
| `call_snapshot` | | The current call's engine output, cut down, to replay in a window opened mid-call |
| `share` | | Share my screen |
| `join_from_clipboard` | | Paste link |
| `join_link` | `text` | Join with a link or code typed in |
| `cancel_knock` | | Stop waiting |
| `answer_knock` | `user_id`, `let_in`, `control` | Let in (with or without control), or turn away |
| `give_control` | `identity` | Trust a guest in the call with control |
| `ring` | `user_id` | Ring a contact |
| `answer_ring` | `call_id`, `accept` | Accept or decline |
| `end` | | Stop sharing, or leave |

Each answers with the view after it, or fails with one sentence. The engine's own output still
arrives on the `engine` event, as before.

A window opened during a call replays `call_snapshot` through the same reducer as the `engine`
event and ends up in the same state as a window that listened from the start: the call's first
and latest `started`, the latest of each kind of event, an open control request, the restart
notice, the last error, the last 300 log lines and `exited`.

## Checking it

`bench/app-accounts-e2e.sh` runs the real app in its script mode against the local backend and
LiveKit, with `tc-account` and `tc-engine` as the other person: sharing, a knock let in too soon
and then with control, trusted control and a revoke, letting in without control, turning away,
joining by link and the host ending the call, rings both ways, and the switch to H.264 with a
fresh pass. It checks that no token or pass turns up in anything it captured.
