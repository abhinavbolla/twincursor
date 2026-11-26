TwinCursor for Windows
======================

Join a pair-programming call from Windows: watch the other person's Mac screen at full
quality, talk, and take control of their Mac when they give it to you.

Windows can join calls but can't share its own screen yet, so there is no "Share my screen"
and no "Call" button. Someone on a Mac shares and sends you the call link, or rings you.

Start
-----
1. Unzip the whole folder (right-click the zip, Extract All) to a short path, for example
   C:\TwinCursor. Running it from inside the zip doesn't work.
2. Double-click TwinCursor.exe.
3. Windows may say it protected your PC, because this build isn't signed. Click
   "More info", then "Run anyway".
4. TwinCursor lives in the notification area at the bottom right of the taskbar (look under
   the ^ arrow if you don't see it). Click its icon to open or close the panel.
5. Click "Sign in with GitHub". Your browser opens; sign in there, then come back.

Join a call
-----------
- Copy the call link the sharer sent you, then click "Paste link" in the panel. You wait
  until the sharer lets you in.
- Or, when someone rings you, click Join on the card in the panel.

The sharer's screen opens in its own window. A small bar on top of your screen shows the call:
mute, leave, and asking for control.

In the viewer window
--------------------
F9   ask for control, and give it back
M    mute or unmute your mic (F8 while you're in control)

While you're in control, your mouse and keyboard go to the sharer's Mac. The sharer sees that
you're in control and can take it back at any time.

The first time the call uses your microphone, Windows may ask whether TwinCursor may use it.
If nobody hears you, check Settings > Privacy > Microphone ("Allow desktop apps to access
your microphone").

Needs
-----
- Windows 10 or 11, 64-bit.
- The Microsoft Edge WebView2 Runtime. Windows 11 has it, and most Windows 10 PCs have it
  through Edge. Without it, TwinCursor says so when it starts and links to
  https://developer.microsoft.com/microsoft-edge/webview2/ ; install the "Evergreen
  Bootstrapper" from there and start TwinCursor again.

Reporting a problem
-------------------
TwinCursor writes what it does to a log file:

  %LOCALAPPDATA%\dev.twincursor.desktop\logs\twincursor.log

(paste that into the File Explorer address bar). It never contains your sign-in or the call
link. Send its last lines with your report, along with what you did and what you expected.

To sign out, open the panel, click your picture, then "Sign out". Quit TwinCursor from the same
menu, or right-click the tray icon.

TwinCursor is free software under the GNU Affero General Public License, version 3
(LICENSE.txt).
