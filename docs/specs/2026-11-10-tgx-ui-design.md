# tgx — UI design

Status: draft for approval · Date: 2026-11-10 · Companion to
[`2026-11-09-tgx-client-design.md`](2026-11-09-tgx-client-design.md)

The visual source of truth is [`docs/mockups/tgx-ui.html`](../mockups/tgx-ui.html). It
has two views, switched in the page header:

- **Prototype** — every screen, clickable. The "Перейти к экрану…" menu jumps to any of
  them; the same screens open by URL hash (`#login=code`, `#lock`, `#search=…`,
  `#call=in`, `#settings=diag`, `#offline`, `#compact`, `#dark`).
- **Design system** (`#ds`) — tokens, components in all states, and the screen map.
  Specimens are rendered by the prototype's own functions and colour values are read
  back from the live CSS, so this view cannot drift from the prototype.

This document records what a builder needs that a picture does not carry: the rules,
the numbers, and which decisions are still open. Interface language is Russian.

## 1. Principles

1. **The route is always visible.** The status bar shows the route chain and the
   active hop. A direct connection carries a warning tag. There are no silent
   fallbacks: a forced route that fails shows "нет маршрута", not a quiet switch.
2. **Network facts are set in monospace.** Hosts, latency, DC ids, sizes, logs use
   Cascadia Mono; everything else uses Segoe UI. Data and chrome never look alike.
3. **Familiar, then yours.** The default look follows Telegram's conventions (wallpaper
   with a doodle pattern, saturated outgoing bubble, tails, bright avatars), so nothing
   feels missing next to the official client. Every colour, the wallpaper, bubble
   radius, tails and avatar shape are user settings, and a theme travels as a file
   (§2.4). Still cheap to draw: no blur, no decorative motion, system fonts only;
   gradients only on the wallpaper and avatars.
4. **Local things say they are local.** Saved deleted messages, edit history, notes
   and bookmarks exist only on this PC, and each of them states that in place.

## 2. Tokens

### 2.1 Colour

Defaults are the built-in presets **Дневная** (light) and **Ночная** (dark). Every token
can be overridden per mode (§2.4).

| Token | Light | Dark | Use |
|---|---|---|---|
| `win` | `#ffffff` | `#17212b` | Headers, composer, settings window, menus |
| `pane` | `#ffffff` | `#17212b` | Chat list, status bar |
| `rail` | `#eef1f4` | `#111a22` | Folder rail |
| `chat` | `#cfdcc4` | `#0e1621` | Conversation background when the wallpaper is a plain colour |
| `wall` | green-blue gradient | deep blue gradient | Wallpaper under the pattern |
| `pattern` / `pattern-o` | `#2f5d2a` / 6.5 % | `#8fb4d8` / 4.5 % | Doodle pattern colour and opacity over the wallpaper |
| `in` | `#ffffff` | `#182533` | Incoming bubble |
| `out` | `#e2f6d1` | `#2b5278` | Outgoing bubble (its own colour, not derived from the accent) |
| `in-meta` | `#8b9298` | `#6d7f8f` | Time in incoming bubbles |
| `out-meta` | `#3f9a45` | `#86b5e0` | Time, ticks, quote bar in outgoing bubbles |
| `sel` | `#f1f3f5` | `#202c38` | Hover, search field |
| `sel-on` / `sel-on-ink` | accent / `#ffffff` | `#2b5278` / `#ffffff` | Selected chat row and its text |
| `line` | `#e4e7ea` | `#0d141b` | Borders and dividers, always 1 px |
| `text` | `#121417` | `#f2f4f6` | Primary text |
| `muted` | `#6c7277` | `#7d8e9e` | Secondary text, timestamps, captions |
| `faint` | `#9aa1a7` | `#5f6f7e` | State icons, placeholders, disabled. Never text |
| `accent` | `#3a8ee6` | `#5aa9ec` | Actions, links, badges, own unread counters |
| `accent-ink` | `#ffffff` | `#ffffff` | Text and icons on accent |
| `accent-soft` | accent over `win`, 12 % | 16 % | Quotes, link previews, reactions |
| `chip-bg` / `chip-ink` | translucent dark green / white | translucent navy / `#dfe7ee` | Date chips and service messages over the wallpaper |
| `ok` / `warn` / `bad` | `#2e8a4f` / `#9a6410` / `#d0443a` | `#5cc98a` / `#e5aa4e` / `#ec6f62` | Connected / connecting, direct route / error |

`accent-soft` is derived. Slint has no `color-mix`, so compute it when the theme or
accent changes (linear sRGB mix) and expose it as a plain colour.

Accent swatches (light / dark): blue `#3a8ee6` / `#5aa9ec` (default), teal `#2b8a78` /
`#5fbfa9`, violet `#7b5cd6` / `#a48cf0`, terracotta `#c2603a` / `#ee9268`, raspberry
`#c94f86` / `#ec86b4`, graphite `#4a525a` / `#aab2b9`, plus any colour from a picker.
Avatars without a photo take one of eleven bright top-to-bottom gradients by a hash of
the name, with white initials.

### 2.2 Type

Segoe UI Variable Text → Segoe UI for the interface; Cascadia Mono → Consolas for
network data. No fonts are bundled.

| Role | Size / weight |
|---|---|
| Login screen title | 22 / 600 |
| Profile name | 17 / 600 |
| Settings window title | 16 / 600 |
| Text input | 15 / 400 |
| Messages and body | 14 / 400, line height 1.4, user-adjustable 12–18 |
| Chat title, sender name | 14 and 13 / 600 |
| Preview, captions, hints | 13 / 400, `muted` |
| List time, counters, date chips | 12 / 400 |
| Time inside a bubble, rail labels | 11 / 400 |
| Status bar, routes, logs | 12 mono |
| Login code, latency readout | 24–34 mono / 600 |

### 2.3 Metrics

| Part | px |
|---|---|
| Title bar | 32 |
| Folder rail | 68 (52 compact) |
| Chat list | 310 (280 compact) |
| Chat header | 54 (46 compact) |
| Info panel | 320, pushes the conversation, does not overlay it |
| Status bar | 26 |
| Minimum window | 1060 × 620 |
| Settings window | 470 × up to 680 |
| History search | 720 |
| Drawer | 290 |
| Bubble | max 540 and 76 % of the column |
| Settings row | min height 44 |
| Icon button | 34 × 34 |

Spacing steps: 2, 4, 6, 8, 10, 12, 16, 18, 24. Radii: 3 tags, 6 buttons and inputs,
8 toasts, 10 bubbles and menus, 12 windows, full for avatars and round buttons. One
shadow for everything that floats; flat surfaces are separated by a line, not a
shadow. Layer order, bottom to top: drawer, settings, search, login, photo viewer,
call and notification, context menu, lock screen.

Icons: 18 × 18 grid, 1.5 px stroke, round caps and joins, no fill. Drawn at 18 px,
22 px in the rail. The full set with names is in the design-system view.

### 2.4 Themes and customisation

A theme is presets plus overrides, never a fork of the defaults:

- **Presets per mode.** Light: Дневная, Тёплая (the earlier warm-grey look). Dark:
  Ночная, Полночь (near-black for OLED), Графит. The OS or the user picks the mode;
  each mode remembers its own preset.
- **Overrides.** Any token from §2.1 can be set per mode; unset tokens fall back to
  the preset. "Все цвета темы" lists every token with a colour picker and a per-token
  reset. Changes apply live.
- **Wallpaper per mode:** from the theme, one of four gradients, a plain colour
  (`chat`), or a user image (cover-fit). Pattern on/off and its opacity 0–20 %.
- **Shape:** bubble radius 2–22 px (inner corners of a series are a third of it),
  tails on the last bubble of a series on/off, avatars round or rounded square (30 %),
  message text 12–18 px.
- **Theme file** `*.tgx-theme`, JSON:
  `{"format":"tgx-theme/1","name":…,"light":{"preset":…,"colors":{token:value}},"dark":{…},"shape":{"radius":…,"tails":…,"avaShape":…,"pattern":…}}`.
  On import, unknown tokens and non-string or over-long values are dropped and an
  unknown `format` is rejected with a message; a theme file never carries code,
  URLs or images.

In Slint: one `Theme` global holds every token as a runtime `in-out property`; the
Rust side resolves preset + overrides + derived colours and writes them on start and
on every change. Nothing in `.slint` files uses a literal colour. The pattern is one
tinted tile image (alpha mask coloured by `pattern`), the tail is a small `Path`
in the bubble colour.

## 3. Components and their states

Listed here so nothing is missed in implementation; the look of each is in the
design-system view.

- **Chat list row** (60 px, avatar 46): default, hover, selected (filled with accent),
  unread counter, muted counter, pinned, typing, draft, online dot.
- **Bubble**: incoming, outgoing; first-in-series corner; sender name and avatar in
  groups; reply quote; forwarded; link preview; reactions (mine / others); edited;
  bookmarked; delivery state — waiting for network (clock), sent (one tick), read
  (two ticks); **locally saved deleted message** (dashed outline, red caption).
- **Media**: photo with caption, album, voice (waveform, unheard dot, speed,
  transcription), round video (220 → 300 while playing), file (download / loading /
  open), poll (before / after vote), sticker, bot keyboard.
- **Separators**: date chip, service message, unread divider.
- **Composer**: empty (mic), with text (send), recording, reply bar, bot menu,
  channel footer with a single action.
- **Controls**: primary and secondary button, row button, icon button, filter chip,
  text field, underlined settings field, monospace technical field, code cells,
  switch, radio, checkbox, range, progress bar.
- **Indicators**: counter badge, state dot (ok / warn / bad / off), tag, route hop,
  keycap.
- **Floating**: context menu (with reaction row), popover, toast, message
  notification, edit-history popover, tray menu.
- **Status bar**: ready, connecting, direct with warning, forced route with no route,
  offline, plus optional ghost-mode tag and download summary.

## 4. Screens

| Screen | Milestone | Notes |
|---|---|---|
| Login: phone, code, cloud password | MVP | Route line under the form shows how the login itself is connected |
| Login by QR | MVP, optional | |
| Chat list, conversation, text and photos, reply | MVP | |
| Connection: mode, route order, own proxies | MVP | |
| Status bar | MVP | Click opens Connection; latency opens Diagnostics; downloads open Downloads |
| Offline state | MVP | Banner in the list header with countdown and retry; outgoing messages show a clock and are sent on reconnect |
| Voice, round video, files, stickers, reactions, polls | M1 | |
| Windows notifications | M1 | |
| Downloads | M1 | Active / paused / error / queued / done; parallelism and speed limit |
| Power saving, tray | M1 | Tray menu repeats the route line |
| Network diagnostics, anti-DPI | M2 | Latency readout with one-minute sparkline, per-DC table, switch log, DoH / ClientHello split / SNI |
| Ghost mode | M3 | Tag in the status bar while any option is on |
| Deleted and edited messages | M3 | In place in the chat and as a journal with a word-level diff |
| Accounts | M3 | Switcher in the drawer header |
| Passcode and lock screen | M3 | Four digits, auto-lock, Windows Hello |
| History search | M4 | Ctrl+K; type chips, chat scope, regex toggle; searches text, file names and voice transcriptions, including saved deleted messages |
| Bookmarks and contact notes | M4 | |
| Export and backup | M4 | Format, content, period, schedule |
| Theme, accent, compact mode, folders | M5 | |
| Hotkeys | M5 | Fixed set now, custom bindings later |
| Auto-reply, translate, summary | M5 | The AI options state that message text leaves the PC |
| Calls | M6 | Docked panel, always dark; collapses to a pill |
| Updates | M6 | Shows signature check before the install button |

## 5. Behaviour rules

- **Esc** closes the topmost layer only: recording, history search, context menu, viewer, settings page
  (one step back), drawer, popover, reply, info panel — in that order.
- **Hotkeys**: Ctrl+K history search, Ctrl+F search in chat, Alt+↑/↓ previous / next
  chat, Ctrl+, settings, Ctrl+J downloads, Ctrl+L lock, double click replies.
- **Motion** only answers an action. 150 ms for switches and hover, 200 ms for the
  drawer, info panel and toasts, 250 ms for notifications, 300 ms shake on an input
  error, 1.4 s highlight on a message that was jumped to. The "interface animations"
  setting and the OS reduced-motion setting both turn all of it off.
- **Errors** sit next to the field, say what is wrong and what to do, and do not
  apologise. **Empty screens** name the next action.
- **Destructive items** are red and last in a menu. Irreversible ones (clear journal,
  forget passcode) say so in the line below.
- **Privacy while locked**: notifications arrive without message text.
- **Status bar copy** is lowercase and terse; state is always given as a word next to
  the coloured dot, never by colour alone.

## 6. Accessibility

Contrast was computed with the WCAG formula from the live token values (the same
table is in the design-system view):

| Pair | Light | Dark |
|---|---|---|
| `text` on `win` / `in` / `out` | 15.6 / 16.9 / 14.1 | 13.7 / 12.4 / 8.3 |
| `muted` on `win` / `pane` / `sel` | 5.7 / 5.3 / 4.7 | 6.1 / 6.4 / 5.3 |
| `accent` on `win` / `accent-soft` | 5.4 / 4.6 | 7.1 / 5.2 |
| `accent-ink` on `accent` | 5.9 | 7.3 |
| `ok` / `warn` on `pane`, `bad` on `win` | 5.1 / 5.1 / 5.2 | 8.3 / 8.3 / 5.9 |
| `faint` on `win` (icons only) | 3.4 | 3.9 |

All text pairs reach 4.5:1 and icon pairs 3:1 for the default accent. The other four
accents were checked for `accent` on `win` and `accent-ink` on `accent` only
(5.0–10.2). Keyboard focus is a 2 px accent outline with 1 px offset on buttons; text
fields show focus by an accent border. Message text size is adjustable 12–18 px.

Not covered by the mockup and still to design in Slint: keyboard navigation inside the
chat list and message history, and screen-reader names for icon buttons (the mockup
gives them as tooltips).

## 7. Changes made to the earlier mockup

- `muted`, `faint`, `ok`, `warn` darkened in the light theme and `faint` lightened in
  the dark theme to meet the contrast targets above; timestamps moved from `faint` to
  `muted`; the muted unread counter uses `muted` as its background.
- Font sizes 12.5 and 13.5 px folded into 13 px.
- Status bar speed readout now belongs to the download summary and appears only while
  something is downloading.
- 2026-11-19, owner feedback ("looks worse than Telegram"): moved to a Telegram-like
  default — blue accent, white list, wallpaper with doodle pattern, green/blue
  outgoing bubble with its own colour, tails, larger radius, bright gradient avatars,
  pill search field, opaque date chips over the wallpaper. Added the theme system
  (§2.4). The old warm-grey palette survives as the Тёплая and Графит presets. The
  contrast table in §6 still describes the old palette and is to be recomputed from
  the design-system view.

## 8. Builder's calls (mine, not owner decisions)

- New feature screens live as pages of the settings window rather than separate
  windows, to keep one window and one navigation model.
- The call panel is always dark regardless of theme, like the photo viewer.
- The passcode is four digits.
- History search includes locally saved deleted messages and marks them.
- Deleted messages stay in their place in the chat instead of only in the journal.
- The status bar keeps Latin units (`ms`, `MB/s`) as a telemetry line; settings pages
  use Russian units.

## 9. Open questions

1. **Call media path.** The call panel reserves a line for how audio travels
   ("через сервер Telegram, 64 мс"). Whether calls work at all on a blocked network,
   and what that line can truthfully say, is unknown until M6 research.
2. **Product name and mark.** The `tgx` square mark is a placeholder.
3. **Variable-height message list and rich text in Slint** remain the spike risk from
   the client design; this design assumes they hold. If the UI toolkit changes, the
   tokens and metrics here carry over unchanged.
4. **Folder editing** (order, rules) has a settings entry but no editor screen yet.
