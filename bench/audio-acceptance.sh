#!/usr/bin/env bash
# The voice checks, in one go. Steps 1, 2 and 5 need someone at the Mac; 3 and 4 make no sound
# and touch nothing.
#
#   1. Voice delay through real speakers and mic (bench/audio-loopback.sh): plays tones out loud
#      for about 30 s. Volume up, quiet room.
#   2. Echo cancellation through real speakers and mic (bench/audio-echo.sh): talks out loud for
#      about a minute. Same needs.
#   3. Mute, both ways, headless: the sharer mutes its synthetic voice for 5 s and the viewer
#      must hear no marker in that time and markers again after; then the viewer mutes its mic
#      and the sharer must be told.
#   4. The screen's latency with voice off and on (bench/run-matrix.sh av-off av-on): voice
#      may add at most 3 ms to the median from capture to decode.
#   5. The sharer's mute hotkey, pressed for real (⌃⌥⌘M): needs an unlocked screen and
#      Accessibility permission for this terminal, to post the key press.
#
# Needs `livekit-server --dev` running, a release build, and microphone access for this
# terminal. Results land in bench/results/audio-acceptance-<time>/.
# STEPS="3 4" bench/audio-acceptance.sh runs only those steps.
set -uo pipefail

cd "$(dirname "$0")/.."
BIN=${BIN:-target/release/tc-spike}
OUT=bench/results/audio-acceptance-$(date +%s)
mkdir -p "$OUT"
status=0
note() { echo; echo "=== $*"; }
want() { [[ " ${STEPS:-1 2 3 4 5} " == *" $1 "* ]]; }
pass() { echo "PASS: $*"; }
fail() { echo "FAIL: $*"; status=1; }
# The logs are coloured; the checks read them plain.
plain() { sed $'s/\x1b\\[[0-9;]*m//g' "$1"; }
wait_for() { # file, text, tries of 0.25 s
  for _ in $(seq "$3"); do plain "$1" | grep -q "$2" && return 0; sleep 0.25; done
  return 1
}
stop() { kill -INT "$1" 2>/dev/null; wait "$1" 2>/dev/null; }

if want 1; then
note "1. Voice delay through real speakers and mic"
LABEL=accept-loopback bench/audio-loopback.sh || status=1
fi

if want 2; then
note "2. Echo cancellation through real speakers and mic"
LABEL=accept-echo bench/audio-echo.sh
case $? in
  0) ;;
  2) echo "INCONCLUSIVE: the room wasn't quiet enough to judge; run step 2 again" ;;
  *) status=1 ;;
esac
fi

if want 3; then
note "3. Mute, both ways"
room="accept-mute-$$"
"$BIN" share --room "$room" --capture pattern --codec h264 --audio pattern --mute-for 8:13 \
  >"$OUT/mute-share.log" 2>&1 &
sharer=$!
wait_for "$OUT/mute-share.log" "voice track published" 60
"$BIN" view --room "$room" --headless --audio off --measure-audio --duration 20 \
  --label mute-sharer --out "$OUT" >"$OUT/mute-view.log" 2>&1
stop "$sharer"
result=$(ls -1t "$OUT"/mute-sharer-*.json 2>/dev/null | head -1)
if [[ -n $result ]] && jq -e '.report.audio | .mutes == 1 and .muted_s > 4.5 and .muted_s < 5.5
    and .heard_while_muted == 0 and .heard_after_unmute >= 3
    and .bursts_heard - .heard_after_unmute >= 3' "$result" >/dev/null; then
  pass "$(jq -r '.report.audio | "the sharer muted for \(.muted_s * 10 | round / 10) s: no marker heard in that time, \(.bursts_heard - .heard_after_unmute) before and \(.heard_after_unmute) after"' "$result")"
else
  fail "the sharer's mute didn't silence its voice for the viewer: $(jq -c '.report.audio | {bursts_heard, mutes, muted_s, heard_while_muted, heard_after_unmute}' "$result" 2>/dev/null)"
fi

room="accept-mute-viewer-$$"
"$BIN" share --room "$room" --capture pattern --codec h264 --audio off >"$OUT/mute-share2.log" 2>&1 &
sharer=$!
wait_for "$OUT/mute-share2.log" "screen track published" 60
"$BIN" view --room "$room" --headless --audio mic --mute-for 4:8 --duration 12 \
  --label mute-viewer --out "$OUT" >"$OUT/mute-view2.log" 2>&1
stop "$sharer"
muted=$(plain "$OUT/mute-share2.log" | grep -c "the viewer's voice muted=true")
unmuted=$(plain "$OUT/mute-share2.log" | grep "the viewer's voice" | tail -1 | grep -c "muted=false")
if [[ $muted -eq 1 && $unmuted -eq 1 ]]; then
  pass "the viewer muted its mic and unmuted it, and the sharer was told both times"
else
  fail "the sharer wasn't told about the viewer's mute (see $OUT/mute-share2.log)"
fi
fi

if want 4; then
note "4. The screen's latency with voice off and on"
before=$(mktemp)
touch "$before"
bench/run-matrix.sh av-off av-on >"$OUT/matrix.log" 2>&1 || true
p50() { # label: the screen's capture → decode time, median, from the run just made
  local f
  f=$(find bench/results -maxdepth 1 -name "$1-[0-9]*.json" -newer "$before" | sort | tail -1)
  [[ -n $f ]] && jq -r '.report.one_way_presented.p50' "$f"
}
off=$(p50 av-off)
on=$(p50 av-on)
rm -f "$before"
if [[ -n $off && -n $on ]] && awk "BEGIN { exit !($on - $off <= 3) }"; then
  pass "$(awk "BEGIN { printf \"capture to decode p50 %.1f ms without voice, %.1f ms with (%+.1f ms)\", $off, $on, $on - $off }")"
else
  fail "voice added more than 3 ms to the screen: capture to decode p50 ${off:-?} ms without, ${on:-?} ms with (see $OUT/matrix.log)"
fi
fi

if want 5; then
note "5. The sharer's mute hotkey, pressed for real"
room="accept-hotkey-$$"
"$BIN" share --room "$room" --capture pattern --codec h264 --audio mic >"$OUT/hotkey-share.log" 2>&1 &
sharer=$!
wait_for "$OUT/hotkey-share.log" "voice track published" 60
"$BIN" view --room "$room" --headless --audio off --duration 14 --label hotkey --out "$OUT" \
  >"$OUT/hotkey-view.log" 2>&1 &
viewer=$!
wait_for "$OUT/hotkey-view.log" "audio track subscribed" 60
sleep 2
swift bench/control/post-hotkey.swift m
wait_for "$OUT/hotkey-view.log" "the sharer's voice muted=true" 20 && told_muted=1 || told_muted=0
sleep 2
swift bench/control/post-hotkey.swift m
wait_for "$OUT/hotkey-share.log" "Your microphone is on" 20 && back_on=1 || back_on=0
wait "$viewer"
stop "$sharer"
said=$(plain "$OUT/hotkey-share.log" | grep -c "Your microphone is muted")
last=$(plain "$OUT/hotkey-view.log" | grep "the sharer's voice" | tail -1 | grep -c "muted=false")
if [[ $said -eq 1 && $told_muted -eq 1 && $back_on -eq 1 && $last -eq 1 ]]; then
  pass "⌃⌥⌘M muted the sharer's mic and unmuted it; the sharer was told in its terminal and the viewer both times"
else
  fail "the mute hotkey didn't work (said muted: $said, viewer told: $told_muted, back on: $back_on, viewer told unmuted: $last). Is the screen unlocked, and may this terminal post key presses?"
fi
fi

echo
if [[ $status -eq 0 ]]; then echo "The voice checks that ran all passed."; else echo "Some checks FAILED: see above."; fi
exit $status
