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
# Set REVOKE=r to take control back partway, by typing r into the sharer's terminal, or
# REVOKE=hotkey to press the global hotkey (⌃⌥⌘R) for real. Typing must stop at once and nothing
# may stay held (default script: bench/control/revoke/*.json). The hotkey needs an unlocked
# screen and Accessibility permission for this terminal, to post the key press.
# SHARE_FLAGS and VIEW_FLAGS add flags to either side, e.g. SHARE_FLAGS="--audio pattern".
set -euo pipefail

cd "$(dirname "$0")/.."
BIN=${BIN:-target/release/tc-spike}
ANSWER=${ANSWER:-y}
REVOKE=${REVOKE:-}
OUT=bench/results/control-e2e-$(date +%s)
mkdir -p "$OUT"
# Lists the sharer's on-screen windows, to check the "is controlling" label (hard rule 4).
swiftc -O -o "$OUT/windows" bench/control/windows.swift
label_shown() { "$OUT/windows" "$1" | grep -q '"name":"TwinCursor control label"'; }

scripts=("$@")
if [[ ${#scripts[@]} -eq 0 ]]; then
  if [[ -n $REVOKE ]]; then scripts=(bench/control/revoke/*.json); else scripts=(bench/control/*.json); fi
fi

failed=0
for script in "${scripts[@]}"; do
  name=$(basename "$script" .json)
  dir=$OUT/$name
  mkdir -p "$dir"
  room="e2e-$name-$$"
  echo "=== $name (answer: $ANSWER)"

  # The sharer reads the answer from a pipe, as if typed into its terminal.
  mkfifo "$dir/answer"
  # shellcheck disable=SC2086  # the extra flags are meant to split
  "$BIN" share --room "$room" --capture pattern --resolution 720 --no-stats \
    --allow-control --inject "record:$dir/actions.jsonl" ${SHARE_FLAGS:-} <"$dir/answer" >"$dir/share.log" 2>&1 &
  sharer=$!
  exec 3>"$dir/answer"

  # shellcheck disable=SC2086
  "$BIN" view --room "$room" --headless --no-stats --duration 120 --label "e2e-$name" \
    --out "$dir" --name "e2e guest" --control-script "$script" --sent-log "$dir/sent.jsonl" \
    ${VIEW_FLAGS:-} >"$dir/view.log" 2>&1 &
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

  if [[ -n $REVOKE && $asked -eq 1 ]]; then
    for _ in $(seq 40); do grep -q "now controls this computer" "$dir/share.log" && break; sleep 0.25; done
    sleep 1.5
    case $REVOKE in
      r) echo r >&3 ;;
      hotkey) swift bench/control/post-hotkey.swift ;;
      *) echo "REVOKE must be r or hotkey"; exit 2 ;;
    esac
  fi

  # While the guest is in control, the sharer must see who it is.
  label_during=0
  if [[ $ANSWER == y ]]; then
    for _ in $(seq 400); do
      if label_shown "$sharer"; then label_during=1; break; fi
      kill -0 "$viewer" 2>/dev/null || break
      sleep 0.05
    done
  fi

  viewer_ok=1
  wait "$viewer" || viewer_ok=0
  sleep 0.3
  label_after=0
  label_shown "$sharer" && label_after=1
  kill -INT "$sharer" 2>/dev/null || true
  wait "$sharer" 2>/dev/null || true
  exec 3>&-
  rm -f "$dir/answer"

  if [[ -n $REVOKE ]]; then
    # Revoked partway: the viewer must notice, the sharer must say so, and the recording must
    # be the start of the run followed only by releases.
    if [[ $viewer_ok -eq 0 ]] && grep -q "control ended while the script was still running" "$dir/view.log" \
      && grep -q "Revoked: .* no longer has control" "$dir/share.log" \
      && "$BIN" replay --revoked --sent "$dir/sent.jsonl" --actions "$dir/actions.jsonl"; then
      echo "$name: PASS (revoked with $REVOKE)"
    else
      echo "$name: FAIL (logs in $dir)"
      failed=1
    fi
  elif [[ $ANSWER == y ]]; then
    if [[ $label_during -ne 1 || $label_after -ne 0 ]]; then
      echo "$name: FAIL (the control label was $([[ $label_during -eq 1 ]] || echo "missing during control")$([[ $label_after -eq 1 ]] && echo "still up after control"))"
      failed=1
    elif [[ $viewer_ok -eq 1 ]] && "$BIN" replay --sent "$dir/sent.jsonl" --actions "$dir/actions.jsonl"; then
      echo "$name: PASS (label shown during control, gone after)"
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
