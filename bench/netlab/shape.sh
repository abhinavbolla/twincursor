#!/bin/sh
# Shapes this container's outgoing traffic on eth0 with tc netem.
#   shape clear
#   shape set  [--to IP] NETEM_ARGS...   e.g. shape set --to 172.30.0.30 delay 20ms 5ms loss 1%
#   shape change [--to IP] NETEM_ARGS... (same, without resetting first; for timed steps)
#   shape block-udp | unblock-udp       (forces ICE over TCP)
# With --to, only packets to that IP are shaped (a prio band with a u32 filter); otherwise all.
#
# A "rate X" argument is not given to netem: netem's own rate combined with jitter computes
# send times from jittered timestamps, builds a backlog and tail-drops far more than asked
# (we saw 40% loss at a fraction of the rate). Rate limiting goes to a token bucket (tbf)
# under netem instead, with a queue of about 80 ms like a home router's.
set -eu
dev=eth0
cmd=${1:-}; shift || true

to=
if [ "${1:-}" = "--to" ]; then to=$2; shift 2; fi

# Split "rate X" out of the netem arguments.
rate=
netem_args=
while [ $# -gt 0 ]; do
  if [ "$1" = "rate" ]; then rate=$2; shift 2; continue; fi
  netem_args="$netem_args $1"; shift
done
# netem needs at least one parameter.
[ -n "$netem_args" ] || netem_args=" delay 0ms"
tbf() { tc qdisc "$1" dev $dev parent "$2" handle "$3" tbf rate "$rate" burst 15kb latency 80ms; }

case $cmd in
  clear)
    tc qdisc del dev $dev root 2>/dev/null || true
    iptables -D OUTPUT -p udp -j DROP 2>/dev/null || true
    ;;
  set)
    tc qdisc del dev $dev root 2>/dev/null || true
    # shellcheck disable=SC2086
    if [ -n "$to" ]; then
      tc qdisc add dev $dev root handle 1: prio bands 4
      tc qdisc add dev $dev parent 1:4 handle 40: netem $netem_args
      [ -n "$rate" ] && tbf add 40:1 41:
      tc filter add dev $dev parent 1: protocol ip prio 1 u32 match ip dst "$to"/32 flowid 1:4
    else
      tc qdisc add dev $dev root handle 1: netem $netem_args
      [ -n "$rate" ] && tbf add 1:1 10:
    fi
    true
    ;;
  change)
    # shellcheck disable=SC2086
    if [ -n "$to" ]; then
      tc qdisc change dev $dev parent 1:4 handle 40: netem $netem_args
      [ -n "$rate" ] && tbf change 40:1 41:
    else
      tc qdisc change dev $dev root handle 1: netem $netem_args
      [ -n "$rate" ] && tbf change 1:1 10:
    fi
    true
    ;;
  block-udp)
    iptables -A OUTPUT -p udp -j DROP
    ;;
  unblock-udp)
    iptables -D OUTPUT -p udp -j DROP 2>/dev/null || true
    ;;
  *)
    echo "usage: shape clear | set [--to IP] ARGS | change [--to IP] ARGS | block-udp | unblock-udp" >&2
    exit 2
    ;;
esac
