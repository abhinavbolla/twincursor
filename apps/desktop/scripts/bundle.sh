#!/usr/bin/env bash
# Builds TwinCursor.app: the release helper (tc-spike) goes into Contents/MacOS next to the app,
# and the bundle is signed ad hoc (no Developer ID needed; it runs on the Mac that built it).
#
#   apps/desktop/scripts/bundle.sh        # from anywhere in the repo
#
# The result is in target/release/bundle/macos/TwinCursor.app.
set -euo pipefail

app_dir="$(cd "$(dirname "$0")/.." && pwd)"
root="$(cd "$app_dir/../.." && pwd)"
triple="$(rustc -vV | sed -n 's/^host: //p')"

echo "==> building the helper (tc-spike, release)"
cargo build --release --locked --manifest-path "$root/Cargo.toml" -p twincursor-media-spike

# Tauri's externalBin wants the binary named for the target it runs on.
mkdir -p "$app_dir/src-tauri/binaries"
cp "$root/target/release/tc-spike" "$app_dir/src-tauri/binaries/tc-spike-$triple"

echo "==> building and bundling the app"
cd "$app_dir"
[ -d node_modules ] || npm ci
# tauri.bundle.json adds the helper. It isn't in tauri.conf.json because Tauri's build script
# copies every externalBin at compile time, and plain `cargo build --workspace` shouldn't need it.
npx tauri build --bundles app --config src-tauri/tauri.bundle.json "$@"

bundle="$root/target/release/bundle/macos/TwinCursor.app"
echo "==> $bundle"
codesign --verify --deep --strict "$bundle" && echo "signature ok"
