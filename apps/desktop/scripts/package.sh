#!/usr/bin/env bash
# Builds a disk image people can open: TwinCursor.app (tc-engine inside; signed ad hoc, or with
# TWINCURSOR_SIGN_IDENTITY as bundle.sh says) next to a link to /Applications.
#
#   apps/desktop/scripts/package.sh                              # no backend
#   apps/desktop/scripts/package.sh ~/.config/twincursor/backend.conf   # pointed at your backend
#   apps/desktop/scripts/package.sh --legacy-keys path/to/testbuild.env # LiveKit key in the build
#
# The backend file names your Supabase project's URL and its public key, one NAME=value per line
# (see apps/desktop/backend.example.conf):
#
#   TWINCURSOR_BUILD_SUPABASE_URL=https://YOUR-PROJECT.supabase.co   (or SUPABASE_URL)
#   TWINCURSOR_BUILD_SUPABASE_KEY=sb_publishable_...                 (or SUPABASE_PUBLISHABLE_KEY,
#                                                                     or SUPABASE_ANON_KEY)
#
# Both values are public by design. An app built with them never carries a LiveKit key or
# secret: the backend's call-pass function signs a short-lived pass for each call. So this script
# clears every LiveKit and server variable from the build's environment, refuses a backend file
# that names one (or a Supabase secret key), and after building checks that the bundled engine
# has no server key of its own and that the binaries don't contain any LiveKit value your shell
# had set.
#
# Without a file, the app has no backend: it says it isn't connected to a server.
#
# --legacy-keys FILE is the old test-build mode, kept for the Windows viewer's test server: it
# bakes a LiveKit server's URL, API key and API secret into tc-engine (TWINCURSOR_BUILD_SERVER_URL,
# _API_KEY, _API_SECRET, or LIVEKIT_URL, LIVEKIT_API_KEY, LIVEKIT_API_SECRET). Anyone with that
# image can read the secret out of it. It never has a backend, and can't be combined with one.
#
# Values never reach the terminal, except the backend's host and the first 8 characters of its
# public key. The result is target/release/packages/TwinCursor-<version>-macos-arm64.dmg (or under
# $CARGO_TARGET_DIR). It builds with hdiutil only: no Finder scripting, no sudo.
set -euo pipefail

# Tracing would print every value with every command that touches it.
case "$-" in
  *x*)
    echo "package.sh: refusing to run with 'set -x' (it would print the values it builds in)" >&2
    exit 2
    ;;
esac

usage() {
  echo "usage: $0 [BACKEND_FILE]" >&2
  echo "       $0 --legacy-keys ENV_FILE    (LiveKit key and secret in the build; no backend)" >&2
  exit 2
}

mode=none
file=""
case "$#" in
  0) ;;
  1)
    case "$1" in
      -h | --help) usage ;;
      --legacy-keys) usage ;;
      -*)
        echo "package.sh: unknown option $1" >&2
        usage
        ;;
      *)
        mode=backend
        file="$1"
        ;;
    esac
    ;;
  2)
    [ "$1" = --legacy-keys ] || usage
    mode=legacy
    file="$2"
    ;;
  *) usage ;;
esac

if [ "$(uname -s)" != Darwin ] || [ "$(uname -m)" != arm64 ]; then
  echo "package.sh: builds on an Apple silicon Mac only" >&2
  exit 2
fi

if [ -n "$file" ] && { [ ! -f "$file" ] || [ ! -r "$file" ]; }; then
  echo "package.sh: can't read $file" >&2
  exit 2
fi

app_dir="$(cd "$(dirname "$0")/.." && pwd)"
root="$(cd "$app_dir/../.." && pwd)"
target="${CARGO_TARGET_DIR:-$root/target}"

server_build_vars=(TWINCURSOR_BUILD_SERVER_URL TWINCURSOR_BUILD_API_KEY TWINCURSOR_BUILD_API_SECRET)
livekit_vars=(LIVEKIT_URL LIVEKIT_API_KEY LIVEKIT_API_SECRET)
server_runtime_vars=(TWINCURSOR_SERVER_URL TWINCURSOR_API_KEY TWINCURSOR_API_SECRET)
backend_build_vars=(TWINCURSOR_BUILD_SUPABASE_URL TWINCURSOR_BUILD_SUPABASE_KEY)
backend_runtime_vars=(TWINCURSOR_SUPABASE_URL TWINCURSOR_SUPABASE_KEY)

# LiveKit values the caller's shell has set. An app with a backend must not contain any of them;
# they are checked against the built binaries below, never printed.
leaked_candidates=()
for var in "${server_build_vars[@]}" "${livekit_vars[@]}" "${server_runtime_vars[@]}"; do
  value="${!var:-}"
  value="${value#"${value%%[![:space:]]*}"}"
  value="${value%"${value##*[![:space:]]}"}"
  # The dev server's well-known values are in every engine; short values would match by chance.
  case "$value" in
    "" | ws://localhost:7880 | devkey | secret) continue ;;
  esac
  if [ "${#value}" -ge 8 ]; then
    leaked_candidates+=("$value")
  fi
done

# Start from nothing, so only the file decides what is built in.
unset "${server_build_vars[@]}" "${livekit_vars[@]}" "${server_runtime_vars[@]}" \
  "${backend_build_vars[@]}" "${backend_runtime_vars[@]}"

backend_url=""
backend_key=""

# Reads NAME=value lines without running the file: blank lines and # comments are skipped, an
# `export ` prefix and one pair of surrounding quotes are dropped.
read_backend_file() {
  local line name value lineno=0
  while IFS= read -r line || [ -n "$line" ]; do
    lineno=$((lineno + 1))
    line="${line%$'\r'}"
    case "$line" in
      "" | "#"*) continue ;;
    esac
    line="${line#export }"
    if [[ ! "$line" =~ ^[[:space:]]*([A-Za-z_][A-Za-z0-9_]*)=(.*)$ ]]; then
      echo "package.sh: $file line $lineno isn't NAME=value" >&2
      exit 2
    fi
    name="${BASH_REMATCH[1]}"
    value="${BASH_REMATCH[2]}"
    value="${value%"${value##*[![:space:]]}"}"
    case "$value" in
      \"*\") value="${value:1:${#value}-2}" ;;
      \'*\') value="${value:1:${#value}-2}" ;;
    esac
    case "$name" in
      TWINCURSOR_BUILD_SUPABASE_URL | SUPABASE_URL) backend_url="$value" ;;
      TWINCURSOR_BUILD_SUPABASE_KEY | SUPABASE_PUBLISHABLE_KEY | SUPABASE_ANON_KEY) backend_key="$value" ;;
      LIVEKIT_* | TWINCURSOR_BUILD_SERVER_* | TWINCURSOR_BUILD_API_* | TWINCURSOR_SERVER_* | TWINCURSOR_API_*)
        echo "package.sh: $file sets $name. An app with a backend never carries a LiveKit server's" >&2
        echo "  key or secret: those go only in the call-pass function's secrets. Remove it." >&2
        exit 2
        ;;
      *SECRET* | *SERVICE_ROLE*)
        echo "package.sh: $file sets $name. Only the project URL and its public key go in an app." >&2
        exit 2
        ;;
      *)
        echo "package.sh: $file sets $name, which this script doesn't use" >&2
        exit 2
        ;;
    esac
  done <"$file"
}

# Refuses a key that isn't the public one: a secret key (sb_secret_...) or a legacy JWT whose
# role is service_role.
check_public_key() {
  local key="$1" payload json
  case "$key" in
    sb_secret_*)
      echo "package.sh: that's a secret key (sb_secret_...). Use the publishable key." >&2
      exit 2
      ;;
    sb_publishable_*) return 0 ;;
  esac
  if [[ "$key" =~ ^[A-Za-z0-9_-]+\.([A-Za-z0-9_-]+)\.[A-Za-z0-9_-]+$ ]]; then
    payload="$(printf '%s' "${BASH_REMATCH[1]}" | tr '_-' '/+')"
    while [ $((${#payload} % 4)) -ne 0 ]; do payload="$payload="; done
    json="$(printf '%s' "$payload" | base64 -D 2>/dev/null || true)"
    case "$json" in
      *'"role":"anon"'* | *'"role": "anon"'*) return 0 ;;
      *service_role*)
        echo "package.sh: that's the service_role key. Use the public (anon or publishable) key." >&2
        exit 2
        ;;
    esac
    echo "package.sh: the key is a JWT whose role isn't anon. Use the public (anon or publishable) key." >&2
    exit 2
  fi
  echo "package.sh: the key doesn't look like a Supabase public key (sb_publishable_... or the anon JWT)" >&2
  exit 2
}

case "$mode" in
  backend)
    read_backend_file
    if [ -z "$backend_url" ] || [ -z "$backend_key" ]; then
      echo "package.sh: $file needs both TWINCURSOR_BUILD_SUPABASE_URL and TWINCURSOR_BUILD_SUPABASE_KEY" >&2
      exit 2
    fi
    backend_url="${backend_url%/}"
    case "$backend_url" in
      https://?* | http://127.0.0.1:* | http://localhost:*) ;;
      *)
        echo "package.sh: the backend URL must start with https:// (http:// only for 127.0.0.1 or localhost)" >&2
        exit 2
        ;;
    esac
    case "$backend_url$backend_key" in
      *YOUR-PROJECT* | *REPLACE_ME*)
        echo "package.sh: $file still has the example's placeholders" >&2
        exit 2
        ;;
    esac
    check_public_key "$backend_key"
    export TWINCURSOR_BUILD_SUPABASE_URL="$backend_url"
    export TWINCURSOR_BUILD_SUPABASE_KEY="$backend_key"
    backend_host="${backend_url#*://}"
    backend_host="${backend_host%%/*}"
    echo "==> backend: $backend_host, public key ${backend_key:0:8}…"
    ;;
  legacy)
    set -a
    # shellcheck disable=SC1090 # the file is the caller's choice
    . "$file"
    set +a
    for i in 0 1 2; do
      var="${server_build_vars[$i]}"
      other="${livekit_vars[$i]}"
      if [ -z "${!var:-}" ] && [ -n "${!other:-}" ]; then
        export "$var=${!other}"
      fi
      if [ -z "${!var:-}" ]; then
        echo "package.sh: $file sets neither $var nor $other" >&2
        exit 2
      fi
    done
    for var in "${backend_build_vars[@]}" SUPABASE_URL SUPABASE_ANON_KEY SUPABASE_PUBLISHABLE_KEY; do
      if [ -n "${!var:-}" ]; then
        echo "package.sh: $file sets $var. --legacy-keys builds have no backend; use a backend file instead." >&2
        exit 2
      fi
    done
    unset "${livekit_vars[@]}" "${server_runtime_vars[@]}" "${backend_build_vars[@]}" "${backend_runtime_vars[@]}"
    echo "==> LEGACY: baking a LiveKit server's key and secret into tc-engine (no backend)"
    ;;
  none)
    echo "==> no backend file: the app will say it isn't connected to a server"
    ;;
esac

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

fail() {
  echo "package.sh: $*" >&2
  # Don't leave an image that failed its checks where it could be handed out.
  rm -f "$dmg"
  exit 1
}

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
app_bin="$mounted/Contents/MacOS/$executable"
engine_bin="$mounted/Contents/MacOS/tc-engine"
echo "contents: $(cd "$mount_point" && ls -A | tr '\n' ' ')"

codesign --verify --deep --strict "$mounted"
echo "signature: ok ($(codesign -dv "$mounted" 2>&1 | sed -n 's/^Signature=//p'))"

for bin in "$app_bin" "$engine_bin"; do
  archs="$(lipo -archs "$bin")"
  echo "architecture of $(basename "$bin"): $archs"
  [ "$archs" = arm64 ] || fail "expected arm64 only"
done

# Whether a binary contains a value, without the value on any command line.
contains() {
  LC_ALL=C grep -a -q -F -f <(printf '%s\n' "$2") "$1"
}

# What the bundled engine would use, never the secret. The runtime variables would override
# what's built in, so leave them out.
config="$(env -u "${server_runtime_vars[0]}" -u "${server_runtime_vars[1]}" -u "${server_runtime_vars[2]}" \
  "$engine_bin" config 2>/dev/null)"
case "$config" in
  *'"source":"dev"'*) engine_source=dev ;;
  *'"source":"build"'*) engine_source=build ;;
  *) fail "tc-engine config said something unexpected" ;;
esac
case "$config" in
  *'"has_secret":false'*) engine_secret=no ;;
  *) engine_secret=yes ;;
esac

if [ "$mode" = legacy ]; then
  echo "engine server: $config"
  [ "$engine_source" = build ] || fail "--legacy-keys build, but tc-engine has no server built in"
else
  # An app for a backend, or with none, must carry no server key of its own.
  if [ "$engine_source" != dev ] || [ "$engine_secret" != no ]; then
    fail "tc-engine carries a server key (source $engine_source). This image must not be handed out."
  fi
  echo "engine server: dev only (no LiveKit key built in)"
  for value in ${leaked_candidates[@]+"${leaked_candidates[@]}"}; do
    if contains "$app_bin" "$value" || contains "$engine_bin" "$value"; then
      fail "a LiveKit value from your shell's environment is in the built app. This image must not be handed out."
    fi
  done
  if [ "${#leaked_candidates[@]}" -gt 0 ]; then
    echo "LiveKit values from your shell: ${#leaked_candidates[@]} checked, none in the binaries"
  fi
fi

if [ "$mode" = backend ]; then
  contains "$app_bin" "$backend_url" || fail "the backend URL isn't in the built app"
  contains "$app_bin" "$backend_key" || fail "the backend's public key isn't in the built app"
  if contains "$engine_bin" "$backend_url"; then
    fail "the backend URL turned up in tc-engine, which should know nothing about accounts"
  fi
  echo "backend in the app: $backend_host (URL and public key found; none in tc-engine)"
fi

hdiutil detach -quiet "$mount_point"
rmdir "$mount_point"
mount_point=""

size_bytes="$(stat -f %z "$dmg")"
echo "size: $((size_bytes / 1024 / 1024)) MB ($size_bytes bytes)"
echo "sha256: $(shasum -a 256 "$dmg" | cut -d' ' -f1)"
echo "==> $dmg"

case "$mode" in
  backend)
    echo "Built for the backend at $backend_host. It carries no LiveKit key or secret; calls get theirs from the call-pass function."
    ;;
  legacy)
    echo "LEGACY build: tc-engine carries the LiveKit server's key and secret from $file, and the app has no backend."
    echo "Anyone with this image can read the secret: share it only with people you trust with that server,"
    echo "and run package.sh again without --legacy-keys afterwards so target/ holds no secret."
    ;;
  none)
    echo "Built with no backend: the app says it isn't connected to a server. Pass a backend file to point it at yours (docs/self-hosting.md)."
    ;;
esac
