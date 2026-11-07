#!/usr/bin/env bash
# Remote control through the network lab: the scripted guest of bench/control-e2e.sh, under
# each profile's network conditions. The sharer records what it would inject (nothing real:
# containers have no screen), and `tc-spike replay` checks that every key, click, scroll and
# character arrived once and in order.
#
# Needs Docker and an image built with the current code (bench/netlab/run.sh --build), and a
# release build on this machine for `tc-spike replay`. Usage:
#   bench/netlab/control.sh [profile ...]          (default: lan wifi-bad 4g hotel)
#
# Stalls aren't here: after a stall of over a second the sharer releases held input on purpose
# (ADR 0003), so "everything arrived as sent" isn't the right check. The scripted traces in
# crates/control/src/tests.rs cover them.
#   SCRIPT=bench/control/smoke.json bench/netlab/control.sh hotel
set -euo pipefail
cd "$(dirname "$0")"

SCRIPT=${SCRIPT:-../control/smoke.json}
BIN=${BIN:-../../target/release/tc-spike}
SFU=172.30.0.10
VIEWER=172.30.0.30
URL=ws://$SFU:7880
OUT=../results/netlab/control-$(date +%s)
mkdir -p "$OUT"

# name | SFU→viewer | viewer→SFU, as in run.sh
PROFILES=(
  "lan||"
  "wifi-good|delay 2ms 2ms loss 0.1%|delay 2ms 2ms loss 0.1%"
  "wifi-bad|delay 8ms 8ms loss 1% 25%|delay 8ms 8ms loss 1% 25%"
  "4g|delay 25ms 5ms loss 0.5% rate 12mbit|delay 25ms 5ms loss 0.5% rate 4mbit"
  "hotel|delay 40ms 10ms loss 2% rate 3mbit|delay 40ms 10ms loss 2% rate 1mbit"
  "intercontinental|delay 75ms|delay 75ms"
)

dc() { docker compose -f compose.yml "$@"; }

wanted() {
  local name=$1; shift
  [[ $# -eq 0 ]] && set -- lan wifi-bad 4g hotel
  for w in "$@"; do [[ $w == "$name" ]] && return 0; done
  return 1
}

dc up -d >/dev/null
dc cp "$SCRIPT" viewer:/tmp/script.json
failed=0
for entry in "${PROFILES[@]}"; do
  IFS='|' read -r name down up <<<"$entry"
  wanted "$name" "$@" || continue
  dir=$OUT/$name
  mkdir -p "$dir"
  echo "=== $name"

  for svc in sfu-net viewer sharer; do dc exec -T "$svc" shape clear; done
  # shellcheck disable=SC2086
  [[ -n $down ]] && dc exec -T sfu-net shape set --to "$VIEWER" $down
  # shellcheck disable=SC2086
  [[ -n $up ]] && dc exec -T viewer shape set $up

  room="control-$name-$$"
  dc exec -T sharer sh -c 'pkill -INT tc-spike; sleep 1; rm -f /tmp/answer /tmp/actions.jsonl; touch /tmp/answer' || true
  # The sharer reads its answer from a file that grows, as if typed into its terminal.
  dc exec -d sharer sh -c "tail -f /tmp/answer | tc-spike share --url $URL --room $room \
    --capture pattern --codec vp8 --resolution 720 --no-stats --allow-control --headless \
    --inject record:/tmp/actions.jsonl >/tmp/share.log 2>&1"
  for _ in $(seq 60); do
    dc exec -T sharer grep -q "screen track published" /tmp/share.log 2>/dev/null && break
    sleep 1
  done

  dc exec -d viewer sh -c "tc-spike view --url $URL --room $room --headless --no-stats \
    --duration 180 --out /tmp --name 'lab guest' --control-script /tmp/script.json \
    --sent-log /tmp/sent.jsonl >/tmp/view.log 2>&1; echo \$? >/tmp/view.exit"
  dc exec -T viewer rm -f /tmp/view.exit
  for _ in $(seq 120); do
    dc exec -T sharer grep -q "asks to control this computer" /tmp/share.log 2>/dev/null && break
    sleep 0.5
  done
  dc exec -T sharer sh -c 'echo y >> /tmp/answer'

  for _ in $(seq 360); do
    dc exec -T viewer test -f /tmp/view.exit && break
    sleep 0.5
  done
  dc exec -T sharer sh -c 'pkill -INT tc-spike; sleep 1; pkill -f "tail -f /tmp/answer"' || true
  dc cp sharer:/tmp/actions.jsonl "$dir/actions.jsonl"
  dc cp sharer:/tmp/share.log "$dir/share.log"
  dc cp viewer:/tmp/sent.jsonl "$dir/sent.jsonl"
  dc cp viewer:/tmp/view.log "$dir/view.log"

  if "$BIN" replay --sent "$dir/sent.jsonl" --actions "$dir/actions.jsonl"; then
    echo "$name: PASS"
  else
    echo "$name: FAIL (logs in $dir)"
    failed=1
  fi
done
for svc in sfu-net viewer sharer; do dc exec -T "$svc" shape clear; done
exit $failed
