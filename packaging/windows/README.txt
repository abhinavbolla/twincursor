TwinCursor viewer for Windows
=============================

Watch a Mac's shared screen and talk. Windows can join a call but can't share its own screen
yet.

This viewer has no sign-in. It can't join a call started from the TwinCursor menu bar app on a
Mac; it joins a room by code on the server in server.txt, where the sharer starts the call from
a terminal with the same server and room code.

Start
-----
1. Unzip the whole folder (right-click the zip, Extract All). Running from inside the zip
   doesn't work.
2. Double-click "Join TwinCursor".
3. Windows may warn that it protected your PC, because this build isn't signed. Click
   "More info", then "Run anyway". If it asks whether to run a file from the internet, click
   Run.
4. Type your name and the room code the sharer sent you.

The sharer's screen opens in its own window. Close it to leave the call.

In the viewer
-------------
F9   ask for control, and give it back
M    mute or unmute your mic (F8 while you're in control)

F9 asks for control; only a sharer whose app can answer the request can grant it. While
you're in control, your mouse and keyboard go to the sharer's Mac. The sharer sees that you're
in control and can take it back at any time.

server.txt
----------
"Join TwinCursor" reads the server's address and keys from server.txt in this folder
(see server.txt.example). Without one it tries a development server on this PC, which
won't be running. Whoever sent you TwinCursor normally includes server.txt.

The key and secret in server.txt let anyone start calls on that server. Keep the folder to
yourself and don't pass it on.

Reporting a problem
-------------------
The black window behind the viewer shows what TwinCursor is doing. If something goes wrong,
copy its last lines into your report along with what you did and what you expected.

TwinCursor is free software under the GNU Affero General Public License, version 3 (LICENSE).
