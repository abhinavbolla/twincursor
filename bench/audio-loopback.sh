#!/usr/bin/env bash
# Voice delay through real speakers and a real mic, on one Mac.
#
# The sharer sends marker tones (`--audio pattern`). The viewer plays them on the speakers with
# the real device layer (`--audio mic`, which also sends its own mic, echo-cancelled). A raw mic
# recording (bench/audio/record-mic.swift) goes to `tc-spike listen`, which times each burst
# from the moment the sharer started it. The recorder's input buffer stands in for the talker's
# mic, so the result is close to mouth to ear: codec, network, jitter buffer, speakers, air, mic.
# The viewer also reports when each burst left the jitter buffer, which splits the device and
# air part from the rest.
#
# Needs `livekit-server --dev` running, a release build, microphone access for this terminal,
# the volume up, and a quiet room for about $DURATION seconds: it plays tones out loud.
#   bench/audio-loopback.sh            DURATION=30 LABEL=loopback by default
#   SPEAKER=1 MIC=0 bench/audio-loopback.sh    pick devices (see `tc-spike devices`)
set -euo pipefail
cd "$(dirname "$0")/.."

DURATION=${DURATION:-30}
LABEL=${LABEL:-loopback}
BIN=target/release/tc-spike
RESULTS=bench/results
ROOM=loopback-$$
OUT=$(mktemp -d)

device_flags=()
[[ -n ${SPEAKER:-} ]] && device_flags+=(--speaker "$SPEAKER")
[[ -n ${MIC:-} ]] && device_flags+=(--mic "$MIC")

swiftc -O -o "$OUT/record-mic" bench/audio/record-mic.swift

pids=()
cleanup() {
  for pid in "${pids[@]}"; do kill -INT "$pid" 2>/dev/null || true; done
  wait 2>/dev/null || true
}
trap cleanup EXIT

"$BIN" share --room "$ROOM" --capture pattern --codec h264 --audio pattern >"$OUT/share.log" 2>&1 &
pids+=($!)
for _ in $(seq 30); do
  grep -q "voice track published" "$OUT/share.log" && break
  sleep 0.5
done
grep -q "voice track published" "$OUT/share.log" || { cat "$OUT/share.log"; exit 1; }

"$OUT/record-mic" 2>"$OUT/mic.log" \
  | "$BIN" listen --room "$ROOM" --duration $((DURATION + 3)) --label "$LABEL-mic" \
    --out "$RESULTS" >"$OUT/listen.log" 2>&1 &
listener=$!
sleep 1

echo "playing tones for ${DURATION}s; keep the room quiet"
"$BIN" view --room "$ROOM" --headless --audio mic ${device_flags[@]+"${device_flags[@]}"} --measure-audio \
  --duration "$DURATION" --label "$LABEL-viewer" --out "$RESULTS" >"$OUT/view.log" 2>&1
wait "$listener" || true

mic=$(ls -1t "$RESULTS/$LABEL"-mic-*.json | head -1)
viewer=$(ls -1t "$RESULTS/$LABEL"-viewer-*.json | head -1)
jq -rn --slurpfile m "$mic" --slurpfile v "$viewer" '
  def r(x): if x == null then "-" else (x * 10 | round / 10 | tostring) end;
  $m[0] as $m | $v[0] as $v
  | ($m.audio.one_way // {}) as $air | ($v.report.audio.one_way // {}) as $out
  | "sharer → heard in the room:      p50 \(r($air.p50)) ms   p95 \(r($air.p95)) ms   (\($air.count // 0) bursts)",
    "sharer → left the jitter buffer: p50 \(r($out.p50)) ms   p95 \(r($out.p95)) ms   (\($out.count // 0) bursts)",
    "speakers, air and mic:           p50 \(r(if $air.p50 and $out.p50 then $air.p50 - $out.p50 else null end)) ms",
    "loudest sound at the mic:        \(r($m.loudest_dbfs)) dBFS"'
heard=$(jq '.audio.one_way.count // 0' "$mic")
sent=$(jq '.report.audio.one_way.count // 0' "$viewer")
if (( heard * 2 < sent )); then
  echo "FAIL: the mic heard $heard of $sent bursts: turn the volume up or quiet the room"
  exit 1
fi
echo "results: $mic $viewer"
