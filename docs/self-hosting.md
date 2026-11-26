# Run your own TwinCursor

TwinCursor needs a backend for accounts and a media server for calls. This guide sets up both,
builds the Mac app pointed at them, and checks a call between two people. It takes about an hour
the first time.

The pieces, and what each one does:

- **A Supabase project** holds accounts, contacts and calls, and hands out *call passes*:
  LiveKit tokens good for one person, one call and 10 minutes to join. Everything it runs is in
  [`supabase/`](../supabase/README.md).
- **A LiveKit server** relays the screen, the voice and remote-control input between the two
  apps.
- **A GitHub OAuth app** lets people sign in with GitHub.
- **The Mac app**, built with your Supabase project's URL and public key inside.

Why it is built this way is in [ADR 0011](adr/0011-accounts-and-call-passes.md).

## 1. What you need

- **A Supabase project.** The free tier is enough to try it. Supabase's cloud is the simplest;
  their [self-hosted stack](https://supabase.com/docs/guides/self-hosting) also works if it can
  run Edge Functions, but this guide describes the cloud dashboard.
- **A LiveKit server.** [LiveKit Cloud](https://cloud.livekit.io)'s free tier, or your own
  [`livekit-server`](https://docs.livekit.io/home/self-hosting/deployment/) with TLS. You need its
  URL (`wss://…`), an API key and its API secret.
- **A GitHub account,** to create the OAuth app.
- **An Apple silicon Mac with [Xcode 26](https://developer.apple.com/xcode/),**
  [Rust](https://rustup.rs) 1.90 or newer and [Node](https://nodejs.org) 20.19 or newer, to build
  the app. The first build downloads a prebuilt WebRTC library of about 277 MB.
- **The [Supabase CLI](https://supabase.com/docs/guides/local-development/cli/getting-started)**
  if you deploy from your own machine (section 2, option B).

Fork or clone the repository:

```bash
git clone https://github.com/abhinavbolla/twincursor
cd twincursor
```

## 2. The backend

The backend is one database migration
(`supabase/migrations/20261126120000_accounts_and_calls.sql`: tables, access rules and the
functions the app calls) and one Edge Function (`supabase/functions/call-pass`). Deploy them one
of two ways.

### Option A: Supabase's GitHub integration

This deploys from your fork every time you push to its production branch.

1. Push the repository to a GitHub repository of your own.
2. In the Supabase dashboard, open **Project Settings > Integrations**. Under **GitHub
   Integration**, click **Authorize GitHub** and allow it on GitHub's page.
3. Choose your repository, set **Working directory** to `.` (the `supabase/` folder is at the
   repository's root), turn on **Deploy to production**, and click **Enable integration**.
4. Push to the production branch (usually `main`). Supabase applies the migrations and deploys
   the Edge Functions that `supabase/config.toml` declares, which is `call-pass`.

The integration doesn't apply the Auth settings in `config.toml`, so section 3 is done by hand
either way. See Supabase's
[GitHub integration](https://supabase.com/docs/guides/deployment/branching/github-integration)
guide for what it deploys.

### Option B: the Supabase CLI

From the repository's root, with your project's reference (the part before `.supabase.co` in
its URL):

```bash
supabase login
supabase link --project-ref YOUR-PROJECT
supabase db push
supabase functions deploy call-pass
```

`db push` applies the migrations in `supabase/migrations`; `link` and `db push` may ask for the
database password you chose when you created the project. `functions deploy` takes `verify_jwt = true` from `supabase/config.toml`, so only
requests with a signed-in user's token reach the function.

### The function's secrets

`call-pass` needs three secrets: the LiveKit server it signs passes for. These are the only
place the LiveKit secret goes. Put them in a file outside the repository, for example
`~/.config/twincursor/livekit.env`:

```
LIVEKIT_URL=wss://YOUR-LIVEKIT-HOST
LIVEKIT_API_KEY=YOUR-API-KEY
LIVEKIT_API_SECRET=YOUR-API-SECRET
```

and set them on the project:

```bash
supabase secrets set --env-file ~/.config/twincursor/livekit.env --project-ref YOUR-PROJECT
```

Or add them one by one in the dashboard's Edge Functions secrets. Supabase gives the function
`SUPABASE_URL` and `SUPABASE_ANON_KEY` itself.

On LiveKit Cloud, the project's settings create an API key and secret and show the project's
URL ([keys and tokens](https://docs.livekit.io/home/cloud/keys-and-tokens/)). Without the
three secrets the function answers `500` and its log says which are missing.

## 3. Sign-in with GitHub

1. **Find your callback URL.** In the Supabase dashboard, open **Authentication**, then **Sign
   In / Providers** under Configuration, and expand **GitHub**. The **Callback URL** is
   `https://YOUR-PROJECT.supabase.co/auth/v1/callback`.
2. **Create a GitHub OAuth app** at GitHub's
   [Developer settings > OAuth Apps](https://github.com/settings/developers) with **New OAuth
   App** (or **Register a new application**):
   - **Application name**: what people see when they approve it, such as "TwinCursor".
   - **Homepage URL**: any page about your setup, such as your fork.
   - **Authorization callback URL**: the callback URL from step 1.
   - Leave **Enable Device Flow** off.

   Register it, copy the **Client ID**, then **Generate a new client secret** and copy it.
3. **Turn on the GitHub provider.** Back in **Sign In / Providers > GitHub**, turn on **GitHub
   Enabled**, paste the client ID and secret, and save.
4. **Allow the app's redirect.** Open **Authentication > URL Configuration** and add
   `http://127.0.0.1:53682/callback` to **Redirect URLs**. The app finishes sign-in on that
   address on the person's own Mac, and Supabase refuses to send a browser anywhere not on the
   list.
5. **Turn email sign-in off.** In **Sign In / Providers**, turn the **Email** provider off.
   TwinCursor takes each person's name, handle and picture from their GitHub identity, which
   they can't change through Supabase. An email account would have no GitHub identity, and its
   name would come from data the person can rewrite, so anyone could call themselves anything.
   The name is what the sharer reads before letting someone in and giving control.

Supabase's guide to [GitHub sign-in](https://supabase.com/docs/guides/auth/social-login/auth-github)
has screenshots of these pages.

## 4. Build the app pointed at your backend

The app needs two values from your Supabase project:

- the **project URL**, `https://YOUR-PROJECT.supabase.co`;
- its **public key**: the publishable key (`sb_publishable_…`) or the legacy `anon` key, both
  under **Settings > API Keys**.

Both are public by design. Every copy of the app carries them, and anyone can read them out of
it. They let an app find your project and ask to sign in, nothing more: a signed-out client can
call none of TwinCursor's functions, row-level security lets each signed-in person read only
their own rows and the people they share a call with, no table can be written directly, and
only the `call-pass` function holds the LiveKit secret. Never build in the `service_role` key, a
secret key (`sb_secret_…`) or any LiveKit value.

Copy the example file outside the repository and fill it in:

```bash
mkdir -p ~/.config/twincursor
cp apps/desktop/backend.example.conf ~/.config/twincursor/backend.conf
```

```
TWINCURSOR_BUILD_SUPABASE_URL=https://YOUR-PROJECT.supabase.co
TWINCURSOR_BUILD_SUPABASE_KEY=sb_publishable_…
```

Then build the disk image:

```bash
apps/desktop/scripts/package.sh ~/.config/twincursor/backend.conf
```

It builds the engine and the app, bundles and ad-hoc signs `TwinCursor.app`, and writes
`target/release/packages/TwinCursor-<version>-macos-arm64.dmg`. It refuses a file with anything
but those two values in it, clears every LiveKit variable from the build, and fails if the built
app carries a LiveKit key of its own. The last lines name your project's host and say the build
carries no LiveKit key or secret. [Releasing](RELEASING.md#builds-for-people-to-install) has the
details.

**Open it on a Mac.** The build is signed ad hoc, not by Apple, so macOS stops it the first time.
Drag TwinCursor to Applications and open it; when macOS says it can't verify the app, click
**Done**. Then open **System Settings > Privacy & Security**, and under **Security** click
**Open Anyway** next to the message about TwinCursor, and confirm. You do this once per Mac. The
[testing guide](testing-guide.md#install-it) has the same steps for the people you send it to.

A build made without a backend file opens, but its panel says "This build isn't connected to a
server."

## 5. Try it

You need two people with GitHub accounts, on two Macs, each with the app you built.

1. **Both:** click the TwinCursor icon in the menu bar, then **Sign in with GitHub**. The browser
   asks you to approve your OAuth app; then it says "You're signed in. You can close this tab."
   and the panel shows your name.
2. **Sharer:** click **Share my screen**. The first time, macOS asks for Screen Recording: allow
   TwinCursor in System Settings, then click **Share my screen** again. macOS also asks for the
   microphone when the call starts. The call link is now on your clipboard. Send it to the
   other person.
3. **Guest:** copy the link, then click **Paste link** in the panel. The bar says you're waiting
   to be let in.
4. **Sharer:** the call bar shows the guest knocking. After a second, **Let in** turns on; click
   it. The guest sees the sharer's screen in its own window, and both can talk.

Each of you now lists the other under **People**, so next time you can ring them with **Call**.
The [test plan](test-plan.md) goes through everything else.

If something fails:

- **Sign-in doesn't come back to the app**: check the redirect URL in step 4 of section 3, and
  that the OAuth app's callback is your project's.
- **"Can't reach the server."**: check the project URL in your backend file and rebuild.
- **Sharing or joining fails as it starts**: check the function's three secrets, and its logs
  in the dashboard.

## 6. Running the checks locally

You don't need a hosted project to work on the backend or the app. `supabase start` runs the
whole backend in Docker, `livekit-server --dev` runs LiveKit, and the benches drive both:
[`supabase/README.md`](../supabase/README.md) has the database and function tests, and the
Commands section of [`CLAUDE.md`](../CLAUDE.md) lists `bench/account-e2e.sh` and
`bench/app-accounts-e2e.sh`.

## 7. What you are responsible for

Running a backend means holding other people's data and keys.

- **The data.** Your Supabase project holds each person's GitHub name, handle, picture URL and
  email address, who is whose contact, and when each call happened and who was in it
  ([ADR 0011](adr/0011-accounts-and-call-passes.md#consequences)). It never receives the screen,
  the voice or anyone's input.
- **The media.** Your LiveKit server relays every call's screen, voice and keystrokes, and can
  read them: LiveKit decrypts media to forward it. Whoever can read that server's memory or
  traffic after TLS can see and hear calls ([threat model](threat-model.md#known-limits)).
- **The LiveKit secret.** Keep it only in the function's secrets. Anyone who has it can mint a
  token for any room on your LiveKit server. If it leaks, make a new key on the LiveKit server,
  set the new values on the function, and delete the old key.
- **Who can sign in.** Anyone with a GitHub account can create an account on your project: there
  is no list of allowed GitHub accounts in TwinCursor. An account alone shows them nothing of
  anyone else's: they can knock only with a link, and ring only people who have them as a
  contact. To close the door, turn off **Allow new users to sign up** in Supabase's Auth settings
  ([general configuration](https://supabase.com/docs/guides/auth/general-configuration)) once
  your people have signed in; people who already have an account can still sign in. Turn it on
  again while someone new signs in for the first time.
- **Your OAuth app's secret** lives in Supabase's GitHub provider settings. Regenerate it on
  GitHub if it leaks.
- **Updates.** A new version of TwinCursor may come with a new migration. Apply it (by pushing
  with option A, or `supabase db push`) before handing out the new app.
