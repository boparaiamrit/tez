---
name: tez-desktop-app
description: Use when changing tez-app, the GPUI Kit desktop app in app/ (pages, cards, sidebar, title bar, menus, popovers, dialogs, switches, toasts, tray, theme or palette, keyboard shortcuts), when an app build or startup error mentions gpui, or when asked for screenshots, a UI audit, or proof that the app looks right in light and dark.
---

# tez-app: the GPUI Kit desktop app

`app/` is a native window plus tray icon on GPUI Kit 0.10 (gpui-pre 0.3.12). It holds no state of its
own: it polls the daemon's API (`app/src/api.rs`, 127.0.0.1:7979, header `X-Tez: app`) and renders.

| File | Holds |
|---|---|
| `main.rs` | global `Tez` state, the 250 ms loop, fetch/apply, `actions!` and `bind_shortcuts`, `navigate()`, the single-instance listener on 127.0.0.1:7978 |
| `frame.rs` | window frame: title bar, icon rail, side list, panel, Home, site pages, settings sheet, dialogs, toasts |
| `pages.rs` | Services, PHP and Node.js, new site from GitHub, `select()` combo box, version pickers |
| `ui.rs` | the design system: `Pal::dark()` / `Pal::light()`, `BODY`, fonts, and every shared part |
| `tray.rs`, `api.rs` | tray menu; HTTP client and open-URL/editor/terminal helpers |

## Design system: tokens only

- **Color:** only `Pal` fields (`p.text`, `p.text2`, `p.text3`, `p.card`, `p.accent`, `p.line2`...). Never a
  hex literal in a page. To change a color, edit `Pal::dark()`/`Pal::light()` in `ui.rs` and run
  `scripts/contrast.py`. Chart colors are `Pal.series`, in a fixed, color-blind-checked order.
- **Type:** body `ui::BODY` (14 px Windows/Linux, 13 px macOS); `h1` 22 px semibold; muted 13 px;
  font `ui::sans()`; mono JetBrains Mono.
- **Size:** controls 32 px tall (28 small) with 6 px corners; sidebar rows 32 px; cards 10 px corners
  with `card_shadow()`; grids through `grid_of(items, cols)` (14 px gap).
- **Parts:** build from `ui.rs` (`button`, `text_btn`, `switch_busy`, `cardbox`, `stat_strip`, `field`,
  `text_input`, `spinner`, `skeleton_rows`). At most one primary button per view.
- **Behaviour:** switches are optimistic: flip at once, add a `Busy` entry with a spinner, toast when
  done, and the card never moves or disappears while busy. Loading shows skeleton rows, not a blank.

## GPUI gotchas, each of which broke the app once

| Symptom | Fix |
|---|---|
| Clicks pass through a popover, menu, sheet or dialog to what is underneath | `.occlude()` on its root (and on a full-window backdrop). `deferred(anchored()...)` does not block the mouse. |
| Title-bar buttons, menus or back/forward do nothing | `window_control_area(WindowControlArea::Drag)` only on the empty spacer (`tb-drag`), never on the whole bar |
| Panel blank after the sidebar toggles | the scroll container's id must include the layout: `panel-scroll-{page}-{side_hidden}` |
| `actions!` does not compile | `extern crate gpui_kit as gpui;` at the crate root |
| Crash at start: `InvalidKeystrokeError` | `KeyBinding::new` takes a `&'static str` and rejects `secondary-`: `if cfg!(target_os = "macos") { "cmd-f" } else { "ctrl-f" }` |
| An action must focus an input | actions get no `window`: set state in `cx.on_action`, then call `window.focus(&h, cx)` in render, as the jump overlay does |
| Typed text vanishes on re-render | create `InputState` with `window.use_keyed_state(...)` (`ui::text_input` does) |
| Borrow errors after `let t = tez(cx)` | copy the values you need out of `t` before any `cx`-mutating call |
| `if`/`else` element types differ | `.into_any_element()` on both arms |
| `flex_grow`/`flex_shrink` errors | they take an argument: `flex_shrink(0.)` |

A new shortcut takes four edits: the name in `actions!`, a binding in `bind_shortcuts`, a
`cx.on_action` handler, and the menu hint through `ui::keys("Mod+F")`.

## Build

`cargo build --release -p tez-app -j 4` (`-j 2` when memory is tight). Debug builds of the app have run
the machine out of memory. The app on screen is the installed copy in `~/.tez/bin`, so building into
`target/` never fights a locked exe. Putting a new build on screen is a deploy: only when the user asks
(see the tez-development skill).

## Proof without touching the desktop

The user's desktop is live, with other apps on it. A capture script once clicked into a chat app.

- **Never inject input.** No `SendInput`, no posted or sent mouse or key messages, no pyautogui, no
  clicking or typing anywhere. For shortcuts, clicks and hover, tell the user exactly what to press and
  what should happen, and list them under "not verified by me".
- **Never capture the screen.** Capture tez's own window only, with `scripts/snap.py`.
- **Navigate only through the app's own port:** `go <target>` sent to 127.0.0.1:7978 (`scripts/tour.py`).

| Tool (`python -I <skill>/scripts/...`) | Does |
|---|---|
| `contrast.py app/src/ui.rs` | WCAG ratio of every text/background pair in both palettes. Must end `0 failing pairs`. |
| `snap.py <out.png>` | DPI-aware PrintWindow of the tez window only (without DPI awareness the capture is cropped). Refuses if no visible tez window. Needs Pillow. |
| `tour.py <dir> go:<target> wait:<s> snap:<name> ...` | navigates and captures in one run, e.g. `go:theme:light wait:0.6 snap:home-light` |

`go` targets (`navigate()` in `main.rs`):
- **Look only:** `home`, `sites`, `site:<name>[:<tab>]`, `services[:<name>]`, `php`, `node`, `dumps`,
  `mail`, `logs`, `settings:<section>`, `new:<source>`, `open:<select-id>[:<query>]`, `menu:<key>`,
  `ctx:<kind>:<name>`, `remove:<site>[:files]` (opens the dialog; removes nothing), `scroll:top|bottom`,
  `offline`, `close-note`.
- **Change real state:** `theme:light|dark|system` is saved to the user's settings: note their theme
  first and set it back. `flip:svc|site|share:<name>` acts like clicking the switch: it stops or starts a
  real service or site, and `flip:share` puts a site on the internet. Only with the user's go-ahead.

Before capturing:
1. `snap.py` needs the window open and not minimised. If tez is closed to the tray, ask the user
   before opening it (`tez open`).
2. The sidebar shows the user's real site names. Captures for the README or anything public use demo
   names (hello, marketplace, recipes, storefront).
3. PrintWindow cannot render Mica or vibrancy; the backdrop comes out plain. Say so rather than
   claiming it looks right.

Then Read every PNG and check: edges aligned to the grid, nothing clipped on the right or bottom, text
readable in both themes, one primary button, loading and empty states present. Report what you looked
at, and what you could not check.
