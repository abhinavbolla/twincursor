# TwinCursor: Architecture

Status: living document. It describes what exists today: the menu bar app, the engine (media
path, voice, remote control), the Supabase backend for accounts and calls, and the measurement
tools. Validate each decision with a spike before building on it.

## Overview

```
 Sharer (desktop app)                                   Guest (desktop app)
 ┌──────────────────────────────┐                      ┌──────────────────────────────┐
 │ Tauri UI (webview)           │                      │ Tauri UI (webview)           │
 │  panel · call bar            │                      │  panel · call bar            │
 ├──────────────────────────────┤                      ├──────────────────────────────┤
 │ Rust core                    │   video/audio (RTP)  │ Rust core                    │
 │  capture → encode ──────────────────► LiveKit SFU ──────► decode → native viewer   │
 │  inject input ◄────────────────── data channel ◄───────── capture local input      │
 │  cursor overlay window       │                      │  (mouse, keys, scroll)       │
 └──────────────────────────────┘                      └──────────────────────────────┘
                 ▲                                                    ▲
                 └──── HTTPS: sign-in, contacts, calls, call passes ──┘
                        Supabase (Auth, Postgres, call-pass function)
```

The Rust core is split into crates:
- `crates/media`: capture, encoding, the LiveKit session, voice and the latency and quality
  measurements.
- `crates/control`: the rules of remote control (consent, input checks, the deadman, revoke) and
  input injection, with no network or window code.
- `crates/engine`: what has to own the main thread (the viewer window, the sharer's global
  hotkeys and "is controlling" overlay), the viewer's input mapping, and the control threads on
  both sides.
- `crates/media-spike` builds `tc-spike`, a command line over the engine that every bench script
  drives.
- `crates/engine-cli` builds `tc-engine`, the engine process the desktop app starts. It speaks
  the commands and events in `crates/ipc` as JSON lines on stdin and stdout; see
  [the IPC protocol](ipc.md).
- `crates/account` is the client for the backend: GitHub sign-in, the session, and the calls the
  app makes. It also builds `tc-account`, for trying a backend from the terminal
  ([accounts](accounts.md)).
- `supabase/` is the backend itself: the database migration and the `call-pass` function
  ([Accounts and calls](#6-accounts-and-calls)).
- `apps/desktop` is the menu bar app ([The desktop app](#the-desktop-app)).

The engine knows nothing about accounts. The app signs in, asks the backend who may join, and
starts the engine with a call pass in its environment.

## Key decisions

### 1. Tauri 2 for the app shell, Rust for all media
- Tauri keeps the app small and gives us a Rust process for free.
- The webview **only** renders app UI. Hopp ran WebRTC inside Tauri's WebKit webview and ran
  into audio glitches, browser codec limits, no access to frame buffers, and no WebRTC in the
  default WebKitGTK build (which blocks Linux). They are now migrating media to Rust. We start there.
- The shared screen is shown in a **native window** we render ourselves. That also lets us
  apply render-side sharpening and upscaling. Today it uses winit + softbuffer (CPU conversion),
  which costs about 9–11 ms per frame. The real viewer should render the decoder's
  CVPixelBuffer with Metal/wgpu.
- Capture on macOS uses ScreenCaptureKit directly (ADR 0001).

#### What is captured
- **One display per share.** A share is about one display, chosen once when it starts
  (`engine::screen::choose`): the main display (the one with the menu bar, `CGMainDisplayID`),
  or the one `--display <id>` names (`tc-engine displays` / `tc-spike displays` list them). It
  is found among ScreenCaptureKit's displays by its id; if ScreenCaptureKit doesn't list it, the
  share fails with a message instead of capturing another display. That id is handed to
  everything that needs a screen: the capture (`CaptureConfig::display`), the injector
  (`sharer::Surface::Display`), the label and the guest-pointer tag (`SharerUi::new`), the size
  the tiers start from (`capture::display_pixels`), and the permissions check. None of them
  picks a display on its own. ScreenCaptureKit's `displays()` comes in no promised order, and
  its first entry isn't necessarily the main display.
- **The whole display** (the default) leaves out every TwinCursor window: the sharer's own
  (such as the remote-control label), the desktop app that started it (`--exclude-app-pid`,
  ADR 0007) and any other TwinCursor process, so a viewer on the same Mac doesn't show the
  picture inside itself. A process counts as TwinCursor if it is this process, runs the same
  executable, or has one of our bundle identifiers or executable names
  (`media::capture::select::is_twincursor`). ScreenCaptureKit can only leave out apps that
  have a window when the filter is made, so the sharer looks again every second and updates
  the filter when one appears or quits: a viewer started after the sharer can appear in the
  stream for up to a second. `tc-spike windows` never lists TwinCursor windows either.
- **One window** (`share --window`): ScreenCaptureKit's single-window filter, which captures the
  window whatever covers it, title bar included. The window is sent at its own pixel size
  (points × the display's scale), through the same tier rules as a display. When it fits the
  tier, it is captured at exactly that size and cropped to the encoder's size (a few pixels at
  the right and bottom edges), so every pixel arrives 1:1; scaling by a fraction of a percent
  would blur all of them. The sharer checks the window's size five times a second; once a new
  size has held for 0.6 s (so dragging an edge reconfigures once, after the drag), capture is
  reconfigured for it and the send size changes through the same path the auto-throttle uses.
  A minimised window leaves the last picture on screen until it comes back, and closing the
  window (or quitting its app) ends the share with a message. A query that matches no window,
  or several, lists them and exits rather than guessing. A number is only ever a window id: if
  no window has it, the share fails without falling back to a title that contains the number.
- **One app** (`share --app`): the display filter that includes only that app's windows, at the
  display's size.
- **No control while one window or app is shared.** Input is mapped onto a whole display, so a
  click on the window's picture would land somewhere unrelated. `--allow-control` with
  `--window` or `--app` is accepted, but the control session is closed from the start
  (`Controller::close`): every request is turned down with "Control isn't available while one
  window is shared.", which the guest and the sharer both see, every trust is refused with the
  same sentence, and nothing is injected. That holds until input is mapped onto windows. The
  protocol's `Deny` carries the reason; peers from before it ignore it.
- The viewer asks for a window whose inner size in physical pixels is the frame's, and
  softbuffer draws into a layer at the screen's scale, so a frame that fits on the monitor is
  shown one stream pixel per screen pixel. Its HUD says "shown 1:1" or "scaled to N%". When
  the frame size changes it resizes itself again, unless the guest has resized it by hand
  (any size it didn't ask for); then frames are drawn into the size they chose. The sharer
  sends no more than the window can show (see [Sizing to the viewer](#sizing-to-the-viewer)),
  so a frame a few pixels smaller than the window, from rounding, is drawn 1:1 and centred
  with thin black bars rather than stretched by a fraction of a percent, which would blur every
  line of text. Frames are scaled only when larger than the window, or at least 5% smaller.

#### The desktop app
`apps/desktop` is a menu bar app with two small windows: the **panel**, which hangs from the
menu bar icon (sign in, Share my screen, Paste link, people), and the **call bar**, which is on
screen, on top and on every Space for as long as a call is ([the UI](ui.md)). It starts the
engine, `tc-engine`, for each call and shows only text and controls; the shared screen is the
engine's own native window (ADR 0007, and [the IPC protocol](ipc.md)).

- **Windows** (`windows.rs`, with the rules in `surfaces.rs`). Both windows are made hidden at
  launch and are real titled windows whose title bar shows nothing (no title, no traffic lights;
  the web content fills them), so macOS draws their rounded corners, hairline and shadow; no
  transparent windows and no private API. Each is as tall as its page says (`fit_window`),
  keeping its top edge. The panel hides when it loses focus, except while a sign-in or a
  permission prompt it started is up. The call bar is shown without becoming key, so the app the
  person is typing in keeps the keyboard, and the time it came on screen is when arming starts.
  Closing windows never quits; Quit (the panel's menu, the icon's menu, ⌘Q) ends or leaves the
  call first.
- **Rust side** (`src-tauri`). `engine.rs` builds the engine's flags (role, quality, voice, and
  the room and name for a room-code call), starts it with its stdin held open for the call, reads
  every stdout line as an event and every stderr line as a log line, and ends it: `leave`, then
  closing stdin, then killing it, each only if the step before didn't work within a few seconds.
  It holds no Tauri types, so its lifecycle is tested against a stand-in engine script. A
  sharer's engine always gets `--allow-control` (requests reach the window; nothing is granted
  until the person clicks Allow for that request) and `--exclude-app-pid` with the app's own
  process id, so the app's windows are never in the picture. A control request makes sure the
  call bar is on screen, without focusing it. The window's commands are typed (`ipc::Command`),
  so only commands the protocol knows reach the engine.
- **Calls with accounts** (`calls.rs`, [calls](calls.md)). Share, join by link, let in, ring and
  leave run in the Rust side, which calls the backend through `crates/account`, starts every
  engine with a call pass in its environment (no `--room` or `--name`), and keeps one view of it
  all that it sends to the window on every change (the `home` event). It enforces arming: a
  knock can't be let in, and a control request can't be allowed, within a second of it first
  being on screen (delivered to the window, with the call bar visible), so a click or key press
  already on its way can't do it. For "Let in with control" it sends the engine
  `trust_on_arrival` once, and the engine keeps that promise, so every revoke cancels it; the window itself can't send a trust. It polls the backend every two
  seconds while signed in (the host's heartbeat and the guest's knock), backing off to ten after
  repeated network failures.
- **Quality.** 1080p and 1440p share with H.264; 4K with HEVC, which calls with accounts always
  use. When a guest announces decoders without HEVC, the app restarts its engine with H.264 (same
  room, name and resolution; a fresh call pass for a call with accounts) and stays on H.264 for
  the rest of the call; a guest that hasn't announced yet changes nothing. An HEVC engine that
  fails before sending any video falls back the same way, once. Control doesn't carry over to the
  new engine, and a command meant for the old one never reaches it.
- **Getting into a room.** The engine joins with a **call pass** when its environment has one
  (`TWINCURSOR_PASS`): a LiveKit token the account server signed for one identity
  (`sharer-<account id>` or `guest-<account id>`), one name and one room. The engine reads the
  pass's claims to check it fits the call (role, room, expiry) but doesn't verify its signature:
  it holds no secret, and LiveKit verifies the pass on connect. Without a pass, it mints its own
  token from an API key and secret for `--room` and `--name`, as below. See
  [IPC](ipc.md#getting-into-the-room).
- **Settings.** The server the build baked into `tc-engine` is the default (`tc-engine config`).
  Overrides are saved by the Rust side as JSON in the app's config folder (mode 0600) and handed
  to a room-code call's engine in its environment, never its flags (calls with accounts take
  their server from the call pass, and only scripts start room-code calls now). The window learns whether a secret is set,
  never its value.
- **Permissions.** Each is asked for when it is first needed, in a fresh `tc-engine
  permissions` process since macOS applies a new grant only to processes started after it:
  `--request screen_recording` on Share, `accessibility` on giving control. Sharing is blocked
  only by a missing Screen Recording grant; joining never is. While one is missing, the panel or
  the bar offers to open its System Settings pane.
- **Webview** (`src`, React). A pure reducer turns the engine's output into call state; it is
  tested against `crates/ipc/fixtures`. Outside Tauri, in development builds only, a mock engine
  and a page with every state of the panel and the bar let them be checked in a browser.
- **Script mode.** `twincursor-desktop --script PATH --report PATH` shares the synthetic pattern
  with voice off and input recorded, uses a clipboard in memory, answers knocks and control
  requests as the script says (through the same arming checks as a click), and writes what it
  sees as JSON lines, including every `home` view. It is the only path that answers a request
  without a click, exists only behind that flag, and can't inject real input or touch the real
  clipboard. Its windows exist but stay hidden unless the script sets `"windows": true`.
  `bench/app-e2e.sh` and `bench/app-accounts-e2e.sh` use it.

### 2. WebRTC through LiveKit (self-hostable SFU)
- LiveKit's Rust SDK (`livekit` crate, built on libwebrtc) gives us congestion control,
  NAT traversal, simulcast, and data channels without building our own.
- Self-hosting is a LiveKit server (LiveKit Cloud or `livekit-server`) next to a Supabase project
  ([self-hosting](self-hosting.md)).
- Peer-to-peer without an SFU is an option for 1:1 calls later. Not v1.
- One LiveKit participant per person (screen, camera, and audio are tracks, not separate participants).

### 3. Encoding: tune for latency, choose per platform
Codecs are configurable (`tc-spike share --codec`). Measurements are in
[performance.md](performance.md):
- **H.264 (hardware) is the default** (ADR 0002). It matches VP8's latency on macOS at about
  1/5 of the sharer's CPU. Its text is measurably sharper (SSIM p5 0.9995 vs 0.98), and it
  uses ~4x less bandwidth.
- **VP8** is the software fallback.
- **VP9** was worse on every measure.
- Simulcast is off for screen share.
- **Resolution tiers** (`--resolution`): 720, 1080 (default), 1440, 2160 or native lines; the
  width follows the display's aspect ratio, rounded to a multiple of 16 and the height to a
  multiple of 4 so encoders don't crop (1662 became 1660, 1046 became 1044), and no tier
  upscales. On a 3024x1964 MacBook panel: 1104x720, 1664x1080,
  2224x1440, and 2160 is the panel's own 3024x1964. ScreenCaptureKit scales on the GPU. Default
  max bitrates are 2.5 / 5 / 8 / 15 Mbps, and 12 Mbps for native; the bandwidth estimate
  usually sets the actual target lower.
- **HEVC** (`--codec h265`) is opt-in for 4K, through VideoToolbox like H.264
  ([ADR 0008](adr/0008-hevc-for-4k.md)). H.264 stays the default.
- The tier is a ceiling: the sharer sends no more than the largest viewer can show, so the
  viewer shows frames 1:1 (sharpest text) wherever the source has the pixels (see
  [Sizing to the viewer](#sizing-to-the-viewer)).
- `is_screencast` showed no frame-rate cap and no latency difference.

Encoder tuning (Hopp's rate-control and buffer settings, [research note](research/hopp-lessons.md)) needs a
custom libwebrtc build. Do it only if real-network numbers call for it.

### 4. Remote control over a data channel
- The guest's viewer captures mouse, keyboard and scroll events and normalizes coordinates
  to the shared surface (0..1). It sends them over LiveKit data channels using
  `crates/protocol` messages:
  - clicks, keys and scrolls on the reliable, ordered channel: they must never drop or reorder;
  - pointer moves on the lossy channel: a lost move is replaced by the next one, and it avoids
    retransmission tails. Each click carries its own position, so it can't land in the wrong
    place by overtaking a move. ADR 0003 has the reasoning, including stale moves after a stall.
- The sharer validates the events (control granted? within the shared display?) and injects
  them with `enigo` (macOS CGEvent, Windows SendInput, Linux X11/libei). `crates/protocol`
  rejects malformed or out-of-range messages, and `crates/control` decides what gets through:
  the consent state, held keys and the deadman, stale moves, and the sharer winning the pointer.
  It has no network or platform code, so its tests are scripted traces on a fake clock.
- Consent: a request becomes a prompt (`control_asked` to the app, or `tc-spike`'s terminal), and
  only the sharer's answer to that request grants it. The one other grant is the sharer's own
  standing one, `trust_control` from the app ([ADR 0010](adr/0010-trusted-control.md)): for the
  rest of the call that guest's requests are granted without a prompt. `Controller` keeps the
  trusted identities; any revoke clears them all, in every state, and leaving clears that
  guest's. Trust is never written anywhere and no protocol message carries it. The names in the
  prompt, the label and the app's events come from each guest's token (with a call pass, the
  account's name), not from the guest's own hello.
- Media doesn't know what the bytes mean. `media::link` carries the `tc.control` topic both ways
  on tasks of its own, with its own copy of the room's events, so input never waits behind stats
  polls or republishing. It starts after the room is joined, so it reports the participants and
  a screen already there as if they had just arrived. The guest's side, `control::guest`, asks for control, sends moves at
  most 120 times a second (the newest position always goes out), heartbeats every 250 ms, and
  repeats an unanswered request every second. The link counts messages not yet sent, and the
  guest waits for them before leaving the room, so the end of a fast burst isn't cut off.
- **A joining viewer's first packets don't count.** LiveKit relays a viewer's data at once, but
  the sharer's room hears that the viewer joined only later (about 2 to 3 s on a local server,
  measured with both sides' logs). Until then the SDK hands each packet over without a sender,
  and the link drops it, because the sender's identity is what consent is checked against. It
  happens on both channels, reliable and lossy. It is one-way: a viewer always knows the sharer
  before it sees the screen, so the sharer's answers arrive. Nothing on the viewer's side shows
  when the window closes, so the viewer's hello (name, platform, decoders) is offered every
  250 ms until the sharer answers with `HelloAck`, then not again; every 2 s after 15 s
  unanswered, for a sharer from before `HelloAck`. The answer is about the hello only: it
  grants nothing and says nothing about control. The viewer offers the hello again after a full
  reconnect (the sharer forgot it when the old connection left) and to each new sharer. A
  request for control that may arrive before the hello goes after one, so the sharer is asked
  by name. `bench/engine-e2e.sh` joins a viewer to a call under way and checks that the
  sharer's app has its decoders within 1 s of showing the viewer, without a request.
- The viewer maps window positions through the letterbox the frame is drawn in, so clicks land
  right at any window size, and ignores presses on the black bars (releases always go, clamped
  to the edge). Losing focus sends `ReleaseAll`. F9 asks for control and gives it back; the
  title says which state you're in, so you never type blind. The macOS app menu is off, so ⌘Q
  and ⌘H reach the sharer.
- Revoking doesn't wait for anything. The sharer's global hotkey (⌃⌥⌘R) and the terminal's `r`
  trip the same `RevokeSwitch`: an atomic, no lock or channel. The injector is wrapped in
  `Gated`, which checks the switch before every action and lets only releases through while a
  revoke is pending, so injection stops even if the control thread is behind. The control
  thread then revokes properly, telling the guest and releasing everything held.
- On macOS, `control::macos::MacInjector` posts real events through `enigo` (CGEvent). Events
  come from a private event source, so the sharer's own held keys don't leak into the guest's
  input. Positions map onto the shared display in global points (`control::display::to_points`):
  the injector is made with that display's id and maps onto nothing else. It looks the display
  up again every 100 ms, so a new resolution or arrangement mid-call is followed. If the display
  goes away (unplugged, or asleep), the injector stops injecting anything but releases, the
  control session takes control back, tells the guest, and is closed for the rest of the share:
  the sharer sees "The shared display is gone, so control isn't available.", and later requests
  are turned down with it. If ScreenCaptureKit stops capturing it too, the share ends with its
  message. With the
  synthetic pattern (benches), input maps onto the main display, as before. Fractional scroll is
  carried over, so slow trackpad scrolls aren't lost. Keys go as Mac virtual key codes from a table of
  `KeyboardEvent.code` names. Nothing but releases is injected while the screen is locked (see
  the threat model). It is made at the first grant (or the first trust), not when sharing
  starts, so sharing with `--allow-control` needs no Accessibility until someone is given
  control; without it, that grant is turned down and the sharer told what to allow. The revoke
  hotkey is registered at start either way and never depends on the injector.
- `tc-spike probe` is a full-screen window that records the input it receives, using the
  viewer's own event mapping. `bench/control-inject.sh` sends a script through the whole path
  (guest, network, sharer, CGEvent, macOS, probe) and checks the text, shortcuts, clicks and
  scrolls that arrive. The probe measures clicks against the display, not its own window: on a
  Mac with a notch, a full-screen window starts below the camera housing (32 pt on a 14-inch
  MacBook Pro), so window-relative positions read a few percent too high.
- Real input lands wherever the pointer and the key window are, so that check sends nothing on
  trust. The probe opens on the display the injector targets (it asks `engine::screen::choose`,
  as the share does, with the same `--display`: a full-screen window otherwise opens wherever macOS likes, and once
  opened on an external display above the laptop's while the input went to the laptop's), and
  its first record says it is the key window of the frontmost app and covers that display. It
  records `lost_focus` the moment that stops being true. The guest's script holds at two gates
  (`{"do": "gate"}`): after one pointer move, which must reach the probe where it was aimed,
  and after the clicks, which must all arrive in place, before any key is sent. A watcher takes
  control back at the first `lost_focus`. The sharer's own process doesn't take focus at
  launch (winit activates an app by default, and did take the keyboard from the probe).
- `bench/control-e2e.sh` runs a sharer and a scripted headless guest on one machine. The sharer
  records what it would inject, and `tc-spike replay` checks it against what the guest sent:
  every key, click, scroll and character once and in order, and pointer moves never invented
  or reordered.
- The sharer always sees who is controlling (hard rule 4). A label at the top of the screen says
  "<name> is controlling this computer" and how to take it back, for as long as control lasts,
  and says who is asking while a request is open. A name tag follows the guest's pointer, so
  the sharer can tell the two pointers apart. Both are small opaque windows: status-level,
  on every Space and over full-screen apps, click-through, never focused, drawn with the HUD's
  8x8 font. They're not "stationary": macOS keeps a stationary window in the on-screen list
  after it's ordered out. Screen capture leaves the sharer's own app out, so the guest doesn't
  see them in the stream. ScreenCaptureKit can only do that for an app that already has windows
  when capture starts, so both windows are made, hidden, before the network thread starts
  capture; made on the first grant, they were captured. `bench/control-e2e.sh` checks that the
  label is on screen during control and gone after.
- macOS needs Screen Recording and Accessibility permissions. The desktop app asks for each
  when it is first needed (see [The desktop app](#the-desktop-app)).
- Keyboard layouts: the viewer sends the physical key and the text it produced, and the sharer
  types the text unless it's a shortcut. ADR 0004 has the rules.

### 5. Voice
The decisions and what they cost are in [ADR 0006](adr/0006-voice.md).
- Both sides send their mic and play the other's, through WebRTC's own device layer
  (`PlatformAudio` in the LiveKit SDK: CoreAudio on macOS, WASAPI on Windows), wrapped in
  `crates/media/src/audio/device.rs`. We write no device or echo cancellation code.
- WebRTC's echo cancellation, noise suppression and gain control run on the mic, with the
  speaker output as the echo reference. This SDK version always turns all three on for a device
  track, so there's no switch to turn them off.
- Opus at 32 kbit/s mono with RED: each packet repeats the previous frame, which keeps hotel-grade
  loss inaudible for 35 kbit/s more (see docs/performance.md, Voice in the lab).
- The voice is its own track and doesn't hold the screen back to stay in sync with it: a shared
  screen should never wait for a voice. Measured with voice off and on, the screen's latency is
  the same.
- Without `PlatformAudio`, WebRTC uses a silent built-in device. That's what the synthetic voice
  and the network lab use.
- macOS asks for microphone access the first time. Without it CoreAudio records silence rather
  than failing, so `tc-spike` checks the permission first and says where to turn it on.
- **Mute** is LiveKit's track mute: the track goes silent at the source, and the server tells
  the other side, so each end can show that the other is muted. The viewer mutes with `M`, or
  F8 while in control (when `M` would be typed on the sharer's machine), and its window title
  says so. The sharer has no window, so its key is a global hotkey, ⌃⌥⌘M, which works from any
  app. A key press only records the wish (`VoiceState`); the session loop that owns the track
  applies it.
- **Devices come and go mid-call.** The SDK doesn't report changes, so each side looks at the
  device list every 2 s (about 5 ms). If a mic or speakers picked by name are gone, it moves to
  the system default; on the system default, it follows the system when that moves to another
  device (headphones plugged in, AirPods back in their case). The decision is a pure function
  (`device::fallback`), tested against fake device lists.
- Devices are selected with the SDK's live switch, also when joining: if the other side is
  already talking, playout has started, and a plain "set" is refused once a device is running.

### 6. Accounts and calls
The backend is a Supabase project, not a server of our own
([ADR 0011](adr/0011-accounts-and-call-passes.md)). The details are in
[`supabase/README.md`](../supabase/README.md), [accounts](accounts.md) and [calls](calls.md).
- **Supabase Auth** signs people in with GitHub only. The app runs the OAuth code flow with PKCE
  through a one-time listener on `127.0.0.1:53682`, and keeps the session in its config folder.
  Names, handles and pictures come from the GitHub identity.
- **Postgres** holds profiles, contacts, calls and call members. Row-level security lets each
  person read only their own rows and the people in a live call with them; no table can be
  written directly, and every change is an RPC function that checks who is calling. The app
  polls `inbox` every 2 seconds: the host's heartbeat, and how knocks and rings arrive.
- **The `call-pass` Edge Function** signs a LiveKit token for one person, one room and 10
  minutes to join, after the database says that person may join. The LiveKit API secret is one
  of its secrets and is nowhere else; the app carries only the project URL and its public key,
  baked in at build time (`apps/desktop/src-tauri/build.rs`).
- **The engine joins with the pass** (`TWINCURSOR_PASS`) and never talks to Supabase. The
  backend decides who may join a call, never who may control: that is the sharer's app and
  engine alone.
- It never touches media: screen, voice and input go through LiveKit.

## Measuring latency

Built into `crates/media/src/{marker,latency}.rs`, `tc-spike` and `bench/run-matrix.sh`:
1. The viewer sends a tick (lossy data packet) every 247 ms. The interval must not be a multiple
   of the frame time. At 250 ms (15 frames at 60 fps) every tick lands at the same point of the
   frame clock, so one run samples a single point of the real 14–33 ms spread and p50 jumps
   between runs.
2. The sharer stamps the latest tick id into outgoing frames as a black/white block marker in
   the top-left of the luma plane (4 guard bits + a 20-bit id). It re-sends the last frame
   immediately when a tick arrives.
3. The viewer reads the marker after decoding. **RTT** = tick sent → marker frame presented
   (viewer clock only, so it's valid across machines).
4. Each frame also carries the sharer's capture timestamp in the RTP packet trailer
   (`PTF_USER_TIMESTAMP`). **One-way** = capture → presented. Valid on one machine or with
   synced clocks.
5. Every run logs p50/p95/max and writes JSON to `bench/results/`.

Limitation: the marker is stamped after capture, so the app redraw and part of the capture
delay aren't in RTT. A planned `--marker screen` mode fixes that.

Every media-path PR reports before and after numbers (`bench/run-matrix.sh`).

`tc-spike view --measure-input` measures the data path input takes, with pings on both data
channels that the sharer echoes. It reports the round trip and, where both ends share a clock
(one machine, the network lab), the one-way delay. Use one-way for input: the round trip's
return leg shares the viewer's downlink with the video.

**Input SLO:** one-way p50 ≤ network delay + 15 ms and p95 ≤ network delay + 40 ms on clean links.

Voice gets the same treatment, with the marker in the sound instead of the picture
(`crates/media/src/audio/marker.rs`). With `share --audio pattern`, the sharer sends a quiet tone
and, for each tick, a 110 ms burst: a sync tone, then six tones that spell the tick id and a
check. `view --measure-audio` finds the bursts with Goertzel filters as the audio plays out, and
reports:
- **round trip:** tick sent → burst heard, on the viewer's clock;
- **one way:** burst started → heard. The sharer sends each burst's start time on `tc.audio`, so
  this needs a shared clock, like the video's one way;
- **A/V offset:** a tick's burst heard minus its video frame shown;
- **concealment:** the share of audio the receiver had to make up, from WebRTC's stats. Time
  the other side spent muted is left out: a muted track sends nothing, and WebRTC counts all of
  that silence as made up.

The synthetic voice needs no mic or permission, so it runs headless and in the network lab. It
skips echo cancellation and the device buffers, which only real devices have.

`bench/audio-loopback.sh` adds those, on one Mac. The viewer plays the sharer's marker tones on
the real speakers, and a raw mic recording (`bench/audio/record-mic.swift`, AVAudioEngine, no
processing) goes to `tc-spike listen`, which times each burst from when the sharer started it.
The recorder stamps samples with their capture time, so its own buffering doesn't count. Its
mic stands in for the talker's, but the sharer's tones skip WebRTC's recording buffer, so mouth
to ear is this plus that buffer (see docs/performance.md, Where the time goes). Taking away the
viewer's own "left the jitter buffer" time leaves WebRTC's playout, the speakers, the air and the
mic.

`bench/audio-echo.sh` checks echo cancellation the same way. The sharer sends recorded speech
(`--audio file`), the viewer plays it and sends its mic back, and `tc-spike listen --echo`
compares three sounds on one clock: what the sharer said, the raw mic, and the mic as the sharer
hears it back.
- **Echo:** while only the sharer talks, the returned track must be at least 30 dB quieter than
  the raw mic, once the canceller has had 5 s.
- **The local voice:** a second voice played out loud while the sharer is silent must come
  through. Played again over the sharer (double talk), it's reported, not judged.
- **The room:** the speech has 2 s pauses, and the raw mic in them shows whether the room was
  quiet. Someone talking nearby is passed on, as it should be, and holds the canceller back, so
  a failed run in such a room counts as inconclusive.

Both scripts stop if another app is recording from the mic, most likely a call: the people on
it would hear everything played.

## Measuring quality

Latency alone doesn't tell us whether text is readable, so we measure picture quality too:
- `tc-spike share --capture pattern` streams a synthetic dark-theme code editor
  (`crates/media/src/pattern.rs`), drawn from the frame number alone:
  - tiny 8 px code on the left, 16 px code on the right, coloured syntax;
  - the left pane alternates 10 s of gentle scrolling with 10 s of fast scrolling (one line
    per frame, the hardest case for screen codecs);
  - a line being typed and a blinking cursor.
  The frame number travels in a second marker slot next to the latency marker. When the sharer
  scales a frame (to a viewer's viewport or an auto-throttle level), it stamps the number into
  the scaled frame again at the slot's normal size, as it does the latency marker, so scaled
  frames are scored too; real screen captures never get the slot.
- `tc-spike view --measure-quality` regenerates the expected frame and compares the decoded
  luma with it on a background thread (`crates/media/src/quality.rs`, frames are skipped
  while it's busy):
  - **SSIM** (8x8 blocks; drops with blur and blocking, which is what makes text unreadable);
  - **PSNR**.
  Both skip the marker area. Results report p50, p5 and min, plus SSIM during fast scrolling
  only, and SSIM per frame size received (`ssim_by_size`).
- `--headless` runs the viewer without a window. Latency is then measured at decode. Use it
  for benchmarks and the network lab.
- For a real screen, `--snapshot-every N` on both ends saves the frame carrying every Nth tick
  as a PGM in `bench/snapshots/` (git-ignored: it holds real screen content), and
  `tc-spike compare sharer-N.pgm viewer-N.pgm` prints SSIM and PSNR.

## Network lab and SLOs

`bench/netlab/` runs a pattern sharer, a headless viewer and the LiveKit SFU in Docker, each
with a fixed IP on a private bridge. Each profile's conditions are applied with `tc netem`:
- the sharer's uplink is shaped on its own interface;
- the viewer's uplink is shaped on its own interface;
- the viewer's downlink is shaped on the SFU's interface, filtered to the viewer's IP.

Run it with `bench/netlab/run.sh [--build] [profile ...]` (`CODEC`, `RES`, `DURATION`,
`ADAPT=off`, `AUDIO=off`, `SHARE_FLAGS`, `VIEW_FLAGS`, `TAG`), and summarise with
`bench/netlab/report.sh`. The synthetic voice runs alongside the screen unless `AUDIO=off`, and
the report has a table for each.

Lab notes:
- Rate limits use a token bucket (tbf) under netem. netem's own `rate` combined with jitter
  tail-drops far more than configured.
- The synthetic voice thread runs at a real-time FIFO priority (the containers have
  `SYS_NICE`), as it does on macOS. At normal priority, the lab's CPU load delayed its frames by
  up to 70 ms, and the receiver read that as network jitter.
- Building tc-spike for Linux needs clang 21+ (libwebrtc's hermetic libc++) and an explicit
  link to GLib/GIO (see the lab's Dockerfile). Linux in Docker has no hardware
encoder, so the lab uses VP8 (H.264 there is OpenH264 at ~35 fps). Mac hardware H.264 is
checked on real machines instead.

| profile | conditions (one way) |
|---|---|
| lan | none |
| wifi-good | 2 ms ±2 ms, 0.1% loss, both ways |
| wifi-bad | 8 ms ±8 ms, 1% loss with 25% correlation, both ways |
| 4g | 25 ms ±5 ms, 0.5% loss; 12 Mbit down, 4 Mbit up |
| hotel | 40 ms ±10 ms, 2% loss; 3 Mbit down, 1 Mbit up |
| intercontinental | 75 ms each way (150 ms RTT) |
| sharer-uplink-3mbit | sharer uplink 3 Mbit, 10 ms |
| bandwidth-drop | sharer uplink 10 → 1.5 → 10 Mbit, 20 s each |
| udp-blocked | viewer drops all outgoing UDP (ICE must use TCP) |
| outage-5s | viewer cut off completely for 5 s at t = 25 s |
| stall-0.6s, stall-1.2s, stall-2.5s | viewer cut off both ways, every 15 s, three times |

**SLOs** (checked by `report.sh`; "network RTT" is the RTT the profile adds):
- lan, wifi-good: RTT p50 ≤ network RTT + 25 ms; p95 ≤ network RTT + 60 ms; SSIM p5 ≥ 0.97;
  no stall (a second under 5 fps).
- ≤ 2% average loss: at most one freeze a minute.
- bandwidth-drop: no stall longer than 3 s.
- outage-5s: video back within 3 s of the network returning (longest stall ≤ 8 s).
- stall-*: video back within 3 s of the network returning (longest stall ≤ 6 s).
- udp-blocked: the call connects and measures latency.

**Voice SLOs** ("network delay" is what the profile adds from sharer to viewer, one way):
- lan, wifi-good: one way p50 ≤ network delay + 60 ms and p95 ≤ network delay + 100 ms; at most
  1% of the audio concealed.
- wifi-bad, 4g, hotel, intercontinental, sharer-uplink-3mbit: one way p95 ≤ network delay +
  150 ms; at most 2% concealed.

These are product targets, set from the Mac (48 ms one way on one machine) and ITU G.114's
150 ms comfort limit for mouth to ear. The lab's own CPU stalls push its voice delay past them
(see docs/performance.md, Network lab).

## Sizing to the viewer

The tier (`--resolution`) says the most a sharer will send. What a viewer can show depends on
its screen and window, so each viewer tells the sharer, and the sharer sends no more than that
([ADR 0009](adr/0009-size-to-the-viewer.md)).
Pixels nobody sees aren't encoded, sent or decoded, and the picture is drawn 1:1 instead of
being scaled up (blurry text) or down (wasted pixels).
- **The viewer reports a viewport**: the largest area, in physical pixels, the picture can take
  there (`protocol::Message::Viewport`, on the control topic, validated to 64..16384 pixels a
  side). While the window follows the picture's size, that's the monitor's usable area (what the
  window could grow to); once the guest has resized the window by hand, it's the window's size.
  Neither depends on the picture received, so the size the sharer picks never makes the viewer
  report something else: it settles in one step. A viewport is sent once it has held for 0.3 s
  (dragging an edge sends one report, after the drag), at once to a sharer just found, and again
  every 2 s, since nothing confirms it arrived. A joining viewer's first packets are dropped
  until the sharer's room has heard it joined (see "Remote control over a data channel"),
  so the viewport also goes with every offer of the hello, and once more when the sharer
  answers it: the sharer has a joining viewer's viewport within about 0.25 s of being able to
  read it, and the viewer gets the tier's size until then.
- **The sharer fits the source into it**: the display (or shared window) scaled to fit inside the
  viewport with its aspect ratio, never upscaled, rounded as the tiers are (width to 16, height
  to 4, so never larger than the viewport); then limited by the tier and by the auto-throttle's
  current level. On a 3024x1964 Retina sharer, a maximised viewer on the same kind of Mac
  (viewport 2872x1728) gets 2656x1728 at `--resolution native`, and a 1920x1080 viewport gets 1664x1080
  whatever the tier above it.
- **Several viewers**: the one that can show the largest picture decides, and one that leaves
  stops counting. A viewer that hasn't reported within 2 s of joining (a viewer from before
  viewports, or a headless one run without `--viewport`) gets the tier's size, as before, so
  it's never sent less than it would have been.
- **Rate limit**: the viewport the size follows changes at most once a second, so a guest
  sending a stream of viewports changes nothing faster, and can only pick a size at or below
  the sharer's tier.
- The new size goes through the same path the auto-throttle's steps and a shared window's
  resize take: the send loop scales frames to it (libyuv) and libwebrtc reconfigures the
  encoder. Capture stays at the tier's size.
- The auto-throttle works out each level's bitrate floor from the size actually sent: floors
  grow with the pixel count to the power 0.8, so a 2160p setting sending a 1080p picture needs
  1080p's floor, not 2160p's. Levels that send the same picture (2160p, 1440p and 1080p all
  send 1280x720 to a 720p viewport) are one step, not several.
- The sharer's log and its stats (`sized_by` in the snapshot it sends) say what decided the
  size: the setting, the auto-throttle or a viewer. `share --ignore-viewport` sends the tier's
  size whatever viewers show, for A/B runs; an exact `--width`/`--height` is never changed.
- `bench/viewport-e2e.sh` checks it end to end with headless viewers (`view --viewport WxH`,
  and `--viewport-at SECS:WxH` to change it mid-run): sizes for one and two viewers, a viewport
  above the tier, none at all, a change mid-call, and text quality at the fitted size.

## Auto-throttle (adaptive frame rate and resolution)

libwebrtc adapts the bitrate to the bandwidth estimate, and for screen share it keeps the
resolution and drops the frame rate when squeezed. That isn't enough on its own:
- When the budget is too small for the resolution, every frame is blurry.
- When the *viewer's* downlink is the bottleneck, the sharer's estimate doesn't see it.
  Without simulcast the SFU can only drop packets, and the LiveKit SDK has no public API to
  change the max bitrate mid-call.

`crates/media/src/adapt.rs` walks a ladder that halves the frame rate before giving up
resolution, because text needs pixels more than 60 fps:

1440p 60 → 1440p 30 → 1080p 30 → 720p 30 → 720p 15

The ladder starts at what the user picked. 2160p steps down to 1440p (2160p 60 → 2160p 30 →
1440p 30 → …). Native steps down through 2160p only on a display taller than 2160 lines; on a
shorter one 2160p would be the same size, so it goes straight to 1440p.
- **Step down one level** after 3 s of trouble:
  - the budget (encoder target, capped by the bandwidth estimate) is under 90% of the
    level's floor. Floors are 0.5 / 1.0 / 1.6 / 3.0 Mbps for 720 / 1080 / 1440 / 2160 at
    60 fps with H.264 or HEVC, ~2x for VP8, and scale with (fps/60)^0.75. Native is 2.5 Mbps
    on a display shorter than 2160 lines (and so is 2160 there, being the same size); from
    2160 lines up it grows from 3.0 Mbps with (pixels / 4K pixels)^0.8;
  - or the encoder is CPU-limited;
  - or it's bandwidth-limited below half the level's frame rate;
  - or the viewer reports ≥ 5% loss or ≥ 2 freezes a second.
- **Step up** after 10 s of calm with 1.5x the next level's floor available. That wait
  doubles (up to 80 s) when it flaps.
- The viewer sends its stats back on `tc.stats`, so the sharer sees its downlink.
- The sharer captures at the chosen tier. The send loop scales frames to the level's size
  (libyuv) and paces them to its frame rate; libwebrtc reconfigures the encoder by itself. The
  viewer's window keeps its size, and lower tiers are scaled up to it.
- Levels larger than the viewers' viewport send the viewport's size, and their floors are that
  size's ([Sizing to the viewer](#sizing-to-the-viewer)).
- On a big drop it jumps straight to the first level the budget can carry, instead of
  walking down one level every few seconds while the picture stalls.
- **Bitrate cap for the viewer's downlink** (`BitrateCap`). Frame rate and resolution don't
  lower the bitrate on their own: the encoder spends whatever its target allows. So when the
  viewer loses ≥ 5% for 4 s while we send more than it receives, the sharer caps its max
  bitrate at 75% of the viewer's goodput. It raises the cap 1.5x after 30 calm seconds.
  - The SDK has no public way to change the max bitrate mid-call, so this republishes the
    track: the viewer sees it drop and come back.
  - In the lab, VP8 into a 3 Mbit, 2%-loss downlink decoded 0 fps uncapped, and 18 fps with
    no stall at a 1.5 Mbps cap.
- The HUD shows "adapted to 1080p 30 fps: <reason>; capped at 1.5 Mbps (viewer downlink)".
  `share --no-adapt` turns both off.

## Call health stats

`crates/media/src/netstats.rs` polls WebRTC stats once a second on both ends (`Room::get_stats`)
and turns the cumulative counters into per-second `NetSnapshot`s:
- link RTT and path (ICE candidate type and protocol, e.g. `host udp`, `relay tcp`);
- receive: bitrate, fps, resolution, loss %, jitter, freezes, dropped frames, NACK/PLI, decoder,
  codec, decode time per frame;
- send: bitrate vs target, fps, resolution, loss reported by the SFU, retransmit %, bandwidth
  estimate, what limits quality (`cpu`/`bandwidth`), encoder, codec, encode time per frame.

The viewer's results file has a `media` block that sums these up over the call: the sizes
actually sent and received, the codec negotiated, median encode and decode time per frame,
mean bitrate, and how many seconds each quality limitation lasted.

The sharer sends its snapshot to the viewer on data topic `tc.stats`, so the viewer sees both
ends. `connection_quality` grades the call Good/Fair/Poor; the worst signal wins:

| signal | good below | fair below |
|---|---|---|
| link RTT | 80 ms | 200 ms |
| loss | 1% | 5% |
| jitter | 30 ms | 80 ms |
| freezes in the last second | none | under 500 ms in total |

The viewer draws this as a HUD in the top-right corner (away from the latency marker) and
writes the per-second history into the results JSON (`net`). `--no-stats` turns it all off,
for A/B latency runs.

With voice in the call the HUD ends with a voice line:

```
voice mic -32 dB  sharer -28 dB  buffer 44 ms  made up 0.3%
```

- `mic` and `sharer` are how loud each voice was over the last second (RMS, dBFS), from
  WebRTC's own running sums, so a mic that picks up nothing shows as one. They read `MUTED`
  and `muted` instead while either side is muted, and the line turns amber while you are.
- `buffer` is how long the sharer's voice waits in the jitter buffer; `made up` is concealment,
  amber from 1% and red from 5%.
- When a run measures it (`--measure-audio`), the voice's one-way delay is added.

SDK quirk: with current LiveKit servers the SDK uses one peer connection for both directions
and reports it as `publisher_stats`; `subscriber_stats` is then empty. The poller handles both.

## Platform plan

macOS first, Linux next (Wayland via PipeWire capture and libei input, plus X11), Windows after that.
