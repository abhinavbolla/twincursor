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

**Ring a contact.** Shares first if you aren't sharing, then rings them. Their row in People
says "Ringing…", then "In the call" once they answer. A ring nobody answers in 60 seconds shows
"No answer", and a declined one "Declined", for 10 seconds; then the row offers **Call** again.

**Being rung.** You see who is calling. Accepting leaves any call you're in, then joins; declining
tells the backend.

**Stop or Leave.** The engine leaves first, then the backend is told: the call ends for the host,
or you leave it as a guest. If the engine ends on its own (its window was closed, or the
connection failed), the app tells the backend the same way. A guest whose call has ended on the
server leaves by itself and says "The call ended."

**A call left open by a crash.** If you're signed in and not sharing, but the backend still has a
call of yours open, the app ends it.

## On an invite-only server

The backend may let only people on its invite list start calls
([self-hosting](self-hosting.md#6-choose-who-can-start-calls)). The home view's
`account.can_host` says whether you may; the server decides it and sends it with every poll.
When you may not, **Share my screen** and **Call** are refused in the app with "This server is
invite-only. Ask whoever runs it to add your GitHub handle.", without asking the server, and the
panel offers neither. The server refuses them anyway. If it refuses a share or a ring the app
thought was allowed (you were taken off the list since the last poll), the app shows the same
sentence and switches to the invite-only panel at once. **Ask to be invited** (`ask_to_host`)
tells whoever runs the server; `account.asked_to_host` remembers it across launches. Joining by
link, being let in, answering a ring and leaving work as for anyone. A call you are hosting when
you're taken off the list goes on until you stop it.

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
offer to open System Settings. Giving control needs Accessibility, asked the same way, on Let in
with control, Give control and Allow alike. Letting someone in "with control" without it still
lets them in, without control, and says why. Allow on a control request without it turns that
request down, so the guest isn't left waiting, and says why ("Sam wasn't given control:
TwinCursor needs Accessibility."); they can ask again once it is granted. The
microphone is asked for by the engine when a call starts.

The app asks macOS once per run. While a permission is needed, the app checks it again
(`tc-engine permissions`, which never prompts): every 2 s for two minutes after the need
appeared or you last clicked, opened Settings or reset, then every 15 s, and not at all after ten
minutes, until the next click checks afresh. So the window stops asking soon after it is turned
on; a re-check never gives anyone control. A guest who was let in without control, or whom Give
control couldn't trust, is then `owed`: the bar offers **Give control to Sam**, armed a second
after it is shown like a knock, and clicking it does exactly what Give control does (or, for
someone let in who has never been in the call since, what Let in with control does). The offer
ends when that guest leaves the call after being in it, is no longer let in (turned away or
removed), or the call ends, so it is never a promise for someone who has gone. Any revoke ends it
too, the hotkey and Take back alike, even before anything was given: Give control in the menu
stays for a deliberate grant.

Still refused after macOS was asked, and after either Open Settings or another attempt, the
permission is `stuck`. The window then offers **Reset and ask again** (`reset_permission`):
`tccutil reset Accessibility dev.twincursor.desktop` (or `ScreenCapture`) clears TwinCursor's own
entry, never another app's, and macOS is asked again. That is the way out when System Settings
shows TwinCursor switched on but macOS doesn't apply it, as can happen when the entry belongs to a
differently signed copy of the app (see [Releasing](RELEASING.md)).

A grant made in the middle of a call needs no new call: the sharer's engine makes its input
injector at the first grant and tries again at every later one, asking macOS each time. If the
engine still refuses while the app's fresh check says yes, the bar says "macOS says
Accessibility is on, but this share can't use it yet. Stop and share again for it to take
effect."

Every check, request and reset is written to the app's log with its answer, whether macOS was
asked, and tccutil's exit code; a re-check is written only when its answer changes.

## What the window gets

Every change to accounts and calls arrives on the `home` event as one view, which `home_state`
also returns, so a window that opens later starts out right:

```json
{
  "account": { "configured": true, "signed_in": true, "me": { "id": "…", "name": "…", "handle": "…", "avatar_url": null }, "signing_in": false, "can_host": true, "asked_to_host": false },
  "stage": { "kind": "hosting", "call_id": "…", "link": "twincursor://call/…" },
  "contacts": [{ "id": "…", "name": "…", "handle": "…", "avatar_url": null }],
  "rings": [{ "call_id": "…", "host": { "id": "…", "name": "…", "handle": null, "avatar_url": null } }],
  "knocks": [{ "user_id": "…", "name": "…", "handle": null, "avatar_url": null, "armed": false }],
  "members": [{ "user_id": "…", "name": "…", "handle": null, "avatar_url": null, "state": "admitted" }],
  "notice": "The call link is on your clipboard.",
  "error": null,
  "needs": null,
  "stuck": false,
  "owed": null,
  "server_problem": null
}
```

`stage.kind` is `idle`, `starting`, `hosting` (with `call_id` and `link`), `knocking` or `joined`
(with the `host`), or `ending`. `needs` is `screen_recording`, `accessibility` or `null`;
`stuck` says it is still refused after asking; `owed` is `null` or `{ "identity", "name",
"armed" }`, the guest Give control is offered for once Accessibility is on.

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
| `give_owed_control` | | Give control to `owed`, once armed |
| `reset_permission` | `which` | Reset TwinCursor's own entry for a needed permission and ask macOS again |
| `ring` | `user_id` | Ring a contact |
| `answer_ring` | `call_id`, `accept` | Accept or decline |
| `ask_to_host` | | Ask to be invited, on an invite-only server |
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
