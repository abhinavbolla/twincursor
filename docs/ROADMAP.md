# Roadmap

TwinCursor is built in stages. Each one ends with something you can run and demo, and we work on
one at a time. The first public release comes once builds are signed and the hands-on checks
below pass.

## Done

**Sharp, low-latency screen sharing.** A native pipeline from ScreenCaptureKit through LiveKit to a
native viewer, measured on real networks.

- 1080p at 60 fps from Mac to Windows over home Wi-Fi, with sharp text
- H.264 by default, VP8 as a fallback, and resolutions up to 1440p and native
- 4K at 60 fps with HEVC ([numbers](performance.md#4k))
- Auto-throttle that keeps text readable when bandwidth drops
- Send only the size the viewer shows, so a 4K sharer doesn't spend 4K on a smaller window
  ([numbers](performance.md#a-4k-sharer-and-a-smaller-viewer))
- Share one app window instead of the whole screen (terminal tools), and leave our own windows
  out of the capture
- Call health stats and an on-screen HUD
- Latency and text-quality measurement, and a Docker network lab with pass/fail targets

The results are in [performance.md](performance.md).

**Remote control on macOS.** The guest uses the sharer's Mac with their own mouse and keyboard,
with consent built in.

- The input path measured, and its transport chosen ([ADR 0003](adr/0003-input-transport.md))
- Input event messages and the control session, with the keyboard model
  ([ADR 0004](adr/0004-keyboard-model.md))
- Grant and revoke control, with a global revoke hotkey (⌃⌥⌘R), and real input injection
- A label and a name tag on the sharer's screen while a guest is in control
- Trusted control: the sharer lets one guest take control without a prompt for the rest of the
  call ([ADR 0010](adr/0010-trusted-control.md))

**Voice.** Both sides talk through their mic and speakers, so pairing needs no second app
([ADR 0006](adr/0006-voice.md)).

- Voice latency measured without a mic, and the screen shown not to wait for the voice
- Real mic and speakers on both sides, with device selection and devices that change mid-call
- Echo cancellation and noise suppression, checked offline and through real speakers
- Mute on both sides

**The menu bar app.** A Tauri shell that starts the engine as a second process from inside the
app bundle ([ADR 0007](adr/0007-shell-and-engine.md), [the UI](ui.md)).

- A panel under the menu bar icon, and a call bar that stays on top during a call
- Permissions asked for when first needed: Screen Recording, Microphone, Accessibility
- Let in and Allow wait a second after appearing, so a stray click can't answer them
- A bundled app and disk image, with a backend built in and no LiveKit secret
- A Windows viewer package for the terminal, and a one-page guide for testers

**The app on Windows, as a guest.** The same tray app on Windows 10 and 11, for joining a Mac's
call: sign in with GitHub, paste a link or answer a ring, wait to be let in, watch, talk, take
control. It can't share. Built, unit-tested and started in CI on every change (alone, and as a
guest joining a room on a LiveKit dev server), and shipped as a zip.

**Accounts and calls.** A Supabase backend instead of a server of our own
([ADR 0011](adr/0011-accounts-and-call-passes.md)).

- Sign in with GitHub
- Call links: the host shares, the guest pastes the link and waits, the host lets them in, with
  control or without
- Contacts: people you've been in a call with, whom you can ring
- Call passes: a short-lived LiveKit token for each person and call, so no secret ships in the
  app
- Self-hosting: a [guide](self-hosting.md) to running it all on your own Supabase project and
  LiveKit server

## Now

**Hands-on checks.** Things only people at real machines can confirm.

- [ ] A call between two laptops on different networks, at 4K
- [ ] The macOS permission prompts name TwinCursor in the bundled app, and its windows stay out
  of the shared screen
- [ ] Edit a real file on the sharer's Mac from another computer
- [ ] A call over speakers on both ends, without headphones
- [ ] The Windows app on a real PC: sign in, join by link and by a ring, watch, talk, take
  control; the tray icon and the panel's place on the taskbar, at 100 % and other scales

## Next

**First public release on macOS.**

- [ ] Signed and notarized builds, and auto-update
- [ ] Voice mouth to ear under 150 ms (about 180–190 ms today): 10 ms Opus frames and a smaller
  macOS audio device layer, both below the LiveKit SDK ([budget](performance.md#where-the-time-goes))
- [ ] Live bitrate control without republishing the track, and FEC for lossy links
- [ ] 4K on a real 4K display
- [ ] Encoder tuning, sharpening and a faster Metal or wgpu viewer
- [ ] Real-world checks: Mac to Mac, Mac on Ethernet, Network Link Conditioner profiles
- [ ] True glass-to-glass latency with an on-screen marker (`--marker screen`)
- [ ] Presence and instant ringing, instead of polling every two seconds
- [ ] Call links that open the app when clicked
- [ ] Landing page

**After that.**

- [ ] Camera
- [ ] End-to-end encryption, so the LiveKit server relays media it can't read
- [ ] Sharing from Windows (screen capture and input injection there), and a signed installer
- [ ] Teams

## Then: your own devices

Connect to your own computers when nobody is at them, from a laptop or a phone, with the same
latency and sharpness as pairing. The consent model is in
[ADR 0005](adr/0005-unattended-access.md) (proposed).

- [ ] Enroll devices and clients to your account, with a second factor
- [ ] A background agent that accepts your enrolled clients while you're signed in to the computer
- [ ] An on-screen label, a start sound and a local log for every unattended session
- [ ] Direct peer-to-peer connections, with the server as a relay only when needed
- [ ] Multiple monitors, clipboard sync, file drop and sound from the remote computer
- [ ] Mobile viewer for iOS and Android: native decoding, a trackpad mode alongside direct touch,
      pinch to zoom that follows the cursor, and a keyboard with a modifier bar

## Later

Linux (Wayland and X11), multiple cursors with three or more people, annotations, and editor
integrations.
