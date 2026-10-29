#!/usr/bin/env bash
# Summarises network lab results (bench/results/netlab/*.json) as a markdown table and checks
# them against the SLOs in docs/ARCHITECTURE.md. Uses the newest result per label.
set -euo pipefail
cd "$(dirname "$0")/../results/netlab"

# Added network RTT per profile (ms), from run.sh's delays. (A function, not an associative
# array: macOS ships bash 3.2.)
net_rtt() {
  case $1 in
    wifi-good) echo 4 ;; wifi-bad) echo 16 ;; 4g) echo 50 ;; hotel) echo 80 ;;
    intercontinental) echo 150 ;; sharer-uplink-3mbit) echo 10 ;; bandwidth-drop) echo 5 ;;
    *) echo 0 ;;
  esac
}

echo "| run | fps | RTT p50 | RTT p95 | SSIM p5 | fast-scroll SSIM p5 | loss | freezes | longest stall | path | SLO |"
echo "|---|---|---|---|---|---|---|---|---|---|---|"
for label in $(ls -1 *.json 2>/dev/null | sed -E 's/-[0-9]+\.json$//' | sort -u); do
  # Exact label: "4g-vp8-1080" must not pick up "4g-vp8-1080-noadapt".
  f=$(ls -1t | grep -E "^${label}-[0-9]+\.json$" | head -1)
  # label = <profile>-<codec>-<res>[-<tag>][-noadapt]; profiles have no digits-only parts.
  profile=$(echo "$label" | sed -E 's/-(vp8|h264|vp9|av1)-.*$//')
  net=$(net_rtt "$profile")
  jq -r --arg profile "$profile" --arg label "$label" --argjson net "$net" '
    def r(x): if x == null then "-" else (x * 10 | round / 10 | tostring) end;
    def r4(x): if x == null then "-" else (x * 10000 | round / 10000 | tostring) end;
    [.net[] | .local.recv] as $recv
    | ([$recv[] | select(. != null) | .loss_pct] | if length > 0 then add / length else null end) as $loss
    | ([$recv[] | select(. != null) | .freezes] | add // 0) as $freezes
    # Longest run of seconds with under 5 fps received (a stall).
    | (reduce ([.net[] | (.local.recv.fps // 0) < 5])[] as $s ({cur: 0, max: 0};
        if $s then {cur: (.cur + 1), max: ([.max, .cur + 1] | max)} else {cur: 0, max: .max} end)
        | .max) as $stall
    | .report.rtt_presented as $rtt
    | .report.ssim as $ssim
    | (if ($profile == "outage-5s") then ($stall <= 8)
       elif ($profile == "bandwidth-drop") then ($stall <= 3)
       elif ($profile == "udp-blocked") then ($rtt != null)
       elif ($profile == "lan" or $profile == "wifi-good") then
         ($rtt != null and $rtt.p50 <= $net + 25 and $rtt.p95 <= $net + 60
          and $ssim != null and $ssim.p5 >= 0.97 and $stall == 0)
       elif ($loss != null and $loss <= 2) then ($freezes <= (.duration_s / 60))
       else true end) as $ok
    | "| \($label) | \(.avg_fps | floor) | \(r($rtt.p50)) | \(r($rtt.p95)) | \(r4($ssim.p5)) | \(r4(.report.ssim_fast_scroll.p5)) | \(r($loss))% | \($freezes) | \($stall) s | \([.net[] | .local.path // empty] | last // "-") | \(if $ok then "pass" else "**FAIL**" end) |"
  ' "$f"
done
