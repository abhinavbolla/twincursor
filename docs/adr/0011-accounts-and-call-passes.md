# ADR 0011: Accounts on Supabase, and call passes

Status: accepted (2026-11-26)

## Context
Calls started with a room code typed into both apps. Anyone with the code, the app and the
server could join, names were whatever people typed, and every build that reached a hosted
LiveKit server carried that server's API secret, from which anyone could mint a token for any
room. Pairing needs more than that: knowing who someone is, a list of people you pair with, a
link that lets someone ask to join rather than join, and no secret in the app.

The plan had been a server of our own: Rust with axum, Postgres, and a docker compose file with
LiveKit and Caddy. That is a lot to build, run and secure before the first call with an account,
and most of it (sign-in, sessions, a database with access rules) is what a hosted backend already
offers.

## Decision
- **The backend is a Supabase project** (`supabase/`): Supabase Auth for sign-in, Postgres for
  profiles, contacts, calls and call members, and one Edge Function. There is no server of our
  own to run.
- **People sign in with GitHub, and only GitHub.** The app uses the OAuth code flow with PKCE
  through a one-time listener on `127.0.0.1:53682` ([accounts](../accounts.md)). Names, handles
  and pictures come from the GitHub identity, which the person can't edit through Supabase, so
  the name others see is the one GitHub has. Email sign-in is only on in the local stack, for
  tests.
- **The database decides who may do what.** Row-level security lets each person read only their
  own rows, their contacts and the people in a live call with them. No table can be written
  directly: every change is an RPC function that checks who is calling (`start_call`, `knock`,
  `answer_knock`, `ring`, `inbox`, …).
- **A call pass is the only way into a call.** The `call-pass` function asks the database
  (`pass_claims`) whether the signed-in person may join a call, and if so signs a LiveKit token
  for that one room, identity (`sharer-<account id>` or `guest-<account id>`) and name. The
  LiveKit API secret is a secret of that function and nowhere else. The app carries only the
  project URL and its public key.
- **The engine stays account-free.** `tc-engine` takes the pass from its environment
  (`TWINCURSOR_PASS`), checks that it fits the call, and joins. It never talks to Supabase. The
  terminal tools and benches still run against a local LiveKit with its development keys.
- **The server never grants control.** It decides who may join, and nothing else about a call.
  Control, trusted control included, is given only by the sharer's own app to its own engine
  ([ADR 0010](0010-trusted-control.md)). No database row, pass or message from the backend can
  put anyone in control.
- **Clients poll.** While signed in, the app calls `inbox` every 2 seconds. It is the host's
  heartbeat and how knocks and rings arrive. The app doesn't use Supabase Realtime.

## Consequences
- **What the backend knows about people:** each account's GitHub name, handle and picture URL,
  and its email address (Supabase Auth keeps it); who is whose contact; which calls each person
  hosted, knocked on, was let into, turned away from, rang or declined, and when; and the room
  names. Whoever runs the Supabase project can read all of it.
- **What it never sees:** the screen, the voice, or anyone's input. Those go between the apps
  through LiveKit. But LiveKit decrypts media to forward it, so whoever runs the LiveKit server
  can see and hear the call ([threat model](../threat-model.md)). End-to-end encryption is
  future work.
- **Self-hosting needs two services, not one binary:** a Supabase project (Supabase's cloud, or
  Supabase's own self-hosted stack) and a LiveKit server (LiveKit Cloud or `livekit-server`),
  plus a GitHub OAuth app. [Self-hosting](../self-hosting.md) walks through it.
- **A build is tied to one backend.** The project URL and public key are baked in at build time
  (`apps/desktop/scripts/package.sh`). A build with none says it isn't connected to a server.
- **A pass is good for 10 minutes to join, and nothing takes it back.** Ending a call, turning
  someone away or their leaving stops new passes, but doesn't disconnect anyone already in the
  LiveKit room: the host's app does that by ending its engine. A server-side kick is future work.
- **Ringing and knocking are as fast as the poll:** up to 2 seconds, longer if the app is
  backing off after network failures. A call whose host's app stops polling stops being live
  after 45 seconds.
- **The anon role can call nothing.** Every function needs a signed-in user, so the public key
  in the app lets nobody do anything until they sign in.
- **Anyone with a GitHub account can sign up** on a project that allows sign-ups. Restricting
  who may join a project is left to the person running it (Supabase's own settings).
- **Addendum: an invite list for starting calls.** Because every build carries the backend's
  public key, an open server lets any GitHub account start calls on the host's LiveKit server. A
  second migration adds an owner-only invite list keyed by GitHub handle and a
  `server_settings.invite_only` switch, off by default so applying it changes nothing. When it is
  on, `start_call` and `ring` refuse anyone not on the list (`not_invited`); joining, being let
  in, answering a ring and guest passes are unchanged, and a host taken off the list keeps a live
  call until it ends. The server decides; `inbox().me.can_host` only tells the app what to show.
  Sign-up itself stays open, and guests in an invited host's call still use minutes.
- **Not built yet, on purpose:** presence (who is online), teams, a Windows app with sign-in (the
  Windows viewer still joins by room code on a keyed server), and call links that open the app
  when clicked (people paste them).
