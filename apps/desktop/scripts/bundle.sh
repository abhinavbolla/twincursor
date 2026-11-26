#!/usr/bin/env bash
# Builds TwinCursor.app: the release engine (tc-engine) goes into Contents/MacOS next to the
# app, and the bundle is signed ad hoc (no Developer ID needed; it runs on the Mac that built it).
#
#   apps/desktop/scripts/bundle.sh        # from anywhere in the repo
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
codesign --verify --deep --strict "$bundle" && echo "signature ok"
