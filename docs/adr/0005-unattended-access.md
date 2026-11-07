# ADR 0005: Unattended access to your own devices

Status: proposed (2026-11-07)

## Context
TwinCursor is built for pairing: someone sits at the shared computer and grants control to a
guest. The same engine can also connect you to your own computers when nobody is at them, for
example your desk Mac from a laptop or a phone. Generic remote-desktop tools do this, but they
tend to be laggy, blurry, and awkward on a phone.

Today's consent rule assumes a person at the shared computer who can accept a request. With
unattended access, nobody is there to accept. So we need a different source of consent that is
just as strict, not a way around consent.

## Decision
- **Consent is given once, at enrollment, by the owner at the device.** Enrolling a device needs
  someone at that computer, signed in to the owner's account, to turn on "Allow access to this
  device" and confirm it with the operating system's own authentication (Touch ID or the login
  password). The device then creates a key pair. The private key never leaves it.
- **Only the owner's account can connect unattended.** The device accepts a session only after the
  server proves who you are, and the device checks a signature from one of your enrolled
  clients. A room link or a guest invite is never enough. Sharing a device with someone else uses
  the normal pairing flow, which needs a person at it to grant control.
- **Each client is enrolled too.** A new laptop or phone has to be approved from a device that's
  already enrolled, or by signing in again with a second factor. You can see and remove enrolled
  clients from any device.
- **The device always shows that it's being accessed.** A label stays on screen for the whole
  session, saying which client is connected, and a sound plays when a session starts. Anyone at the
  computer can end it with the same global revoke hotkey as in pairing, and that hotkey also turns
  unattended access off until the owner turns it on again.
- **If someone is at the computer, they're asked.** When there has been local input in the last few
  minutes, the device asks the person there to accept, as in pairing. The owner can make the device
  always ask.
- **Everything is logged on the device.** Each session's client, start and end time stay in a local
  log that a remote session can't clear.
- **Off by default, and easy to turn off.** Unattended access is a per-device setting that starts
  off. Signing out, removing the device, or the revoke hotkey turns it off.

The existing pairing rules stay as they are. This adds a second way to grant control, limited to
your own devices.

## Consequences
- Hard rule 4 in `CLAUDE.md` and the ground rules in `CONTRIBUTING.md` need a sentence for this
  case when this ADR is accepted: control is granted either live by the person at the computer, or
  in advance by the owner for their own enrolled clients.
- The threat model needs a new row: someone who has taken over your account. Client enrollment and
  the second factor are what stop them, so those have to be built before unattended access ships.
- The device needs a background agent that runs whenever you're signed in to the computer, not
  only when the app is open. Reaching the macOS lock screen or login window would need a
  privileged background service. That's a separate decision, and the first version needs a
  signed-in user.
- Connecting to your own device is one to one, so a direct peer-to-peer connection, with the
  server only as a relay, can save a network hop. That's a separate ADR, with measurements.
- The server running TwinCursor can still see what passes through it until end-to-end encryption
  exists, as in pairing.
