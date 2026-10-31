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

## Not done yet

Builds aren't signed or notarized. That needs an Apple Developer ID, and it's required before the
first public release. Until then macOS will refuse to open a downloaded `tc-spike` until you clear
its quarantine flag, and Windows may show a SmartScreen warning.
