# tgx

A lightweight Telegram client for Windows with a built-in WebSocket transport, so it
connects where Telegram is throttled or blocked without a separate proxy app.

Status: early development. The transport and the two technical spikes are done; the
client itself is not usable yet.

- Design: [`docs/specs/2026-11-09-tgx-client-design.md`](docs/specs/2026-11-09-tgx-client-design.md)
- UI: [`docs/specs/2026-11-10-tgx-ui-design.md`](docs/specs/2026-11-10-tgx-ui-design.md),
  clickable prototype in [`docs/mockups/tgx-ui.html`](docs/mockups/tgx-ui.html)

## Building

Rust stable on Windows x64. TDLib is downloaded prebuilt by `tdlib-rs`.

```
cargo test -p tgx-transport
```

Running anything that talks to Telegram needs your own `api_id`/`api_hash` from
<https://my.telegram.org>, passed as `TGX_API_ID` / `TGX_API_HASH`.

## Licence

GPL-3.0-only, see [`LICENSE`](LICENSE). The transport is a port of
[Flowseal/tg-ws-proxy](https://github.com/Flowseal/tg-ws-proxy) (MIT); see
[`crates/tgx-transport/NOTICE`](crates/tgx-transport/NOTICE).

tgx is an unofficial client and is not affiliated with Telegram.
