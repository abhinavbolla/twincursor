# Accounts

An account is a GitHub sign-in on a TwinCursor backend ([Supabase](https://supabase.com);
see [`supabase/README.md`](../supabase/README.md)). It gives you a profile (your GitHub name,
login and picture), contacts, and calls that only the people you let in can join. The backend
never sees the call's screen, voice or input, and it never decides remote control: the
sharer's app does.

What the backend stores about people, and what it never sees, is in
[ADR 0011](adr/0011-accounts-and-call-passes.md#consequences). Setting one up is in
[self-hosting](self-hosting.md).

The code is in `crates/account` (no Tauri in it) and `apps/desktop/src-tauri/src/accounts.rs`.
What the app does with an account (sharing, joining by link, letting people in, ringing) is in
[calls](calls.md).

## Signing in

1. The app makes a random secret, the *verifier*, and keeps it in memory. It sends only its
   SHA-256 (the *challenge*) along with the request.
2. The app starts listening on `127.0.0.1:53682`, the loopback address only, and opens
   `<backend>/auth/v1/authorize?provider=github&redirect_to=http://127.0.0.1:53682/callback&code_challenge=…&code_challenge_method=s256`
   in your browser.
3. You approve TwinCursor on GitHub. The backend sends your browser back to
   `http://127.0.0.1:53682/callback?code=…`. The listener answers only that path and gives up
   after five minutes, or when you click Cancel.
4. The app trades the code and the verifier for a session at
   `POST <backend>/auth/v1/token?grant_type=pkce`. A code is useless without the verifier, so a
   code seen by anyone else can't be used to sign in. The browser tab then says "You're signed
   in. You can close this tab."

This is the OAuth authorization-code flow with PKCE, as Supabase Auth implements it
([PKCE flow](https://supabase.com/docs/guides/auth/sessions/pkce-flow),
[redirect URLs](https://supabase.com/docs/guides/auth/redirect-urls)). The backend must
allow `http://127.0.0.1:53682/callback` as a redirect URL (`supabase/config.toml` does for the
local stack).

## Staying signed in, and signing out

The session is an access token (an hour) and a refresh token. When the access token has less
than a minute left, the app swaps the refresh token for a new pair; calls made at the same moment wait for that one refresh. If the backend says the refresh
token is no good, you are signed out. If the backend can't be reached, you stay signed in and
the app says "Can't reach the server."

Signing out tells the backend to end this session (`POST /auth/v1/logout?scope=local`; your
other devices stay signed in), then deletes the session file, even if the backend couldn't be
reached.

## What is stored where

| What | Where | Who can read it |
|---|---|---|
| The session (access and refresh token, expiry, account id) | macOS: `session.json` in the app's config folder (`~/Library/Application Support/dev.twincursor.desktop/`). Windows: `session.json` in `%LOCALAPPDATA%\dev.twincursor.desktop\` | macOS: your user only (mode 0600). Windows: your user, SYSTEM and administrators (the profile folder's permissions; no mode is set). Replaced atomically on both |
| The PKCE verifier and the code | The app's memory, for one sign-in | Nobody else |
| The backend's URL and public key | Built into the app, or the environment | Anyone: they are public |

The session file holds a bearer credential: anyone who can read it can act as you until you sign
out (see [the threat model](threat-model.md)). Once builds are signed, the session moves to the
macOS Keychain; an unsigned build would make macOS ask for Keychain access after every update.
On Windows it stays in the local (not roaming) app data folder, so a roaming profile doesn't
copy it to other PCs; it isn't encrypted (DPAPI would protect it only from people without your
Windows login, which the folder's permissions already do while Windows runs).

## What the window sees

The app's window (the webview) asks the Rust side for the account state and gets only
`{ configured, signed_in, me: { id, name, handle, avatar_url } | null, signing_in, can_host,
asked_to_host }`. `can_host` is whether the server lets this person start calls (true until it
has said otherwise, and from a server without an invite list); `asked_to_host` whether they asked
to be invited ([calls](calls.md#on-an-invite-only-server)). It never
receives a token, a code or the verifier: tokens in `crates/account` are `Secret`s, which can't
be serialized or printed, and the tests check the exact shape the window gets. Avatars load only
from `https://avatars.githubusercontent.com`, the one image host the window's content security
policy allows.

| Command | Answers |
|---|---|
| `account_state` | The state above |
| `sign_in` | Opens the browser; resolves with the state once signed in, or fails with a sentence |
| `cancel_sign_in` | Stops a sign-in waiting for the browser |
| `sign_out` | The state, signed out |

## Pointing the app at a backend

The app looks in this order, each value separately:

1. `TWINCURSOR_SUPABASE_URL` and `TWINCURSOR_SUPABASE_KEY` in its environment, for a dev run:

   ```sh
   cd apps/desktop
   TWINCURSOR_SUPABASE_URL=http://127.0.0.1:54421 TWINCURSOR_SUPABASE_KEY=<ANON_KEY from supabase status> \
     npm run tauri dev
   ```

2. Values baked in at build time from `TWINCURSOR_BUILD_SUPABASE_URL` and
   `TWINCURSOR_BUILD_SUPABASE_KEY` (`apps/desktop/src-tauri/build.rs`). To make an app for other
   people, use `apps/desktop/scripts/package.sh BACKEND_FILE`, which sets them from a file and
   checks that no LiveKit key goes in with them ([Releasing](RELEASING.md#builds-for-people-to-install)).

3. Neither: the panel says "This build isn't connected to a server." and nothing else works.

`TWINCURSOR_SESSION_FILE`, if set, is where the app keeps the session instead of its config
folder, for tests that sign in with `tc-account` first.

The URL must be `https://`, except `http://` on `127.0.0.1` or `localhost` for a local stack.

## `tc-account`

A development tool in `crates/account` for benches and for trying a backend by hand. The app
doesn't bundle it. It reads the backend from `TWINCURSOR_SUPABASE_URL` and
`TWINCURSOR_SUPABASE_KEY`, keeps the session in the file `--session` names, and prints JSON. A
failure prints `{"error": "<sentence>", "kind": "<word>"}` and exits with status 1.

```sh
tc-account --session s.json sign-in                         # GitHub, in the browser
TC_ACCOUNT_PASSWORD=… tc-account --session s.json password-sign-in --email a@example.test
tc-account --session s.json whoami                          # with can_host and asked_to_host
tc-account --session s.json inbox [--hosting CALL]
tc-account --session s.json start-call                      # {call_id, code}
tc-account --session s.json knock CODE
tc-account --session s.json answer-knock CALL USER --let-in | --turn-away
tc-account --session s.json ring CONTACT
tc-account --session s.json answer-ring CALL --accept | --decline
tc-account --session s.json leave CALL
tc-account --session s.json end CALL
tc-account --session s.json remove-contact CONTACT
tc-account --session s.json ask-to-host                     # on an invite-only server
tc-account --session s.json pass CALL                       # who the pass is for, no token
eval "$(tc-account --session s.json pass CALL --print-env)" # sets TWINCURSOR_SERVER_URL, TWINCURSOR_PASS
tc-account --session s.json sign-out
```

`password-sign-in` is a test hook: only a local stack turns email sign-in on, and a hosted
backend offers GitHub alone. `pass --print-env` is the only way the tool prints a pass, so a
script can start `tc-engine` with it.

`bench/account-e2e.sh` runs the whole chain on one Mac against the local stack and local
LiveKit: two people sign in, one starts a call, the other knocks and is let in, both join with
their passes, and someone never let in is refused. Then it makes the server invite-only, as an
owner would with the service key: the invited person can start a call, the other is told the
server is invite-only but can still knock, be let in and get a guest pass, and asks to host. It also checks that no token turns up in
anything it captured. The GitHub round trip itself needs a GitHub OAuth app and a person, so the
bench stops at checking that the local auth server accepts the sign-in page and redirects to
GitHub.
