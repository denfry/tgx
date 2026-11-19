// No console window in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod core;
mod model;

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use tokio::sync::mpsc;

use crate::core::{Cmd, Config, UiMsg};

slint::include_modules!();

fn main() -> Result<(), slint::PlatformError> {
    let app = AppWindow::new()?;
    let chats = Rc::new(VecModel::<ChatRow>::default());
    let messages = Rc::new(VecModel::<Msg>::default());
    app.set_chats(ModelRc::from(chats.clone()));
    app.set_messages(ModelRc::from(messages.clone()));

    let (tx, rx) = mpsc::unbounded_channel::<Cmd>();
    match config() {
        Ok(cfg) => start_core(&app, cfg, rx),
        Err(e) => {
            app.set_fatal_error(e.into());
            app.set_screen("fatal".into());
        }
    }

    {
        let tx = tx.clone();
        app.on_submit_login(move |step, value| {
            let _ = tx.send(Cmd::Login {
                step: step.into(),
                value: value.into(),
            });
        });
    }
    {
        let tx = tx.clone();
        app.on_open_chat(move |key| {
            if let Ok(id) = key.parse() {
                let _ = tx.send(Cmd::OpenChat(id));
            }
        });
    }
    {
        let tx = tx.clone();
        app.on_send(move |text| {
            let _ = tx.send(Cmd::Send(text.into()));
        });
    }

    app.run()?;

    // Let TDLib close its database cleanly before the process exits.
    let (done_tx, done_rx) = tokio::sync::oneshot::channel();
    if tx.send(Cmd::Quit(done_tx)).is_ok() {
        let _ = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread().enable_time().build();
            if let Ok(rt) = rt {
                rt.block_on(async {
                    let _ = tokio::time::timeout(Duration::from_secs(4), done_rx).await;
                });
            }
        })
        .join();
    }
    Ok(())
}

fn config() -> Result<Config, String> {
    let api_id = option_env!("TGX_API_ID")
        .map(str::to_string)
        .or_else(|| std::env::var("TGX_API_ID").ok())
        .ok_or("В этой сборке нет api_id. Задайте TGX_API_ID и TGX_API_HASH (my.telegram.org).")?
        .trim()
        .parse::<i32>()
        .map_err(|_| "TGX_API_ID должен быть числом.")?;
    let api_hash = option_env!("TGX_API_HASH")
        .map(str::to_string)
        .or_else(|| std::env::var("TGX_API_HASH").ok())
        .ok_or("В этой сборке нет api_hash. Задайте TGX_API_HASH.")?;
    // TGX_DATA_DIR keeps development and test sessions apart from the real one.
    let data_dir = match std::env::var_os("TGX_DATA_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => {
            let base = std::env::var_os("LOCALAPPDATA").ok_or("Не задана переменная LOCALAPPDATA.")?;
            PathBuf::from(base).join("tgx").join("td")
        }
    };
    std::fs::create_dir_all(&data_dir)
        .map_err(|e| format!("Не удалось создать {}: {e}", data_dir.display()))?;
    Ok(Config {
        api_id,
        api_hash,
        data_dir,
    })
}

fn start_core(app: &AppWindow, cfg: Config, rx: mpsc::UnboundedReceiver<Cmd>) {
    let weak = app.as_weak();
    let sink: core::UiSink = Arc::new(move |msg: UiMsg| {
        let _ = weak.upgrade_in_event_loop(move |app| apply(&app, msg));
    });
    std::thread::Builder::new()
        .name("tgx-core".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .expect("tokio runtime");
            rt.block_on(core::run(cfg, rx, sink));
        })
        .expect("spawn core thread");
}

fn apply(app: &AppWindow, msg: UiMsg) {
    match msg {
        UiMsg::Screen {
            screen,
            error,
            busy,
            hint,
        } => {
            app.set_screen(screen.into());
            app.set_login_error(error.into());
            app.set_login_busy(busy);
            app.set_password_hint(hint.into());
        }
        UiMsg::Fatal(e) => {
            app.set_fatal_error(e.into());
            app.set_screen("fatal".into());
        }
        UiMsg::Conn { state, level } => {
            app.set_conn_state(state.into());
            app.set_conn_level(level);
        }
        UiMsg::RouteNote(n) => app.set_route_note(n.into()),
        UiMsg::Chats(rows) => {
            let rows: Vec<ChatRow> = rows
                .into_iter()
                .map(|r| ChatRow {
                    key: r.key.into(),
                    title: r.title.into(),
                    prefix: r.prefix.into(),
                    preview: r.preview.into(),
                    time: r.time.into(),
                    unread: r.unread,
                    muted: r.muted,
                    pinned: r.pinned,
                    ini: r.ini.into(),
                    ava: r.ava,
                    saved: r.saved,
                })
                .collect();
            set_rows(&app.get_chats(), rows);
        }
        UiMsg::OpenChat(h) => {
            app.set_chat_title(h.title.into());
            app.set_chat_subtitle(h.subtitle.into());
            app.set_chat_ini(h.ini.into());
            app.set_chat_ava(h.ava);
            app.set_chat_saved(h.saved);
            app.set_chat_group(h.group);
        }
        UiMsg::Messages { rows, jump } => {
            let rows: Vec<Msg> = rows
                .into_iter()
                .map(|r| Msg {
                    kind: r.kind,
                    out: r.out,
                    text: r.text.into(),
                    time: r.time.into(),
                    sender: SharedString::from(r.sender),
                    sender_ini: r.sender_ini.into(),
                    sender_ava: r.sender_ava,
                    first: r.first,
                    last: r.last,
                    state: r.state,
                })
                .collect();
            set_rows(&app.get_messages(), rows);
            if jump {
                app.set_scroll_token(app.get_scroll_token().wrapping_add(1));
            }
        }
    }
}

/// Replaces the rows of a `VecModel` in place, so Slint reuses item instances.
fn set_rows<T: Clone + 'static>(model: &ModelRc<T>, rows: Vec<T>) {
    if let Some(vm) = model.as_any().downcast_ref::<VecModel<T>>() {
        vm.set_vec(rows);
    }
}
