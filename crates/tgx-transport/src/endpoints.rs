//! Telegram WebSocket endpoints per DC and the DC -> IP pinning table
//! (values from tg-ws-proxy `proxy/utils.py` / `proxy/config.py`).

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};

use crate::obfs::ClientInit;

/// Which Telegram data centre a client connection is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DcTarget {
    pub dc: u16,
    pub media: bool,
    pub test_dc: bool,
}

impl DcTarget {
    /// `ClientInit` already carries the normalised DC (test offset removed).
    pub fn from_init(i: &ClientInit) -> Self {
        DcTarget {
            dc: i.dc,
            media: i.media,
            test_dc: i.test_dc,
        }
    }
}

/// WebSocket hostnames to try, in order.
pub fn ws_domains(t: DcTarget) -> Vec<String> {
    let dc = if t.dc == 203 { 2 } else { t.dc };
    if t.media {
        vec![
            format!("kws{dc}-1.web.telegram.org"),
            format!("kws{dc}.web.telegram.org"),
        ]
    } else {
        vec![format!("kws{dc}.web.telegram.org")]
    }
}

pub fn ws_path(t: DcTarget) -> &'static str {
    if t.test_dc { "/apiws_test" } else { "/apiws" }
}

/// IP the WSS socket is pinned to, per DC (SNI/Host stay the kws domain).
#[derive(Clone, Debug)]
pub struct WsRoutes {
    pub ip: HashMap<u16, IpAddr>,
}

impl Default for WsRoutes {
    fn default() -> Self {
        let web = IpAddr::V4(Ipv4Addr::new(149, 154, 167, 220));
        WsRoutes {
            ip: HashMap::from([(2, web), (4, web)]),
        }
    }
}

impl WsRoutes {
    pub fn lookup(&self, t: DcTarget) -> Option<IpAddr> {
        let dc = if t.dc == 203 { 2 } else { t.dc };
        self.ip.get(&dc).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(dc: u16, media: bool) -> DcTarget {
        DcTarget {
            dc,
            media,
            test_dc: false,
        }
    }

    #[test]
    fn plain_domains() {
        assert_eq!(ws_domains(t(4, false)), vec!["kws4.web.telegram.org"]);
    }

    #[test]
    fn media_domains_order() {
        assert_eq!(
            ws_domains(t(5, true)),
            vec!["kws5-1.web.telegram.org", "kws5.web.telegram.org"]
        );
    }

    #[test]
    fn dc203_maps_to_dc2() {
        assert_eq!(ws_domains(t(203, false)), vec!["kws2.web.telegram.org"]);
    }

    #[test]
    fn test_dc_path() {
        assert_eq!(
            ws_path(DcTarget {
                dc: 2,
                media: false,
                test_dc: true
            }),
            "/apiws_test"
        );
        assert_eq!(ws_path(t(2, false)), "/apiws");
    }

    #[test]
    fn default_routes_only_dc2_dc4() {
        let r = WsRoutes::default();
        for dc in [1, 3, 5] {
            assert_eq!(r.lookup(t(dc, false)), None);
        }
        let ip: IpAddr = "149.154.167.220".parse().unwrap();
        for dc in [2, 4] {
            assert_eq!(r.lookup(t(dc, false)), Some(ip));
            assert_eq!(r.lookup(t(dc, true)), Some(ip));
        }
    }
}
