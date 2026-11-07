# TwinCursor: Product

## One line

Pair program with someone remotely as if you were sitting at the same desk: you see their
screen with no noticeable lag, you talk, and either of you can take the keyboard and mouse.

## Why

- **Tuple** is excellent but closed-source and paid per seat.
- **Hopp** (AGPL) is the main open-source alternative. It's macOS-first, with Windows paused
  and Linux in alpha as of 2026.
- Generic tools (Zoom, Meet, Slack huddles) aren't built for this: too much lag, blurry text,
  and remote control is clumsy or missing.

TwinCursor aims to be a fast, self-hostable, open pair-programming tool that also treats Linux
as a real platform.

## Who it's for

- Developers pairing remotely, mostly in twos and sometimes in small groups.
- Teams who want to self-host (privacy, compliance, cost).
- Open-source maintainers pairing with contributors for free.

## Core experience (v1)

1. Start a call and send a link, or click a teammate's name.
2. Talk right away (voice first, camera optional).
3. Share a screen or a single window. The viewer gets crisp, readable text at low latency.
4. The sharer grants control. The guest gets their own cursor and can type and click on the sharer's machine.
5. Either person can revoke control instantly with a global hotkey.

## Quality bar

- Glass-to-glass latency under 100 ms on a good network at 1080p. Code text must be
  readable without zooming.
- Remote typing feels local: no dropped or reordered keystrokes, and correct keyboard
  layouts.
- Low CPU: pairing for hours must not make the laptop fans spin up.

## After v1: your own devices

The same engine can connect you to your own computers when nobody is at them: your desk Mac from
a laptop, or from your phone on the couch. Free remote-desktop tools do this, but they're often
laggy, blurry, and awkward on a phone. TwinCursor should feel like
sitting at the computer, with a mobile app that's built for touch, not a desktop squeezed onto a
small screen.

Consent works differently here, because nobody is at the computer to accept a request. The owner
enrolls each device and client in advance, and the device always shows when it's being accessed.
See [ADR 0005](adr/0005-unattended-access.md).

## Not in v1

Recording, chat, whiteboard, calls with more than 4 people, and an AI pair (maybe later). Mobile
apps and access to your own devices come after v1 (above).

## Open questions

- Accounts: email magic link, GitHub OAuth, or both?
- Hosted offering? If yes, how do we fund it (donations, paid hosted tier)?
- Linux: when to start, and Wayland (PipeWire + libei) vs X11 priority.
