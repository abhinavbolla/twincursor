# Testing TwinCursor

Thanks for trying TwinCursor. A call has two people: the **sharer** shows their Mac's screen; the
**guest** watches, talks, and can use the sharer's mouse and keyboard when the sharer allows it.
Both sign in with GitHub. The build you were sent already knows its server, so there is nothing
to set up.

You need an Apple silicon Mac (M1 or later) with macOS 15 Sequoia or later, and a GitHub account.

## Install it

1. Download `TwinCursor-<version>-macos-arm64.dmg` and open it.
2. Drag **TwinCursor** onto **Applications**, then eject the disk image.
3. Open TwinCursor from Applications. The build isn't signed by Apple yet, so macOS stops it the
   first time and says it can't verify the app. Click **Done** (not Move to Trash).
4. Choose Apple menu > **System Settings**, then click **Privacy & Security** in the sidebar
   (scroll down if you need to). Under **Security**, next to the message about TwinCursor, click
   **Open Anyway**, confirm with **Open Anyway** again, and enter your login password. The
   button appears for about an hour after you tried to open the app. You only do this once.

If macOS instead says TwinCursor "is damaged and can't be opened", run this in Terminal and open
it again:

```bash
xattr -dr com.apple.quarantine /Applications/TwinCursor.app
```

TwinCursor lives in the menu bar: click its icon (two chevrons) to open the panel. During a
call, a small bar stays on top of your screen.

## Sign in

Click **Sign in with GitHub** in the panel. Your browser opens GitHub; approve the app, and the
page says you're signed in. The panel then shows your picture at the top. If it says "This build
isn't connected to a server.", you have a build without a server: ask for another.

## Permissions

TwinCursor asks macOS for each permission the first time it needs it, and macOS asks you once
for each. While one is missing, TwinCursor offers **Open Settings** to go to its page in System
Settings.

| Permission | Asked for | Why |
| --- | --- | --- |
| Screen & System Audio Recording | The first time you share | To send your screen to the guest. |
| Microphone | When your first call starts | To send your voice. |
| Accessibility | The first time you click **Let in with control**, **Give control** or **Allow** | To type and click for the guest while you allow it. |

After allowing Screen Recording in System Settings, click **Share my screen** again.

## Start a call

1. **Sharer:** click **Share my screen**. The call link goes on your clipboard: send it to your
   guest (chat or email). The bar at the top of your screen says "Sharing your screen".
2. **Guest:** copy the link you were sent, then click **Paste link** in the panel. The bar says
   you're waiting to be let in.
3. **Sharer:** the bar shows who is knocking. The buttons turn on after a second. Click **Let
   in**, or **Let in with control** to let them take control whenever they like during this
   call. **Turn away** says no.

The guest sees the sharer's screen in its own window, and you can both talk. After your first
call, each of you appears under **People** in the other's panel: click **Call** to ring them.
The person you ring sees "… is calling" in their panel, with **Join** and **Decline**.

## What to try

1. **Watch.** Have the sharer move around: scroll code, switch apps, play a video. Is the text
   sharp? Does it keep up?
2. **Talk.** Voice is always on. Mute with the mic button in the bar; the sharer can also press
   ⌃⌥⌘M from any app, and the guest **M** in the viewer window.
3. **Ask for control.** The guest clicks **Control** in the bar, or presses **F9** in the viewer
   window.
4. **Allow it.** If the guest was let in with control, they get it at once. Otherwise the
   sharer's bar shows the request: click **Allow** (it turns on after a second) or **Deny**. The
   guest clicks into the shared screen and uses it like their own: type, click, scroll, use
   shortcuts. The sharer's bar turns red and says who is controlling.
5. **Take it back.** The sharer presses **⌃⌥⌘R** (Control-Option-Command-R) from any app, or
   clicks **Take back**. The guest can give it back with **Give back** or **F9**.
6. **Leave.** The sharer clicks **Stop**; the guest clicks **Leave** or closes the viewer
   window. When the sharer stops, the guest's call ends by itself.

## What to report

- **What happened**, and what you expected instead. Screenshots or a screen recording help.
- **The numbers.** During the call, click **⋯** (More) in the bar, then **Copy numbers**, and
  paste what it copies.
- Your Mac model, your macOS version, and your network (home Wi-Fi, office, mobile hotspot).

## Known limits

- **Unsigned builds.** macOS warns before the first launch, as described above.
- **Ringing isn't instant.** Knocks and rings arrive within about two seconds.
- **The server can see the call.** The screen and voice pass through the call server, which
  could read them if whoever runs it wanted to. Don't share secrets on screen, and don't type
  passwords while in control.
- **4K takes a moment.** The first 20 seconds or so of a 4K share may run at 30 fps before
  reaching 60.
- **Windows** has a viewer you run from a folder, without sign-in, for test servers only. If you
  were sent one, its README explains it.
