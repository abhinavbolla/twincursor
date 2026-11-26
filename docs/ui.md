# The panel and the call bar

TwinCursor's everyday UI is two small surfaces, each meant for its own native window:

- **The panel** (`apps/desktop/src/Panel.tsx`), opened from the menu bar icon: who you are,
  Share my screen, Paste link, and the people you can call.
- **The call bar** (`apps/desktop/src/CallBar.tsx`), the only thing on screen during a call: a
  440 × 48 pill that grows downward for a knock, a control request, a notice or its menu.

Both read the same two things: the home view from the Rust side (`useHome`) and the call, built
by replaying `call_snapshot` and then listening to the engine (`useCall`, in `useSurface.ts`).
What each shows is a pure function, tested over every state: `panelModel` (`panelModel.ts`) and
`barModel` (`barModel.ts`). The components only draw what those return.

A window loads one surface with `?view=panel` or `?view=bar`; without `view`, the page is the
panel.

## In the app

The app lives in the menu bar. Each surface is its own native window, made hidden at launch so
it is ready the moment it is needed (`apps/desktop/src-tauri/src/windows.rs`; where they go and
when they show is decided in `surfaces.rs`, as plain tested functions).

| | The panel | The call bar |
| --- | --- | --- |
| Window | `panel`, 340 pt wide | `bar`, 440 pt wide |
| Height | Its content's, from 120 to 560 pt; the people list scrolls inside | Its content's, from 48 pt; it grows downward |
| Where | Centred under the menu bar icon, 6 pt below the menu bar, on the icon's display, kept 8 pt from its edges. Before the icon's place is known: top right of the main display | Where it was last dragged (kept with the app's settings), moved onto a display if that one is gone or it would hang off. At first: top centre of the main display, 12 pt under the menu bar |
| Shows | On a click on the icon, its menu's **Open TwinCursor**, the Dock icon, or launching the app (with focus); on a ring while not in a call (without focus) | Whenever a call is: starting, sharing, knocking, in a call, ending. Never takes focus when it appears |
| Hides | When it loses focus, on a second click on the icon, on Escape. Not while a sign-in it started is in the browser (it comes back when that ends) or a macOS permission prompt it brought up is showing | When the call is over |
| Level | Above other windows, on the current Space | Above other windows, on every Space and over full-screen apps |

- **The menu bar icon** is the mark as a template image (drawn in code, `mark.rs`), so macOS
  tints it for light and dark menu bars. A click opens or closes the panel; a right click or a
  Control-click opens its menu: **Open TwinCursor**, **Quit TwinCursor**.
- **Native look.** The windows are ordinary titled windows whose title bar shows nothing: no
  title, no traffic lights, no separator, and the page fills the window edge to edge. So macOS
  draws the corners, the hairline and the shadow, as for any window, in light and dark. Inside a
  window the surface drops its own radius, border and shadow (`.in-window` in `styles.css`), and
  the page and the window behind it are the surface's colour, so nothing else shows at the
  corners. No transparent windows and no private API. In the controlling state the row keeps
  its wash, keycaps and **Take back**; the red hairline is left out, since a line along the edge
  would be cut by the window's rounded corners.
- **Fitting.** The page measures the surface (`useWindowFit`, `inWindow.ts`) and the app resizes
  the window in one step, keeping its top edge: the panel hangs from the icon, the bar grows
  downward.
- **Focus.** Nothing is focused when a window shows or after it loses focus, so a stray Enter or
  Space can't press a button. The call bar doesn't become key when it appears, and its first
  click acts, so answering a knock takes one click and typing stays in the app you were in.
- **Dragging.** The call bar moves by its row's background; buttons in the row stay buttons.
- **Arming.** Let in and Allow wait a second from when the knock or request could be seen: the
  later of when it reached the bar and when the bar came on screen ([calls](calls.md)).
- **Quitting.** Closing or hiding a window never quits. **Quit TwinCursor** (in the panel's menu,
  the icon's menu, or ⌘Q) ends or leaves any call first, then quits.
- **Looking at the other appearance.** `TWINCURSOR_APPEARANCE=light` or `=dark` in the app's
  environment draws it that way whatever the system says, without changing System Settings.

## The panel

340 px wide, as tall as its content up to 520 px; the people list scrolls inside.

| State | What it shows |
| --- | --- |
| Not set up | "This build isn't connected to a server." and "See the self-hosting guide to set one up." |
| Signed out | "Sign in to share your screen and call people." and **Sign in with GitHub**; a sign-in error as one line under it |
| Signing in | "Finish signing in in your browser…" and a quiet **Cancel** |
| Home | Rings on top ("**Sam Rivera** is calling", **Decline**, **Join**); **Share my screen**; **Paste link** with "Join with a link someone sent you"; **People**, one 36 px row each with a **Call** button |
| No people | "People you've been in a call with show up here." |
| Starting | Share reads "Starting…", nothing moves |
| Sharing | "You're sharing your screen" with **Stop sharing**, the call link with **Copy link**; Call rings people into the call, and rows say "Ringing…" or "In the call" |
| Knocking | "Waiting for **Alex** to let you in…" with **Cancel** |
| In a call | "In **Alex**'s call" with **Leave** |
| Needs Screen Recording | "TwinCursor needs Screen Recording to share your screen." with **Open Settings** |
| Error, notice | One line each, closable; an error in the bad colour and stays until closed, a notice goes after 6 s (paused under the pointer) |
| Server problem | A muted line pinned to the bottom |

The header holds the mark, "TwinCursor" and your avatar, which opens: your name and @handle,
**Settings…** (the server settings, as a second page with a back arrow), **Sign out** and
**Quit TwinCursor**. Signed out, a More button opens the same menu without Sign out.

## The call bar

The row, left to right: a status dot (connected, reconnecting pulsing, not connected), up to
three faces overlapped by 6 px and "+N", one line that says what matters most now, then the
row's one button, **Mute**, **More**, and **Stop** (sharer) or **Leave** (guest).

| State | Line | In the row |
| --- | --- | --- |
| Nobody yet | "Sharing your screen" | **Copy link** ("Copied" for 2 s) |
| With guests | "With **Sam**", "With **Sam and Kim**", "With **Sam and 2 others**" | |
| Trusted | "**Sam** can take control" | **Take back** |
| Controlled | "**Sam** is controlling", in the bad wash with a bad hairline | ⌃ ⌥ ⌘ R keycaps, **Take back** (danger) |
| Reconnecting | "Reconnecting…" | |
| Guest knocking | "Waiting for **Alex** to let you in…" | **Cancel** in place of Leave |
| Guest watching | "**Alex**'s screen" | **Control** |
| Guest asked | "Asked for control…" (the face says whose call) | **Never mind** |
| Guest controlling | "Controlling **Alex**'s Mac" | F9 keycap, **Give back** |

While anyone is in control, the row drops the faces and More so the line, the hotkey and the
button fit. Names in the row are first names. Buttons keep their size and the line gives way,
but a name is never cut below about six characters: before that, the row drops the faces, then
the keycaps (`fit.ts`; Take back's tooltip still names ⌃⌥⌘R).

Sections under the row:

- **Knock** (`role="alertdialog"`): avatar, "**Sam Rivera** wants to join", @handle (nothing for someone without one; the name then
  sits centred beside the avatar); **Turn
  away**, **Let in with control**, **Let in**. Until the Rust side arms the knock, the two let-in
  buttons are off and a thin line runs across the card's top. More knocks queue below.
- **Control request**: "**Sam Rivera** wants to control your Mac", "They'll use your mouse and
  keyboard until you take control back", **Deny** and **Allow** (off for `ALLOW_AFTER_MS`, with
  the same line).
- **Needs Accessibility**: "TwinCursor needs Accessibility to give control." **Open Settings**.
- **Notices**: errors first, then notices; at most two, each closable. A notice goes away by
  itself after 6 s, paused while the pointer is over it; an error stays until closed
  (`notices.ts`).
- **Menu** (More; closes on a second click or Escape): the call link and **Copy link**, the
  numbers in mono in three columns with **Copy numbers**, a row per guest with **Give control**
  ("Can take control" or "Controlling" once given), and "Your mic: …" with ⌃ ⌥ ⌘ M.

## Design rules

- Direction: fast, exact, companionable. Graphite surfaces, the icon's indigo as the only
  accent (on dark the primary button is off-white), keycaps for hotkeys, numbers in mono.
- One opaque surface per window (`--float`), a hairline and the floating shadow; never
  transparent, since the bar sits over other apps. Light and dark each have their own values.
- 13 px text, 12 px meta, names in 500, at most two text colours plus muted.
- Spacing on the 4 px scale; rows share a left edge; space for changing buttons is kept so
  nothing jumps. Motion uses the one easing curve, 180 ms, and stops for reduced motion.
- One primary button per surface; quiet buttons for the rest; danger only for Take back and the
  muted mic. A ring takes the primary from Share my screen.
- Long names truncate with an ellipsis; names are isolated (`<bdi>`) for right-to-left scripts;
  initials use one character for scripts without case.
- Copy is plain and short, in sentence case; no exclamation marks, no "please", no periods on
  labels and hints.
- Everything is reachable by Tab with a visible focus ring; cards are labelled alert dialogs.
- A window shows with nothing focused, so a stray Enter or Space can't act; the first Tab moves
  focus to the first control.
- Inside a window the window draws the corners and edge, so the controlled bar loses its red
  hairline and gets a stronger wash instead (`--bad-wash-strong`). The window grows to fit an open
  account menu and shrinks back when it closes (`fitHeight` in `inWindow.ts`).

## Seeing every state

`npm run dev` in `apps/desktop`, then open `http://localhost:5173/` with:

- `?view=panel&state=<name>`: `off`, `out`, `out-menu`, `signing`, `home`, `empty`, `many`, `ring`, `rings`,
  `needs`, `starting`, `hosting`, `knocking`, `joined`, `error`, `settings`, `offline`
- `?view=bar&state=<name>`: `alone`, `knock`, `knock2`, `knock-nohandle`, `with`, `trusted`,
  `controlling`, `asked`, `needs`, `menu`, `notice`, `g-knocking`, `g-watching`, `g-asked`,
  `g-controlling`, `reconnecting`, `muted`, `long-trusted`, `long-controlling`,
  `long-g-controlling`
- `?view=gallery&of=panel` or `&of=bar`: every state of a surface in a grid
- `?view=panel` or `?view=bar` without a state: the live mock with its dev panel;
  `?view=both`: the panel and the bar side by side on the live mock
- add `&frame=desktop` for a busy pretend desktop behind, `&theme=light` or `&theme=dark` to pin
  the theme, `&window=1` to see a surface as its native window shows it (edge to edge, in a frame
  of the height the window would be given), `&menu=1` to open the panel's account menu or the
  bar's menu in any state

An unknown state lists the valid ones. The states are plain data in `src/scenarios.ts` (a home
view and the engine output so far) and are also the fixtures for the view-model tests. Each
state can be clicked through: a small pretend Rust side (`src/scenarioBridge.ts`) answers the
flows.
