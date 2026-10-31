# ADR 0003: Transport for remote-control input

Status: accepted (2026-10-31)

## Context
Remote-control input goes from the guest's viewer, through the LiveKit server, to the sharer. A
lost or reordered key corrupts what the guest types, while a stale pointer move is only
worthless. LiveKit offers two data channels: reliable (ordered, retransmits) and lossy
(unordered, no retransmits). We measured both with video running
([results](../performance.md#input-path)):

- Both add under 1 ms on top of the network.
- Under loss, reliable has a retransmission tail and lossy doesn't.
- In a stall, lossy doesn't drop old packets. They queue and arrive up to 2.4 s late after a 2.5 s
  stall, in order, so a sequence number can't identify them. Reliable can recover more slowly than
  the stall itself: a 1.2 s stall usually delayed input by about 2.8 s.

Both channels share one SCTP connection, which may be why lossy waits during a stall. We haven't
confirmed that.

## Decision
- **Keys, clicks and scrolls use the reliable channel.** Each click carries its own position, so
  it can't land in the wrong place by overtaking a move.
- **Pointer moves use the lossy channel,** with a sequence number to drop reordered moves.
- **Moves carry the viewer's timestamp.** The sharer keeps the smallest arrival-minus-timestamp as
  a baseline, which needs no shared clock, and drops any move more than 150 ms over it.
- **Held input has a deadman.** With no heartbeat for 1 s, the sharer releases every held key and
  button. After that, late key-ups are no-ops and a late button-down doesn't start a drag. Keys
  typed during the stall still apply, in order.

## Consequences
- The guest's cursor can't replay old movement after a stall.
- Typed text survives stalls, but can arrive seconds late. The viewer should show that the link is
  stalled, so the guest doesn't type blind.
- A drag can end early during a stall of more than 1 s. That's the safe failure: a stuck button or
  modifier on the sharer's machine is worse.
- The 150 ms limit is a starting point, to be tuned by hand once remote control works.
- LiveKit's data tracks may avoid queueing for moves. Revisit this if they do.
