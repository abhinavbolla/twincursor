# Test plan for a build

A hands-on pass over everything the app does, for two people on two Macs. It takes about an hour
and a half. Testers who only want to try a call can follow the shorter
[testing guide](testing-guide.md).

Mark each line pass or fail. For a fail, note what happened, and copy the numbers (see
[What to send back](#what-to-send-back)).

## What you need

- **Two Apple silicon Macs** on macOS 15 or later, each with the disk image of a build made for
  your backend ([Releasing](RELEASING.md#builds-for-people-to-install)), and two GitHub accounts.
  One Mac is the **sharer**, the other the **guest**; swap roles for a second pass if you can.
- For a first-run pass, Macs that have never run TwinCursor, or with its permissions removed in
  System Settings > Privacy & Security.
- Both plugged in and not in Low Power Mode, on different networks if possible (one on a phone
  hotspot is fine). A second display on the sharer's Mac for section 11.
- A second way to talk (a phone call) for the parts where voice is being tested.

## 1. Install and first open

| # | Do | Expect |
|---|---|---|
| 1.1 | Open the disk image, drag TwinCursor to Applications, open it | macOS blocks it once; **Open Anyway** in System Settings > Privacy & Security lets it through |
| 1.2 | Open it a second time | It opens with no warning. The panel hangs from the TwinCursor icon in the menu bar |
| 1.3 | Click the icon; click it again; press Escape with the panel open; click another app | The panel opens, closes, closes, closes |
| 1.4 | Right-click the icon | A menu with **Open TwinCursor** and **Quit TwinCursor** |
| 1.5 | Open a build made without a backend, if you have one | The panel says "This build isn't connected to a server." and nothing else works. **self-hosting guide** opens the guide on GitHub in your browser |

## 2. Sign in

| # | Do | Expect |
|---|---|---|
| 2.1 | **Sign in with GitHub** | The browser opens GitHub; the panel says "Finish signing in in your browser…" and stays open |
| 2.2 | Approve the app on GitHub | The tab says "You're signed in. You can close this tab."; the panel shows your picture and **Share my screen** |
| 2.3 | Click your picture | Your name and @handle, **Sign out**, **Quit TwinCursor** |
| 2.4 | Quit and reopen the app | Still signed in |
| 2.5 | Start a sign-in, then **Cancel** | Back to **Sign in with GitHub** |
| 2.6 | **Sign out**, then sign in again | Signed out at once; signing in works |
| 2.7 | Turn Wi-Fi off for a minute while signed in | A muted "Can't reach the server." at the bottom of the panel; you stay signed in, and it goes away once Wi-Fi is back |

## 3. Permissions, when first needed

On a Mac that hasn't granted TwinCursor anything yet.

| # | Do | Expect |
|---|---|---|
| 3.1 | Sharer: **Share my screen** | macOS asks for Screen Recording, naming TwinCursor. The share doesn't start; the panel says "TwinCursor needs Screen Recording to share your screen." with **Open Settings** |
| 3.2 | Allow it in System Settings, click **Share my screen** again | It shares, without restarting the app |
| 3.3 | Either side, first call | macOS asks for the microphone, naming TwinCursor |
| 3.4 | Guest, with nothing granted: **Paste link** and join | Joining works: watching needs no permission |
| 3.5 | Sharer, with Accessibility not granted: **Let in with control** | macOS asks for Accessibility. The guest is let in without control, and the bar says why, with **Open Settings** |
| 3.6 | Grant Accessibility, then **Give control** to that guest from the bar's menu (**⋯**) | It works without restarting the app |
| 3.7 | Sharer, with Accessibility not granted: **Allow** a plain request | macOS asks for Accessibility (if it hasn't this run). The guest isn't given control and their request closes; the bar says "… wasn't given control: TwinCursor needs Accessibility." with **Open Settings**. After granting it, the guest asks again and **Allow** works |

## 4. Share and join by link

| # | Do | Expect |
|---|---|---|
| 4.1 | Sharer: **Share my screen** | The call bar appears at the top centre: "Sharing your screen", **Copy link**. A notice says the link is on your clipboard |
| 4.2 | Paste the clipboard somewhere | `twincursor://call/` and 32 letters and digits |
| 4.3 | Sharer: open the panel | "You're sharing your screen", the link, **Copy link**, **Stop sharing** |
| 4.4 | Guest: copy the link, **Paste link** | The bar says "Waiting for <sharer> to let you in…" with **Cancel** |
| 4.5 | Guest: **Paste link** with the code alone, or the link with spaces around it | Same as 4.4 |
| 4.6 | Guest: **Paste link** with something else on the clipboard | "That isn't a TwinCursor call link."; nothing else happens |
| 4.7 | Guest: **Cancel** while waiting | The bar goes; the sharer's knock card goes within a few seconds |
| 4.8 | Guest: paste the link of a call that has ended | A sentence saying the call has ended or wasn't found |

## 5. Letting people in

| # | Do | Expect |
|---|---|---|
| 5.1 | Guest knocks; sharer looks at the bar | "<name> wants to join" with their @handle. **Let in** and **Let in with control** are off for a second, with a thin line across the top; **Turn away** works at once |
| 5.2 | Sharer clicks **Let in** as fast as possible after the knock appears | Nothing, or "Too soon. Try again." |
| 5.3 | Sharer: **Let in** | The guest's viewer window opens on the sharer's screen; the sharer's bar says "With <name>" |
| 5.4 | Sharer is typing in another app when the knock appears | The typing stays in that app; the bar never takes the keyboard |
| 5.5 | Another guest knocks; sharer: **Turn away** | The guest is told "<sharer> didn't let you in." Pasting the same link again is refused |
| 5.6 | Sharer: **Let in with control** for a new guest | The bar says "<name> can take control" with **Take back** |
| 5.7 | The sharer's TwinCursor windows, seen from the guest | The panel and the bar aren't in the picture |

## 6. Control

| # | Do | Expect |
|---|---|---|
| 6.1 | A guest let in without control clicks **Control** in their bar (or F9 in the viewer) | Guest: "Asked for control…" with **Never mind**. Sharer's bar: "<name> wants to control your Mac", **Deny** and **Allow**, Allow off for a second |
| 6.2 | Sharer: **Deny** | The guest is told no. Guest clicks and types: nothing happens on the sharer's Mac |
| 6.3 | Guest asks again; sharer: **Allow** | Sharer's bar turns red: "<name> is controlling", ⌃⌥⌘R keycaps, **Take back**. A label at the top of the sharer's screen names the guest, and a name tag follows their pointer. Guest's bar: "Controlling <sharer>'s Mac" |
| 6.4 | Guest: type a paragraph in a text editor, with capitals and symbols | Every character arrives, in order |
| 6.5 | Guest: click, double-click, drag to select, right-click, scroll | Each does what it would locally |
| 6.6 | Guest: shortcuts (copy, paste, undo, switch tabs) | They act on the sharer's Mac |
| 6.7 | Sharer: press ⌃⌥⌘R while another app is in front | Control ends at once; the guest's input stops working; the label goes away. Next time the guest asks, the sharer is asked again, even if they were trusted |
| 6.8 | Allow again; sharer clicks **Take back** | As above |
| 6.9 | Allow again; guest clicks **Give back** or presses F9 | Control is given back |
| 6.10 | Guest let in with control (5.6) asks | They get control at once, no prompt; the label still shows |
| 6.11 | That guest gives control back, then asks again | Granted again without a prompt |
| 6.12 | Sharer: **Let in with control**, then ⌃⌥⌘R before the guest's window has opened; guest asks | The sharer is asked: the revoke took the promise back |
| 6.13 | Sharer: **⋯** > **Give control** next to a guest | That guest's row says "Can take control"; their next request is granted at once |
| 6.14 | Guest asks; sharer does nothing for a minute | Nothing is granted. The guest can cancel with **Never mind** |
| 6.15 | Guest asks, then leaves before the sharer answers | The request goes away; nothing is granted |
| 6.16 | Sharer uses their own mouse while the guest controls | The sharer's own input always works |
| 6.17 | Sharer locks the screen during control | Nothing the guest types reaches the lock screen |

## 7. People and ringing

After section 5, each of you is in the other's **People**.

| # | Do | Expect |
|---|---|---|
| 7.1 | Both: open the panel | The other person is listed with their picture, name, @handle and **Call** |
| 7.2 | A (not in a call): **Call** next to B | A starts sharing. A's row for B says "Ringing…". Within a few seconds B's panel opens by itself, without taking the keyboard: "<A> is calling", **Decline**, **Join** |
| 7.3 | B: **Join** | B joins A's call without knocking; A's row says "In the call" |
| 7.4 | Ring again; B: **Decline** | B's ring goes. A's row says "Declined" for about 10 seconds, then shows **Call** again |
| 7.5 | Ring again; B does nothing for a minute | The ring goes from B's panel; A's row says "No answer" for about 10 seconds, then shows **Call** again |
| 7.6 | Note for 7.4 and 7.5 | Whether A noticed the "Declined" or "No answer" in time (A's panel must be open to see it) |
| 7.7 | B is in another call when A rings | Note what B sees |

## 8. The call bar

| # | Do | Expect |
|---|---|---|
| 8.1 | Drag the bar by its background | It moves; buttons still click. It reopens where you left it in the next call |
| 8.2 | Click into other apps | The bar stays on top |
| 8.3 | Make an app full screen | The bar is over it |
| 8.4 | Switch to another Space | The bar is there too |
| 8.5 | The mic button; then ⌃⌥⌘M from another app (sharer); **M** in the viewer (guest) | Each mutes; the other side hears nothing and is shown that you're muted. Again unmutes |
| 8.6 | **⋯** | The call link with **Copy link**, the numbers, a row per guest with **Give control**, and your mic |
| 8.7 | Light and dark appearance (System Settings > Appearance) | Both look right, with nothing showing at the corners |

## 9. Quality

Read the numbers in **⋯** on both sides. The calls with accounts share at 4K with HEVC.

| # | Do | Expect |
|---|---|---|
| 9.1 | Guest: make the viewer window full screen on a large display | **Resolution** grows to what the window can show, up to the sharer's display (3840×2160 from a 4K display). **Codec** HEVC |
| 9.2 | Scroll code and switch apps on the sharer | **Frame rate** near 60 after the first 20 seconds or so (30 before is normal). Small text is sharp |
| 9.3 | Look at **Bitrate** and **Round trip** | Note both. On home Wi-Fi, round trip should be well under 100 ms |
| 9.4 | Guest: make the viewer window small, then large again | **Resolution** follows within a few seconds; **Shown at** says 1:1 when the picture fits the window |
| 9.5 | Play a video on the sharer's screen for a minute | Motion stays smooth; the bitrate rises and falls back afterwards |
| 9.6 | Leave a mostly still screen up for two minutes | The picture stays sharp, no drift into blur |
| 9.7 | Guest: move the pointer and type while in control | What the guest sees trails the sharer by a blink at most. Say so if it feels like more |
| 9.8 | Voice: talk on headphones, then both on speakers, talking over each other | Clear, no obvious delay, and no echo of your own voice |

## 10. Ending and leaving

| # | Do | Expect |
|---|---|---|
| 10.1 | Guest: **Leave** | The guest's bar and viewer go; the sharer's bar loses them and keeps sharing |
| 10.2 | Guest: close the viewer window | The same as Leave |
| 10.3 | Sharer: **Stop** | The sharer's bar goes. Within a few seconds the guest's call ends by itself and says "The call ended." |
| 10.4 | Sharer: **Quit TwinCursor** (⌘Q) during control | Control and sharing stop at once; the guest's call ends; no TwinCursor process is left in Activity Monitor |
| 10.5 | Guest: quit during a call | The sharer's bar loses them |
| 10.6 | Start a new call straight after | It works, with a new link |
| 10.7 | Paste an old call's link | Refused: that call has ended |

## 11. Two displays (sharer)

| # | Do | Expect |
|---|---|---|
| 11.1 | Share with a second display connected | Note which display the guest sees |
| 11.2 | Guest takes control and clicks near each edge of the picture | Clicks land where the guest points |
| 11.3 | Click the menu bar icon on each display | The panel opens under the icon on that display |
| 11.4 | Drag the bar to the second display, then unplug it | The bar comes back onto the remaining display |

## 12. Bad networks

| # | Do | Expect |
|---|---|---|
| 12.1 | Guest on a phone hotspot | The call works; the size or frame rate may step down, and steps back up on a good network |
| 12.2 | Sharer: Wi-Fi off for five seconds, then on | The bar says "Reconnecting…", then the call resumes by itself |
| 12.3 | Sharer: Wi-Fi off for a minute | The call ends with a sentence saying why |
| 12.4 | Guest: Wi-Fi off while waiting to be let in | After a while: "Can't reach the server, so you stopped waiting to join." |

## Windows guest

The Windows app joins calls as a guest (it can't share). The sharer is a Mac running the menu
bar app built for the same backend. Note the Windows version (**Settings > System > About**), the
display's resolution and scale, and where the taskbar is. After each failure, send the last
lines of `%LOCALAPPDATA%\dev.twincursor.desktop\logs\twincursor.log`.

| # | Do | Expect |
|---|---|---|
| WG.1 | Extract the zip to a short path, double-click **TwinCursor.exe** | SmartScreen warns once; **More info > Run anyway**. The panel opens by the tray icon, standing on the taskbar; no taskbar button appears |
| WG.2 | Click the tray icon twice; click elsewhere while the panel is open | The panel closes and opens; it closes when it loses focus. Right-click gives **Open** and **Quit TwinCursor** |
| WG.3 | The icon on a light and on a dark taskbar | Readable on both (white mark on indigo) |
| WG.4 | **Sign in with GitHub** | The browser opens GitHub; after approving, the panel shows your picture. The log says "sign-in: signed in" |
| WG.5 | Look at the signed-in panel | "Sharing from Windows isn't available yet. You can join a call with a link." where Share would be; **Paste link** is the main button; **People** has no **Call** buttons |
| WG.6 | Copy the sharer's link, click **Paste link** | "Waiting for … to let you in…" in the panel and in the bar; the sharer sees the knock |
| WG.7 | Sharer lets you in | The shared screen opens in its own window; the sharer's bar says HEVC was switched to H.264 for you (at 4K); text is readable |
| WG.8 | Talk; mute with the bar and with **M** | Both hear each other; Windows asks for the microphone at most once |
| WG.9 | **Control** in the bar (or **F9**), sharer allows | You type and click on the Mac; **F9** or **Give back** returns it; the sharer's ⌃⌥⌘R takes it back |
| WG.10 | The sharer rings you from **People** | "… is calling" with **Join** and **Decline** in the panel, which comes up without taking the keyboard |
| WG.11 | Drag the bar elsewhere, end the call, start another | The bar comes back where you left it, on top of other windows |
| WG.12 | **Leave**; then **Quit TwinCursor** | The viewer closes; Task Manager shows no `tc-engine.exe` left running |
| WG.13 | At 125 % or 150 % display scale, and with a second display | The panel stands on the taskbar by the icon, the right size, on the display with the taskbar |

Numbers to send: **Copy numbers** from the bar on both sides during WG.7 (resolution, frame rate,
bitrate, round trip).

## Windows terminal viewer

The terminal viewer (`TwinCursor-terminal-viewer-…zip`) has no accounts and can't join a call
started from the menu bar app. It joins a room by code on a LiveKit server whose key and secret
are in its `server.txt` ([Releasing](RELEASING.md#builds-with-a-livekit-key-legacy)). A Mac
sharer for it runs `tc-engine share --room CODE --name NAME` from a terminal against the same
server, which can't answer control requests; so this pass covers watching and voice only.

| # | Do | Expect |
|---|---|---|
| W.1 | Extract the zip, double-click **Join TwinCursor** | SmartScreen warns once; **More info > Run anyway** opens a window asking for a name and a room code |
| W.2 | Enter the code the sharer used | The sharer's screen opens in its own window; text is readable |
| W.3 | Talk; press **M** in the viewer | Both hear each other; M mutes |

Windows doesn't decode HEVC here. The menu bar app switches a call to H.264 for such a guest; the
terminal sharer uses the default H.264.

## What to send back

For each fail, and once for a normal call:

- The **Copy numbers** text from both sides (**⋯** in the bar).
- Both Macs' models and macOS versions, and each side's network.
- A screenshot or a short phone video when the picture or the delay is the problem.
