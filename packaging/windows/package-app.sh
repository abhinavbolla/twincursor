#!/usr/bin/env bash
# Builds the Windows app as a zip people unzip and double-click: TwinCursor.exe (the app, with
# its page built in), tc-engine.exe next to it, LICENSE and a README. Runs on Windows in Git
# Bash (CI's windows-latest runner, or a PC with Rust, Node and the VS Build Tools):
#
#   packaging/windows/package-app.sh            # backend from the environment, or none
#
# The backend is baked in from TWINCURSOR_BUILD_SUPABASE_URL and TWINCURSOR_BUILD_SUPABASE_KEY
# when both are set (the workflows take them from the repository variables of the same names).
# Both are public values: the project's URL and its publishable (or anon) key. With neither,
# the app has no backend and says it isn't connected to a server.
#
# The zip never carries a LiveKit key or secret: every LiveKit and server variable is cleared
# before building, a secret Supabase key is refused, and the built files are checked: the
# engine must have no server key of its own, no LiveKit value the caller's environment had may
# appear in either binary, and the backend's URL and key must be in the app and not in the
# engine. Values are never printed, except the backend's host and the key's first 8 characters.
#
# The result is dist/TwinCursor-<version>-windows-x64.zip (a folder of the same name inside).
# Release builds carry no debug info here, and CARGO_BUILD_JOBS is honoured (one job works).
set -euo pipefail

case "$-" in
  *x*)
    echo "package-app.sh: refusing to run with 'set -x' (it would print the values it builds in)" >&2
    exit 2
    ;;
esac

case "$(uname -s)" in
  MINGW* | MSYS* | CYGWIN*) ;;
  *)
    echo "package-app.sh: builds on Windows only (Git Bash)" >&2
    exit 2
    ;;
esac

root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$root"
target="${CARGO_TARGET_DIR:-$root/target}"

server_build_vars=(TWINCURSOR_BUILD_SERVER_URL TWINCURSOR_BUILD_API_KEY TWINCURSOR_BUILD_API_SECRET)
livekit_vars=(LIVEKIT_URL LIVEKIT_API_KEY LIVEKIT_API_SECRET)
server_runtime_vars=(TWINCURSOR_SERVER_URL TWINCURSOR_API_KEY TWINCURSOR_API_SECRET)
backend_runtime_vars=(TWINCURSOR_SUPABASE_URL TWINCURSOR_SUPABASE_KEY)

# LiveKit values the caller's environment has: checked against the binaries, never printed.
leaked_candidates=()
for var in "${server_build_vars[@]}" "${livekit_vars[@]}" "${server_runtime_vars[@]}"; do
  value="${!var:-}"
  value="${value#"${value%%[![:space:]]*}"}"
  value="${value%"${value##*[![:space:]]}"}"
  case "$value" in
    "" | ws://localhost:7880 | ws://127.0.0.1:7880 | devkey | secret) continue ;;
  esac
  if [ "${#value}" -ge 8 ]; then
    leaked_candidates+=("$value")
  fi
done
unset "${server_build_vars[@]}" "${livekit_vars[@]}" "${server_runtime_vars[@]}" "${backend_runtime_vars[@]}"

backend_url="${TWINCURSOR_BUILD_SUPABASE_URL:-}"
backend_key="${TWINCURSOR_BUILD_SUPABASE_KEY:-}"
backend_url="${backend_url%"${backend_url##*[![:space:]]}"}"
backend_key="${backend_key%"${backend_key##*[![:space:]]}"}"
unset TWINCURSOR_BUILD_SUPABASE_URL TWINCURSOR_BUILD_SUPABASE_KEY

check_public_key() {
  local key="$1" payload json
  case "$key" in
    sb_secret_*)
      echo "package-app.sh: that's a secret key (sb_secret_...). Use the publishable key." >&2
      exit 2
      ;;
    sb_publishable_*) return 0 ;;
  esac
  if [[ "$key" =~ ^[A-Za-z0-9_-]+\.([A-Za-z0-9_-]+)\.[A-Za-z0-9_-]+$ ]]; then
    payload="$(printf '%s' "${BASH_REMATCH[1]}" | tr '_-' '/+')"
    while [ $((${#payload} % 4)) -ne 0 ]; do payload="$payload="; done
    json="$(printf '%s' "$payload" | base64 -d 2>/dev/null || true)"
    case "$json" in
      *'"role":"anon"'* | *'"role": "anon"'*) return 0 ;;
    esac
    echo "package-app.sh: the key is a JWT whose role isn't anon. Use the public key." >&2
    exit 2
  fi
  echo "package-app.sh: the key doesn't look like a Supabase public key (sb_publishable_... or the anon JWT)" >&2
  exit 2
}

if [ -n "$backend_url" ] || [ -n "$backend_key" ]; then
  if [ -z "$backend_url" ] || [ -z "$backend_key" ]; then
    echo "package-app.sh: set both TWINCURSOR_BUILD_SUPABASE_URL and TWINCURSOR_BUILD_SUPABASE_KEY, or neither" >&2
    exit 2
  fi
  backend_url="${backend_url%/}"
  case "$backend_url" in
    https://?*) ;;
    *)
      echo "package-app.sh: the backend URL must start with https://" >&2
      exit 2
      ;;
  esac
  check_public_key "$backend_key"
  export TWINCURSOR_BUILD_SUPABASE_URL="$backend_url"
  export TWINCURSOR_BUILD_SUPABASE_KEY="$backend_key"
  backend_host="${backend_url#*://}"
  backend_host="${backend_host%%/*}"
  mode=backend
  echo "==> backend: $backend_host, public key ${backend_key:0:8}…"
else
  mode=none
  echo "==> no backend: the app will say it isn't connected to a server"
fi

# No debug info in this release build: smaller files, and a much lighter link.
export CARGO_PROFILE_RELEASE_DEBUG=0

echo "==> building the page"
(cd apps/desktop && npm ci --no-audit --no-fund && npm run build)

echo "==> building the engine and the app"
cargo build --locked --release -p twincursor-engine-cli
# tauri/custom-protocol puts the page inside the exe (a plain build would load it from a
# development server instead).
cargo build --locked --release -p twincursor-desktop --features tauri/custom-protocol

version="$(sed -n 's/^version = "\(.*\)"/\1/p' apps/desktop/src-tauri/Cargo.toml | head -1)"
name="TwinCursor-$version-windows-x64"
dist="$root/dist"
folder="$dist/$name"
rm -rf "$folder" "$dist/$name.zip"
mkdir -p "$folder"
cp "$target/release/twincursor-desktop.exe" "$folder/TwinCursor.exe"
cp "$target/release/tc-engine.exe" "$folder/tc-engine.exe"
cp LICENSE "$folder/LICENSE.txt"
cp packaging/windows/app/README.txt "$folder/README.txt"

fail() {
  echo "package-app.sh: $*" >&2
  rm -rf "$folder" "$dist/$name.zip"
  exit 1
}

contains() {
  LC_ALL=C grep -a -q -F -f <(printf '%s\n' "$2") "$1"
}

app_bin="$folder/TwinCursor.exe"
engine_bin="$folder/tc-engine.exe"

echo "==> checking it"
config="$("$engine_bin" config 2>/dev/null)"
case "$config" in
  *'"source":"dev"'*) ;;
  *) fail "tc-engine carries a server of its own. This package must not be handed out." ;;
esac
case "$config" in
  *'"has_secret":false'*) ;;
  *) fail "tc-engine carries a server secret. This package must not be handed out." ;;
esac
echo "engine server: dev only (no LiveKit key built in)"
for value in ${leaked_candidates[@]+"${leaked_candidates[@]}"}; do
  if contains "$app_bin" "$value" || contains "$engine_bin" "$value"; then
    fail "a LiveKit value from the environment is in the built files. This package must not be handed out."
  fi
done
echo "LiveKit values from the environment: ${#leaked_candidates[@]} checked, none in the binaries"
if [ "$mode" = backend ]; then
  contains "$app_bin" "$backend_url" || fail "the backend URL isn't in the built app"
  contains "$app_bin" "$backend_key" || fail "the backend's public key isn't in the built app"
  contains "$engine_bin" "$backend_url" && fail "the backend URL turned up in tc-engine"
  echo "backend in the app: $backend_host (URL and public key found; none in tc-engine)"
fi

(cd "$dist" && 7z a -bd -tzip "$name.zip" "$name" >/dev/null)
echo "contents: $(cd "$folder" && ls | tr '\n' ' ')"
size_bytes="$(wc -c <"$dist/$name.zip" | tr -d ' ')"
echo "size: $((size_bytes / 1024 / 1024)) MB ($size_bytes bytes)"
echo "sha256: $(sha256sum "$dist/$name.zip" | cut -d' ' -f1)"
echo "==> $dist/$name.zip (and the folder $folder)"
case "$mode" in
  backend) echo "Built for the backend at $backend_host. It carries no LiveKit key or secret." ;;
  none) echo "Built with no backend: the app says it isn't connected to a server." ;;
esac
