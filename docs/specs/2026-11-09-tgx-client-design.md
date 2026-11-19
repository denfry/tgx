# tgx — custom Telegram desktop client with built-in WS proxy

Status: draft for approval · Date: 2026-11-09 · Working name: `tgx` (final name TBD)
UI design: [`2026-11-10-tgx-ui-design.md`](2026-11-10-tgx-ui-design.md), prototype and design system: [`docs/mockups/tgx-ui.html`](../mockups/tgx-ui.html)

## 1. Intent

A personal Telegram desktop client for Windows that **just works where Telegram is
throttled or blocked**, without a separate proxy app, and that is **fast to build,
fast to develop and light at runtime**. It replaces day-to-day use of Telegram
Desktop for the owner and becomes the base for custom features (privacy, local
history, automation) that official clients do not offer.

Why not keep forking tdesktop: the owner finds every part of it slow — multi-hour
dependency builds, long rebuilds, a huge codebase that is slow to change, and poor
connectivity under blocking. The answer is not "a faster language" (tdesktop is
already C++/Qt) but a **smaller architecture**: TDLib owns the protocol, storage and
sync; this project owns only UI, networking transport and custom features.

## 2. Goals (MVP)

1. Windows 10/11 x64, single `tgx.exe` + `tdjson.dll` (plus Slint runtime deps if any).
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

Order (owner decision): **WS → user-provided MTProxy/SOCKS5 → (later) own CDN relay
→ direct only if explicitly enabled in settings** (default off). Rationale: the network
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
  via env vars / a git-ignored config, never committed.
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
switch log); **own WS relay on a CDN** (Cloudflare Worker / own VPS) as the next link
in the chain if Telegram's WS endpoints get blocked; **DoH and anti-DPI** (DNS over
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

**M6 — heavy:** **calls** (voice/video via tgcalls — largest item), **auto-update**
(GitHub Releases with signature verification, needed once distributed).

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
  tg-ws-proxy (MIT, attribution).

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
