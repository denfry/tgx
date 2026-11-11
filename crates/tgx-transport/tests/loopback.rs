//! End-to-end: fake TDLib client <-> relay <-> fake Telegram WS endpoint.

mod common;

use std::sync::atomic::Ordering;
use std::time::Duration;

use common::{FakeClient, FakeTelegram, LoopDialer, intermediate_packet, write_all};
use tgx_transport::{DcTarget, ProtoTag, RelayEvent, start_relay, start_relay_with_secret};
use tokio::io::AsyncReadExt;
use tokio::net::TcpStream;
use tokio::time::{Instant, sleep, timeout};

const SECRET: [u8; 16] = [0x5a; 16];
const T: Duration = Duration::from_secs(5);

async fn next_event(rx: &mut tokio::sync::mpsc::Receiver<RelayEvent>) -> RelayEvent {
    timeout(T, rx.recv())
        .await
        .expect("event in time")
        .expect("channel open")
}

/// Sends one packet through the relay and checks both directions.
async fn ping(client: &mut FakeClient, s: &mut TcpStream, packet: &[u8]) {
    let wire = client.encrypt(packet);
    write_all(s, &wire).await;
    let reply = timeout(T, client.read_plain(s, 5 + packet.len()))
        .await
        .expect("reply in time");
    assert_eq!(&reply[..5], b"pong:");
    assert_eq!(&reply[5..], packet);
}

#[tokio::test]
async fn relays_payload_both_ways() {
    let mut tg = FakeTelegram::start(false).await;
    let (dialer, dials) = LoopDialer::to(tg.addr);
    let mut relay = start_relay_with_secret(dialer, SECRET).await.unwrap();

    let mut client = FakeClient::new(&SECRET, ProtoTag::Intermediate, 2);
    let mut s = TcpStream::connect(relay.addr).await.unwrap();
    write_all(&mut s, &client.init).await;
    let packet = intermediate_packet(8, 1);
    ping(&mut client, &mut s, &packet).await;

    let seen = tg.seen.recv().await.unwrap();
    assert_eq!(
        seen.frame, packet,
        "one WS frame per transport packet, exact plaintext"
    );
    assert_eq!(seen.proto, [0xee; 4]);
    assert_eq!(seen.dc_idx, 2);
    assert!(matches!(
        next_event(&mut relay.events).await,
        RelayEvent::Connected(DcTarget {
            dc: 2,
            media: false,
            test_dc: false
        })
    ));
    assert_eq!(dials.load(Ordering::SeqCst), 1);
    relay.shutdown().await;
}

#[tokio::test]
async fn init_arrives_in_fragments() {
    let mut tg = FakeTelegram::start(false).await;
    let (dialer, _) = LoopDialer::to(tg.addr);
    let relay = start_relay_with_secret(dialer, SECRET).await.unwrap();

    let mut client = FakeClient::new(&SECRET, ProtoTag::Abridged, 4);
    let mut s = TcpStream::connect(relay.addr).await.unwrap();
    for range in [0..10, 10..40, 40..64] {
        write_all(&mut s, &client.init[range]).await;
        sleep(Duration::from_millis(20)).await;
    }
    let mut packet = vec![0x02];
    packet.extend_from_slice(&[9; 8]);
    ping(&mut client, &mut s, &packet).await;
    let seen = tg.seen.recv().await.unwrap();
    assert_eq!((seen.proto, seen.dc_idx), ([0xef; 4], 4));
    relay.shutdown().await;
}

#[tokio::test]
async fn junk_is_rejected_without_dial() {
    let (dialer, dials) = LoopDialer::failing();
    let mut relay = start_relay_with_secret(dialer, SECRET).await.unwrap();

    let mut junk = b"GET / HTTP/1.1\r\n\r\n".to_vec();
    junk.resize(64, b' ');
    let mut s = TcpStream::connect(relay.addr).await.unwrap();
    write_all(&mut s, &junk).await;
    assert!(matches!(
        next_event(&mut relay.events).await,
        RelayEvent::BadHandshake
    ));

    // The relay drains for at most 10 s, then closes. Virtual time from here.
    tokio::time::pause();
    let start = Instant::now();
    let mut buf = [0u8; 16];
    let n = s.read(&mut buf).await.unwrap_or(0);
    assert_eq!(n, 0, "relay must not answer junk");
    assert!(start.elapsed() <= Duration::from_secs(11));
    assert_eq!(dials.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn tls_client_hello_is_rejected_without_dial() {
    let (dialer, dials) = LoopDialer::failing();
    let mut relay = start_relay_with_secret(dialer, SECRET).await.unwrap();
    let mut hello = vec![0x16, 0x03, 0x01, 0x02, 0x00];
    hello.resize(64, 0x42);
    let mut s = TcpStream::connect(relay.addr).await.unwrap();
    write_all(&mut s, &hello).await;
    assert!(matches!(
        next_event(&mut relay.events).await,
        RelayEvent::BadHandshake
    ));
    assert_eq!(dials.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn unreachable_dc_reported_and_closed() {
    let (dialer, dials) = LoopDialer::failing();
    let mut relay = start_relay_with_secret(dialer, SECRET).await.unwrap();

    let client = FakeClient::new(&SECRET, ProtoTag::Intermediate, -3);
    let mut s = TcpStream::connect(relay.addr).await.unwrap();
    write_all(&mut s, &client.init).await;
    match next_event(&mut relay.events).await {
        RelayEvent::DcUnreachable { target, .. } => {
            assert_eq!(
                target,
                DcTarget {
                    dc: 3,
                    media: true,
                    test_dc: false
                }
            )
        }
        other => panic!("expected DcUnreachable, got {other:?}"),
    }
    let mut buf = [0u8; 1];
    let n = timeout(T, s.read(&mut buf))
        .await
        .expect("closed in time")
        .unwrap_or(0);
    assert_eq!(n, 0);
    assert_eq!(dials.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn upstream_close_closes_client() {
    let tg = FakeTelegram::start(true).await;
    let (dialer, _) = LoopDialer::to(tg.addr);
    let relay = start_relay_with_secret(dialer, SECRET).await.unwrap();

    let mut client = FakeClient::new(&SECRET, ProtoTag::Intermediate, 2);
    let mut s = TcpStream::connect(relay.addr).await.unwrap();
    write_all(&mut s, &client.init).await;
    ping(&mut client, &mut s, &intermediate_packet(4, 7)).await;
    let mut buf = [0u8; 1];
    let n = timeout(Duration::from_secs(1), s.read(&mut buf))
        .await
        .expect("client closed within 1 s")
        .unwrap_or(0);
    assert_eq!(n, 0);
    relay.shutdown().await;
}

#[tokio::test]
async fn parallel_clients_are_independent() {
    let tg = FakeTelegram::start(false).await;
    let (dialer, dials) = LoopDialer::to(tg.addr);
    let relay = start_relay_with_secret(dialer, SECRET).await.unwrap();
    let addr = relay.addr;

    let tasks: Vec<_> = (0..8u8)
        .map(|i| {
            tokio::spawn(async move {
                let dc_idx = if i % 2 == 0 { 2 } else { -4 };
                let mut client = FakeClient::new(&SECRET, ProtoTag::PaddedIntermediate, dc_idx);
                let mut s = TcpStream::connect(addr).await.unwrap();
                write_all(&mut s, &client.init).await;
                for round in 0..3u8 {
                    ping(
                        &mut client,
                        &mut s,
                        &intermediate_packet(16 + 4 * i as usize, i * 10 + round),
                    )
                    .await;
                }
            })
        })
        .collect();
    for t in tasks {
        t.await.unwrap();
    }
    assert_eq!(dials.load(Ordering::SeqCst), 8);
    relay.shutdown().await;
}

#[tokio::test]
async fn tdlib_secret_format() {
    let (dialer, _) = LoopDialer::failing();
    let relay = start_relay(dialer).await.unwrap();
    let s = relay.tdlib_secret();
    assert_eq!(s.len(), 34);
    assert!(s.starts_with("dd"));
    assert!(
        s[2..]
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    );
    assert_eq!(relay.addr.ip().to_string(), "127.0.0.1");
    assert_ne!(relay.addr.port(), 0);

    let (dialer, _) = LoopDialer::failing();
    let other = start_relay(dialer).await.unwrap();
    assert_ne!(other.tdlib_secret(), s, "secret is random per run");
    relay.shutdown().await;
    other.shutdown().await;
}

/// TDLib only avoids the `16 03 01 02` prefix, so a genuine init may start
/// with 0x16 (1 in 256 connections); it must not be mistaken for TLS.
#[tokio::test]
async fn init_starting_with_0x16_is_accepted() {
    let mut tg = FakeTelegram::start(false).await;
    let (dialer, _) = LoopDialer::to(tg.addr);
    let relay = start_relay_with_secret(dialer, SECRET).await.unwrap();

    let mut client = FakeClient::new(&SECRET, ProtoTag::Intermediate, 2);
    client.init[0] = 0x16; // byte 0 is not part of the key, IV or tail
    let mut s = TcpStream::connect(relay.addr).await.unwrap();
    write_all(&mut s, &client.init).await;
    let packet = intermediate_packet(8, 3);
    ping(&mut client, &mut s, &packet).await;
    assert_eq!(tg.seen.recv().await.unwrap().frame, packet);
    relay.shutdown().await;
}
