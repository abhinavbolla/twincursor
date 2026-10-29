#!/usr/bin/env bash
# Network lab: runs the pattern sharer and a headless viewer through LiveKit inside Docker,
# with each profile's network conditions applied by tc netem, and collects the viewer's JSON
# results in bench/results/netlab/. Summarise them with bench/netlab/report.sh.
#
# Needs Docker running. Usage:
#   bench/netlab/run.sh [--build] [profile ...]    (no profiles = all)
#   CODEC=vp8 RES=1080 DURATION=60 ADAPT=off bench/netlab/run.sh hotel
set -euo pipefail
cd "$(dirname "$0")"

DURATION=${DURATION:-60}
# Linux in Docker has no hardware encoder; VP8 is the software path we'd use there too.
CODEC=${CODEC:-vp8}
RES=${RES:-1080}
# Extra sharer flags, e.g. SHARE_FLAGS="--fps 30", and a label suffix for them.
SHARE_FLAGS=${SHARE_FLAGS:-}
TAG=${TAG:-}
# Auto-throttle on (default) or off, to compare.
ADAPT=${ADAPT:-on}
SFU=172.30.0.10
VIEWER=172.30.0.30
URL=ws://$SFU:7880

# name | SFU→viewer (viewer download) | viewer→SFU (viewer upload) | sharer→SFU (sharer upload)
# Each field is tc netem arguments, or empty. Delays are one way, so RTT is about twice them.
PROFILES=(
  "lan|||"
  "wifi-good|delay 2ms 2ms loss 0.1%|delay 2ms 2ms loss 0.1%|"
  "wifi-bad|delay 8ms 8ms loss 1% 25%|delay 8ms 8ms loss 1% 25%|"
  "4g|delay 25ms 5ms loss 0.5% rate 12mbit|delay 25ms 5ms loss 0.5% rate 4mbit|"
  "hotel|delay 40ms 10ms loss 2% rate 3mbit|delay 40ms 10ms loss 2% rate 1mbit|"
  "intercontinental|delay 75ms|delay 75ms|"
  "sharer-uplink-3mbit|||delay 10ms rate 3mbit"
  "bandwidth-drop|||delay 5ms rate 10mbit"
  "udp-blocked|||"
  "outage-5s|||"
)

dc() { docker compose -f compose.yml "$@"; }

wanted() {
  local name=$1; shift
  [[ $# -eq 0 ]] && return 0
  for w in "$@"; do [[ $w == "$name" ]] && return 0; done
  return 1
}

# Timed changes during the viewer's run, for the dynamic profiles. Runs in the background.
steps() {
  case $1 in
    bandwidth-drop)
      sleep 20; dc exec -T sharer shape change delay 5ms rate 1500kbit
      sleep 20; dc exec -T sharer shape change delay 5ms rate 10mbit ;;
    outage-5s)
      sleep 25
      dc exec -T sfu-net shape set --to "$VIEWER" loss 100%
      dc exec -T viewer shape set loss 100%
      sleep 5
      dc exec -T sfu-net shape clear
      dc exec -T viewer shape clear ;;
  esac
}

if [[ ${1:-} == --build ]]; then
  shift
  dc build
fi
mkdir -p ../results/netlab
dc up -d

for entry in "${PROFILES[@]}"; do
  IFS='|' read -r name down up sharer_up <<<"$entry"
  wanted "$name" "$@" || continue
  label="$name-$CODEC-$RES${TAG:+-$TAG}"
  adapt_flag=
  if [[ $ADAPT == off ]]; then
    label="$label-noadapt"
    adapt_flag=--no-adapt
  fi
  echo "=== $label"

  for svc in sfu-net viewer sharer; do dc exec -T "$svc" shape clear; done
  # shellcheck disable=SC2086
  [[ -n $down ]] && dc exec -T sfu-net shape set --to "$VIEWER" $down
  # shellcheck disable=SC2086
  [[ -n $up ]] && dc exec -T viewer shape set $up
  # shellcheck disable=SC2086
  [[ -n $sharer_up ]] && dc exec -T sharer shape set $sharer_up
  [[ $name == udp-blocked ]] && dc exec -T viewer shape block-udp

  dc exec -T sharer sh -c 'pkill -INT tc-spike; sleep 1' || true
  dc exec -d sharer sh -c "tc-spike share --url $URL --capture pattern --codec $CODEC --resolution $RES $adapt_flag $SHARE_FLAGS >/tmp/share.log 2>&1"
  for _ in $(seq 60); do
    dc exec -T sharer grep -q "screen track published" /tmp/share.log 2>/dev/null && break
    sleep 1
  done

  steps "$name" &
  stepper=$!
  dc exec -T viewer tc-spike view --headless --measure-quality --url "$URL" \
    --duration "$DURATION" --label "$label" --out /results >/dev/null 2>&1 \
    || echo "viewer failed for $label"
  wait "$stepper" 2>/dev/null || true
  dc exec -T sharer sh -c 'pkill -INT tc-spike' || true
done

for svc in sfu-net viewer sharer; do dc exec -T "$svc" shape clear; done
echo "results in bench/results/netlab/; summarise with bench/netlab/report.sh"
