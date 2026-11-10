#!/usr/bin/env bash
# Echo cancellation through real speakers and a real mic, on one Mac.
#
# The sharer sends recorded speech as its voice (`--audio file`): the far end. The viewer plays
# it on the speakers and sends its own mic, echo-cancelled, as in a call. `tc-spike listen
# --echo` receives both voices and a raw recording of the same mic
# (bench/audio/record-mic.swift), and compares them:
#   - While only the far end talks, what comes back to it is echo. Once the canceller has had
#     5 s, it must be at least 30 dB quieter than the mic heard it.
#   - A local voice, played out loud while the far end is silent, must come through.
#   - The local voice again while the far end talks (double talk) is reported, not judged.
#   - In the far end's pauses the raw mic shows whether the room was quiet. Someone talking
#     nearby is passed on, as it should be, and holds the canceller back, so a failed run in a
#     room that wasn't quiet is inconclusive (exit 2), not a failure (exit 1).
#
# The voices are macOS's text to speech (`say`).
#
# Needs `livekit-server --dev` running, a release build, microphone access for this terminal,
# the volume up, and a quiet room for about a minute: it talks out loud.
#   bench/audio-echo.sh                           LABEL=echo by default
#   SPEAKER=1 MIC=0 bench/audio-echo.sh           pick devices (see `tc-spike devices`)
#   FAR_VOICE=Daniel LOCAL_VOICE=Samantha ...     the voices (`say -v '?'` lists them)
set -euo pipefail
cd "$(dirname "$0")/.."
source bench/audio/mic-free.sh

LABEL=${LABEL:-echo}
FAR_VOICE=${FAR_VOICE:-Daniel}
LOCAL_VOICE=${LOCAL_VOICE:-Samantha}
BIN=target/release/tc-spike
RESULTS=bench/results
ROOM=echo-$$
OUT=$(mktemp -d)

device_flags=()
[[ -n ${SPEAKER:-} ]] && device_flags+=(--speaker "$SPEAKER")
[[ -n ${MIC:-} ]] && device_flags+=(--mic "$MIC")

swiftc -O -o "$OUT/record-mic" bench/audio/record-mic.swift

# The far end: sentences with 2 s pauses, where the room can be heard on its own...
FAR_ALONE="Let's start with the failing test and see what it prints. [[slnc 2000]] \
The second assertion is the one that breaks, right after the cache is cleared. [[slnc 2000]] \
I think the lookup returns the old entry, so scroll down to where we store it. [[slnc 2000]] \
There, on that line, the key is built before the name is trimmed."
# ...and, after a gap for the local voice, without pauses, for the local voice to talk over.
FAR_AGAIN="So if we trim the name first and then build the key, both paths agree, the stale \
entry is never written, and the test should pass without touching the cache at all. Let's \
try that and run the whole suite once more to be sure."
LOCAL="Wait, before you change it, can we check who else calls that function? I remember \
at least one other place."

speak() { # voice, file, text
  say -v "$1" --file-format=WAVE --data-format=LEI16@48000 -o "$2" "$3"
}
seconds() { awk -v bytes="$(stat -f%z "$1")" 'BEGIN { printf "%.1f", bytes / 96000 }'; }
calc() { awk "BEGIN { printf \"%.1f\", $* }"; }

speak "$LOCAL_VOICE" "$OUT/local.wav" "$LOCAL"
speak "$FAR_VOICE" "$OUT/far-alone.wav" "$FAR_ALONE"
local_s=$(seconds "$OUT/local.wav")
# Seconds after the far end's first word: the local voice plays 2 s after the far end stops,
# the far end starts again 2.5 s after that, and the local voice joins in 2 s later.
alone_from=$(calc "$(seconds "$OUT/far-alone.wav") + 2")
alone_to=$(calc "$alone_from + $local_s")
again_from=$(calc "$alone_to + 2.5")
both_from=$(calc "$again_from + 2")
both_to=$(calc "$both_from + $local_s")
gap_ms=$(calc "(2 + $local_s + 2.5) * 1000")
speak "$FAR_VOICE" "$OUT/far.wav" "$FAR_ALONE [[slnc ${gap_ms%.*}]] $FAR_AGAIN [[slnc 3000]]"
# The file ends with 3 s of silence and then starts over; stop inside that.
total=$(calc "$(seconds "$OUT/far.wav") - 1")
if awk "BEGIN { exit !($both_to + 2 > $total) }"; then
  echo "the far end stops before the local voice does: pick a faster LOCAL_VOICE"
  exit 1
fi

pids=()
cleanup() {
  for pid in "${pids[@]}"; do kill -INT "$pid" 2>/dev/null || true; done
  wait 2>/dev/null || true
}
trap cleanup EXIT

published() { # log file, seconds to wait
  for _ in $(seq $(($2 * 10))); do
    grep -q "voice track published" "$1" && return
    sleep 0.1
  done
  cat "$1"
  exit 1
}

"$OUT/record-mic" 2>"$OUT/mic.log" \
  | "$BIN" listen --room "$ROOM" --label "$LABEL" --out "$RESULTS" --echo \
    --local-voice "$alone_from:$alone_to" --local-voice "$both_from:$both_to" \
    --wav "$RESULTS/$LABEL-wav" >"$OUT/listen.log" 2>&1 &
listener=$!
"$BIN" view --room "$ROOM" --headless --audio mic ${device_flags[@]+"${device_flags[@]}"} \
  >"$OUT/view.log" 2>&1 &
pids+=($!)
published "$OUT/view.log" 15
# A moment of the room on its own before anyone talks.
sleep 2

echo "talking for ${total}s; keep the room quiet"
"$BIN" share --room "$ROOM" --capture pattern --codec h264 --audio file \
  --audio-file "$OUT/far.wav" >"$OUT/share.log" 2>&1 &
pids+=($!)
# The file starts as the track is published; everything else is timed from here.
published "$OUT/share.log" 15
(sleep "$alone_from"; exec afplay "$OUT/local.wav") &
pids+=($!)
(sleep "$both_from"; exec afplay "$OUT/local.wav") &
pids+=($!)
sleep "$total"
kill -INT "$listener"
wait "$listener" || true

result=$(ls -1t "$RESULTS/$LABEL"-[0-9]*.json | head -1)
jq -r '
  def r(x): if x == null then "-" else (x * 10 | round / 10 | tostring) end;
  def signed(x): if x == null then "-" elif x >= 0 then "+" + r(x) else r(x) end;
  .echo as $e
  | "far end, at the mic:   \(r($e.settled.mic_p95_dbfs)) dBFS",
    "back to the far end:   \(r($e.settled.returned_p95_dbfs)) dBFS   (loudest 10 ms \(r($e.settled.returned_loudest_dbfs)) dBFS)",
    "echo removed:          \(r($e.removed_db)) dB   (first 5 s: \(r($e.removed_settling_db)) dB)",
    "local voice:           \(r($e.local_voice.mic_p95_dbfs)) dBFS at the mic, \(r($e.local_voice.returned_p95_dbfs)) dBFS sent on (\(signed($e.local_voice_db)) dB)",
    "both talking:          \(r($e.double_talk.returned_p95_dbfs)) dBFS sent on (\(signed($e.double_talk_db)) dB against the local voice alone)",
    "  half of the time under: \(r($e.double_talk.returned_p50_dbfs)) dBFS (alone: \(r($e.local_voice.returned_p50_dbfs)) dBFS)",
    "room in the pauses:    \(r($e.quiet.mic_p95_dbfs)) dBFS over \(r($e.quiet.seconds)) s (\(if $e.room_quiet == null then "too short to tell" elif $e.room_quiet then "quiet" else "not quiet" end))"' "$result"
echo "results: $result"
echo "recordings: $RESULTS/$LABEL-wav/ (mic.wav: the room; far.wav: what the far end said; returned.wav: what it heard back)"
verdict=$(jq -r '.echo.verdict' "$result")
prefix=$(echo "$verdict" | tr '[:lower:]' '[:upper:]')
[[ $verdict == pass ]] && prefix=NOTE
jq -r '.echo.problems[]' "$result" | sed "s/^/$prefix: /"
case $verdict in
  pass) echo "PASS" ;;
  inconclusive) exit 2 ;;
  *) exit 1 ;;
esac
