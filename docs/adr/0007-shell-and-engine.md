# ADR 0007: The desktop app and the engine run as two processes

Status: proposed (2026-11-14)

## Context
The engine exists today as `tc-spike`: capture, encoding, WebRTC, voice, the native viewer
window, the "is controlling" label, the revoke hotkey and input injection. Its latency numbers
were measured on that code. The desktop app is a Tauri 2 shell (lobby, settings, call controls)
and has to drive that engine without slowing it down.

Running both in one process doesn't work on macOS:

- **Only one event loop can own the main thread.** Tauri runs its own (tao), and winit, which the
  viewer, the overlay label and the hotkeys use, refuses to start a second one in the same
  process.
- **Tauri doesn't hand raw keyboard and mouse events to Rust** for a native window. The viewer
  needs every key, modifier and scroll delta to forward control to the sharer.

Moving the viewer, overlay and hotkeys onto Tauri's loop would mean rewriting measured code on the
latency-critical path, and capturing keys through AppKit by hand.

## Decision
- **Two processes.** The shell (`apps/desktop`) shows windows and text only; no media runs in its
  webview. The engine is a helper binary that the shell starts with `std::process::Command` and
  that keeps all media and control code, unchanged.
- **The helper lives inside the bundle.** `tauri build` copies it into
  `TwinCursor.app/Contents/MacOS` (`bundle.externalBin`) and signs it with the app. macOS
  attributes a process started by the app to the app, so the Screen Recording, Accessibility
  and Microphone grants a person gives TwinCursor apply to the helper, and the prompts name
  TwinCursor. `Info.plist` carries `NSMicrophoneUsageDescription`, and the hardened runtime gets
  the `com.apple.security.device.audio-input` entitlement (on the app and the helper).
- **The helper never outlives the shell.** It runs with `--exit-on-stdin-close`: the shell holds
  the helper's stdin open for as long as it runs, and when the shell quits or dies, the kernel
  closes the pipe and the helper exits. A screen is never shared with no window showing it.
- **A grant reaches a fresh helper, not the app's restart.** macOS applies a new permission to
  processes started after it was given. Every permission check starts a new helper, and the
  shell replaces its long-running helper with a new one after a check, rather than asking the
  person to quit and reopen TwinCursor.
- **Capture leaves the shell's windows out.** The helper gets the shell's process id explicitly
  (`--exclude-app-pid`) and adds that application to ScreenCaptureKit's exclusion list, next to
  its own. Excluding by application, not by window, is what keeps windows the shell opens after
  capture starts out of the stream.
- **`tc-spike permissions`** reports what macOS lets the helper do, as one JSON object, and with
  `--request` asks for what is missing. The shell's "Run checks" button uses it.

## What was verified
On an Apple silicon Mac, macOS 26, with an ad-hoc signed build (`apps/desktop/scripts/bundle.sh`):

- **The bundle builds and starts both processes.** `open TwinCursor.app`, then
  `pgrep -fl TwinCursor.app/Contents/MacOS` lists `twincursor-desktop` and
  `tc-spike permissions --watch --exit-on-stdin-close`, the helper's parent being the shell.
- **The helper exits when the shell is killed.** After `kill -9` of the shell, the helper was gone
  81 ms later. A first version didn't exit: it logged a line before exiting, the log write to
  the dead shell's stderr failed and panicked the watcher thread. The helper now exits without
  writing anything.
- **macOS attributes the helper to TwinCursor, not to whatever started the build.** The same
  `tc-spike permissions` binary reports Screen Recording, Accessibility and Microphone as granted
  when run from a terminal that has them, and `not_granted`, `not_granted` and `not_asked` when
  the bundled app starts it (`open -n TwinCursor.app --args --run-checks out.json`, which runs the
  checks that need no prompt and writes the results).
- **Closing the helper's stdin** made it exit in 7 ms (the same unattended run).
- **The shell is cheap at idle.** Over 30 s, sampled every 2 s with `ps -o %cpu,rss`: the shell at
  0.0–0.1% CPU and 73–91 MB resident, the helper at 0.0–0.1% and 13 MB.
- **Signatures.** `codesign -dv --verbose=2` shows the app as `Identifier=dev.twincursor.desktop`
  and the helper as `Identifier=tc-spike-<hash>`, both `Signature=adhoc`, flags `adhoc,runtime`,
  no team. `codesign -d -r-` shows each one's designated requirement as its `cdhash` alone.

## What a person at the Mac has to confirm
These need someone to answer macOS prompts, so they weren't run unattended. Open the bundled
app, press **Run checks** and answer each prompt; the window shows pass or fail for each.

1. The Screen Recording, Accessibility and Microphone prompts name **TwinCursor**, and after
   allowing each one, a fresh helper reports it as granted (the first three checks).
2. Screen Recording takes effect in a newly started helper without quitting and reopening the
   app, or, if it doesn't, that reopening the app is enough.
3. The helper captures the screen, and both probe windows stay out of the capture: the one open
   before capture starts and the one opened after it (the capture checks compare a capture
   without the exclusion, where the probe must show, with one that has it).
4. **Grants after a rebuild.** With ad-hoc signing, macOS identifies the app by its code hash,
   so a rebuilt bundle may look like a new app and lose its grants. Rebuild, run the checks
   again, and note whether the grants survived.

## Consequences
- Commands and events between the shell and the engine cross a process boundary. They will
  travel as JSON lines over the helper's stdin and stdout, the pipes that already tie the two
  lifetimes together. The shell holds no media state, so a crash in the engine ends the call but
  not the app.
- The revoke hotkey and the control label stay in the engine, so taking control back never
  depends on the shell being responsive.
- The sharer's terminal prompt for control reads stdin, which `--exit-on-stdin-close` now owns,
  so the two can't be combined until consent moves to the shell's own window.
- ScreenCaptureKit only lists an application while it has a window, so the shell must have a
  window open when capture starts; a shell with only a tray icon would need its windows added
  to the filter as they open.
- Each bundle is two binaries; the helper is about 41 MB of the 49 MB bundle.
- **A stable signing identity belongs to the release milestone.** Because ad-hoc grants are tied
  to the code hash, people who update an ad-hoc build may have to grant permissions again. A
  Developer ID signature gives a designated requirement based on the bundle identifier and team,
  so grants survive updates. That identity, notarization and auto-update come with the first
  release.
- The app's identifier (`dev.twincursor.desktop`) is what macOS files grants under; changing it
  later resets everyone's permissions.
