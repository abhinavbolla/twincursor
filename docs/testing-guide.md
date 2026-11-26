# Testing TwinCursor

Thanks for trying TwinCursor. A call has two people: the **sharer**, on a Mac, shows their screen;
the **guest**, on a Mac or a Windows PC, watches, talks, and can take control of the sharer's
mouse and keyboard when the sharer allows it. There's no sign-up: whoever sent you the build
also sent the server it uses.

## On a Mac (sharer or guest)

You need an Apple silicon Mac (M1 or later) with macOS 15 Sequoia or later.

1. Download `TwinCursor-<version>-macos-arm64.dmg` and open it.
2. Drag **TwinCursor** onto **Applications**, then eject the disk image.
3. Open TwinCursor from Applications. The test build isn't signed by Apple yet, so macOS stops
   it the first time and says it can't verify the app. Click **Done** (not Move to Trash).
4. Choose Apple menu > **System Settings**, then click **Privacy & Security** in the sidebar
   (scroll down if you need to). Under **Security**, next to the message about TwinCursor, click
   **Open Anyway**, confirm with **Open Anyway** again, and enter your login password. The
   button appears for about an hour after you tried to open the app. You only do this once.

If macOS instead says TwinCursor "is damaged and can't be opened", run this in Terminal and open
it again:

```bash
xattr -dr com.apple.quarantine /Applications/TwinCursor.app
```

### Permissions

TwinCursor asks macOS for each permission the first time it needs it, and macOS asks you once
for each. While one is missing, TwinCursor offers **Open Settings** to go to its page in System
Settings.

| Permission | Needed for | Why |
| --- | --- | --- |
| Screen & System Audio Recording | Sharing | To send your screen to the guest. |
| Accessibility | Sharing, if you let a guest take control | To type and click for the guest while you allow it. |
| Microphone | Voice (sharing or joining) | To send your voice. Turn Voice off to skip it. |

macOS applies some of these only after the app restarts.

### Start a call

TwinCursor lives in the menu bar: click its icon (two chevrons) to open it. During a call, a
small bar at the top of the screen shows who is there and lets you stop or leave.

1. Click **Sign in with GitHub** and finish signing in in your browser.
2. **Sharer:** click **Share my screen**. The call link goes on your clipboard: send it to your
   guest (chat or email).
   **Guest:** copy the link you were sent, then click **Paste link**.
3. **Sharer:** the bar shows the guest knocking: click **Let in**. The shared screen opens in its
   own window on the guest's side.

## On Windows (guest only)

Windows can watch, talk and take control, but can't share its screen yet.

1. Download `TwinCursor-viewer-<version>-windows-x64.zip`, right-click it and choose
   **Extract All**.
2. In the extracted folder, double-click **Join TwinCursor**. Windows may say it protected your
   PC, because the build isn't signed: click **More info**, then **Run anyway**.
3. Type your name and the room code the sharer sent you, and press Enter after each.

The shared screen opens in its own window. The black window behind it shows what's happening;
leave it open.

## What to try

1. **Watch.** Have the sharer share at 4K and move around: scroll, switch apps, play a video.
   Is the text sharp? Does it keep up?
2. **Talk.** Voice is on unless you turned it off. Mute with **M** in the guest's viewer window,
   or ⌃⌥⌘M on the sharer's Mac, from any app.
3. **Ask for control.** The guest clicks **Ask for control** in the app, or presses **F9** in the
   viewer window.
4. **Allow it.** The sharer's TwinCursor window comes to the front with the request; click
   **Allow** (it turns on after a second). The guest clicks into the shared screen and uses it
   like their own: type, click, scroll, use shortcuts.
5. **Take it back.** The sharer presses **⌃⌥⌘R** (Control-Option-Command-R) from any app, or
   clicks **Take back control**. The guest can also give it back with **F9**.
6. **Leave.** Either side clicks **Leave**, or closes the viewer window.

## What to report

- **What happened**, and what you expected instead. Screenshots or a screen recording help.
- **The numbers.** During the call, on the Mac app, click **Copy numbers** and paste the line.
- **The log.** On the Mac app, open **Details** at the bottom of the window and click **Copy**.
  On Windows, copy the last lines of the black window.
- Your Mac or PC model, your OS version, and your network (home Wi-Fi, office, mobile hotspot).

## Known limits

- **Unsigned builds.** macOS and Windows warn before the first launch, as described above.
- **Windows is viewer-only.** Sharing from Windows comes later. A Windows guest can't decode
  HEVC, so a 4K share switches to H.264 when one joins; the picture pauses for a moment.
- **The build carries the server's key.** Anyone who has the app or the zip can read it and start
  calls on that server, so please don't pass them on.
- **4K takes a moment.** The first 20 seconds or so of a 4K share may run at 30 fps before
  reaching 60.
