#!/usr/bin/env bash
# Builds a disk image testers can open: TwinCursor.app (ad hoc signed, tc-engine inside) next to
# a link to /Applications.
#
#   apps/desktop/scripts/package.sh                      # for the local dev server
#   apps/desktop/scripts/package.sh path/to/testbuild.env  # with that server built in
#
# The env file sets TWINCURSOR_BUILD_SERVER_URL, TWINCURSOR_BUILD_API_KEY and
# TWINCURSOR_BUILD_API_SECRET (see .env.testbuild.example), or LiveKit's own names for them:
# LIVEKIT_URL, LIVEKIT_API_KEY and LIVEKIT_API_SECRET. Its values never reach the terminal,
# but anyone with the disk image can read the secret out of it: give it only to people you'd
# give the server's key to (docs/threat-model.md).
#
# The result is target/release/packages/TwinCursor-<version>-macos-arm64.dmg (or under
# $CARGO_TARGET_DIR). It builds with hdiutil only: no Finder scripting, no sudo.
set -euo pipefail

# Tracing would print the secret with every command that touches it.
case "$-" in
  *x*)
    echo "package.sh: refusing to run with 'set -x' (it would print the server's secret)" >&2
    exit 2
    ;;
esac

if [ "$#" -gt 1 ]; then
  echo "usage: $0 [testbuild.env]" >&2
  exit 2
fi

app_dir="$(cd "$(dirname "$0")/.." && pwd)"
root="$(cd "$app_dir/../.." && pwd)"
target="${CARGO_TARGET_DIR:-$root/target}"

build_vars=(TWINCURSOR_BUILD_SERVER_URL TWINCURSOR_BUILD_API_KEY TWINCURSOR_BUILD_API_SECRET)
livekit_vars=(LIVEKIT_URL LIVEKIT_API_KEY LIVEKIT_API_SECRET)
runtime_vars=(TWINCURSOR_SERVER_URL TWINCURSOR_API_KEY TWINCURSOR_API_SECRET)

# Start from nothing, so only the env file decides what is built in.
unset "${build_vars[@]}" "${livekit_vars[@]}"
if [ "$#" -eq 1 ]; then
  env_file="$1"
  if [ ! -f "$env_file" ] || [ ! -r "$env_file" ]; then
    echo "package.sh: can't read $env_file" >&2
    exit 2
  fi
  set -a
  # shellcheck disable=SC1090 # the file is the caller's choice
  . "$env_file"
  set +a
  for i in 0 1 2; do
    var="${build_vars[$i]}"
    other="${livekit_vars[$i]}"
    if [ -z "${!var:-}" ] && [ -n "${!other:-}" ]; then
      export "$var=${!other}"
    fi
    if [ -z "${!var:-}" ]; then
      echo "package.sh: $env_file sets neither $var nor $other" >&2
      exit 2
    fi
  done
fi

if [ "$(uname -s)" != Darwin ] || [ "$(uname -m)" != arm64 ]; then
  echo "package.sh: builds on an Apple silicon Mac only" >&2
  exit 2
fi

"$app_dir/scripts/bundle.sh"

app="$target/release/bundle/macos/TwinCursor.app"
plist="$app/Contents/Info.plist"
version="$(plutil -extract CFBundleShortVersionString raw -o - "$plist")"
executable="$(plutil -extract CFBundleExecutable raw -o - "$plist")"

out_dir="$target/release/packages"
dmg="$out_dir/TwinCursor-$version-macos-arm64.dmg"
staging="$out_dir/staging"
mount_point=""

cleanup() {
  if [ -n "$mount_point" ]; then
    hdiutil detach -quiet "$mount_point" || hdiutil detach -quiet -force "$mount_point" || true
    rmdir "$mount_point" 2>/dev/null || true
  fi
  rm -rf "$staging"
}
trap cleanup EXIT

echo "==> making $dmg"
rm -rf "$staging"
mkdir -p "$staging"
# ditto keeps the bundle's signature, symlinks and extended attributes intact.
ditto "$app" "$staging/TwinCursor.app"
ln -s /Applications "$staging/Applications"
hdiutil create -quiet -ov -volname TwinCursor -srcfolder "$staging" -format UDZO \
  -imagekey zlib-level=9 "$dmg"

echo "==> checking it"
hdiutil verify -quiet "$dmg"
echo "image checksum: ok"

mount_point="$(mktemp -d "${TMPDIR:-/tmp}/twincursor-dmg.XXXXXX")"
hdiutil attach -quiet -nobrowse -readonly -noautoopen -mountpoint "$mount_point" "$dmg"
mounted="$mount_point/TwinCursor.app"
echo "contents: $(cd "$mount_point" && ls -A | tr '\n' ' ')"

codesign --verify --deep --strict "$mounted"
echo "signature: ok ($(codesign -dv "$mounted" 2>&1 | sed -n 's/^Signature=//p'))"

for bin in "$executable" tc-engine; do
  archs="$(lipo -archs "$mounted/Contents/MacOS/$bin")"
  echo "architecture of $bin: $archs"
  if [ "$archs" != arm64 ]; then
    echo "package.sh: expected arm64 only" >&2
    exit 1
  fi
done

# What the bundled engine would use, never the secret. The runtime variables would override
# what's built in, so leave them out.
config="$(env -u "${runtime_vars[0]}" -u "${runtime_vars[1]}" -u "${runtime_vars[2]}" \
  "$mounted/Contents/MacOS/tc-engine" config 2>/dev/null)"
echo "server: $config"

hdiutil detach -quiet "$mount_point"
rmdir "$mount_point"
mount_point=""

size_bytes="$(stat -f %z "$dmg")"
echo "size: $((size_bytes / 1024 / 1024)) MB ($size_bytes bytes)"
echo "sha256: $(shasum -a 256 "$dmg" | cut -d' ' -f1)"
echo "==> $dmg"

case "$config" in
  *'"source":"build"'*)
    echo "Built for the server in $1. Anyone with this image can read its key: share it only with people you trust with that server."
    ;;
  *)
    echo "Built for the local dev server (no env file given): it only works with livekit-server --dev on the same Mac."
    ;;
esac
