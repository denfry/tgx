//! App core: owns the WS relay and the TDLib client, turns TDLib updates into
//! plain view data and pushes it to the UI through a callback. Runs on tokio.
//!
//! Logs never contain message text, phone numbers, codes or passwords (spec §5.6).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use chrono::{Local, TimeZone};
use tdlib_rs::enums::{
    AuthorizationState, ChatList as TdChatList, ChatType, ConnectionState, LogStream,
    MessageContent, MessageSender, MessageSendingState, OptionValue, ProxyType, Update,
};
use tdlib_rs::functions as f;
use tdlib_rs::types::{
    self, FormattedText, InputMessageText, LogStreamFile, Proxy, ProxyTypeMtproto,
};
use tgx_transport::{RelayEvent, RelayHandle, WsRoutes, WssDialer, start_relay};
use tokio::sync::mpsc;

use crate::model::{
    self, ChatEntry, ChatKind, ChatList, HistMsg, LastMsg, Row, SendState, avatar_tone, initials,
};

const LOG_MAX_BYTES: i64 = 10 * 1024 * 1024;
const FLUSH_EVERY: Duration = Duration::from_millis(60);
const HISTORY_TARGET: usize = 60;
const SAVED_TITLE: &str = "Избранное";

pub struct Config {
    pub api_id: i32,
    pub api_hash: String,
    pub data_dir: PathBuf,
}

/// Commands from the UI.
pub enum Cmd {
    /// `step` is the screen the value was entered on: phone | code | password.
    Login { step: String, value: String },
    OpenChat(i64),
    Send(String),
    Quit(tokio::sync::oneshot::Sender<()>),
}

#[derive(Clone, Debug)]
pub struct ChatRowData {
    pub key: String,
    pub title: String,
    pub prefix: String,
    pub preview: String,
    pub time: String,
    pub unread: i32,
    pub muted: bool,
    pub pinned: bool,
    pub ini: String,
    pub ava: i32,
    pub saved: bool,
}

#[derive(Clone, Debug)]
pub struct MsgRowData {
    pub kind: i32,
    pub out: bool,
    pub text: String,
    pub time: String,
    pub sender: String,
    pub sender_ini: String,
    pub sender_ava: i32,
    pub first: bool,
    pub last: bool,
    pub state: i32,
}

#[derive(Clone, Debug)]
pub struct ChatHeader {
    pub title: String,
    pub subtitle: String,
    pub ini: String,
    pub ava: i32,
    pub saved: bool,
    pub group: bool,
}

/// Everything the core tells the UI.
pub enum UiMsg {
    Screen {
        screen: &'static str,
        error: String,
        busy: bool,
        hint: String,
    },
    Fatal(String),
    Conn { state: &'static str, level: i32 },
    RouteNote(String),
    Chats(Vec<ChatRowData>),
    OpenChat(ChatHeader),
    Messages { rows: Vec<MsgRowData>, jump: bool },
}

pub type UiSink = Arc<dyn Fn(UiMsg) + Send + Sync>;

/// Results of TDLib calls made off the main loop.
enum Internal {
    LoginFailed(String),
    History { chat_id: i64, msgs: Vec<types::Message> },
}

struct Core {
    client_id: i32,
    ui: UiSink,
    cfg: Arc<Config>,
    relay: RelayHandle,
    internal: mpsc::UnboundedSender<Internal>,
    screen: &'static str,
    password_hint: String,
    my_id: i64,
    users: HashMap<i64, String>,
    chats: ChatList,
    open: Option<i64>,
    history: Vec<HistMsg>,
    chats_dirty: bool,
    history_dirty: bool,
    jump: bool,
}

pub async fn run(cfg: Config, mut cmds: mpsc::UnboundedReceiver<Cmd>, ui: UiSink) {
    let mut relay = match start_relay(WssDialer::new(WsRoutes::default())).await {
        Ok(r) => r,
        Err(e) => {
            ui(UiMsg::Fatal(format!("Не удалось запустить встроенный транспорт: {e}")));
            return;
        }
    };
    let mut relay_events = std::mem::replace(&mut relay.events, mpsc::channel(1).1);

    let client_id = tdlib_rs::create_client();
    let stop = Arc::new(AtomicBool::new(false));
    let mut updates = spawn_receiver(stop.clone());
    if let Err(e) = configure_logging(client_id, &cfg.data_dir).await {
        eprintln!("tdlib logging: {e}");
    }

    let (internal, mut internal_rx) = mpsc::unbounded_channel();
    let mut core = Core {
        client_id,
        ui,
        cfg: Arc::new(cfg),
        relay,
        internal,
        screen: "loading",
        password_hint: String::new(),
        my_id: 0,
        users: HashMap::new(),
        chats: ChatList::default(),
        open: None,
        history: Vec::new(),
        chats_dirty: false,
        history_dirty: false,
        jump: false,
    };
    core.show_screen("loading", String::new(), false);

    let mut tick = tokio::time::interval(FLUSH_EVERY);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let quit = loop {
        tokio::select! {
            Some(u) = updates.recv() => core.on_update(u),
            Some(i) = internal_rx.recv() => core.on_internal(i),
            Some(e) = relay_events.recv() => core.on_relay(e),
            cmd = cmds.recv() => match cmd {
                Some(Cmd::Quit(done)) => break Some(done),
                Some(c) => core.on_cmd(c),
                None => break None,
            },
            _ = tick.tick() => core.flush(),
        }
    };

    let _ = tokio::time::timeout(Duration::from_secs(3), f::close(client_id)).await;
    stop.store(true, Ordering::Relaxed);
    core.relay.shutdown().await;
    if let Some(done) = quit {
        let _ = done.send(());
    }
}

/// `tdlib_rs::receive` must be polled continuously: it also delivers the
/// responses awaited by `functions::*`.
fn spawn_receiver(stop: Arc<AtomicBool>) -> mpsc::UnboundedReceiver<Update> {
    let (tx, rx) = mpsc::unbounded_channel();
    std::thread::Builder::new()
        .name("tdlib-receive".into())
        .spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                if let Some((update, _client)) = tdlib_rs::receive()
                    && tx.send(update).is_err()
                {
                    break;
                }
            }
        })
        .expect("spawn tdlib receive thread");
    rx
}

async fn configure_logging(client_id: i32, dir: &std::path::Path) -> Result<(), String> {
    f::set_log_verbosity_level(1, client_id)
        .await
        .map_err(|e| e.message)?;
    let log = LogStreamFile {
        path: dir.join("td.log").to_string_lossy().into_owned(),
        max_file_size: LOG_MAX_BYTES,
        redirect_stderr: false,
    };
    f::set_log_stream(LogStream::File(log), client_id)
        .await
        .map_err(|e| e.message)
}

impl Core {
    fn show_screen(&mut self, screen: &'static str, error: String, busy: bool) {
        self.screen = screen;
        (self.ui)(UiMsg::Screen {
            screen,
            error,
            busy,
            hint: self.password_hint.clone(),
        });
    }

    fn on_relay(&mut self, e: RelayEvent) {
        match e {
            RelayEvent::Connected(_) => (self.ui)(UiMsg::RouteNote(String::new())),
            RelayEvent::DcUnreachable { target, error } => {
                eprintln!("relay: DC{} unreachable: {error}", target.dc);
                (self.ui)(UiMsg::RouteNote(format!("DC{} недоступен через WS", target.dc)));
            }
            RelayEvent::BadHandshake => eprintln!("relay: bad handshake rejected"),
        }
    }

    fn on_cmd(&mut self, cmd: Cmd) {
        let id = self.client_id;
        match cmd {
            Cmd::Login { step, value } => {
                let value = value.trim().to_string();
                if value.is_empty() {
                    return;
                }
                let screen = self.screen;
                self.show_screen(screen, String::new(), true);
                let tx = self.internal.clone();
                tokio::spawn(async move {
                    let r = match step.as_str() {
                        "phone" => f::set_authentication_phone_number(value, None, id).await,
                        "code" => f::check_authentication_code(value, id).await,
                        "password" => f::check_authentication_password(value, id).await,
                        _ => return,
                    };
                    if let Err(e) = r {
                        let _ = tx.send(Internal::LoginFailed(login_error(&e)));
                    }
                });
            }
            Cmd::OpenChat(chat_id) => self.open_chat(chat_id),
            Cmd::Send(text) => {
                let Some(chat_id) = self.open else { return };
                let text = text.trim().to_string();
                if text.is_empty() {
                    return;
                }
                tokio::spawn(async move {
                    let content = tdlib_rs::enums::InputMessageContent::InputMessageText(
                        InputMessageText {
                            text: FormattedText {
                                text,
                                entities: Vec::new(),
                            },
                            link_preview_options: None,
                            clear_draft: true,
                        },
                    );
                    if let Err(e) = f::send_message(chat_id, None, None, None, content, id).await {
                        eprintln!("sendMessage failed: {} {}", e.code, e.message);
                    }
                });
            }
            Cmd::Quit(_) => unreachable!("handled by the run loop"),
        }
    }

    fn on_internal(&mut self, i: Internal) {
        match i {
            Internal::LoginFailed(msg) => {
                let screen = self.screen;
                self.show_screen(screen, msg, false);
            }
            Internal::History { chat_id, msgs } => {
                if self.open != Some(chat_id) {
                    return;
                }
                let mut hist: Vec<HistMsg> = msgs.iter().map(|m| self.hist_msg(m)).collect();
                hist.sort_by_key(|m| m.id);
                hist.dedup_by_key(|m| m.id);
                // Keep messages that arrived while the history was loading.
                for m in self.history.drain(..) {
                    if hist.binary_search_by_key(&m.id, |h| h.id).is_err() {
                        hist.push(m);
                    }
                }
                hist.sort_by_key(|m| m.id);
                self.history = hist;
                self.history_dirty = true;
                self.jump = true;
                self.mark_read(self.history.iter().filter(|m| !m.outgoing).map(|m| m.id).collect());
            }
        }
    }

    fn open_chat(&mut self, chat_id: i64) {
        let id = self.client_id;
        if let Some(prev) = self.open.replace(chat_id)
            && prev != chat_id
        {
            tokio::spawn(async move {
                let _ = f::close_chat(prev, id).await;
            });
        }
        self.history.clear();
        self.history_dirty = true;
        self.jump = true;
        if let Some(c) = self.chats.get(chat_id) {
            (self.ui)(UiMsg::OpenChat(ChatHeader {
                title: c.title.clone(),
                subtitle: match c.kind {
                    ChatKind::Saved => "сообщения себе",
                    ChatKind::Private => "личный чат",
                    ChatKind::Group => "группа",
                    ChatKind::Channel => "канал",
                }
                .into(),
                ini: initials(&c.title),
                ava: avatar_tone(c.id),
                saved: c.kind == ChatKind::Saved,
                group: c.kind == ChatKind::Group,
            }));
        }
        let tx = self.internal.clone();
        tokio::spawn(async move {
            let _ = f::open_chat(chat_id, id).await;
            let msgs = load_history(chat_id, id).await;
            let _ = tx.send(Internal::History { chat_id, msgs });
        });
    }

    fn mark_read(&self, ids: Vec<i64>) {
        let (Some(chat_id), false) = (self.open, ids.is_empty()) else {
            return;
        };
        let id = self.client_id;
        tokio::spawn(async move {
            let _ = f::view_messages(chat_id, ids, None, true, id).await;
        });
    }

    fn on_update(&mut self, u: Update) {
        match u {
            Update::AuthorizationState(s) => self.on_auth(s.authorization_state),
            Update::ConnectionState(c) => {
                let (state, level) = match c.state {
                    ConnectionState::WaitingForNetwork => ("нет сети", 2),
                    ConnectionState::ConnectingToProxy => ("подключение к транспорту", 1),
                    ConnectionState::Connecting => ("подключение", 1),
                    ConnectionState::Updating => ("обновление", 1),
                    ConnectionState::Ready => ("готово", 0),
                };
                (self.ui)(UiMsg::Conn { state, level });
            }
            Update::Option(o) => {
                if o.name == "my_id"
                    && let OptionValue::Integer(v) = o.value
                {
                    self.my_id = v.value;
                    let me = v.value;
                    self.chats.update(me, |c| {
                        c.kind = ChatKind::Saved;
                        c.title = SAVED_TITLE.into();
                    });
                    self.chats_dirty = true;
                }
            }
            Update::User(u) => {
                let u = u.user;
                let name = format!("{} {}", u.first_name, u.last_name).trim().to_string();
                self.users.insert(u.id, name);
            }
            Update::NewChat(n) => {
                let c = n.chat;
                let kind = match &c.r#type {
                    ChatType::Private(p) if p.user_id == self.my_id && self.my_id != 0 => {
                        ChatKind::Saved
                    }
                    ChatType::Private(_) | ChatType::Secret(_) => ChatKind::Private,
                    ChatType::BasicGroup(_) => ChatKind::Group,
                    ChatType::Supergroup(s) if s.is_channel => ChatKind::Channel,
                    ChatType::Supergroup(_) => ChatKind::Group,
                };
                let (order, pinned) = main_position(&c.positions);
                let last = c.last_message.as_ref().map(|m| self.last_msg(m, kind));
                self.chats.insert(ChatEntry {
                    id: c.id,
                    title: if kind == ChatKind::Saved { SAVED_TITLE.into() } else { c.title },
                    kind,
                    order,
                    pinned,
                    last,
                    unread: c.unread_count,
                    muted: c.notification_settings.mute_for > 0,
                    last_read_outbox: c.last_read_outbox_message_id,
                });
                self.chats_dirty = true;
            }
            Update::ChatTitle(t) => {
                let saved = self.chats.get(t.chat_id).is_some_and(|c| c.kind == ChatKind::Saved);
                if !saved {
                    self.chats_dirty |= self.chats.update(t.chat_id, |c| c.title = t.title);
                }
            }
            Update::ChatLastMessage(l) => {
                let kind = self.chats.get(l.chat_id).map(|c| c.kind);
                let last = kind.and_then(|k| l.last_message.as_ref().map(|m| self.last_msg(m, k)));
                let pos = (!l.positions.is_empty()).then(|| main_position(&l.positions));
                self.chats_dirty |= self.chats.update(l.chat_id, |c| {
                    c.last = last;
                    if let Some((order, pinned)) = pos {
                        c.order = order;
                        c.pinned = pinned;
                    }
                });
            }
            Update::ChatPosition(p) => {
                if matches!(p.position.list, TdChatList::Main) {
                    let (order, pinned) = (p.position.order, p.position.is_pinned);
                    self.chats_dirty |= self.chats.update(p.chat_id, |c| {
                        c.order = order;
                        c.pinned = pinned;
                    });
                }
            }
            Update::ChatReadInbox(r) => {
                self.chats_dirty |= self.chats.update(r.chat_id, |c| c.unread = r.unread_count);
            }
            Update::ChatReadOutbox(r) => {
                self.chats_dirty |= self
                    .chats
                    .update(r.chat_id, |c| c.last_read_outbox = r.last_read_outbox_message_id);
                self.history_dirty |= self.open == Some(r.chat_id);
            }
            Update::ChatNotificationSettings(n) => {
                let muted = n.notification_settings.mute_for > 0;
                self.chats_dirty |= self.chats.update(n.chat_id, |c| c.muted = muted);
            }
            Update::NewMessage(n) => {
                let m = n.message;
                if self.open == Some(m.chat_id) {
                    let h = self.hist_msg(&m);
                    let incoming = !h.outgoing;
                    if let Err(pos) = self.history.binary_search_by_key(&h.id, |x| x.id) {
                        self.history.insert(pos, h);
                        self.history_dirty = true;
                    }
                    if incoming {
                        self.mark_read(vec![m.id]);
                    }
                }
            }
            Update::MessageSendSucceeded(s) => {
                if self.open == Some(s.message.chat_id) {
                    self.history.retain(|h| h.id != s.old_message_id);
                    let h = self.hist_msg(&s.message);
                    if let Err(pos) = self.history.binary_search_by_key(&h.id, |x| x.id) {
                        self.history.insert(pos, h);
                    }
                    self.history_dirty = true;
                }
            }
            Update::MessageSendFailed(s) => {
                eprintln!("message send failed: {} {}", s.error.code, s.error.message);
                if self.open == Some(s.message.chat_id) {
                    for h in self.history.iter_mut().filter(|h| h.id == s.old_message_id) {
                        h.id = s.message.id;
                        h.state = SendState::Failed;
                    }
                    self.history.sort_by_key(|m| m.id);
                    self.history_dirty = true;
                }
            }
            Update::MessageContent(c) => {
                if self.open == Some(c.chat_id) {
                    let text = content_text(&c.new_content);
                    for h in self.history.iter_mut().filter(|h| h.id == c.message_id) {
                        h.text = text.clone();
                        self.history_dirty = true;
                    }
                }
            }
            Update::DeleteMessages(d) if d.is_permanent && self.open == Some(d.chat_id) => {
                let before = self.history.len();
                self.history.retain(|h| !d.message_ids.contains(&h.id));
                self.history_dirty |= before != self.history.len();
            }
            _ => {}
        }
    }

    fn on_auth(&mut self, state: AuthorizationState) {
        match state {
            AuthorizationState::WaitTdlibParameters => self.send_parameters(),
            AuthorizationState::WaitPhoneNumber => self.show_screen("phone", String::new(), false),
            AuthorizationState::WaitCode(_) => self.show_screen("code", String::new(), false),
            AuthorizationState::WaitPassword(p) => {
                self.password_hint = p.password_hint;
                self.show_screen("password", String::new(), false);
            }
            AuthorizationState::Ready => {
                self.show_screen("main", String::new(), false);
                let id = self.client_id;
                tokio::spawn(async move {
                    match f::load_chats(Some(TdChatList::Main), 100, id).await {
                        Ok(()) => {}
                        Err(e) if e.code == 404 => {} // everything is already loaded
                        Err(e) => eprintln!("loadChats: {} {}", e.code, e.message),
                    }
                });
            }
            AuthorizationState::LoggingOut | AuthorizationState::Closing => {}
            AuthorizationState::Closed => {
                (self.ui)(UiMsg::Fatal("Сеанс TDLib закрыт. Перезапустите tgx.".into()))
            }
            _ => (self.ui)(UiMsg::Fatal(
                "Этот шаг входа tgx пока не поддерживает (регистрация или вход по почте). \
                 Войдите в аккаунт один раз через официальное приложение и повторите."
                    .into(),
            )),
        }
    }

    /// Points TDLib at the local relay, then sends the parameters. Proxy
    /// requests are queued until `setTdlibParameters`, so `addProxy` is sent
    /// first and applied first on initialisation (spike finding).
    fn send_parameters(&self) {
        let id = self.client_id;
        let cfg = self.cfg.clone();
        let proxy = Proxy {
            server: "127.0.0.1".into(),
            port: i32::from(self.relay.addr.port()),
            r#type: ProxyType::Mtproto(ProxyTypeMtproto {
                secret: self.relay.tdlib_secret(),
            }),
        };
        let ui = self.ui.clone();
        tokio::spawn(async move {
            let dir = cfg.data_dir.to_string_lossy().into_owned();
            // join! polls in order, so addProxy is sent before the parameters.
            let (added, params) = tokio::join!(
                f::add_proxy(proxy, true, id),
                f::set_tdlib_parameters(
                    false,
                    dir.clone(),
                    dir,
                    String::new(),
                    true,
                    true,
                    true,
                    false,
                    cfg.api_id,
                    cfg.api_hash.clone(),
                    "ru".into(),
                    "Desktop".into(),
                    String::new(),
                    env!("CARGO_PKG_VERSION").into(),
                    id,
                )
            );
            if let Err(e) = params {
                ui(UiMsg::Fatal(format!("TDLib не принял параметры: {}", e.message)));
                return;
            }
            match added {
                // The relay port changes every run: drop proxies of earlier runs.
                Ok(tdlib_rs::enums::AddedProxy::AddedProxy(p)) => remove_other_proxies(p.id, id).await,
                Err(e) => ui(UiMsg::Fatal(format!("TDLib не принял встроенный транспорт: {}", e.message))),
            }
        });
    }

    fn sender(&self, s: &MessageSender) -> (i64, String) {
        match s {
            MessageSender::User(u) => (u.user_id, self.users.get(&u.user_id).cloned().unwrap_or_default()),
            MessageSender::Chat(c) => (
                c.chat_id,
                self.chats.get(c.chat_id).map(|c| c.title.clone()).unwrap_or_default(),
            ),
        }
    }

    fn last_msg(&self, m: &types::Message, kind: ChatKind) -> LastMsg {
        let sender = match kind {
            ChatKind::Group if m.is_outgoing => Some("Вы".to_string()),
            ChatKind::Group => {
                let (_, name) = self.sender(&m.sender_id);
                (!name.is_empty()).then(|| name.split_whitespace().next().unwrap_or("").to_string())
            }
            _ => None,
        };
        LastMsg {
            text: content_text(&m.content),
            date: i64::from(m.date),
            outgoing: m.is_outgoing,
            sender,
        }
    }

    fn hist_msg(&self, m: &types::Message) -> HistMsg {
        let (sender_key, sender_name) = self.sender(&m.sender_id);
        HistMsg {
            id: m.id,
            date: i64::from(m.date),
            outgoing: m.is_outgoing,
            sender_key,
            sender_name,
            text: content_text(&m.content),
            state: match &m.sending_state {
                Some(MessageSendingState::Pending(_)) => SendState::Pending,
                Some(MessageSendingState::Failed(_)) => SendState::Failed,
                None => SendState::None,
            },
        }
    }

    fn flush(&mut self) {
        let now = Local::now();
        if self.chats_dirty {
            self.chats_dirty = false;
            let rows = self
                .chats
                .ordered()
                .into_iter()
                .map(|c| {
                    let (prefix, preview, time) = match &c.last {
                        Some(l) => (
                            l.sender.clone().unwrap_or_default(),
                            one_line(&l.text),
                            Local
                                .timestamp_opt(l.date, 0)
                                .single()
                                .map(|t| model::list_time(&t, &now))
                                .unwrap_or_default(),
                        ),
                        None => Default::default(),
                    };
                    ChatRowData {
                        key: c.id.to_string(),
                        title: c.title.clone(),
                        prefix,
                        preview,
                        time,
                        unread: c.unread,
                        muted: c.muted,
                        pinned: c.pinned,
                        ini: initials(&c.title),
                        ava: avatar_tone(c.id),
                        saved: c.kind == ChatKind::Saved,
                    }
                })
                .collect();
            (self.ui)(UiMsg::Chats(rows));
        }
        if self.history_dirty {
            self.history_dirty = false;
            let read_upto = self
                .open
                .and_then(|id| self.chats.get(id))
                .map_or(0, |c| c.last_read_outbox);
            let rows = model::history_rows(&self.history, &Local, &now)
                .into_iter()
                .map(|r| match r {
                    Row::Day(d) => MsgRowData {
                        kind: 1,
                        out: false,
                        text: d,
                        time: String::new(),
                        sender: String::new(),
                        sender_ini: String::new(),
                        sender_ava: 0,
                        first: false,
                        last: false,
                        state: 0,
                    },
                    Row::Msg { msg, first, last } => MsgRowData {
                        kind: 0,
                        out: msg.outgoing,
                        time: Local
                            .timestamp_opt(msg.date, 0)
                            .single()
                            .map(|t| t.format("%H:%M").to_string())
                            .unwrap_or_default(),
                        sender_ava: avatar_tone(msg.sender_key),
                        sender_ini: initials(&msg.sender_name),
                        sender: msg.sender_name,
                        text: msg.text,
                        first,
                        last,
                        state: match (msg.outgoing, msg.state) {
                            (false, _) => 0,
                            (true, SendState::Pending) => 1,
                            (true, SendState::Failed) => 4,
                            (true, _) if msg.id <= read_upto => 3,
                            (true, _) => 2,
                        },
                    },
                })
                .collect();
            (self.ui)(UiMsg::Messages {
                rows,
                jump: std::mem::take(&mut self.jump),
            });
        }
    }
}

async fn load_history(chat_id: i64, id: i32) -> Vec<types::Message> {
    let mut out: Vec<types::Message> = Vec::new();
    let mut from = 0;
    // TDLib may answer the first request with only what it has locally, so
    // keep asking from the oldest message until the target or the start.
    for _ in 0..6 {
        let want = (HISTORY_TARGET - out.len().min(HISTORY_TARGET)) as i32;
        if want == 0 {
            break;
        }
        match f::get_chat_history(chat_id, from, 0, want.max(10), false, id).await {
            Ok(tdlib_rs::enums::Messages::Messages(b)) => {
                let batch: Vec<_> = b.messages.into_iter().flatten().collect();
                let Some(oldest) = batch.last() else { break };
                from = oldest.id;
                out.extend(batch);
            }
            Err(e) => {
                eprintln!("getChatHistory: {} {}", e.code, e.message);
                break;
            }
        }
    }
    out
}

async fn remove_other_proxies(keep: i32, id: i32) {
    if let Ok(tdlib_rs::enums::AddedProxies::AddedProxies(list)) = f::get_proxies(id).await {
        for p in list.proxies.iter().filter(|p| p.id != keep) {
            let _ = f::remove_proxy(p.id, id).await;
        }
    }
}

fn main_position(positions: &[types::ChatPosition]) -> (i64, bool) {
    positions
        .iter()
        .find(|p| matches!(p.list, TdChatList::Main))
        .map_or((0, false), |p| (p.order, p.is_pinned))
}

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Text shown for a message; media gets a short label plus its caption.
pub fn content_text(c: &MessageContent) -> String {
    let with = |label: &str, caption: &FormattedText| {
        if caption.text.is_empty() {
            label.to_string()
        } else {
            format!("{label}, {}", caption.text)
        }
    };
    match c {
        MessageContent::MessageText(t) => t.text.text.clone(),
        MessageContent::MessagePhoto(p) => with("Фото", &p.caption),
        MessageContent::MessageVideo(v) => with("Видео", &v.caption),
        MessageContent::MessageDocument(d) => with(&format!("Файл {}", d.document.file_name), &d.caption),
        MessageContent::MessageAnimation(a) => with("GIF", &a.caption),
        MessageContent::MessageAudio(a) => with("Аудио", &a.caption),
        MessageContent::MessageVoiceNote(v) => with("Голосовое сообщение", &v.caption),
        MessageContent::MessageVideoNote(_) => "Видеосообщение".into(),
        MessageContent::MessageSticker(s) => format!("{} Стикер", s.sticker.emoji),
        MessageContent::MessagePoll(_) => "Опрос".into(),
        MessageContent::MessageContact(_) => "Контакт".into(),
        MessageContent::MessageLocation(_) => "Геопозиция".into(),
        MessageContent::MessageCall(_) => "Звонок".into(),
        _ => "Сообщение".into(),
    }
}

/// User-facing login error; TDLib's text is technical and in English.
fn login_error(e: &types::Error) -> String {
    let m = e.message.as_str();
    if m.contains("PHONE_NUMBER_INVALID") {
        "Номер не подходит. Проверьте код страны, например +7.".into()
    } else if m.contains("PHONE_CODE_INVALID") || m.contains("PHONE_CODE_EMPTY") {
        "Неверный код. Проверьте и введите ещё раз.".into()
    } else if m.contains("PHONE_CODE_EXPIRED") {
        "Код устарел. Запросите новый, начав вход заново.".into()
    } else if m.contains("PASSWORD_HASH_INVALID") {
        "Неверный пароль.".into()
    } else if m.contains("FLOOD_WAIT") || e.code == 429 {
        "Слишком много попыток. Подождите и попробуйте позже.".into()
    } else if m.contains("PHONE_NUMBER_BANNED") {
        "Этот номер заблокирован в Telegram.".into()
    } else {
        format!("Telegram ответил ошибкой: {m}")
    }
}
