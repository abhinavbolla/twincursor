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

## Test builds

Test builds go to a few people by hand, with a server built in so they need no setup. They aren't
releases: nothing is tagged or published. Whoever has one can read the server's API secret out
of it (see [the threat model](threat-model.md)), so give them only to people you'd give the key
to, and rotate the key when the test round ends. Testers follow the
[testing guide](testing-guide.md); before handing a build out, go through the
[test plan](test-plan.md) yourself.

The server's settings live in an env file outside the repository, with the names in
[`.env.testbuild.example`](../.env.testbuild.example). Never commit it.

**Mac disk image.** On an Apple silicon Mac with Xcode 26:

```bash
apps/desktop/scripts/package.sh path/to/testbuild.env
```

It builds `tc-engine` with the server baked in, bundles and ad-hoc signs `TwinCursor.app`, and
writes `target/release/packages/TwinCursor-<version>-macos-arm64.dmg` (under `$CARGO_TARGET_DIR`
if set). It then mounts the image read-only and prints the signature check, the architectures,
the image's size and SHA-256, and what the bundled `tc-engine config` reports: check that
`source` is `build` and `url` is the server you meant. It never prints the secret and refuses to
run under `set -x`. Without an env file it builds for the local dev server and says so.

Afterwards, rebuild without the env file (`apps/desktop/scripts/package.sh`) so the binaries
left in `target` don't carry the secret.

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

## Not done yet

Builds aren't signed or notarized. That needs an Apple Developer ID, and it's required before the
first public release. Until then macOS will refuse to open a downloaded `tc-spike` until you clear
its quarantine flag, and Windows may show a SmartScreen warning.
