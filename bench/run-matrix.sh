#!/usr/bin/env bash
# Runs the M1 latency matrix on one Mac: for each config, start a sharer, run the viewer
# for $DURATION seconds, then stop the sharer. Results land in bench/results/*.json.
#
# Needs `livekit-server --dev` running and a release build:
#   cargo build --release -p twincursor-media-spike
# Usage: bench/run-matrix.sh [config-name ...]   (no args = all configs)
set -euo pipefail

cd "$(dirname "$0")/.."
BIN=target/release/tc-spike
DURATION=${DURATION:-30}
WARMUP=${WARMUP:-5}

# name | sharer flags
CONFIGS=(
  "sck-vp8|--capture sck --codec vp8"
  "sck-vp9|--capture sck --codec vp9"
  "sck-h264|--capture sck --codec h264"
  "sck-vp8-noscreencast|--capture sck --codec vp8 --no-screencast"
  "lk-vp8|--capture lk --codec vp8"
)

wanted() {
  local name=$1; shift
  [[ $# -eq 0 ]] && return 0
  for w in "$@"; do [[ $w == "$name" ]] && return 0; done
  return 1
}

for entry in "${CONFIGS[@]}"; do
  name=${entry%%|*}
  flags=${entry#*|}
  wanted "$name" "$@" || continue

  echo "=== $name ($flags)"
  log=$(mktemp)
  # shellcheck disable=SC2086
  "$BIN" share $flags >"$log" 2>&1 &
  sharer=$!
  # Wait until the track is published (ICE can take a few seconds).
  for _ in $(seq 60); do
    grep -q "screen track published" "$log" && break
    kill -0 "$sharer" 2>/dev/null || { cat "$log"; exit 1; }
    sleep 0.5
  done
  sleep "$WARMUP"
  "$BIN" view --duration "$DURATION" --label "$name" >/dev/null 2>&1 &
  viewer=$!
  # Sample CPU (% of one core) of both processes every 2 s while the viewer runs.
  cpu_log=$(mktemp)
  while kill -0 "$viewer" 2>/dev/null; do
    echo "$(ps -o %cpu= -p "$sharer" | tr -d ' ') $(ps -o %cpu= -p "$viewer" | tr -d ' ')" >>"$cpu_log"
    sleep 2
  done
  wait "$viewer" || echo "viewer failed for $name"
  cpu=$(awk 'NF==2 {s+=$1; v+=$2; n++} END {if (n) printf "sharer %.0f%% viewer %.0f%%", s/n, v/n}' "$cpu_log")
  echo "$name cpu: $cpu" | tee -a bench/results/cpu.txt
  rm -f "$cpu_log"
  grep "captured" "$log" | tail -2 || true
  kill -INT "$sharer" 2>/dev/null || true
  wait "$sharer" 2>/dev/null || true
  rm -f "$log"
done

echo "=== summary (ms)"
{
  printf 'label\tfps\trtt_p50\trtt_p95\toneway_p50\toneway_p95\tticks\n'
  for f in bench/results/*.json; do
    jq -r '[.label, (.avg_fps|floor),
            (.report.rtt_presented.p50 // 0 | floor), (.report.rtt_presented.p95 // 0 | floor),
            (.report.one_way_presented.p50 // 0 | floor), (.report.one_way_presented.p95 // 0 | floor),
            "\(.report.ticks_seen)/\(.report.ticks_sent)"] | @tsv' "$f"
  done | sort
} | column -t -s $'\t'
