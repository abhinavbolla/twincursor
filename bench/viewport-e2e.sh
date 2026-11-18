#!/usr/bin/env bash
# The sharer sends the size viewers can show, checked end to end on one machine with nobody at
# the Mac: no windows, no screen capture, no sound.
#
# Each case starts a sharer on the synthetic pattern (its "display" is 3840x2160) with
# --resolution 2160, and headless viewers that report viewports (`view --viewport`), then reads
# what they received from their results files:
#
#   1920x1080 viewport → 1920x1080      2560x1440 → 2560x1440
#   a viewport above the ceiling → the ceiling (3840x2160)
#   no viewport → the ceiling, and nothing else, all along
#   a viewport change mid-call shows within MAX_CHANGE_S
#   two viewers → the larger one's size; once it leaves, the smaller one's
#   text quality (SSIM against the pattern) at the fitted size
#
# Needs `livekit-server --dev` running, jq, and a release build:
#   cargo build --release -p twincursor-media-spike
# Usage: bench/viewport-e2e.sh   (BIN=path/to/tc-spike to use another build, OUT=dir for logs)
# Exits non-zero if any check fails.
set -euo pipefail

cd "$(dirname "$0")/.."
BIN=${BIN:-target/release/tc-spike}
OUT=${OUT:-bench/results/viewport-e2e-$(date +%s)}
DURATION=${DURATION:-12}
# How long a new viewport may take to show, from when the viewer has it: the viewer waits
# 0.3 s for it to settle, the sharer changes size at most once a second, and the first frame
# at the new size has to arrive.
MAX_CHANGE_S=${MAX_CHANGE_S:-2.5}
# How long after the first frame a joining viewer's own size may take: its first data packets
# are lost until its data channel opens (2 to 4 s on a local server).
MAX_JOIN_S=${MAX_JOIN_S:-6}
# Text quality at the fitted size: the floors the matrix's 1080p rows clear with room to spare.
MIN_SSIM_P50=${MIN_SSIM_P50:-0.98}
MIN_SSIM_P5=${MIN_SSIM_P5:-0.95}
mkdir -p "$OUT"
command -v jq >/dev/null || { echo "needs jq"; exit 1; }

failed=0
pass() { echo "  ok: $1"; }
fail() { echo "  FAIL: $1"; failed=1; }
expect() { # what, got, wanted
  if [[ $2 == "$3" ]]; then pass "$1: $2"; else fail "$1: got $2, wanted $3"; fi
}

sharer=
stop_sharer() {
  if [[ -n $sharer ]]; then
    kill -INT "$sharer" 2>/dev/null || true
    wait "$sharer" 2>/dev/null || true
    sharer=
  fi
}
trap stop_sharer EXIT

# start_sharer CASE [flags...]: a sharer in a room of this case's own.
start_sharer() {
  local case=$1; shift
  room="viewport-e2e-$$-$case"
  "$BIN" share --room "$room" --capture pattern --audio off --resolution 2160 "$@" \
    >"$OUT/$case-share.log" 2>&1 &
  sharer=$!
  for _ in $(seq 60); do
    grep -q "screen track published" "$OUT/$case-share.log" && return 0
    kill -0 "$sharer" 2>/dev/null || break
    sleep 0.5
  done
  cat "$OUT/$case-share.log"
  fail "$case: the sharer didn't start"
  return 1
}

# view CASE NAME [flags...]: a headless viewer for $DURATION seconds (unless flags say other).
view() {
  local case=$1 name=$2; shift 2
  "$BIN" view --room "$room" --audio off --headless --duration "$DURATION" \
    --identity "$name" --label "$case-$name" --out "$OUT/$case-$name" "$@" \
    >"$OUT/$case-$name-view.log" 2>&1
}

result() { ls "$OUT/$1"/*.json 2>/dev/null | head -1 || true; }
# The size frames arrived at last, as WxH.
last_size() { jq -r '.sizes | last | .size | "\(.[0])x\(.[1])"' "$1"; }
# Every size frames arrived at, in order.
all_sizes() { jq -r '[.sizes[].size | "\(.[0])x\(.[1])"] | join(",")' "$1"; }
sized_by() { jq -r '.sharer_net_last.sized_by // "?"' "$1"; }

# simple CASE VIEWPORT WANTED [what decided it]
simple() {
  local case=$1 viewport=$2 wanted=$3 by=${4:-viewer}
  echo "=== $case: viewport $viewport"
  start_sharer "$case" || return 0
  view "$case" v --viewport "$viewport" || fail "$case: the viewer failed"
  stop_sharer
  local f; f=$(result "$case-v")
  [[ -n $f ]] || { fail "$case: no results"; return 0; }
  expect "$case: received at the end" "$(last_size "$f")" "$wanted"
  if [[ $by == viewer ]]; then
    took=$(jq -r --arg size "$wanted" '(.sizes[0].t_s) as $first
      | [.sizes[] | select("\(.size[0])x\(.size[1])" == $size)] | first
      | if . then (.t_s - $first) else "never" end' "$f")
    if [[ $took != never ]] && awk -v t="$took" -v m="$MAX_JOIN_S" 'BEGIN { exit !(t <= m) }'; then
      pass "$case: $wanted from $(printf '%.2f' "$took") s after the first frame"
    else
      fail "$case: $wanted took $took s after the first frame (at most $MAX_JOIN_S)"
    fi
  fi
  [[ $(sized_by "$f") == *"$by"* ]] && pass "$case: sized by $(sized_by "$f")" ||
    fail "$case: sized by $(sized_by "$f"), wanted $by"
}

simple fhd 1920x1080 1920x1080
simple qhd 2560x1440 2560x1440
simple above 7680x4320 3840x2160 setting

echo "=== none: no viewport (as before viewports existed)"
if start_sharer none; then
  view none v || fail "none: the viewer failed"
  stop_sharer
  f=$(result none-v)
  if [[ -n $f ]]; then
    expect "none: every size received" "$(all_sizes "$f")" "3840x2160"
    [[ $(sized_by "$f") == *setting* ]] && pass "none: sized by $(sized_by "$f")" ||
      fail "none: sized by $(sized_by "$f")"
  else
    fail "none: no results"
  fi
fi

echo "=== change: 1920x1080, then 2560x1440 at 8 s, then 1280x720 at 14 s"
if start_sharer change; then
  DURATION=20 view change v --viewport 1920x1080 --viewport-at 8:2560x1440 \
    --viewport-at 14:1280x720 || fail "change: the viewer failed"
  stop_sharer
  f=$(result change-v)
  if [[ -n $f ]]; then
    expect "change: received at the end" "$(last_size "$f")" "1280x720"
    for step in "8 2560x1440" "14 1280x720"; do
      read -r at size <<<"$step"
      took=$(jq -r --arg size "$size" --argjson at "$at" \
        '[.sizes[] | select("\(.size[0])x\(.size[1])" == $size and .t_s >= $at)] | first
         | if . then (.t_s - $at) else "never" end' "$f")
      if [[ $took != never ]] && awk -v t="$took" -v m="$MAX_CHANGE_S" 'BEGIN { exit !(t <= m) }'; then
        pass "change: $size showed $(printf '%.2f' "$took") s after the viewer had it"
      else
        fail "change: $size took $took s (at most $MAX_CHANGE_S)"
      fi
    done
  else
    fail "change: no results"
  fi
fi

echo "=== two: viewers at 1920x1080 and 2560x1440; the larger leaves first"
if start_sharer two; then
  DURATION=24 view two small --viewport 1920x1080 &
  small=$!
  sleep 1
  view two big --viewport 2560x1440 || fail "two: the big viewer failed"
  wait "$small" || fail "two: the small viewer failed"
  stop_sharer
  big=$(result two-big) small=$(result two-small)
  if [[ -n $big && -n $small ]]; then
    expect "two: the big viewer received at the end" "$(last_size "$big")" "2560x1440"
    sizes=$(all_sizes "$small")
    [[ $sizes == *2560x1440*1920x1080 ]] &&
      pass "two: the small viewer got the larger size, then its own: $sizes" ||
      fail "two: the small viewer received $sizes (wanted 2560x1440 while both were in, then 1920x1080)"
  else
    fail "two: no results"
  fi
fi

echo "=== quality: text at the fitted size (1920x1080 from the 4K pattern)"
if start_sharer quality; then
  DURATION=20 view quality v --viewport 1920x1080 --measure-quality || fail "quality: the viewer failed"
  stop_sharer
  f=$(result quality-v)
  if [[ -n $f ]]; then
    expect "quality: received at the end" "$(last_size "$f")" "1920x1080"
    read -r p50 p5 n < <(jq -r '"\(.report.ssim.p50 // 0) \(.report.ssim.p5 // 0) \(.report.ssim.count // 0)"' "$f")
    if awk -v a="$p50" -v b="$p5" -v x="$MIN_SSIM_P50" -v y="$MIN_SSIM_P5" 'BEGIN { exit !(a >= x && b >= y) }' && ((n > 0)); then
      pass "quality: SSIM p50 $p50, p5 $p5 over $n frames"
    else
      fail "quality: SSIM p50 $p50, p5 $p5 over $n frames (at least $MIN_SSIM_P50 and $MIN_SSIM_P5)"
    fi
  else
    fail "quality: no results"
  fi
fi

echo "=== logs and results in $OUT"
if ((failed)); then
  echo "FAILED"
  exit 1
fi
echo "all passed"
