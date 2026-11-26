# Test plan for a test build

A hands-on pass over everything the test app does, for two people on two computers. It takes
about an hour. Testers who only want to try a call can follow the shorter
[testing guide](testing-guide.md).

> The desktop app now lives in the menu bar: a panel under its icon and a call bar during calls
> ([the UI](ui.md)), with calls started by signing in and sharing a link. Sections that mention
> the start screen, room codes or **Join** describe the previous build and are being rewritten.

Mark each line pass or fail. For a fail, note what happened, and copy the numbers and the log
(see [What to send back](#what-to-send-back)).

## What you need

- **Sharer:** an Apple silicon Mac on macOS 15 or later, with the disk image from
  [Test builds](RELEASING.md#test-builds).
- **Guest:** a second Mac with the same disk image, or a Windows PC with the viewer zip. Run the
  plan once with each if you can.
- Both on mains power, on different networks if possible (one on a phone hotspot is fine).
- A second way to talk (a phone call) for the parts where voice is off or being tested.

## 1. Install and first open

| # | Do | Expect |
|---|---|---|
| 1.1 | Open the disk image, drag TwinCursor to Applications, open it | macOS blocks it once; **Open Anyway** in System Settings > Privacy & Security lets it through |
| 1.2 | Open it a second time | It opens with no warning |
| 1.3 | Windows: extract the zip, double-click **Join TwinCursor** | SmartScreen warns once; **More info > Run anyway** opens a window asking for a name |
| 1.4 | Mac: open **Settings** in the app | The server address and key are filled in, and the secret shows as built in, not as text |

## 2. Permissions (Mac)

| # | Do | Expect |
|---|---|---|
| 2.1 | Look at the start screen before allowing anything | Three rows, each "not allowed" or "not asked" |
| 2.2 | Click **Ask** on Screen Recording, allow it in System Settings | The prompt names TwinCursor. The row turns to allowed within a few seconds, without restarting the app |
| 2.3 | The same for Accessibility and Microphone | As above |
| 2.4 | With Screen Recording not allowed, try **Share my screen** | It doesn't start, and says why |
| 2.5 | With nothing allowed and Voice off, click **Join** | Joining works: a guest needs no permission to watch |

## 3. Start a call

| # | Do | Expect |
|---|---|---|
| 3.1 | Sharer: type a name, **Make one**, **Copy** | A code like `7kq2-m9xd-p4tz-hc3w`, and it pastes elsewhere |
| 3.2 | Sharer: **Share my screen** at 1080p | "Sharing your screen", Connected, numbers start within a few seconds |
| 3.3 | Guest: paste the code, **Join** | The sharer's screen opens in its own window; both sides list the other by name |
| 3.4 | Guest: type the code in capitals, without dashes | It still joins |
| 3.5 | Guest: join with a wrong code | No picture, "Waiting for the sharer", and the sharer sees nobody |
| 3.6 | The sharer's TwinCursor window, seen from the guest | It isn't in the picture |

## 4. Picture

Read the **Numbers** line on each side for these.

| # | Do | Expect |
|---|---|---|
| 4.1 | Share at 1080p, scroll code and switch apps | 60 fps or close, text readable, H.264 |
| 4.2 | Share at 1440p | As above at the larger size |
| 4.3 | Share at 4K between two Macs, guest's window full screen | HEVC. 30 fps is normal for the first 20 seconds or so, then 60. Small text is sharp |
| 4.4 | Guest: make the viewer window small, then large again | The size in the numbers follows the window within a few seconds; the picture stays sharp at each size |
| 4.5 | Play a video on the sharer's screen for a minute | Motion stays smooth; the bitrate rises and falls back afterwards |
| 4.6 | Leave a mostly still screen up for two minutes | The picture stays sharp, no drift into blur |
| 4.7 | Guest: move the pointer and type while watching | What the guest sees trails the sharer by a blink at most. Say so if it feels like more |

## 5. Voice

| # | Do | Expect |
|---|---|---|
| 5.1 | Both with Voice on, headphones on, talk | Each hears the other clearly, with no obvious delay |
| 5.2 | Both on speakers, talk over each other | No echo of your own voice coming back |
| 5.3 | Sharer: **Mute**, then ⌃⌥⌘M from another app | The guest hears nothing while muted; the app shows "You're muted" |
| 5.4 | Guest: **M** in the viewer window | The same, the other way |
| 5.5 | One side joins with Voice off | The call works; the app says voice is off in this call |
| 5.6 | Unplug headphones mid-call | Sound moves to the speakers without leaving the call |

## 6. Control

| # | Do | Expect |
|---|---|---|
| 6.1 | Guest: **Ask for control** (or F9 in the viewer) | The sharer's TwinCursor window comes to the front naming the guest. **Allow** is off for the first second |
| 6.2 | Sharer: **Deny** | The guest is told no. Guest clicks and types: nothing happens on the sharer's Mac |
| 6.3 | Guest asks again; sharer: **Allow** | Both sides say who is controlling, and a label on the sharer's screen names the guest |
| 6.4 | Guest: type a paragraph in a text editor, with capitals and symbols | Every character arrives, in order |
| 6.5 | Guest: click, double-click, drag to select, right-click, scroll | Each does what it would locally |
| 6.6 | Guest: shortcuts (copy, paste, undo, switch tabs) | They act on the sharer's Mac. From Windows, note which keys you used |
| 6.7 | Sharer: press ⌃⌥⌘R while another app is in front | Control ends at once; the guest's input stops working; the label goes away |
| 6.8 | Allow again; sharer clicks **Take back control** | As above |
| 6.9 | Allow again; guest presses F9 | Control is given back |
| 6.10 | Guest asks; sharer does nothing for a minute | Nothing is granted. The guest can cancel with **Never mind** |
| 6.11 | Guest asks, then leaves before the sharer answers | The prompt goes away; clicking nothing grants nothing |
| 6.12 | Sharer uses their own mouse while the guest controls | The sharer's own input always works |

## 7. A guest that can't decode HEVC

| # | Do | Expect |
|---|---|---|
| 7.1 | Sharer shares at 4K; a Windows guest joins | The sharer's app says it switched to H.264. The picture comes back within a few seconds and stays |
| 7.2 | A second, Mac guest in the same call | It keeps watching through the switch |

## 8. Networks

| # | Do | Expect |
|---|---|---|
| 8.1 | Guest on a phone hotspot | The call works; the size or frame rate may step down, and steps back up on a good network |
| 8.2 | Sharer: turn Wi-Fi off for five seconds, then on | "Connection lost, reconnecting…", then the call resumes by itself |
| 8.3 | Sharer: Wi-Fi off for a minute | The call ends with a sentence saying why, back at the start screen |

## 9. Ending a call

| # | Do | Expect |
|---|---|---|
| 9.1 | Guest: **Leave** | The sharer's list empties; the sharer keeps sharing |
| 9.2 | Guest: close the viewer window | The same as Leave |
| 9.3 | Sharer: **Leave** | Both return to the start screen; the guest is told the call ended |
| 9.4 | Sharer: quit the app with ⌘Q during control | Sharing and control stop at once; no TwinCursor process is left in Activity Monitor |
| 9.5 | Start a new call straight after | It works with a new or the same code |

## What to send back

For each fail, and once for a normal call at each quality:

- The **Copy numbers** line from both sides.
- The log: **Details > Copy** in the Mac app, or the last lines of the black window on Windows.
- Both computers' models and OS versions, and each side's network.
- A screenshot or a short phone video when the picture or the delay is the problem.
