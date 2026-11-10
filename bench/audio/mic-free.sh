# Sourced by the scripts that play sound out loud. Stops when another app is recording from a
# microphone: most likely a call, and the people on it would hear everything we play.
# FORCE=1 runs anyway.
if [[ ${FORCE:-0} != 1 ]]; then
  recording=$(pmset -g assertions 2>/dev/null | grep -B1 "audio-in" \
    | sed -n 's/.*Created for PID: \([0-9]*\).*/\1/p' | head -1 || true)
  if [[ -n $recording ]]; then
    app=$(ps -p "$recording" -o comm= 2>/dev/null || true)
    echo "${app:-another app} is using the microphone (a call?), and this plays sound out loud."
    echo "Run it when the mic is free, or with FORCE=1."
    exit 1
  fi
fi
