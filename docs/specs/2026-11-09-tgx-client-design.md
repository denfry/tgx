# tgx — custom Telegram desktop client with built-in WS proxy

Status: draft for approval · Date: 2026-11-09 · Working name: `tgx` (final name TBD)
UI design: [`2026-11-10-tgx-ui-design.md`](2026-11-10-tgx-ui-design.md), prototype and design system: [`docs/mockups/tgx-ui.html`](../mockups/tgx-ui.html)

## 1. Intent

A Telegram desktop client for Windows that **just works where Telegram is
throttled or blocked**, without a separate proxy app, and that is **fast to build,
fast to develop and light at runtime**. It replaces day-to-day use of Telegram
Desktop for the owner and becomes the base for custom features (privacy, local
history, automation) that official clients do not offer.

**Audience and distribution (owner decision, 2026-11-19):** the owner plus family and
friends. tgx is handed over privately as a zip, never published. It is **purely
local**: no servers of our own, no accounts of our own, no telemetry or crash
uploads. Everything the app knows stays on the user's PC; the only network traffic
is Telegram's (through the chosen route). Users are not technical, so the defaults
must work on a blocked network with zero setup.

Why not keep forking tdesktop: the owner finds every part of it slow — multi-hour
dependency builds, long rebuilds, a huge codebase that is slow to change, and poor
connectivity under blocking. The answer is not "a faster language" (tdesktop is
already C++/Qt) but a **smaller architecture**: TDLib owns the protocol, storage and
sync; this project owns only UI, networking transport and custom features.

## 2. Goals (MVP)

1. Windows 10/11 x64, a portable zip: `tgx.exe` + `tdjson.dll` + `LICENSES/`. Runs
   from any folder without installation or admin rights.
2. Login: phone → code → 2FA password; QR login is a nice-to-have.
3. Chat list (virtualized, unread counters, last message preview, avatars).
4. Chat view (virtualized history, infinite scroll up), send/receive **text and photos**,
   view received photos, reply rendering (read-only quote is enough).
5. **Built-in WS transport** (see §5) on by default, with an **automatic connection
   chain** and a visible status indicator of which route is active.
6. Settings: route mode (auto / force direct / force WS / custom proxy), custom
   MTProxy/SOCKS5 entry, log-out.

Success for MVP: the owner can log in and chat (text + photos) on a network where
plain Telegram Desktop does not connect, with cold start < 1.5 s to a usable chat
list (cached session) and idle RAM noticeably below Telegram Desktop on the same
account. Clean release build of the app crate (TDLib prebuilt) in minutes, incremental
rebuild in seconds.

## 3. Non-goals for MVP

Voice/video messages playback, stickers/animated emoji, files other than photos,
stories, reactions UI, channels admin, bots/inline keyboards beyond plain text,
calls, notifications beyond a basic Windows toast (toast is optional), multi-account,
macOS/Linux/mobile (Android is planned after MVP, see §10), installer, auto-update,
public distribution.

## 4. Anti-goals (failure even if it "works")

- Feels slower or heavier than Telegram Desktop.
- Silently falls back to a direct connection that leaks to the network observer when
  the user forced a proxy route.
- Requires the user to run or configure any external program.
- Weakens Telegram's end-to-end transport security: the proxy only relays already
  encrypted MTProto bytes; no MITM, no key handling beyond MTProto obfuscation.
- Stores secrets (`api_hash`, session, proxy secrets) in the repo or in plain logs.

## 5. Architecture

### 5.1 Stack (decided)

- **Language:** Rust (stable).
- **Telegram core:** TDLib via the `tdlib-rs` crate (1.4.x ships prebuilt TDLib for
  Windows x86_64/arm64). Fallback if it lags behind TDLib versions: own thin wrapper
  over `tdjson` with a self-built TDLib.
- **UI:** Slint (desktop royalty-free licence is fine for personal use; revisit if
  distributed closed-source).
- **Async/network:** `tokio`, `tokio-tungstenite` (or `fastwebsockets`) with
  `rustls`; system/webpki roots, full certificate validation.

### 5.2 Process layout

One process, three subsystems:

```
 Slint UI thread  <--view-models/commands-->  App core (tokio)
                                                 |  TDLib JSON client (own thread, td_receive loop)
                                                 |  Transport subsystem (tokio tasks)
TDLib --MTProto proxy--> 127.0.0.1:<random port> --> transport --> internet
```

- **App core** owns state (chat list, open chat window of messages), translates TDLib
  updates into compact view-models, and pushes diffs to the UI. TDLib is the source of
  truth; the app keeps only what is on screen plus small caches.
- **UI** never blocks on TDLib; all calls are async with request ids.

### 5.3 Built-in WS transport (core feature)

Modelled on Flowseal/tg-ws-proxy (MIT, Python) — port the logic, keep attribution.

- The transport listens on `127.0.0.1` on a random free port as a **local MTProto
  proxy** with a random per-install secret. TDLib is configured with
  `addProxy(type = proxyTypeMtproto)` pointing at it. Local MTProto (not SOCKS5) is
  chosen because the DC id and media flag are carried in the obfuscated init packet:
  decrypt the 64-byte init with AES-256-CTR (key = SHA-256(prekey ‖ secret)), read the
  protocol tag (abridged / intermediate / padded-intermediate) and the signed
  little-endian DC index (`dc = |idx|`, `media = idx < 0`, test DC if ≥ 10000).
- For each client connection it opens (or takes from a warm pool) a **WSS connection
  to Telegram's web endpoints** for that DC, sends a fresh obfuscated init for the
  same DC/media flag, and relays bytes with re-encryption both ways.
- Exact hostnames/paths (`kws{N}[-1].web.telegram.org/apiws` family) and the
  DC→IP pinning table must be taken from tg-ws-proxy `proxy/pool.py`/`config.py`
  during implementation, not guessed.
- Listener binds to loopback only; rejects bad handshakes (drain with limits, like
  upstream) so it cannot be abused as an open relay.

### 5.4 Connection chain (auto mode)

Order (owner decision): **WS → user-provided MTProxy/SOCKS5 → (later) user-provided
WS relay → direct only if explicitly enabled in settings** (default off). Rationale: the network
observer should not see direct connections to Telegram IPs in auto mode either.
The core probes routes (TDLib `pingProxy` / `testNetwork` + connection-state updates),
switches on sustained failure, remembers the last good route per network, and
re-probes in the background. Forced modes never fall back. The status bar shows the
active route and state (connecting / updating / ready).

*Builder call:* routes are a simple ordered list in MVP; the WSS dialer does **not**
tunnel through a user proxy (no route combinations). Per-DC failures (e.g. a media DC
unreachable over WS) are handled **inside the relay**: retry with backoff, and if a DC
stays unreachable, report it to the route manager, which switches the whole route —
the relay never silently sends a single DC direct.

### 5.5 Own message store (from day one)

Owner decision: the MVP already captures history into an app-owned **SQLite** store
(e.g. `rusqlite`, WAL mode) so that future features — saved deleted/edited messages,
full-text search, export — have data from the first day of use. The capture layer
records every received message plus `updateMessageContent` / `updateMessageEdited` /
`updateDeleteMessages` as append-only events (message id, chat id, date, sender, text,
media metadata — not media files). No UI uses it in MVP beyond being written and
schema-versioned with migrations. The UI still reads from TDLib; TDLib stays the
source of truth for display.

### 5.6 Data & secrets

- TDLib database and files in `%LOCALAPPDATA%\tgx\` (portable mode is a later option).
- `api_id`/`api_hash`: owner's own pair from my.telegram.org, supplied at build time
  via env vars / a git-ignored config, never committed. It is compiled into every
  copy given to family and friends, so anyone with the exe can extract it (as with
  every Telegram client). Consequence: if Telegram restricts this `api_id`, every
  copy stops logging in at once — one more reason privacy features are off by
  default (§11).
- TDLib `database_encryption_key`: empty in MVP; replaced by passcode-derived key in
  the "client password" roadmap item.
- Logs: TDLib verbosity low by default; app logs never contain message text, phone
  numbers or secrets.

## 6. Roadmap (after MVP, owner-selected, rough order)

**M1 — daily driver:** files/documents, voice messages playback, stickers (static →
animated), basic notifications (Windows toasts), reactions display, **download
manager** (queue, parallel large downloads, per-chat auto-download rules and folders),
**resource saver** (tray mode with minimal RAM, cache size limits, animations off).

**M2 — network:** **network diagnostics** panel (per-DC latency, active route, speed,
switch log); **user-provided WS relay** (a Cloudflare Worker or VPS address the user
enters, like a custom proxy) as the next link in the chain if Telegram's WS endpoints
get blocked — tgx ships no relay of its own; **DoH and anti-DPI** (DNS over
HTTPS for WS hosts, TLS ClientHello fragmentation, configurable SNI).

**M3 — privacy & accounts:** **ghost mode** (don't send read receipts / online /
typing), **saved deleted and edited messages** (local store of deleted messages and
edit history), **multi-account** (TDLib client per account), **client password**
(passcode + TDLib DB encryption key derived from it, optional Windows Hello).

**M4 — productivity:** **local full-text search** across synced history (tantivy,
regex), **bookmarks and notes** (local bookmarks on messages, notes on contacts,
reminders), **export/backup** (HTML/JSON/Markdown, scheduled).

**M5 — UI & automation:** themes, compact mode, chat folders/filters, custom hotkeys;
scripting/plugins, auto-replies, AI features (translate, summarize).

**M6 — heavy:** **calls** (voice/video via tgcalls — largest item), **updates**:
there is no update server; the owner sends a new zip. Open question for later: an
opt-in "new version" check against a place the owner controls (e.g. a private
channel), with signature verification, since copies now live on other people's PCs.

**For family and friends (cross-cutting, from M1):** "Скопировать диагностику" in
settings (versions, active route, recent connection errors; never message text,
phone numbers or secrets) so the owner can help remotely; a short Russian
"как запустить" note in the zip, including the Windows SmartScreen warning for an
unsigned exe; client password moves up in priority because family PCs are shared.

Parked (not selected, may come later): proxy sharing to LAN, portable mode, panic
button, keyword alerts/anti-spam.

## 7. Constraints & risks

- **Telegram API terms:** own `api_id`; no "Telegram" name/logo in branding if ever
  distributed. Ghost mode and saving deleted messages are used by known third-party
  clients but can draw `api_id` restrictions — acceptable for personal use, re-assess
  before public release.
- **WS endpoint availability/throughput:** Telegram's web WS endpoints may be blocked
  or throttled, and media DC throughput through WS is unproven → measure in the first
  spike (photo + 50 MB file download time via WS vs direct vs MTProxy).
- **tdlib-rs freshness** vs TDLib API changes; pin versions.
- **Slint maturity** for chat UIs (rich text, selection, large virtual lists with
  variable-height items, image decoding off the UI thread) → validate in the spike.
- **Licences:** TDLib (BSL-1.0), Slint (GPLv3 / royalty-free / commercial),
  tg-ws-proxy (MIT, attribution). Handing copies to friends is distribution: the zip
  carries `LICENSES/` (TDLib, tg-ws-proxy NOTICE, Slint), and the Slint royalty-free
  licence needs its attribution in the app (an About screen); check the exact
  wording of the licence version in use before the first copy goes out.
- **Branding:** copies leave the owner's PC, so the "no Telegram name or logo" rule
  above applies from the first copy.

## 8. First step (before full plan execution)

A throwaway **spike** proving the two riskiest parts: (1) Rust local MTProto→WSS relay
working with TDLib on a blocked network, including a media download; (2) Slint
rendering a 10k-item variable-height message list smoothly. Results feed back into
the plan.

- **Test network:** the owner has a real home/mobile network where Telegram Desktop is
  blocked/throttled — spike and acceptance run there; no block simulation needed for
  manual checks. Unit/integration tests of the relay and route chain stay
  network-independent (loopback fakes).
- **If Slint fails the spike:** the replacement UI is decided after the spike (owner
  decision) — the plan must keep the UI behind view-models so the swap is contained.
- **If WS media throughput is poor:** MVP still ships; media goes through the next
  route in the chain (user MTProxy) — *builder call*.

## 8.1 Measuring the performance targets (builder call)

On the owner's PC with the owner's real account: cold start = process launch to
interactive chat list with an existing TDLib session (median of 5); idle RAM = private
working set after 2 minutes idle with chat list open, compared to Telegram Desktop on
the same account and machine. Recorded in the spike/MVP report, not CI-gated.

## 9. Left to the builder

Crate/module layout, choice between `tokio-tungstenite` and alternatives, view-model
design, image decoding/caching approach, testing strategy details (relay handshake
parsing must have unit tests with known vectors; route chain logic must be testable
without network), CI setup.

## 10. Android (after MVP)

Not part of MVP. Recorded so the desktop work does not close the door on it.

- **Same transport, in process.** The WS relay runs inside the app, as on Windows:
  TDLib connects to `127.0.0.1:<random port>` with `proxyTypeMtproto`. Unlike
  standalone Android proxies such as valnesfjord/tg-ws-proxy-rs (a separate app with a
  foreground service and a `tg://proxy` link for the official client), there is no
  second app, no proxy to add by hand, and no service of its own for the system to
  kill: the relay lives and dies with the client process.
- **Kept portable now.** `tgx-transport` has no platform code (tokio, rustls with
  `ring`, tokio-tungstenite). `scripts/check-android.sh` type-checks it for
  `aarch64-linux-android`, `armv7-linux-androideabi` and `x86_64-linux-android` with
  the NDK clang (needed by `ring`); CI runs it. New platform-neutral crates (TDLib
  session, route manager, message store) are added to that script, and Windows-only
  code stays in the app crate.
- **Open work, in order of cost:**
  1. TDLib for Android. `tdlib-rs` prebuilt TDLib covers desktop only, so TDLib is
     built with the NDK per ABI (not verified yet).
  2. A phone UI: stack navigation, gestures, soft keyboard, safe areas. The tokens in
     the UI design carry over; the desktop layout does not. Slint's Android backend is
     the first candidate, to be checked against the same list benchmark as the spike.
  3. Lifecycle: Doze and process death, and network changes (Wi-Fi ↔ mobile) that
     must re-dial WSS quickly; the route manager re-probes on a network change.
  4. Notifications without a push server of our own.
- Minimum Android 8.0 (API 26), the NDK API level the check uses.

## 11. Features taken from AyuGram

AyuGram (AyuGram/AyuGramDesktop, a tdesktop fork) is the reference for what a power
user expects beyond the official client. It is **GPL-3.0**: we take ideas and
behaviour, never code, so tgx keeps its own licence choice. Feature list taken from
its settings (`Telegram/SourceFiles/ayu/ayu_settings.h`) and feature modules, as of
2026-11-19. Our edge: TDLib does nothing on its own that these features need to
suppress, so most of them are "don't make the call" rather than patches in a
protocol layer.

| Feature | How in tgx (TDLib) | Milestone |
|---|---|---|
| Ghost mode: no read receipts | don't call `viewMessages` / `openChat` read marking; optional "mark read after I reply" | M3 |
| Ghost mode: stay offline | option `online = false`; optional offline after each forced online | M3 |
| Ghost mode: no typing / upload progress | never send `sendChatAction`; verify TDLib sends no upload action itself (not checked) | M3 |
| Ghost mode: don't mark stories seen | don't call `openStory` (stories are M5 at best) | M3 |
| Ghost mode per account, quick toggles in drawer and tray, "locked" options | settings model | M3 |
| Send as scheduled to avoid going online | `messageSchedulingStateSendAtDate` now + a few seconds | M3 |
| Saved deleted messages (anti-recall) | already captured by the §5.5 store from day one; shown in place with a mark | M3 |
| Edit history | `updateMessageContent` / `updateMessageEdited` events from the store | M3 |
| Custom deleted / edited marks (🧹 by default), semi-transparent deleted | UI setting | M3 |
| Message filters: regex hide, per chat, from the context menu | local filter over view-models, never deletes | M4 |
| Local "shadow ban": hide a user's messages everywhere | same filter engine, by sender id | M4 |
| Hide messages from blocked users in groups | `getBlockedMessageSenders` + filter | M4 |
| Zalgo filter | text normalisation in the view-model | M4 |
| Streamer mode: window excluded from screen capture, sensitive data hidden | `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)` on the HWND | M3 |
| Translator | TDLib `translateText` (provider choice later) | M5 |
| Message shot: render selected messages to an image | render the bubble components offscreen | M4 |
| Message details (raw JSON), show peer id (Bot API form), seconds in time | developer toggles in settings | M1 |
| "Messages of this user" in a chat | `searchChatMessages` with `sender_id` | M1 |
| Repeat message (+1) | forward/copy of the message to the same chat | M1 |
| Send confirmations for stickers, GIFs, voice, round videos | composer setting | M1 |
| No sponsored messages, hide stories, hide similar channels | simply never request or render them | MVP (free) |
| Show only added emoji/stickers, unlimited recent stickers | sticker panel settings | M1 |
| Customisable composer buttons, drawer items, context-menu items (visible / with Shift / hidden) | layout settings in the theme/UI model | M5 |
| Appearance: bubble radius, no tails, avatar corners, fonts incl. mono, spoilers revealed, colourful replies on/off, hide premium statuses, hide counters | partly in the theme system (UI spec §2.4); the rest as display settings | M5 |
| Disable link-open warning, no notification delay | settings | M1 |

Not taken:
- **Local Telegram Premium.** Server-enforced limits stay; it only fakes UI and invites
  `api_id` trouble.
- **Webview spoofing as Android, webview size hacks.** Mini apps are out of scope.

Risk, same as §7: ghost mode and anti-recall are used by known clients but can draw
`api_id` restrictions, and all copies share the owner's `api_id` (§5.6). So every
privacy feature is **off by default** and each user turns it on for themselves; tgx
is never published. Ghost state is always visible in the status bar (UI design §4).
