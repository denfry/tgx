//! Local MTProto proxy that relays TDLib connections to Telegram over WSS.
//!
//! Ported from Flowseal/tg-ws-proxy (MIT); see `NOTICE`.

pub mod framing;
pub mod obfs;

pub use framing::PacketSplitter;
pub use obfs::{
    ClientInit, CryptoCtx, HANDSHAKE_LEN, ProtoTag, generate_relay_init, parse_client_init,
};
