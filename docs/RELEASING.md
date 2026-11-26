# Releasing

Versions follow [Semantic Versioning](https://semver.org). The first tag waits until there's
something worth downloading.

1. On a branch, move the entries under `[Unreleased]` in `CHANGELOG.md` to a new
   `## [x.y.z] - YYYY-MM-DD` section, and merge it.
2. Tag `main` and push the tag:

   ```bash
   git tag vx.y.z
   git push origin vx.y.z
   ```

3. The [release workflow](../.github/workflows/release.yml) builds macOS and Windows archives, adds
   checksums and build provenance, and creates a **draft** release from the changelog.
4. Download the archives, try them, and publish the draft.

To check a download's provenance:

```bash
gh attestation verify tc-spike-vx.y.z-macos-arm64.tar.gz --repo abhinavbolla/twincursor
```

## Builds for people to install

A TwinCursor app is built for one backend: a Supabase project whose URL and public key are baked
in. People who install it sign in with GitHub on that backend, and each call gets a call pass
from its `call-pass` function. The app carries no LiveKit key or secret, and must not: anyone
who has an app can read everything in it. Setting up a backend is in
[self-hosting](self-hosting.md). Before handing a build out, go through the
[test plan](test-plan.md); the people you give it to follow the
[testing guide](testing-guide.md).

The release workflow doesn't build the app yet. Make the disk image on an Apple silicon Mac with
Xcode 26, and attach it to the draft release by hand.

**The backend file** holds the two public values, one `NAME=value` per line, as in
[`apps/desktop/backend.example.conf`](../apps/desktop/backend.example.conf). Keep it outside the
repository, for example at `~/.config/twincursor/backend.conf`:

```
TWINCURSOR_BUILD_SUPABASE_URL=https://YOUR-PROJECT.supabase.co
TWINCURSOR_BUILD_SUPABASE_KEY=sb_publishable_…
```

`SUPABASE_URL`, and `SUPABASE_PUBLISHABLE_KEY` or `SUPABASE_ANON_KEY`, work as names too. The
file is read, not run.

**Build the disk image:**

```bash
apps/desktop/scripts/package.sh ~/.config/twincursor/backend.conf
```

It writes `target/release/packages/TwinCursor-<version>-macos-arm64.dmg` (under
`$CARGO_TARGET_DIR` if set). Before building, it:

- refuses a file that sets anything but the URL and the public key: a LiveKit variable, a
  secret key (`sb_secret_…`), the `service_role` key, or a name it doesn't know;
- refuses a URL that isn't `https://` (`http://` only for `127.0.0.1` or `localhost`), and
  the example's placeholders;
- clears `TWINCURSOR_BUILD_SERVER_URL`, `_API_KEY`, `_API_SECRET`, `LIVEKIT_URL`,
  `LIVEKIT_API_KEY`, `LIVEKIT_API_SECRET` and the runtime `TWINCURSOR_SERVER_*` and
  `TWINCURSOR_SUPABASE_*` variables from the build's environment, whatever your shell has set.

After building, it mounts the image read-only and checks:

- the signature (`codesign --verify --deep --strict`) and that both binaries are arm64 only;
- that the bundled `tc-engine config` reports `source` `dev` and no secret: the engine has no
  LiveKit server of its own;
- that no LiveKit value your shell had set (8 characters or longer, other than the local dev
  server's well-known ones) appears in either binary;
- that the app's binary contains the project URL and the public key, and `tc-engine` doesn't
  contain the URL.

Any failed check deletes the image and exits with status 1. It prints the project's host and the
first 8 characters of the key, never the rest, and refuses to run under `set -x`. The last lines
are the image's size and SHA-256 and "Built for the backend at YOUR-PROJECT.supabase.co. It
carries no LiveKit key or secret; …".

Without a file, `package.sh` builds an app with no backend, which says it isn't connected to a
server. That is the right build for trying the app's packaging, not for people.

**Which example file is which:**

| File | For | Holds |
|---|---|---|
| [`apps/desktop/backend.example.conf`](../apps/desktop/backend.example.conf) | `package.sh BACKEND_FILE`: the app with accounts | Supabase project URL and public key: public |
| `.env.testbuild.example` | `package.sh --legacy-keys`, `packaging/windows/assemble.sh` | A LiveKit server's URL, API key and API secret: secret |

## Builds with a LiveKit key (legacy)

The Windows viewer has no accounts: it joins a room by code on a LiveKit server whose key and
secret it has. For a test round with it, a Mac sharer runs `tc-engine` from the terminal against
the same server. `package.sh` can still bake such a server into `tc-engine`:

```bash
apps/desktop/scripts/package.sh --legacy-keys path/to/testbuild.env
```

The env file sets `TWINCURSOR_BUILD_SERVER_URL`, `TWINCURSOR_BUILD_API_KEY` and
`TWINCURSOR_BUILD_API_SECRET` (or `LIVEKIT_URL`, `LIVEKIT_API_KEY`, `LIVEKIT_API_SECRET`), as in
`.env.testbuild.example`. Never commit it. Such a build has no backend (a file with both is
refused), so its menu bar app says it isn't connected to a server. Whoever has it can read the
server's API secret out of it (see [the threat model](threat-model.md)): give it only to people
you'd give the key to, and rotate the key when the round ends. Afterwards, run `package.sh`
again without `--legacy-keys` so the binaries left in `target` don't carry the secret.

**Windows viewer.** The [release workflow](../.github/workflows/release.yml) builds
`TwinCursor-viewer-<ref>-windows-x64.zip` (`tc-engine.exe`, `Join TwinCursor.cmd`, a README and
`server.txt.example`) on every tag and on pull requests that change it or `packaging/`. It holds
no server settings. Download it from the run's artifacts (`windows-x64`), then add a `server.txt`
on a Mac:

```bash
packaging/windows/assemble.sh TwinCursor-viewer-<ref>-windows-x64.zip path/to/testbuild.env
```

That writes `TwinCursor-viewer-<ref>-windows-x64-tester.zip` next to the input and prints its
contents, size and SHA-256, never the values. The env file can use either the build names or the
runtime ones (`TWINCURSOR_SERVER_URL`, `TWINCURSOR_API_KEY`, `TWINCURSOR_API_SECRET`).

## The app icon

`apps/desktop/src-tauri/icons/icon.svg` is the source. After changing it, regenerate the files
the bundle uses and copy them over the ones in that folder:

```bash
cd apps/desktop && npx tauri icon src-tauri/icons/icon.svg -o /tmp/twincursor-icons
```

The bundle needs `32x32.png`, `128x128.png`, `128x128@2x.png` and `icon.icns`.

## Not done yet

Builds aren't signed or notarized. That needs an Apple Developer ID, and it's required before the
first public release. Until then macOS stops the app the first time it opens (**Open Anyway** in
System Settings > Privacy & Security lets it through), refuses a downloaded `tc-spike` until you
clear its quarantine flag, and Windows may show a SmartScreen warning.

The release workflow builds the terminal tools and the Windows viewer, not the Mac app. Building
the app there would take the backend's two public values from repository variables (not
secrets), and make an app with no backend when they're unset; until that exists, the disk image
is made by hand as above.
