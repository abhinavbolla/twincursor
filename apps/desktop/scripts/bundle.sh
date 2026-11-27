#!/usr/bin/env bash
# Builds TwinCursor.app: the release engine (tc-engine) goes into Contents/MacOS next to the
# app, and the bundle is signed ad hoc (no Developer ID needed; it runs on the Mac that built it).
#
#   apps/desktop/scripts/bundle.sh        # from anywhere in the repo
#
# With TWINCURSOR_SIGN_IDENTITY set to a code signing identity already in a keychain (its name or
# SHA-1, as `security find-identity -v -p codesigning` lists it), tc-engine and then the app are
# signed with it instead, keeping their entitlements and hardened runtime;
# TWINCURSOR_SIGN_KEYCHAIN names the keychain to find it in. A fixed identity keeps the app's
# designated requirement the same from build to build, so macOS keeps a granted permission
# across updates; ad hoc, every build is a new app to it. This script never creates or changes a
# certificate, key or keychain.
#
# It builds with whatever TWINCURSOR_BUILD_* variables are in the environment. To make an app
# for people to install, use package.sh instead: it sets the backend from a file, clears every
# LiveKit variable first, and checks the result (docs/RELEASING.md).
#
# The result is in target/release/bundle/macos/TwinCursor.app.
set -euo pipefail

app_dir="$(cd "$(dirname "$0")/.." && pwd)"
root="$(cd "$app_dir/../.." && pwd)"
triple="$(rustc -vV | sed -n 's/^host: //p')"

target="${CARGO_TARGET_DIR:-$root/target}"

echo "==> building the engine (tc-engine, release)"
cargo build --release --locked --manifest-path "$root/Cargo.toml" -p twincursor-engine-cli

# Tauri's externalBin wants the binary named for the target it runs on.
mkdir -p "$app_dir/src-tauri/binaries"
cp "$target/release/tc-engine" "$app_dir/src-tauri/binaries/tc-engine-$triple"

echo "==> building and bundling the app"
cd "$app_dir"
[ -d node_modules ] || npm ci
# tauri.bundle.json adds the engine. It isn't in tauri.conf.json because Tauri's build script
# copies every externalBin at compile time, and plain `cargo build --workspace` shouldn't need it.
npx tauri build --bundles app --config src-tauri/tauri.bundle.json "$@"

bundle="$target/release/bundle/macos/TwinCursor.app"
echo "==> $bundle"
if [ -n "${TWINCURSOR_SIGN_IDENTITY:-}" ]; then
  echo "==> signing with the identity \"$TWINCURSOR_SIGN_IDENTITY\"${TWINCURSOR_SIGN_KEYCHAIN:+ from $TWINCURSOR_SIGN_KEYCHAIN}"
  sign=(codesign --force --sign "$TWINCURSOR_SIGN_IDENTITY")
  if [ -n "${TWINCURSOR_SIGN_KEYCHAIN:-}" ]; then
    sign+=(--keychain "$TWINCURSOR_SIGN_KEYCHAIN")
  fi
  # The engine first: the app's seal covers it. A fixed identifier, so its requirement is fixed too.
  "${sign[@]}" --identifier dev.twincursor.desktop.engine \
    --preserve-metadata=entitlements,flags,runtime "$bundle/Contents/MacOS/tc-engine"
  "${sign[@]}" --preserve-metadata=identifier,entitlements,flags,runtime "$bundle"
else
  echo "==> signed ad hoc: macOS treats every build as a new app, so permissions must be granted again (set TWINCURSOR_SIGN_IDENTITY for a fixed identity)"
fi
codesign --verify --deep --strict "$bundle" && echo "signature ok"
echo "designated requirement: $(codesign -d -r- "$bundle" 2>&1 | sed -n 's/^designated => //p')"
