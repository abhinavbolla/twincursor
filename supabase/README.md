# Backend

TwinCursor's accounts, contacts and calls run on [Supabase](https://supabase.com): Postgres with
row-level security, Supabase Auth (GitHub sign-in), and one Edge Function that hands out LiveKit
tokens. The server never sees media, and it never decides remote control: that stays with the
host's app.

| Path | What it is |
|---|---|
| `config.toml` | The local stack (`supabase start`) |
| `migrations/` | Tables, access rules and the RPC functions the app calls |
| `functions/call-pass/` | Mints a LiveKit token ("call pass") for someone allowed into a call |
| `tests/database/` | pgTAP tests of the access rules (`supabase test db`) |
| `tests/call-pass.sh` | End-to-end check of the function against the local stack |

## Data model

| Table | One row per | Notes |
|---|---|---|
| `profiles` | account | `name` is what the person calls themselves on GitHub (≤ 64 characters, no control characters); `handle` is their GitHub login; `avatar_url` their GitHub avatar. Kept in sync by triggers on `auth.users` and `auth.identities` |
| `calls` | call | `host`, `room` (LiveKit room name), `code` (what a call link carries), `seen_at` (host heartbeat), `ended_at` |
| `call_members` | person in a call other than its host | `state`: `ringing`, `knocking`, `admitted`, `turned_away`, `declined` or `left`; `seen_at`: when a knocking person last knocked |
| `contacts` | (owner, contact) pair | Added both ways when a host lets someone in |

`room` and `code` are independent 128-bit random values, each 32 lowercase hex characters.

Profiles of accounts with a GitHub identity come only from that identity, which the person can't
edit through Supabase: `name` from GitHub's name (or full name, or login). User metadata can be
rewritten by the signed-in user, so it is used only for accounts without a GitHub identity
(name from metadata, then the email's local part, then "Someone").

## The rules

- **A call is live** while it hasn't been ended and its host's app sent a heartbeat in the last 45
  seconds. The heartbeat is `inbox(hosting => call_id)`, sent only while the app is actually
  sharing that call; it also brings back a call that went stale after missed polls. Ringing someone
  counts too. Ended calls never come back, and neither does a call silent for more than 10
  minutes: it is over, so a guest admitted long ago can't get back in when the host calls again.
  A host has one open call at a time: starting a call ends the previous one.
- **After a restart,** `inbox()` without `hosting` still shows the host's open call if it is live.
  An app that isn't sharing it should call `end_call` on it.
- **Joining by link (a knock).** Anyone signed in with a live call's code can knock. The host sees
  who is knocking and lets them in or turns them away, once. A waiting app repeats `knock` every
  few seconds; a knock not repeated for 45 seconds drops out of the host's list and can't be
  answered, until the person knocks again. Someone turned away can't knock again
  on that call; someone who left can, and waits to be let in again. Unknown, ended and stale codes
  all give the same error.
- **Joining by ring.** A host can ring someone only when each has the other as a contact. The ring
  is open for 60 seconds, and only the person rung can answer it. Removing a contact stops them
  ringing you.
- **Call passes.** Only the call-pass function signs LiveKit tokens, and only for a live call's host
  (role `sharer`, identity `sharer-<user id>`) or a member who was let in or accepted a ring (role
  `guest`, identity `guest-<user id>`). A guest's token can publish only the microphone and camera,
  never a screen. The LiveKit API secret exists only in the function's environment.
- **A pass is valid for 10 minutes to join.** Ending a call, turning someone away or their leaving
  stops new passes, but doesn't disconnect anyone already in the media room: the host's app does
  that. A server-side kick is future work.
- **Who can read what.** Your own profile, your contacts' profiles, and the profiles of people in a
  live call with you (host and members, except people turned away or who declined). Calls you host
  or are a member of. Member rows of calls you host, and your own. Your own contacts. Nobody else's.
- **Writes.** No table can be written directly. Every change goes through the functions below,
  which check who is calling. Signed-out (`anon`) clients can call none of them.

## Functions

All are called as the signed-in user through PostgREST (`POST /rest/v1/rpc/<name>`).

| Function | Returns |
|---|---|
| `start_call()` | `[{ call_id, code }]` |
| `knock(code)` | `[{ call_id, state, host_id, host_name, host_handle, host_avatar_url }]` |
| `answer_knock(call_id, user_id, let_in)` | `"admitted"` or `"turned_away"` |
| `ring(contact)` | `[{ call_id, code }]` |
| `answer_ring(call_id, accept)` | `"admitted"` or `"declined"` |
| `leave_call(call_id)`, `end_call(call_id)`, `remove_contact(contact)` | nothing |
| `pass_claims(call_id)` | `[{ room, identity, name, role }]` (used by the call-pass function) |
| `inbox(hosting)` | one JSON object, below. `hosting` (optional) is the call the app is sharing: the host's heartbeat |

```json
{
  "me": { "id": "…", "name": "…", "handle": "…", "avatar_url": "…" },
  "contacts": [{ "id": "…", "name": "…", "handle": "…", "avatar_url": "…" }],
  "hosting": null,
  "rings": [{ "call_id": "…", "host": { "id": "…", "name": "…", "handle": "…", "avatar_url": "…" } }],
  "waiting": [{ "call_id": "…", "state": "knocking", "host": { "id": "…", "name": "…", "handle": "…", "avatar_url": "…" } }]
}
```

`hosting` is `null` unless you have a live open call, then `{ "call_id", "code", "members": [{
"user_id", "name", "handle", "avatar_url", "state" }] }`. A ring nobody answered within 60
seconds shows as `"missed"`, and knocks not repeated for 45 seconds are left out. A `hosting`
argument that isn't your open call is ignored.
`waiting` lists your knocks on live calls, and knocks or rings answered in the last 60 seconds
(`admitted` or `turned_away`). `handle` and `avatar_url` may be `null`.

Errors have a stable message and SQLSTATE (PostgREST puts them in `message` and `code`):

| Message starts with | SQLSTATE | HTTP | When |
|---|---|---|---|
| `call_not_found` | `P0001` | 400 | No such live call, or it isn't yours to manage |
| `not_knocking` | `P0001` | 400 | `answer_knock`: nobody is knocking with that id (answered already, or gone quiet for 45 s) |
| `no_pending_ring` | `P0001` | 400 | `answer_ring`: no ring to you on that live call in the last 60 s |
| `not_in_call` | `P0001` | 400 | `leave_call`: you aren't knocking on or in that call |
| `own_call` | `22023` | 400 | `knock`: you host this call |
| `bad_argument` | `22023` | 400 | A required yes/no was null |
| `not_allowed` | `42501` | 403 | `pass_claims`: you may not join this call |
| `not_a_contact` | `42501` | 403 | `ring`: you aren't each other's contacts |

`inbox` can write (it is the heartbeat), so call it with `POST`; PostgREST runs `GET` requests
read-only.

### call-pass

`POST /functions/v1/call-pass` with the user's access token and `{ "call_id": "<uuid>" }`.
Answers `200 { url, token, room, identity, name, role }`, `400` for a bad body, `401` without a
signed-in user, `403 { "error": "not_allowed" }` when `pass_claims` refuses, and `500` if the
function isn't configured. It needs `LIVEKIT_URL`, `LIVEKIT_API_KEY` and `LIVEKIT_API_SECRET`
as function secrets.

## Running it locally

You need the [Supabase CLI](https://supabase.com/docs/guides/local-development/cli/getting-started),
Docker and [Deno](https://deno.com).

```sh
supabase start                    # from the repository root; ports 54420–54423
supabase test db                  # access rules (pgTAP)
supabase db lint --schema public,private --fail-on warning
(cd supabase/functions/call-pass && deno task test)   # token minting
supabase/tests/call-pass.sh       # the function end to end, with made-up LiveKit settings
supabase stop
```

`call-pass.sh` serves the function itself and stops it when done; run `supabase start` again
afterwards if you want the function served by the stack.

GitHub sign-in needs an OAuth app whose callback is `http://127.0.0.1:54421/auth/v1/callback`. Put
its credentials in `supabase/.env` (git-ignored), which the CLI reads:

```sh
SUPABASE_AUTH_GITHUB_CLIENT_ID=…
SUPABASE_AUTH_GITHUB_SECRET=…
```

Without them the stack still starts; only GitHub sign-in fails. Email and password sign-up is on
locally so the tests can create users. A hosted project should turn email sign-in off and offer
GitHub alone.
