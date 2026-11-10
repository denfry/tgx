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
3. **Quiet and cheap to draw.** One accent colour, warm greys, no gradients, no blur,
   no decorative motion, system fonts only. Nothing in the design needs an effect that
   is expensive in Slint.
4. **Local things say they are local.** Saved deleted messages, edit history, notes
   and bookmarks exist only on this PC, and each of them states that in place.

## 2. Tokens

### 2.1 Colour

| Token | Light | Dark | Use |
|---|---|---|---|
| `win` | `#f7f6f3` | `#1a1b1a` | Main surface: headers, settings window, menus |
| `pane` | `#efede8` | `#161716` | Chat list, status bar, gaps between settings groups |
| `rail` | `#e5e2dc` | `#131413` | Folder rail |
| `chat` | `#f2f0ec` | `#121312` | Conversation background |
| `in` | `#ffffff` | `#222422` | Incoming bubble, text inputs |
| `out` | `#e4eceb` | `#33433e` | Outgoing bubble = accent over `in` (13 % light, 22 % dark) |
| `sel` | `#e2e0da` | `#262826` | Hover and selected row |
| `line` | `#d9d6cf` | `#2b2d2b` | Borders and dividers, always 1 px |
| `text` | `#1d1d1b` | `#e6e5e0` | Primary text |
| `muted` | `#62615c` | `#9b9a94` | Secondary text, timestamps, captions |
| `faint` | `#88867e` | `#787771` | State icons, placeholders, disabled. Never text |
| `accent` | `#2f6f62` | `#6fb3a3` | Actions, selected chat, links, own ticks |
| `accent-ink` | `#ffffff` | `#0e1a17` | Text and icons on accent |
| `accent-soft` | `#dfe6e2` | `#293633` | Quotes, link previews, reactions = accent over `win` (12 % / 18 %) |
| `ok` | `#27704a` | `#6cc28b` | Connected |
| `warn` | `#8a5a0c` | `#e0a64a` | Connecting, direct-route warning |
| `bad` | `#b5402f` | `#e07a68` | Error, delete, no route |

`out` and `accent-soft` are derived, so they change with the accent. Slint has no
`color-mix`; compute both when the theme or accent changes (linear sRGB mix with the
percentages above) and expose them as plain colours.

User-selectable accents (light / dark): teal `#2f6f62` / `#6fb3a3` (default), blue
`#2d5f9a` / `#7aa7da`, rust `#a4532f` / `#e0916b`, plum `#7a4f7d` / `#c49ac6`, graphite
`#3d4247` / `#aab2b9`. Avatars without a photo take one of nine muted tones by a hash
of the name, with white initials.

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
