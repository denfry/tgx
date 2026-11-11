# tgx spike report (2026-11-11)

Spike plan: `docs/superpowers/plans/2026-11-11-spike.md`. Spec: `docs/specs/2026-11-09-tgx-client-design.md` §8.

Status: the transport crate, both spike binaries and the automated measurements are done.
**Not done yet: the owner's manual runs** (login over `ws` / `mtproxy` / `direct` on the
blocked network, the download timings, and the visual no-jump check). The fields marked
_owner_ below need filling in.

## Environment

| | |
|---|---|
| OS | Windows 10 Pro 22H2 (build 19045) |
| CPU / GPU | AMD Ryzen 5 5600G, Radeon integrated graphics |
| RAM | 15.4 GB |
| Toolchain | Rust 1.93.1, stable-x86_64-pc-windows-msvc |
| TDLib | 1.8.61 (tdlib-rs 1.4.0, `download-tdlib`) |
| Slint | 1.18.1 (default backend: winit + femtovg) |
| Network for automated runs | the development machine's network; **not confirmed to be the blocked network** |
| Network for `run` measurements | _owner_ |

## 1. WS transport

### Relay (crate `tgx-transport`)

- 31 automated tests, all passing: handshake vectors (one generated with the upstream
  `_try_handshake`), packet splitting, and loopback end-to-end tests (fragmented init,
  junk/TLS rejection without dialling, unreachable DC, upstream close, 8 parallel clients
  including media DCs).
- Bug found and fixed while doing this: the plan said to reject any init whose first byte
  is `0x16` (a TLS record). TDLib's random init only avoids the `16 03 01 02` prefix, so
  that rule would have dropped about 1 in 256 real connections. Rejection now relies only
  on decoding the handshake. Regression test: `init_starting_with_0x16_is_accepted`.
- rustls is pinned to the `ring` provider; without it the first WSS dial panics at runtime.

### Probe (`relay-tdlib probe`)

Full TLS validation (webpki roots), socket pinned to the IP, SNI/Host = kws domain.
Result on the development network: **30/30 ok**.

| dc | media | ip | domain | result | ms |
|---|---|---|---|---|---|
| 1 | false | 149.154.167.220 | kws1.web.telegram.org | ok | 763 |
| 1 | false | 149.154.175.50 | kws1.web.telegram.org | ok | 765 |
| 1 | true | 149.154.167.220 | kws1-1.web.telegram.org | ok | 756 |
| 1 | true | 149.154.167.220 | kws1.web.telegram.org | ok | 895 |
| 1 | true | 149.154.175.50 | kws1-1.web.telegram.org | ok | 740 |
| 1 | true | 149.154.175.50 | kws1.web.telegram.org | ok | 894 |
| 2 | false | 149.154.167.51 | kws2.web.telegram.org | ok | 443 |
| 2 | false | 149.154.167.220 | kws2.web.telegram.org | ok | 443 |
| 2 | true | 149.154.167.51 | kws2-1.web.telegram.org | ok | 464 |
| 2 | true | 149.154.167.51 | kws2.web.telegram.org | ok | 442 |
| 2 | true | 149.154.167.220 | kws2-1.web.telegram.org | ok | 434 |
| 2 | true | 149.154.167.220 | kws2.web.telegram.org | ok | 449 |
| 3 | false | 149.154.167.220 | kws3.web.telegram.org | ok | 932 |
| 3 | false | 149.154.175.100 | kws3.web.telegram.org | ok | 901 |
| 3 | true | 149.154.167.220 | kws3-1.web.telegram.org | ok | 897 |
| 3 | true | 149.154.167.220 | kws3.web.telegram.org | ok | 940 |
| 3 | true | 149.154.175.100 | kws3-1.web.telegram.org | ok | 932 |
| 3 | true | 149.154.175.100 | kws3.web.telegram.org | ok | 939 |
| 4 | false | 149.154.167.91 | kws4.web.telegram.org | ok | 525 |
| 4 | false | 149.154.167.220 | kws4.web.telegram.org | ok | 519 |
| 4 | true | 149.154.167.91 | kws4-1.web.telegram.org | ok | 465 |
| 4 | true | 149.154.167.91 | kws4.web.telegram.org | ok | 433 |
| 4 | true | 149.154.167.220 | kws4-1.web.telegram.org | ok | 523 |
| 4 | true | 149.154.167.220 | kws4.web.telegram.org | ok | 522 |
| 5 | false | 149.154.167.220 | kws5.web.telegram.org | ok | 989 |
| 5 | false | 149.154.171.5 | kws5.web.telegram.org | ok | 998 |
| 5 | true | 149.154.167.220 | kws5-1.web.telegram.org | ok | 968 |
| 5 | true | 149.154.167.220 | kws5.web.telegram.org | ok | 975 |
| 5 | true | 149.154.171.5 | kws5-1.web.telegram.org | ok | 1108 |
| 5 | true | 149.154.171.5 | kws5.web.telegram.org | ok | 998 |

30/30 combinations ok

Every DC (1–5), media and non-media, answered both on the shared web IP `149.154.167.220`
and on its own DC IP. The relay's default table only routes DC 2 and 4, so DC 1/3/5 need
`--ws-ip` overrides (or a larger default table; see decisions).

### TDLib through the relay (`relay-tdlib run`)

Verified without an account (dummy api_id, scratch data dir):
- TDLib's connections arrive at the relay and decode to the right DC (`connected DC1`).
- A fresh database starts on **DC1**. With the default table (DC 2/4 only) the relay reports
  `DC1 UNREACHABLE` and closes. It never falls back to a direct connection.
- With `--ws-ip 1=149.154.175.50`, TDLib's `pingProxy` (a real request/response with
  Telegram) succeeded through the relay in **144 ms**: MTProto traffic flows end to end
  over WSS. (`WaitPhoneNumber` is reached locally before any network traffic, so it is
  not evidence on its own.)
- TDLib queues proxy requests until `setTdlibParameters`. Awaiting `addProxy` first
  deadlocks, so `addProxy` is sent immediately before the parameters, and TDLib applies
  it first on init.

Owner runs (spec §7, §8.1):

| route | connect → Ready (ms) | first chat batch (ms) | `photo` (ms) | `big` 50 MB (MB/s) | notes |
|---|---|---|---|---|---|
| ws | _owner_ | _owner_ | _owner_ | _owner_ | account DC: _owner_ |
| mtproxy | _owner_ | _owner_ | _owner_ | _owner_ | |
| direct | _owner_ | _owner_ | _owner_ | _owner_ | expected to fail on the blocked network |

How to run:

```
set TGX_API_ID=...   & set TGX_API_HASH=...
cargo run -p relay-tdlib -- run --route ws --ws-ip 1=149.154.175.50 --ws-ip 3=149.154.175.100 --ws-ip 5=149.154.171.5
cargo run -p relay-tdlib -- run --route mtproxy=tg://proxy?server=...&port=...&secret=...
cargo run -p relay-tdlib -- run --route direct
```

Then type `photo`, `big`, `quit`. Saved Messages must contain a photo and a document of about 50 MB.

## 2. Slint message list (`slint-list --release`)

10,000 messages, variable height (1–600 chars, Cyrillic + Latin + emoji, 10 % with a
240×160 photo placeholder), 10 s of auto-scroll at 40 px per 16 ms tick.

| metric | result | pass bar |
|---|---|---|
| frame interval p50 / p95 / max | 16.6 / 17.9 / 25.9 ms | p95 ≤ 20, max ≤ 50 → **pass** |
| private bytes at end | 100.3 MB | < 150 MB → **pass** |
| window creation (`ListWindow::new`) | 4.4–10.8 s (6 runs; 5 of them about 4.4 s) | none set; **problem for cold start** |
| first frame after launch | 4.5–11.3 s | n/a |
| prepend 50 messages with a stable anchor | not reliably possible with ListView (below) | no visible jump (manual): _owner_ |

Findings:
- **Scrolling is fine.** Frames keep up with the 16 ms driver tick across 10k rows.
- **Startup is slow and does not depend on list size.** Window creation took the same
  ~4.4 s for 1,000 and 10,000 rows, and the same with the software renderer, so it is
  Slint/backend initialisation, not the list. Likely cause, not verified: system font
  enumeration. This machine has 827 fonts (432 MB) in `C:\Windows\Fonts`. This directly
  affects the cold-start target (spec §8.1).
- **Prepending with a stable anchor isn't supported by the API.** ListView's
  `content-height` is an estimate that moved by −424k / +312k / +579k px across single
  50-row inserts, so it can't be used to keep the anchor. The spike shifts by
  50 × average row height instead, which is approximate. Exact anchoring needs the app to
  own row measurement or virtualisation.

## 3. Decisions

1. **WS route for MVP:** _owner, after the `ws` run_. From the automated evidence: viable as a
   design, but the default DC→IP table must cover DC 1–5 (all 30 probe combinations
   worked; `149.154.167.220` served every DC). If the owner's network blocks some DCs,
   the route manager switches route as spec §5.4 says.
2. **Slint:** _owner decision (spec §8)_. Scrolling and RAM pass. The two open problems are the
   4–11 s startup (needs profiling; it must be solved or ruled out before MVP UI work) and
   anchored history loading (needs custom row measurement, or a list that grows at the end
   rather than the start).
3. **Changes to the MVP plan:**
   - Default `WsRoutes`: add DC 1, 3, 5 (`149.154.167.220`, or the per-DC IPs from the probe).
   - Keep the `0x16` fix and its regression test; the relay must accept any decodable init.
   - Start TDLib with the proxy request sent just before `setTdlibParameters`.
   - Add a task to profile Slint startup (font loading) before committing to Slint for the MVP UI.
   - History loading UI: design the message list around a model that grows at the end,
     or measure row heights in Rust.
