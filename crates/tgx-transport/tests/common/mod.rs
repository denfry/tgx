//! Loopback fixtures: a TDLib-like client, a fake Telegram WS endpoint and a
//! dialer that connects to it over plain TCP.

#![allow(dead_code)]

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use aes::Aes256;
use ctr::cipher::{KeyIvInit, StreamCipher};
use futures_util::{SinkExt, StreamExt};
use rand::Rng;
use sha2::{Digest, Sha256};
use tgx_transport::{DcTarget, DialError, Dialer, ProtoTag, WsConn};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::protocol::Message;
use tokio_tungstenite::{MaybeTlsStream, accept_hdr_async, client_async};

type Ctr = ctr::Ctr128BE<Aes256>;

fn ctr(key: &[u8], iv: &[u8]) -> Ctr {
    Ctr::new_from_slices(key, iv).unwrap()
}

fn sha_key(prekey: &[u8], secret: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(prekey);
    h.update(secret);
    h.finalize().into()
}

fn reserved(b: &[u8; 64]) -> bool {
    const STARTS: [[u8; 4]; 7] = [
        *b"HEAD",
        *b"POST",
        *b"GET ",
        [0xee; 4],
        [0xdd; 4],
        [0x16, 0x03, 0x01, 0x02],
        *b"OPTI",
    ];
    b[0] == 0xef || STARTS.iter().any(|s| b[..4] == *s) || b[4..8] == [0; 4]
}

fn reversed48(b: &[u8]) -> [u8; 48] {
    let mut r = [0u8; 48];
    r.copy_from_slice(b);
    r.reverse();
    r
}

/// Client side of the obfuscated handshake, as TDLib does it for a `dd` secret.
pub struct FakeClient {
    pub init: [u8; 64],
    enc: Ctr,
    dec: Ctr,
}

impl FakeClient {
    pub fn new(secret: &[u8; 16], proto: ProtoTag, dc_idx: i16) -> Self {
        let mut init = [0u8; 64];
        loop {
            rand::rng().fill_bytes(&mut init);
            if !reserved(&init) {
                break;
            }
        }
        let mut enc = ctr(&sha_key(&init[8..40], secret), &init[40..56]);
        let mut full = init;
        enc.apply_keystream(&mut full);
        let mut tail = [0u8; 8];
        tail[..4].copy_from_slice(&proto.bytes());
        tail[4..6].copy_from_slice(&dc_idx.to_le_bytes());
        for i in 0..8 {
            init[56 + i] ^= tail[i] ^ full[56 + i];
        }
        let rev = reversed48(&init[8..56]);
        let dec = ctr(&sha_key(&rev[..32], secret), &rev[32..48]);
        FakeClient { init, enc, dec }
    }

    pub fn encrypt(&mut self, plain: &[u8]) -> Vec<u8> {
        let mut b = plain.to_vec();
        self.enc.apply_keystream(&mut b);
        b
    }

    pub async fn read_plain(&mut self, s: &mut TcpStream, len: usize) -> Vec<u8> {
        let mut b = vec![0u8; len];
        s.read_exact(&mut b).await.unwrap();
        self.dec.apply_keystream(&mut b);
        b
    }
}

/// Intermediate-transport packet with an `n`-byte payload tagged by `tag`.
pub fn intermediate_packet(n: usize, tag: u8) -> Vec<u8> {
    let mut p = (n as u32).to_le_bytes().to_vec();
    p.extend((0..n).map(|i| tag.wrapping_add(i as u8)));
    p
}

/// What the fake Telegram endpoint saw on one WS connection.
#[derive(Debug)]
pub struct Seen {
    pub proto: [u8; 4],
    pub dc_idx: i16,
    pub frame: Vec<u8>,
}

/// Fake `kws*.web.telegram.org/apiws`: standard obfuscation, replies to
/// every frame with `pong:` + the decrypted frame.
pub struct FakeTelegram {
    pub addr: SocketAddr,
    pub seen: mpsc::UnboundedReceiver<Seen>,
}

impl FakeTelegram {
    pub async fn start(close_after_first_reply: bool) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, seen) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            loop {
                let (tcp, _) = listener.accept().await.unwrap();
                tokio::spawn(serve(tcp, tx.clone(), close_after_first_reply));
            }
        });
        FakeTelegram { addr, seen }
    }
}

async fn serve(tcp: TcpStream, tx: mpsc::UnboundedSender<Seen>, close_after_first_reply: bool) {
    let mut ws = accept_hdr_async(tcp, |_req: &Request, mut resp: Response| {
        resp.headers_mut()
            .insert("Sec-WebSocket-Protocol", HeaderValue::from_static("binary"));
        Ok(resp)
    })
    .await
    .unwrap();
    let init: Vec<u8> = match ws.next().await {
        Some(Ok(Message::Binary(b))) => b.to_vec(),
        other => panic!("expected relay init frame, got {other:?}"),
    };
    assert_eq!(init.len(), 64, "relay init must be one 64-byte frame");
    let mut dec = ctr(&init[8..40], &init[40..56]);
    let mut plain_init = init.clone();
    dec.apply_keystream(&mut plain_init);
    let proto: [u8; 4] = plain_init[56..60].try_into().unwrap();
    let dc_idx = i16::from_le_bytes([plain_init[60], plain_init[61]]);
    let rev = reversed48(&init[8..56]);
    let mut enc = ctr(&rev[..32], &rev[32..48]);

    while let Some(Ok(msg)) = ws.next().await {
        let Message::Binary(b) = msg else { continue };
        let mut frame = b.to_vec();
        dec.apply_keystream(&mut frame);
        let mut reply = b"pong:".to_vec();
        reply.extend_from_slice(&frame);
        let _ = tx.send(Seen {
            proto,
            dc_idx,
            frame,
        });
        enc.apply_keystream(&mut reply);
        if ws.send(Message::Binary(reply.into())).await.is_err() {
            return;
        }
        if close_after_first_reply {
            let _ = ws.close(None).await;
            return;
        }
    }
}

/// Dials the fake endpoint over plain TCP, or always fails.
pub struct LoopDialer {
    pub addr: Option<SocketAddr>,
    pub dials: Arc<AtomicUsize>,
}

impl LoopDialer {
    pub fn to(addr: SocketAddr) -> (Self, Arc<AtomicUsize>) {
        let dials = Arc::new(AtomicUsize::new(0));
        (
            LoopDialer {
                addr: Some(addr),
                dials: dials.clone(),
            },
            dials,
        )
    }

    pub fn failing() -> (Self, Arc<AtomicUsize>) {
        let dials = Arc::new(AtomicUsize::new(0));
        (
            LoopDialer {
                addr: None,
                dials: dials.clone(),
            },
            dials,
        )
    }
}

impl Dialer for LoopDialer {
    async fn dial(&self, t: DcTarget) -> Result<WsConn, DialError> {
        self.dials.fetch_add(1, Ordering::SeqCst);
        let addr = self.addr.ok_or(DialError::NoRoute(t))?;
        let mut req = format!("ws://{addr}/apiws").into_client_request()?;
        req.headers_mut()
            .insert("Sec-WebSocket-Protocol", HeaderValue::from_static("binary"));
        let tcp = TcpStream::connect(addr).await?;
        let (ws, _) = client_async(req, MaybeTlsStream::Plain(tcp)).await?;
        Ok(ws)
    }
}

pub async fn write_all(s: &mut TcpStream, b: &[u8]) {
    s.write_all(b).await.unwrap();
}
