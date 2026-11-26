# Accounts

An account is a GitHub sign-in on a TwinCursor backend ([Supabase](https://supabase.com);
see [`supabase/README.md`](../supabase/README.md)). It gives you a profile (your GitHub name,
login and picture), contacts, and calls that only the people you let in can join. The backend
never sees the call's screen, voice or input, and it never decides remote control: the
sharer's app does.

The code is in `crates/account` (no Tauri in it) and `apps/desktop/src-tauri/src/accounts.rs`.

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
| The session (access and refresh token, expiry, account id) | `session.json` in the app's config folder (`~/Library/Application Support/dev.twincursor.desktop/`) | Your user only (mode 0600, replaced atomically) |
| The PKCE verifier and the code | The app's memory, for one sign-in | Nobody else |
| The backend's URL and public key | Built into the app, or the environment | Anyone: they are public |

The session file holds a bearer credential: anyone who can read it can act as you until you sign
out (see [the threat model](threat-model.md)). Once builds are signed, the session moves to the
macOS Keychain; an unsigned build would make macOS ask for Keychain access after every update.

## What the window sees

The app's window (the webview) asks the Rust side for the account state and gets only
`{ configured, signed_in, me: { id, name, handle, avatar_url } | null, signing_in }`. It never
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

2. Values baked in at build time:

   ```sh
   TWINCURSOR_BUILD_SUPABASE_URL=https://<project>.supabase.co TWINCURSOR_BUILD_SUPABASE_KEY=<public key> \
     cargo build --release -p twincursor-desktop
   ```

3. Neither: the app has no account features and shows nothing about accounts.

The URL must be `https://`, except `http://` on `127.0.0.1` or `localhost` for a local stack.

## `tc-account`

A development tool in `crates/account` for benches and for trying a backend by hand. The app
doesn't bundle it. It reads the backend from `TWINCURSOR_SUPABASE_URL` and
`TWINCURSOR_SUPABASE_KEY`, keeps the session in the file `--session` names, and prints JSON. A
failure prints `{"error": "<sentence>", "kind": "<word>"}` and exits with status 1.

```sh
tc-account --session s.json sign-in                         # GitHub, in the browser
TC_ACCOUNT_PASSWORD=… tc-account --session s.json password-sign-in --email a@example.test
tc-account --session s.json whoami
tc-account --session s.json inbox [--hosting CALL]
tc-account --session s.json start-call                      # {call_id, code}
tc-account --session s.json knock CODE
tc-account --session s.json answer-knock CALL USER --let-in | --turn-away
tc-account --session s.json ring CONTACT
tc-account --session s.json answer-ring CALL --accept | --decline
tc-account --session s.json leave CALL
tc-account --session s.json end CALL
tc-account --session s.json remove-contact CONTACT
tc-account --session s.json pass CALL                       # who the pass is for, no token
eval "$(tc-account --session s.json pass CALL --print-env)" # sets TWINCURSOR_SERVER_URL, TWINCURSOR_PASS
tc-account --session s.json sign-out
```

`password-sign-in` is a test hook: only a local stack turns email sign-in on, and a hosted
backend offers GitHub alone. `pass --print-env` is the only way the tool prints a pass, so a
script can start `tc-engine` with it.

`bench/account-e2e.sh` runs the whole chain on one Mac against the local stack and local
LiveKit: two people sign in, one starts a call, the other knocks and is let in, both join with
their passes, and someone never let in is refused. It also checks that no token turns up in
anything it captured. The GitHub round trip itself needs a GitHub OAuth app and a person, so the
bench stops at checking that the local auth server accepts the sign-in page and redirects to
GitHub.
