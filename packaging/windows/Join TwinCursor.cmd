@echo off
rem Joins a TwinCursor call as a viewer: asks for a name and a room code, then opens the
rem sharer's screen in a window. The server comes from server.txt next to this file, if there
rem is one (see server.txt.example); its values stay in this window's environment and are
rem never shown.
rem
rem Delayed expansion stays off while server.txt is read, so values with exclamation marks
rem survive. Everything typed is then used only through delayed expansion, whose results cmd
rem never parses again, so special characters in a name are passed on as they are.
setlocal EnableExtensions DisableDelayedExpansion
title Join TwinCursor
pushd "%~dp0"

if not exist "tc-engine.exe" (
  echo tc-engine.exe isn't next to this file. Unzip the whole folder first, then run
  echo "Join TwinCursor" from the unzipped folder.
  goto failed
)

if exist "server.txt" (
  for /f "usebackq eol=# tokens=1,* delims==" %%A in ("server.txt") do (
    if /i "%%A"=="TWINCURSOR_SERVER_URL" set "TWINCURSOR_SERVER_URL=%%B"
    if /i "%%A"=="TWINCURSOR_API_KEY" set "TWINCURSOR_API_KEY=%%B"
    if /i "%%A"=="TWINCURSOR_API_SECRET" set "TWINCURSOR_API_SECRET=%%B"
  )
) else (
  echo No server.txt next to this file, so this joins the local development server.
  echo Ask whoever sent you TwinCursor for their server.txt.
  echo.
)

setlocal EnableDelayedExpansion

:ask_name
set "NAME="
set /p "NAME=Your name (what the other person sees): "
if not defined NAME goto ask_name
set "CHECK=!NAME: =!"
if not defined CHECK goto ask_name
rem Quotes and backslashes could break out of the quoted argument below.
set "CHECK=!NAME:"=!"
if defined CHECK set "CHECK=!CHECK:\=!"
if not "!CHECK!"=="!NAME!" (
  echo Please leave out quotes and backslashes.
  goto ask_name
)
if not "!NAME:~64!"=="" (
  echo Names can be up to 64 characters.
  goto ask_name
)

:ask_room
set "ROOM="
set /p "ROOM=Room code (like 7kq2-m9xd-p4tz-hc3w): "
if not defined ROOM goto ask_room
rem The app's rules (apps/desktop/src/room.ts): case, spaces and dashes don't matter, o reads
rem as 0, i and l as 1, and what's left is 16 characters of Crockford's base32.
set "CODE=!ROOM: =!"
if defined CODE set "CODE=!CODE:-=!"
if not defined CODE goto bad_room
rem Replacing is case-insensitive, so replacing a letter with itself lowercases it.
for %%L in (a b c d e f g h i j k l m n o p q r s t u v w x y z) do set "CODE=!CODE:%%L=%%L!"
set "CODE=!CODE:o=0!"
set "CODE=!CODE:i=1!"
set "CODE=!CODE:l=1!"
if "!CODE:~15,1!"=="" goto bad_room
if not "!CODE:~16!"=="" goto bad_room
set "REST=!CODE!"
for %%C in (0 1 2 3 4 5 6 7 8 9 a b c d e f g h j k m n p q r s t v w x y z) do (
  if defined REST set "REST=!REST:%%C=!"
)
if defined REST goto bad_room
set "CODE=!CODE:~0,4!-!CODE:~4,4!-!CODE:~8,4!-!CODE:~12,4!"

echo.
echo Joining !CODE! as !NAME!. Close the viewer window to leave.
echo In the viewer, F9 asks for control and gives it back. M mutes your mic, or F8 while
echo you're in control.
echo.
".\tc-engine.exe" view "--room=!CODE!" "--name=!NAME!"
set "RC=!ERRORLEVEL!"
if not "!RC!"=="0" (
  echo.
  echo TwinCursor stopped with an error ^(exit code !RC!^). The lines above say what happened;
  echo please copy them into your report.
  goto failed
)
popd
exit /b 0

:bad_room
echo That isn't a room code: it should be 16 letters and digits, like 7kq2-m9xd-p4tz-hc3w.
goto ask_room

:failed
echo.
pause
popd
exit /b 1
