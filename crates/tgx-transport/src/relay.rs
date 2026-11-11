//! Local MTProto proxy on loopback: accepts TDLib connections, decodes the
//! obfuscated init, and relays each connection to Telegram over a WebSocket
//! with re-encryption. A DC that cannot be reached is reported and the client
//! connection is closed — the relay never connects to a DC directly.

use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot};
use tokio::task::{JoinHandle, JoinSet};
use tokio::time::{sleep, timeout};
use tokio_tungstenite::tungstenite::Message;

use crate::dial::{DialError, Dialer, WsConn};
use crate::endpoints::DcTarget;
use crate::framing::PacketSplitter;
use crate::obfs::{CryptoCtx, HANDSHAKE_LEN, ProtoTag, generate_relay_init, parse_client_init};

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const BAD_HANDSHAKE_DRAIN_TIMEOUT: Duration = Duration::from_secs(10);
const BAD_HANDSHAKE_DRAIN_LIMIT: usize = 64 * 1024;
const READ_BUF_LEN: usize = 64 * 1024;
const DIAL_ATTEMPTS: u32 = 3;
const DIAL_BACKOFF: Duration = Duration::from_millis(200);
const EVENTS_CAPACITY: usize = 64;
const ACCEPT_ERROR_PAUSE: Duration = Duration::from_millis(100);

/// Diagnostic events for the route manager / UI. Delivery is best effort:
/// when the channel is full, events are dropped rather than blocking relaying.
#[derive(Clone, Debug)]
pub enum RelayEvent {
    Connected(DcTarget),
    DcUnreachable { target: DcTarget, error: String },
    BadHandshake,
}

/// A running relay. Dropping it (or calling [`RelayHandle::shutdown`]) stops
/// the listener and every relayed connection.
pub struct RelayHandle {
    pub addr: SocketAddr,
    pub events: mpsc::Receiver<RelayEvent>,
    secret: [u8; 16],
    stop: oneshot::Sender<()>,
    task: JoinHandle<()>,
}

impl RelayHandle {
    /// The secret in the form TDLib's `proxyTypeMtproto` expects.
    pub fn tdlib_secret(&self) -> String {
        let hex: String = self.secret.iter().map(|b| format!("{b:02x}")).collect();
        format!("dd{hex}")
    }

    pub async fn shutdown(self) {
        let _ = self.stop.send(());
        let _ = self.task.await;
    }
}

/// Starts a relay on `127.0.0.1` with a random port and a random secret.
pub async fn start_relay<D: Dialer>(dialer: D) -> io::Result<RelayHandle> {
    let mut secret = [0u8; 16];
    rand::fill(&mut secret[..]);
    start_relay_with_secret(dialer, secret).await
}

pub async fn start_relay_with_secret<D: Dialer>(
    dialer: D,
    secret: [u8; 16],
) -> io::Result<RelayHandle> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    let addr = listener.local_addr()?;
    let (events_tx, events) = mpsc::channel(EVENTS_CAPACITY);
    let (stop, stop_rx) = oneshot::channel();
    let task = tokio::spawn(accept_loop(
        listener,
        Arc::new(dialer),
        secret,
        events_tx,
        stop_rx,
    ));
    Ok(RelayHandle {
        addr,
        events,
        secret,
        stop,
        task,
    })
}

async fn accept_loop<D: Dialer>(
    listener: TcpListener,
    dialer: Arc<D>,
    secret: [u8; 16],
    events: mpsc::Sender<RelayEvent>,
    mut stop: oneshot::Receiver<()>,
) {
    let mut conns = JoinSet::new();
    loop {
        tokio::select! {
            // Fires on shutdown() and when the handle is dropped.
            _ = &mut stop => break,
            Some(_) = conns.join_next(), if !conns.is_empty() => {}
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => {
                    conns.spawn(handle_conn(stream, dialer.clone(), secret, events.clone()));
                }
                // e.g. descriptor exhaustion: back off instead of spinning.
                Err(_) => sleep(ACCEPT_ERROR_PAUSE).await,
            },
        }
    }
    conns.shutdown().await;
}

async fn handle_conn<D: Dialer>(
    mut stream: TcpStream,
    dialer: Arc<D>,
    secret: [u8; 16],
    events: mpsc::Sender<RelayEvent>,
) {
    let _ = stream.set_nodelay(true);
    let mut init = [0u8; HANDSHAKE_LEN];
    match timeout(HANDSHAKE_TIMEOUT, stream.read_exact(&mut init)).await {
        Ok(Ok(_)) => {}
        _ => return,
    }
    // No first-byte shortcut for TLS (0x16): TDLib's random init may start
    // with it. HTTP, TLS ClientHello and noise all fail to decode instead.
    let Some(client) = parse_client_init(&init, &secret) else {
        let _ = events.try_send(RelayEvent::BadHandshake);
        let _ = timeout(BAD_HANDSHAKE_DRAIN_TIMEOUT, drain(&mut stream)).await;
        return;
    };

    let target = DcTarget::from_init(&client);
    let ws = match dial_with_retry(dialer.as_ref(), target).await {
        Ok(ws) => ws,
        Err(e) => {
            let _ = events.try_send(RelayEvent::DcUnreachable {
                target,
                error: e.to_string(),
            });
            return;
        }
    };
    let _ = events.try_send(RelayEvent::Connected(target));

    let relay_init = generate_relay_init(client.proto, client.dc_idx());
    let ctx = CryptoCtx::new(&client, &secret, &relay_init);
    bridge(stream, ws, relay_init, ctx, client.proto).await;
}

/// Reads and discards at most `BAD_HANDSHAKE_DRAIN_LIMIT` bytes so a
/// misbehaving peer is not answered and does not see an instant reset.
async fn drain(stream: &mut TcpStream) {
    let mut buf = [0u8; 4096];
    let mut remaining = BAD_HANDSHAKE_DRAIN_LIMIT;
    while remaining > 0 {
        let want = remaining.min(buf.len());
        match stream.read(&mut buf[..want]).await {
            Ok(0) | Err(_) => break,
            Ok(n) => remaining -= n,
        }
    }
}

async fn dial_with_retry<D: Dialer>(dialer: &D, target: DcTarget) -> Result<WsConn, DialError> {
    let mut attempt = 0;
    loop {
        match dialer.dial(target).await {
            Ok(ws) => return Ok(ws),
            Err(e) if attempt + 1 >= DIAL_ATTEMPTS => return Err(e),
            Err(_) => {
                let jitter = rand::random_range(0.8..1.2);
                sleep(DIAL_BACKOFF.mul_f64(f64::from(1u32 << attempt) * jitter)).await;
                attempt += 1;
            }
        }
    }
}

/// Copies both directions until either side ends; the other is then dropped
/// (aborted) and both sockets close.
async fn bridge(
    mut client: TcpStream,
    ws: WsConn,
    relay_init: [u8; HANDSHAKE_LEN],
    ctx: CryptoCtx,
    proto: ProtoTag,
) {
    let (mut sink, mut source) = ws.split();
    if sink
        .send(Message::Binary(relay_init.to_vec().into()))
        .await
        .is_err()
    {
        return;
    }
    let (mut up, mut down) = ctx.split();
    let (mut rd, mut wr) = client.split();

    let client_to_tg = async {
        let mut splitter = PacketSplitter::new(proto);
        let mut buf = vec![0u8; READ_BUF_LEN];
        loop {
            let n = match rd.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            let plain = &mut buf[..n];
            let cipher = up.client_to_tg(plain);
            for part in splitter.split(&cipher, plain) {
                if sink.feed(Message::Binary(part.into())).await.is_err() {
                    return;
                }
            }
            if sink.flush().await.is_err() {
                return;
            }
        }
        if let Some(tail) = splitter.flush() {
            let _ = sink.send(Message::Binary(tail.into())).await;
        }
        let _ = sink.close().await;
    };

    let tg_to_client = async {
        while let Some(Ok(msg)) = source.next().await {
            match msg {
                Message::Binary(b) => {
                    let mut b = b.to_vec();
                    down.tg_to_client(&mut b);
                    if wr.write_all(&b).await.is_err() {
                        return;
                    }
                }
                Message::Close(_) => break,
                // Ping/pong are answered by tungstenite; text is not MTProto.
                _ => {}
            }
        }
        let _ = wr.shutdown().await;
    };

    tokio::select! {
        _ = client_to_tg => {}
        _ = tg_to_client => {}
    }
}
