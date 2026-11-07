#!/usr/bin/env bash
# Remote control end to end on one machine, with no window and nothing injected for real.
#
# A sharer (synthetic pattern, --allow-control, recording every action it would inject) and a
# headless viewer that plays a control script. The script answers the sharer's consent prompt
# the way a person would, by typing into its terminal, then `tc-spike replay` checks that every
# key, click, scroll and character arrived once and in order.
#
# Needs `livekit-server --dev` running and a release build:
#   cargo build --release -p twincursor-media-spike
# Usage: bench/control-e2e.sh [script.json ...]   (default: bench/control/*.json)
# Set ANSWER=n to check that a refused request injects nothing.
set -euo pipefail

cd "$(dirname "$0")/.."
BIN=${BIN:-target/release/tc-spike}
ANSWER=${ANSWER:-y}
OUT=bench/results/control-e2e-$(date +%s)
mkdir -p "$OUT"

scripts=("$@")
[[ ${#scripts[@]} -eq 0 ]] && scripts=(bench/control/*.json)

failed=0
for script in "${scripts[@]}"; do
  name=$(basename "$script" .json)
  dir=$OUT/$name
  mkdir -p "$dir"
  room="e2e-$name-$$"
  echo "=== $name (answer: $ANSWER)"

  # The sharer reads the answer from a pipe, as if typed into its terminal.
  mkfifo "$dir/answer"
  "$BIN" share --room "$room" --capture pattern --resolution 720 --no-stats \
    --allow-control --inject "record:$dir/actions.jsonl" <"$dir/answer" >"$dir/share.log" 2>&1 &
  sharer=$!
  exec 3>"$dir/answer"

  "$BIN" view --room "$room" --headless --no-stats --duration 120 --label "e2e-$name" \
    --out "$dir" --name "e2e guest" --control-script "$script" --sent-log "$dir/sent.jsonl" \
    >"$dir/view.log" 2>&1 &
  viewer=$!

  # Answer only once the sharer has asked: consent comes after the request, never before.
  asked=0
  for _ in $(seq 120); do
    if grep -q "asks to control this computer" "$dir/share.log"; then asked=1; break; fi
    kill -0 "$viewer" 2>/dev/null || break
    sleep 0.5
  done
  if [[ $asked -eq 1 ]]; then
    echo "$ANSWER" >&3
  else
    echo "the sharer never asked"
  fi

  viewer_ok=1
  wait "$viewer" || viewer_ok=0
  kill -INT "$sharer" 2>/dev/null || true
  wait "$sharer" 2>/dev/null || true
  exec 3>&-
  rm -f "$dir/answer"

  if [[ $ANSWER == y ]]; then
    if [[ $viewer_ok -eq 1 ]] && "$BIN" replay --sent "$dir/sent.jsonl" --actions "$dir/actions.jsonl"; then
      echo "$name: PASS"
    else
      echo "$name: FAIL (logs in $dir)"
      tail -5 "$dir/view.log"
      failed=1
    fi
  else
    # Refused: the viewer must report it, and nothing may be injected.
    if [[ $viewer_ok -eq 0 ]] && grep -q "turned the request down" "$dir/view.log" \
      && [[ ! -s "$dir/actions.jsonl" ]]; then
      echo "$name: PASS (refused, nothing injected)"
    else
      echo "$name: FAIL (logs in $dir)"
      failed=1
    fi
  fi
done
exit $failed
