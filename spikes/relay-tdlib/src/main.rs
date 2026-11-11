//! Spike: TDLib through the local WS relay, plus a WS endpoint probe.
//!
//!   relay-tdlib probe [--ip <IP>]...
//!   relay-tdlib run --route ws|direct|mtproxy=<tg://proxy link> [--ws-ip <dc>=<ip>]...

mod td;

use std::net::{IpAddr, SocketAddr};
use std::process::ExitCode;
use std::time::Instant;

use tgx_transport::{DcTarget, WsRoutes, WssDialer, ws_domains, ws_path};
use tokio::task::JoinSet;

/// Upstream tg-ws-proxy defaults: the shared web IP and the per-DC IPs.
const WEB_IP: &str = "149.154.167.220";
const DC_DEFAULT_IPS: [(u16, &str); 5] = [
    (1, "149.154.175.50"),
    (2, "149.154.167.51"),
    (3, "149.154.175.100"),
    (4, "149.154.167.91"),
    (5, "149.154.171.5"),
];

const USAGE: &str = "usage:
  relay-tdlib probe [--ip <IP>]...
  relay-tdlib run --route ws|direct|mtproxy=<tg://proxy link> [--ws-ip <dc>=<ip>]...";

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("probe") => match parse_probe_args(&args[1..]) {
            Ok(ips) => {
                probe(ips).await;
                Ok(())
            }
            Err(e) => Err(e),
        },
        Some("run") => match parse_run_args(&args[1..]) {
            Ok(route) => td::run(route).await,
            Err(e) => Err(e),
        },
        _ => Err(USAGE.to_string()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn parse_probe_args(args: &[String]) -> Result<Vec<IpAddr>, String> {
    let mut ips = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--ip" => {
                let v = it.next().ok_or("--ip needs a value")?;
                ips.push(v.parse().map_err(|_| format!("bad IP: {v}"))?);
            }
            other => return Err(format!("unknown argument: {other}\n{USAGE}")),
        }
    }
    Ok(ips)
}

fn parse_run_args(args: &[String]) -> Result<td::Route, String> {
    let mut route = None;
    let mut overrides = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--route" => route = Some(it.next().ok_or("--route needs a value")?.clone()),
            "--ws-ip" => overrides.push(parse_ws_ip(it.next().ok_or("--ws-ip needs a value")?)?),
            other => return Err(format!("unknown argument: {other}\n{USAGE}")),
        }
    }
    match route.as_deref() {
        Some("ws") => Ok(td::Route::Ws { overrides }),
        Some("direct") => Ok(td::Route::Direct),
        Some(r) if r.starts_with("mtproxy=") => parse_mtproxy_link(&r["mtproxy=".len()..]),
        _ => Err(format!("--route is required\n{USAGE}")),
    }
}

fn parse_ws_ip(v: &str) -> Result<(u16, IpAddr), String> {
    let (dc, ip) = v
        .split_once('=')
        .ok_or_else(|| format!("expected <dc>=<ip>, got {v}"))?;
    let dc = dc.parse().map_err(|_| format!("bad DC: {dc}"))?;
    let ip = ip.parse().map_err(|_| format!("bad IP: {ip}"))?;
    Ok((dc, ip))
}

/// Parses `tg://proxy?server=..&port=..&secret=..` (or the t.me/proxy form).
fn parse_mtproxy_link(link: &str) -> Result<td::Route, String> {
    let query = link
        .split_once('?')
        .map(|(_, q)| q)
        .ok_or("proxy link has no query")?;
    let (mut server, mut port, mut secret) = (None, None, None);
    for pair in query.split('&') {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        let v = percent_decode(v);
        match k {
            "server" => server = Some(v),
            "port" => port = Some(v.parse::<u16>().map_err(|_| "bad proxy port")?),
            "secret" => secret = Some(v),
            _ => {}
        }
    }
    Ok(td::Route::MtProxy {
        server: server.ok_or("proxy link has no server")?,
        port: port.ok_or("proxy link has no port")?,
        secret: secret.ok_or("proxy link has no secret")?,
    })
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Ok(hex) = std::str::from_utf8(&b[i + 1..i + 3])
            && let Ok(byte) = u8::from_str_radix(hex, 16)
        {
            out.push(byte);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

struct ProbeRow {
    dc: u16,
    media: bool,
    ip: IpAddr,
    domain: String,
    result: Result<(), String>,
    ms: u128,
}

/// Dials every (DC, media, endpoint, IP) combination once, concurrently, and
/// prints a table. Failures are data, not errors.
async fn probe(extra_ips: Vec<IpAddr>) {
    let mut jobs = JoinSet::new();
    for (dc, dc_ip) in DC_DEFAULT_IPS {
        let mut ips: Vec<IpAddr> = vec![WEB_IP.parse().unwrap(), dc_ip.parse().unwrap()];
        ips.extend(extra_ips.iter().copied());
        for media in [false, true] {
            let t = DcTarget {
                dc,
                media,
                test_dc: false,
            };
            for ip in ips.iter().copied() {
                for domain in ws_domains(t) {
                    jobs.spawn(async move {
                        let dialer = WssDialer::new(WsRoutes {
                            ip: Default::default(),
                        });
                        let start = Instant::now();
                        let r = dialer
                            .dial_domain(SocketAddr::new(ip, 443), &domain, ws_path(t))
                            .await;
                        let ms = start.elapsed().as_millis();
                        let result = match r {
                            Ok(mut ws) => {
                                let _ = ws.close(None).await;
                                Ok(())
                            }
                            Err(e) => Err(e.to_string()),
                        };
                        ProbeRow {
                            dc,
                            media,
                            ip,
                            domain,
                            result,
                            ms,
                        }
                    });
                }
            }
        }
    }
    let mut rows = jobs.join_all().await;
    rows.sort_by(|a, b| (a.dc, a.media, a.ip, &a.domain).cmp(&(b.dc, b.media, b.ip, &b.domain)));
    println!("| dc | media | ip | domain | result | ms |");
    println!("|---|---|---|---|---|---|");
    for r in &rows {
        let res = match &r.result {
            Ok(()) => "ok".to_string(),
            Err(e) => format!("err: {e}"),
        };
        println!(
            "| {} | {} | {} | {} | {} | {} |",
            r.dc, r.media, r.ip, r.domain, res, r.ms
        );
    }
    let ok = rows.iter().filter(|r| r.result.is_ok()).count();
    println!("\n{ok}/{} combinations ok", rows.len());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tg_proxy_link() {
        let r = parse_mtproxy_link("tg://proxy?server=proxy.example&port=443&secret=ee00ff%3D%3D")
            .unwrap();
        match r {
            td::Route::MtProxy {
                server,
                port,
                secret,
            } => {
                assert_eq!(
                    (server.as_str(), port, secret.as_str()),
                    ("proxy.example", 443, "ee00ff==")
                );
            }
            _ => panic!("expected mtproxy route"),
        }
    }

    #[test]
    fn parses_ws_ip_override() {
        assert_eq!(
            parse_ws_ip("1=149.154.175.50").unwrap(),
            (1, "149.154.175.50".parse().unwrap())
        );
        assert!(parse_ws_ip("x=1.2.3.4").is_err());
        assert!(parse_ws_ip("1").is_err());
    }
}
