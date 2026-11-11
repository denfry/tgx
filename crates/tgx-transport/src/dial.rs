//! Opening a WSS connection to Telegram's web endpoint for a DC.

use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::time::Duration;

use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, tungstenite};

use crate::endpoints::{DcTarget, WsRoutes, ws_domains, ws_path};

pub type WsConn = WebSocketStream<MaybeTlsStream<TcpStream>>;

#[derive(Debug, thiserror::Error)]
pub enum DialError {
    #[error("no WS route for {0:?}")]
    NoRoute(DcTarget),
    #[error("timed out")]
    Timeout,
    #[error("io: {0}")]
    Io(#[from] io::Error),
    #[error("websocket: {0}")]
    Ws(#[from] tungstenite::Error),
}

/// Opens an upstream WebSocket for a DC. Abstracted so the relay can be
/// tested against a loopback fake.
pub trait Dialer: Send + Sync + 'static {
    fn dial(&self, t: DcTarget) -> impl Future<Output = Result<WsConn, DialError>> + Send;
}

/// Dials `wss://kws{N}[-1].web.telegram.org/apiws` over a TCP socket pinned
/// to the route IP. TLS is fully verified against webpki roots.
pub struct WssDialer {
    pub routes: WsRoutes,
    pub connect_timeout: Duration,
}

impl WssDialer {
    pub fn new(routes: WsRoutes) -> Self {
        WssDialer {
            routes,
            connect_timeout: Duration::from_secs(10),
        }
    }

    /// Dials one specific hostname over a socket to `addr` (used by `dial` and
    /// by diagnostics that probe each endpoint separately).
    pub async fn dial_domain(
        &self,
        addr: SocketAddr,
        domain: &str,
        path: &str,
    ) -> Result<WsConn, DialError> {
        let mut req = format!("wss://{domain}{path}").into_client_request()?;
        req.headers_mut()
            .insert("Sec-WebSocket-Protocol", HeaderValue::from_static("binary"));
        let attempt = async {
            let tcp = TcpStream::connect(addr).await?;
            tcp.set_nodelay(true)?;
            let (ws, _resp) =
                tokio_tungstenite::client_async_tls_with_config(req, tcp, None, None).await?;
            Ok::<_, DialError>(ws)
        };
        timeout(self.connect_timeout, attempt)
            .await
            .map_err(|_| DialError::Timeout)?
    }
}

impl Dialer for WssDialer {
    async fn dial(&self, t: DcTarget) -> Result<WsConn, DialError> {
        let ip = self.routes.lookup(t).ok_or(DialError::NoRoute(t))?;
        let addr = SocketAddr::new(ip, 443);
        let mut last = DialError::NoRoute(t);
        for domain in ws_domains(t) {
            match self.dial_domain(addr, &domain, ws_path(t)).await {
                Ok(ws) => return Ok(ws),
                Err(e) => last = e,
            }
        }
        Err(last)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpListener;

    /// A peer that is not a TLS server must yield an error, not a panic
    /// (e.g. from a missing rustls crypto provider).
    #[tokio::test]
    async fn tls_failure_is_an_error() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            let _ = s.write_all(b"HTTP/1.1 400 Bad Request\r\n\r\n").await;
        });
        let d = WssDialer::new(WsRoutes::default());
        let r = d.dial_domain(addr, "kws2.web.telegram.org", "/apiws").await;
        assert!(r.is_err());
    }
}
