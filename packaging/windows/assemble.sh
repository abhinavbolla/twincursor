#!/usr/bin/env bash
# Makes a Windows viewer zip a tester can use as is: a copy of the release workflow's
# TwinCursor-viewer-<tag>-windows-x64.zip with a server.txt added, written from an env file.
#
#   packaging/windows/assemble.sh TwinCursor-viewer-<tag>-windows-x64.zip path/to/testbuild.env
#
# The env file sets the server's URL, API key and API secret, either with the runtime names
# (TWINCURSOR_SERVER_URL, TWINCURSOR_API_KEY, TWINCURSOR_API_SECRET) or with the build names
# package.sh uses (TWINCURSOR_BUILD_SERVER_URL, ...), or LiveKit's own (LIVEKIT_URL,
# LIVEKIT_API_KEY, LIVEKIT_API_SECRET). The values are never printed. The result
# is written next to the input as <name>-tester.zip; anyone who has it can read the secret, so
# give it only to people you'd give the server's key to.
set -euo pipefail

case "$-" in
  *x*)
    echo "assemble.sh: refusing to run with 'set -x' (it would print the server's secret)" >&2
    exit 2
    ;;
esac

if [ "$#" -ne 2 ]; then
  echo "usage: $0 VIEWER.zip ENV_FILE" >&2
  exit 2
fi
zip_in="$1"
env_file="$2"
for f in "$zip_in" "$env_file"; do
  if [ ! -f "$f" ] || [ ! -r "$f" ]; then
    echo "assemble.sh: can't read $f" >&2
    exit 2
  fi
done
case "$zip_in" in
  *.zip) ;;
  *)
    echo "assemble.sh: $zip_in isn't a .zip" >&2
    exit 2
    ;;
esac

names=(SERVER_URL API_KEY API_SECRET)
for name in "${names[@]}"; do
  unset "TWINCURSOR_$name" "TWINCURSOR_BUILD_$name" "LIVEKIT_${name#SERVER_}"
done
set -a
# shellcheck disable=SC1090 # the file is the caller's choice
. "$env_file"
set +a

# Every entry sits in one top folder, next to tc-engine.exe.
top="$(zipinfo -1 "$zip_in" | cut -d/ -f1 | sort -u)"
if [ -z "$top" ] || [ "$(printf '%s\n' "$top" | wc -l | tr -d ' ')" != 1 ]; then
  echo "assemble.sh: expected one top folder in $zip_in" >&2
  exit 1
fi
if ! zipinfo -1 "$zip_in" | grep -qxF "$top/tc-engine.exe"; then
  echo "assemble.sh: $zip_in has no $top/tc-engine.exe" >&2
  exit 1
fi

umask 077
work="$(mktemp -d "${TMPDIR:-/tmp}/twincursor-assemble.XXXXXX")"
trap 'rm -rf "$work"' EXIT
mkdir "$work/$top"
server_txt="$work/$top/server.txt"

# CRLF, so Notepad shows it as it should be.
printf '# Written by assemble.sh. Keep it to yourself: the secret starts calls on this server.\r\n' \
  >"$server_txt"
for name in "${names[@]}"; do
  runtime="TWINCURSOR_$name"
  build="TWINCURSOR_BUILD_$name"
  livekit="LIVEKIT_${name#SERVER_}"
  value="${!runtime:-${!build:-${!livekit:-}}}"
  if [ -z "$value" ]; then
    echo "assemble.sh: $env_file sets none of $runtime, $build and $livekit" >&2
    exit 2
  fi
  case "$value" in
    *[[:space:]]* | *'"'*)
      echo "assemble.sh: $runtime's value has spaces or quotes, which server.txt can't hold" >&2
      exit 2
      ;;
  esac
  printf '%s=%s\r\n' "$runtime" "$value" >>"$server_txt"
done

zip_out="${zip_in%.zip}-tester.zip"
cp "$zip_in" "$work/out.zip"
(cd "$work" && zip -q out.zip "$top/server.txt")
mv "$work/out.zip" "$zip_out"
chmod 600 "$zip_out"

echo "contents:"
zipinfo -1 "$zip_out" | sed 's/^/  /'
size_bytes="$(wc -c <"$zip_out" | tr -d ' ')"
echo "size: $((size_bytes / 1024 / 1024)) MB ($size_bytes bytes)"
echo "sha256: $(shasum -a 256 "$zip_out" | cut -d' ' -f1)"
echo "==> $zip_out"
