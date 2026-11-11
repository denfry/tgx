//! Local MTProto proxy that relays TDLib connections to Telegram over WSS.
//!
//! Ported from Flowseal/tg-ws-proxy (MIT); see `NOTICE`.

pub mod dial;
pub mod endpoints;
pub mod framing;
pub mod obfs;

pub use dial::{DialError, Dialer, WsConn, WssDialer};
pub use endpoints::{DcTarget, WsRoutes, ws_domains, ws_path};
pub use framing::PacketSplitter;
pub use obfs::{
    ClientInit, CryptoCtx, HANDSHAKE_LEN, ProtoTag, generate_relay_init, parse_client_init,
};
