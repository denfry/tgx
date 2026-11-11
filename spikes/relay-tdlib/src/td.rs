//! TDLib session over a chosen route, with connection and download timing.
//! Phone, code and password are read without echo and never printed or logged.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use tdlib_rs::enums::{
    AddedProxies, AuthorizationState, Chat, ChatList, LogStream, MessageContent, Messages,
    ProxyType, Update, User,
};
use tdlib_rs::functions as f;
use tdlib_rs::types::{self, LogStreamFile, Proxy, ProxyTypeMtproto};
use tgx_transport::{RelayEvent, RelayHandle, WsRoutes, WssDialer, start_relay};
use tokio::sync::mpsc;

pub enum Route {
    Ws {
        overrides: Vec<(u16, std::net::IpAddr)>,
    },
    Direct,
    MtProxy {
        server: String,
        port: u16,
        secret: String,
    },
}

const LOG_MAX_BYTES: i64 = 10 * 1024 * 1024;
const HISTORY_SCAN: usize = 100;

fn td_err(what: &str) -> impl Fn(types::Error) -> String + '_ {
    move |e| format!("{what}: TDLib error {} {}", e.code, e.message)
}

pub async fn run(route: Route) -> Result<(), String> {
    let api_id: i32 = std::env::var("TGX_API_ID")
        .map_err(|_| "TGX_API_ID is not set (your api_id from my.telegram.org)")?
        .trim()
        .parse()
        .map_err(|_| "TGX_API_ID must be a number")?;
    let api_hash = std::env::var("TGX_API_HASH").map_err(|_| "TGX_API_HASH is not set")?;
    let local = std::env::var("LOCALAPPDATA").map_err(|_| "LOCALAPPDATA is not set")?;
    let data_dir = PathBuf::from(local).join("tgx").join("spike-tdlib");
    std::fs::create_dir_all(&data_dir)
        .map_err(|e| format!("create {}: {e}", data_dir.display()))?;

    let start = Instant::now();
    let ms = move || start.elapsed().as_millis();

    let relay = match &route {
        Route::Ws { overrides } => {
            let mut routes = WsRoutes::default();
            routes.ip.extend(overrides.iter().copied());
            let mut relay = start_relay(WssDialer::new(routes))
                .await
                .map_err(|e| format!("start relay: {e}"))?;
            let events = std::mem::replace(&mut relay.events, mpsc::channel(1).1);
            tokio::spawn(print_relay_events(events, start));
            println!(
                "[{} ms] relay listening on {} (secret set)",
                ms(),
                relay.addr
            );
            Some(relay)
        }
        _ => None,
    };

    let client_id = tdlib_rs::create_client();
    let stop = Arc::new(AtomicBool::new(false));
    let mut updates = spawn_receiver(stop.clone());

    f::set_log_verbosity_level(1, client_id)
        .await
        .map_err(td_err("setLogVerbosityLevel"))?;
    let log = LogStreamFile {
        path: data_dir.join("td.log").to_string_lossy().into_owned(),
        max_file_size: LOG_MAX_BYTES,
        redirect_stderr: false,
    };
    f::set_log_stream(LogStream::File(log), client_id)
        .await
        .map_err(td_err("setLogStream"))?;

    println!("[{} ms] route: {}", ms(), route_name(&route));
    let td = TdSetup {
        client_id,
        api_id,
        api_hash: &api_hash,
        data_dir: &data_dir,
        route: &route,
        relay: relay.as_ref(),
    };
    let proxy_id = authorize(&mut updates, &td, &ms).await?;
    println!("[{} ms] authorization ready", ms());
    remove_stale_proxies(proxy_id, client_id).await?;
    tokio::spawn(print_connection_states(updates, start));

    match f::load_chats(Some(ChatList::Main), 20, client_id).await {
        Ok(()) => {}
        Err(e) if e.code == 404 => {} // everything already loaded
        Err(e) => return Err(td_err("loadChats")(e)),
    }
    let tdlib_rs::enums::Chats::Chats(chats) = f::get_chats(Some(ChatList::Main), 20, client_id)
        .await
        .map_err(td_err("getChats"))?;
    println!(
        "[{} ms] first chat batch: {} chats",
        ms(),
        chats.chat_ids.len()
    );
    for id in &chats.chat_ids {
        if let Ok(Chat::Chat(c)) = f::get_chat(*id, client_id).await {
            println!("  {}", c.title);
        }
    }

    repl(client_id).await;

    let _ = f::close(client_id).await;
    stop.store(true, Ordering::Relaxed);
    if let Some(relay) = relay {
        relay.shutdown().await;
    }
    Ok(())
}

fn route_name(route: &Route) -> &'static str {
    match route {
        Route::Ws { .. } => "ws (local relay)",
        Route::Direct => "direct",
        Route::MtProxy { .. } => "mtproxy",
    }
}

/// `tdlib_rs::receive` must be polled continuously: it also delivers the
/// responses awaited by `functions::*`.
fn spawn_receiver(stop: Arc<AtomicBool>) -> mpsc::UnboundedReceiver<Update> {
    let (tx, rx) = mpsc::unbounded_channel();
    std::thread::spawn(move || {
        while !stop.load(Ordering::Relaxed) {
            if let Some((update, _client)) = tdlib_rs::receive()
                && tx.send(update).is_err()
            {
                break;
            }
        }
    });
    rx
}

/// Enables this route's proxy (or disables proxies for `direct`). Proxy
/// requests are queued by TDLib until `setTdlibParameters`, so this is sent
/// just before the parameters and applied first on initialisation. Returns
/// the id of the added proxy.
async fn configure_proxy(
    route: &Route,
    relay: Option<&RelayHandle>,
    client_id: i32,
) -> Result<Option<i32>, String> {
    let proxy = match (route, relay) {
        (Route::Ws { .. }, Some(relay)) => Proxy {
            server: "127.0.0.1".into(),
            port: i32::from(relay.addr.port()),
            r#type: ProxyType::Mtproto(ProxyTypeMtproto {
                secret: relay.tdlib_secret(),
            }),
        },
        (
            Route::MtProxy {
                server,
                port,
                secret,
            },
            _,
        ) => Proxy {
            server: server.clone(),
            port: i32::from(*port),
            r#type: ProxyType::Mtproto(ProxyTypeMtproto {
                secret: secret.clone(),
            }),
        },
        (Route::Direct, _) => {
            f::disable_proxy(client_id)
                .await
                .map_err(td_err("disableProxy"))?;
            tokio::spawn(ping(None, client_id));
            return Ok(None);
        }
        (Route::Ws { .. }, None) => unreachable!("ws route always starts a relay"),
    };
    tokio::spawn(ping(Some(proxy.clone()), client_id));
    let tdlib_rs::enums::AddedProxy::AddedProxy(added) = f::add_proxy(proxy, true, client_id)
        .await
        .map_err(td_err("addProxy"))?;
    Ok(Some(added.id))
}

/// One real request/response with Telegram over the route (TDLib `pingProxy`;
/// `None` pings directly), proving traffic flows end to end before login.
async fn ping(proxy: Option<Proxy>, client_id: i32) {
    match f::ping_proxy(proxy, client_id).await {
        Ok(tdlib_rs::enums::Seconds::Seconds(s)) => println!(
            "pingProxy: Telegram answered in {:.0} ms",
            s.seconds * 1000.0
        ),
        Err(e) => println!("pingProxy failed: {} {}", e.code, e.message),
    }
}

/// Removes proxies left by earlier runs (the relay port changes every run).
async fn remove_stale_proxies(keep: Option<i32>, client_id: i32) -> Result<(), String> {
    let AddedProxies::AddedProxies(existing) = f::get_proxies(client_id)
        .await
        .map_err(td_err("getProxies"))?;
    for p in existing.proxies.iter().filter(|p| Some(p.id) != keep) {
        f::remove_proxy(p.id, client_id)
            .await
            .map_err(td_err("removeProxy"))?;
    }
    Ok(())
}

struct TdSetup<'a> {
    client_id: i32,
    api_id: i32,
    api_hash: &'a str,
    data_dir: &'a Path,
    route: &'a Route,
    relay: Option<&'a RelayHandle>,
}

/// Drives authorization to `Ready`; returns the id of the proxy it enabled.
async fn authorize(
    updates: &mut mpsc::UnboundedReceiver<Update>,
    td: &TdSetup<'_>,
    ms: &impl Fn() -> u128,
) -> Result<Option<i32>, String> {
    let client_id = td.client_id;
    let mut proxy_id = None;
    while let Some(update) = updates.recv().await {
        let state = match update {
            Update::AuthorizationState(s) => s.authorization_state,
            Update::ConnectionState(c) => {
                println!("[{} ms] connection: {:?}", ms(), c.state);
                continue;
            }
            _ => continue,
        };
        println!("[{} ms] auth: {}", ms(), auth_state_name(&state));
        match state {
            AuthorizationState::WaitTdlibParameters => {
                let dir = td.data_dir.to_string_lossy().into_owned();
                // join! polls in order, so addProxy is sent before the parameters.
                let (proxy, params) = tokio::join!(
                    configure_proxy(td.route, td.relay, client_id),
                    f::set_tdlib_parameters(
                        false,
                        dir.clone(),
                        dir,
                        String::new(),
                        true,
                        true,
                        true,
                        false,
                        td.api_id,
                        td.api_hash.to_string(),
                        "en".into(),
                        "Desktop".into(),
                        String::new(),
                        env!("CARGO_PKG_VERSION").into(),
                        client_id,
                    )
                );
                params.map_err(td_err("setTdlibParameters"))?;
                proxy_id = proxy?;
            }
            AuthorizationState::WaitPhoneNumber => {
                let phone = read_secret("phone number (international format): ").await?;
                if let Err(e) = f::set_authentication_phone_number(phone, None, client_id).await {
                    eprintln!("phone rejected: {}", e.message);
                }
            }
            AuthorizationState::WaitCode(_) => {
                let code = read_secret("login code: ").await?;
                if let Err(e) = f::check_authentication_code(code, client_id).await {
                    eprintln!("code rejected: {}", e.message);
                }
            }
            AuthorizationState::WaitPassword(_) => {
                let password = read_secret("2FA password: ").await?;
                if let Err(e) = f::check_authentication_password(password, client_id).await {
                    eprintln!("password rejected: {}", e.message);
                }
            }
            AuthorizationState::Ready => return Ok(proxy_id),
            AuthorizationState::Closed
            | AuthorizationState::Closing
            | AuthorizationState::LoggingOut => {
                return Err("TDLib closed during authorization".into());
            }
            _ => {
                return Err(
                    "this account needs an authorization step the spike does not support".into(),
                );
            }
        }
    }
    Err("TDLib update stream ended".into())
}

/// Variant name only: some states carry the phone number or email.
fn auth_state_name(s: &AuthorizationState) -> &'static str {
    match s {
        AuthorizationState::WaitTdlibParameters => "WaitTdlibParameters",
        AuthorizationState::WaitPhoneNumber => "WaitPhoneNumber",
        AuthorizationState::WaitCode(_) => "WaitCode",
        AuthorizationState::WaitPassword(_) => "WaitPassword",
        AuthorizationState::Ready => "Ready",
        AuthorizationState::Closing => "Closing",
        AuthorizationState::Closed => "Closed",
        AuthorizationState::LoggingOut => "LoggingOut",
        _ => "other",
    }
}

async fn read_secret(prompt: &'static str) -> Result<String, String> {
    tokio::task::spawn_blocking(move || rpassword::prompt_password(prompt))
        .await
        .map_err(|e| e.to_string())?
        .map(|s| s.trim().to_string())
        .map_err(|e| format!("read input: {e}"))
}

async fn read_line(prompt: &'static str) -> Option<String> {
    tokio::task::spawn_blocking(move || {
        use std::io::Write;
        print!("{prompt}");
        let _ = std::io::stdout().flush();
        let mut s = String::new();
        match std::io::stdin().read_line(&mut s) {
            Ok(0) | Err(_) => None,
            Ok(_) => Some(s.trim().to_string()),
        }
    })
    .await
    .ok()
    .flatten()
}

async fn print_relay_events(mut events: mpsc::Receiver<RelayEvent>, start: Instant) {
    while let Some(e) = events.recv().await {
        let t = start.elapsed().as_millis();
        match e {
            RelayEvent::Connected(d) => {
                println!("[{t} ms] relay: connected DC{}{}", d.dc, media_tag(d.media))
            }
            RelayEvent::DcUnreachable { target, error } => {
                println!(
                    "[{t} ms] relay: DC{}{} UNREACHABLE over WS: {error}",
                    target.dc,
                    media_tag(target.media)
                )
            }
            RelayEvent::BadHandshake => println!("[{t} ms] relay: bad handshake rejected"),
        }
    }
}

fn media_tag(media: bool) -> &'static str {
    if media { " (media)" } else { "" }
}

async fn print_connection_states(mut updates: mpsc::UnboundedReceiver<Update>, start: Instant) {
    while let Some(u) = updates.recv().await {
        if let Update::ConnectionState(c) = u {
            println!(
                "[{} ms] connection: {:?}",
                start.elapsed().as_millis(),
                c.state
            );
        }
    }
}

async fn repl(client_id: i32) {
    println!("commands: photo | big | quit");
    while let Some(cmd) = read_line("> ").await {
        let result = match cmd.as_str() {
            "photo" => download_newest(client_id, Kind::Photo).await,
            "big" => download_newest(client_id, Kind::LargestDocument).await,
            "quit" | "exit" => break,
            "" => continue,
            _ => Err("unknown command".into()),
        };
        if let Err(e) = result {
            println!("{e}");
        }
    }
}

#[derive(Clone, Copy)]
enum Kind {
    Photo,
    LargestDocument,
}

/// Downloads the newest photo / largest document among the last 100 Saved
/// Messages, after deleting any cached copy so the timing is a real download.
async fn download_newest(client_id: i32, kind: Kind) -> Result<(), String> {
    let User::User(me) = f::get_me(client_id).await.map_err(td_err("getMe"))?;
    let Chat::Chat(saved) = f::create_private_chat(me.id, false, client_id)
        .await
        .map_err(td_err("createPrivateChat"))?;
    let messages = recent_messages(saved.id, client_id).await?;

    let file = match kind {
        Kind::Photo => messages.iter().find_map(|m| match &m.content {
            MessageContent::MessagePhoto(p) => p.photo.sizes.last().map(|s| s.photo.clone()),
            _ => None,
        }),
        Kind::LargestDocument => messages
            .iter()
            .filter_map(|m| match &m.content {
                MessageContent::MessageDocument(d) => Some(d.document.document.clone()),
                _ => None,
            })
            .max_by_key(|f| f.size.max(f.expected_size)),
    }
    .ok_or("no matching file in the last 100 Saved Messages")?;

    let _ = f::delete_file(file.id, client_id).await;
    let start = Instant::now();
    let tdlib_rs::enums::File::File(done) = f::download_file(file.id, 32, 0, 0, true, client_id)
        .await
        .map_err(td_err("downloadFile"))?;
    let secs = start.elapsed().as_secs_f64();
    let bytes = done.local.downloaded_size.max(done.size);
    let mb = bytes as f64 / (1024.0 * 1024.0);
    println!(
        "downloaded {:.2} MB in {} ms ({:.2} MB/s), complete: {}",
        mb,
        (secs * 1000.0) as u128,
        if secs > 0.0 { mb / secs } else { 0.0 },
        done.local.is_downloading_completed
    );
    Ok(())
}

async fn recent_messages(chat_id: i64, client_id: i32) -> Result<Vec<types::Message>, String> {
    let mut out = Vec::new();
    let mut from = 0;
    while out.len() < HISTORY_SCAN {
        let want = (HISTORY_SCAN - out.len()) as i32;
        let Messages::Messages(batch) =
            f::get_chat_history(chat_id, from, 0, want, false, client_id)
                .await
                .map_err(td_err("getChatHistory"))?;
        let batch: Vec<_> = batch.messages.into_iter().flatten().collect();
        let Some(last) = batch.last() else { break };
        from = last.id;
        out.extend(batch);
    }
    Ok(out)
}
