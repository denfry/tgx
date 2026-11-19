//! App-side state derived from TDLib updates, kept free of TDLib and Slint types
//! so it can be tested directly. TDLib stays the source of truth (spec §5.2).

use std::collections::HashMap;

use chrono::{DateTime, Datelike, TimeZone};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChatKind {
    Private,
    Group,
    Channel,
    Saved,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LastMsg {
    pub text: String,
    pub date: i64,
    pub outgoing: bool,
    /// Sender name shown before the preview in groups ("Оля: ...").
    pub sender: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ChatEntry {
    pub id: i64,
    pub title: String,
    pub kind: ChatKind,
    /// Position in the main list; 0 means the chat is not in it.
    pub order: i64,
    pub pinned: bool,
    pub last: Option<LastMsg>,
    pub unread: i32,
    pub muted: bool,
    pub last_read_outbox: i64,
}

#[derive(Default)]
pub struct ChatList {
    chats: HashMap<i64, ChatEntry>,
}

impl ChatList {
    pub fn insert(&mut self, chat: ChatEntry) {
        self.chats.insert(chat.id, chat);
    }

    pub fn get(&self, id: i64) -> Option<&ChatEntry> {
        self.chats.get(&id)
    }

    /// Applies `f` to a known chat. Updates for chats TDLib has not announced
    /// with `updateNewChat` are ignored: TDLib always sends that first.
    pub fn update(&mut self, id: i64, f: impl FnOnce(&mut ChatEntry)) -> bool {
        match self.chats.get_mut(&id) {
            Some(c) => {
                f(c);
                true
            }
            None => false,
        }
    }

    /// Chats of the main list, in TDLib order (descending `order`).
    pub fn ordered(&self) -> Vec<&ChatEntry> {
        let mut v: Vec<_> = self.chats.values().filter(|c| c.order != 0).collect();
        v.sort_by(|a, b| b.order.cmp(&a.order).then(b.id.cmp(&a.id)));
        v
    }
}

/// Up to two initials from a title, letters and digits only.
pub fn initials(title: &str) -> String {
    let mut out = String::new();
    for word in title.split_whitespace() {
        if let Some(c) = word.chars().find(|c| c.is_alphanumeric()) {
            out.extend(c.to_uppercase());
            if out.chars().count() == 2 {
                break;
            }
        }
    }
    if out.is_empty() { "?".into() } else { out }
}

pub const AVATAR_TONES: i64 = 11;

/// Stable avatar tone for an id (chat or user), like Telegram's colour by id.
pub fn avatar_tone(id: i64) -> i32 {
    id.unsigned_abs().rem_euclid(AVATAR_TONES as u64) as i32
}

/// Chat-list time: "14:32" today, "вчера", a weekday within a week, else a date.
pub fn list_time<Tz: TimeZone>(at: &DateTime<Tz>, now: &DateTime<Tz>) -> String
where
    Tz::Offset: std::fmt::Display,
{
    let (d, n) = (at.date_naive(), now.date_naive());
    let days = (n - d).num_days();
    match days {
        ..=0 => at.format("%H:%M").to_string(),
        1 => "вчера".into(),
        2..=6 => ["пн", "вт", "ср", "чт", "пт", "сб", "вс"]
            [at.weekday().num_days_from_monday() as usize]
            .into(),
        _ if d.year() == n.year() => at.format("%d.%m").to_string(),
        _ => at.format("%d.%m.%y").to_string(),
    }
}

/// Date chip in the history: "Сегодня", "Вчера", "5 марта", "5 марта 2024".
pub fn day_chip<Tz: TimeZone>(at: &DateTime<Tz>, now: &DateTime<Tz>) -> String {
    const MONTHS: [&str; 12] = [
        "января", "февраля", "марта", "апреля", "мая", "июня", "июля", "августа",
        "сентября", "октября", "ноября", "декабря",
    ];
    let (d, n) = (at.date_naive(), now.date_naive());
    match (n - d).num_days() {
        0 => "Сегодня".into(),
        1 => "Вчера".into(),
        _ => {
            let m = MONTHS[d.month0() as usize];
            if d.year() == n.year() {
                format!("{} {m}", d.day())
            } else {
                format!("{} {m} {}", d.day(), d.year())
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Delivery state TDLib reports; "sent" vs "read" is derived from the chat's
/// last read outbox message when rendering.
pub enum SendState {
    None,
    Pending,
    Failed,
}

/// One history row before series marking.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistMsg {
    pub id: i64,
    pub date: i64,
    pub outgoing: bool,
    pub sender_key: i64,
    pub sender_name: String,
    pub text: String,
    pub state: SendState,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Row {
    Day(String),
    Msg {
        msg: HistMsg,
        first: bool,
        last: bool,
    },
}

/// Messages from one sender within this many seconds form one visual series.
const SERIES_GAP: i64 = 5 * 60;

/// Builds history rows (oldest first): date chips between days and series flags
/// so only the first bubble of a series shows the name and the last the tail.
pub fn history_rows<Tz: TimeZone>(msgs: &[HistMsg], tz: &Tz, now: &DateTime<Tz>) -> Vec<Row> {
    let local = |t: i64| tz.timestamp_opt(t, 0).single();
    let mut rows = Vec::with_capacity(msgs.len() + 4);
    for (i, m) in msgs.iter().enumerate() {
        let prev = i.checked_sub(1).map(|j| &msgs[j]);
        let next = msgs.get(i + 1);
        let day = local(m.date).map(|t| t.date_naive());
        let new_day = prev.is_none_or(|p| local(p.date).map(|t| t.date_naive()) != day);
        if new_day && let Some(t) = local(m.date) {
            rows.push(Row::Day(day_chip(&t, now)));
        }
        let same = |a: &HistMsg, b: &HistMsg| {
            a.sender_key == b.sender_key
                && a.outgoing == b.outgoing
                && (b.date - a.date).abs() <= SERIES_GAP
                && local(a.date).map(|t| t.date_naive()) == local(b.date).map(|t| t.date_naive())
        };
        let first = new_day || prev.is_none_or(|p| !same(p, m));
        let last = next.is_none_or(|n| !same(m, n));
        rows.push(Row::Msg {
            msg: m.clone(),
            first,
            last,
        });
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn chat(id: i64, order: i64) -> ChatEntry {
        ChatEntry {
            id,
            title: format!("c{id}"),
            kind: ChatKind::Private,
            order,
            pinned: false,
            last: None,
            unread: 0,
            muted: false,
            last_read_outbox: 0,
        }
    }

    #[test]
    fn ordered_by_descending_order_and_hides_zero() {
        let mut l = ChatList::default();
        l.insert(chat(1, 10));
        l.insert(chat(2, 30));
        l.insert(chat(3, 0));
        l.insert(chat(4, 20));
        let ids: Vec<_> = l.ordered().iter().map(|c| c.id).collect();
        assert_eq!(ids, [2, 4, 1]);
    }

    #[test]
    fn update_moves_chat_and_ignores_unknown() {
        let mut l = ChatList::default();
        l.insert(chat(1, 10));
        l.insert(chat(2, 20));
        assert!(l.update(1, |c| c.order = 99));
        assert!(!l.update(7, |c| c.order = 1));
        let ids: Vec<_> = l.ordered().iter().map(|c| c.id).collect();
        assert_eq!(ids, [1, 2]);
    }

    #[test]
    fn initials_cases() {
        assert_eq!(initials("Рабочая группа"), "РГ");
        assert_eq!(initials("мама"), "М");
        assert_eq!(initials("  «Rust» по-русски и ещё"), "RП");
        assert_eq!(initials("🙂🙂"), "?");
        assert_eq!(initials(""), "?");
    }

    #[test]
    fn avatar_tone_is_stable_and_in_range() {
        for id in [-1001234567890_i64, -5, 0, 7, i64::MIN, i64::MAX] {
            let t = avatar_tone(id);
            assert!((0..AVATAR_TONES as i32).contains(&t));
            assert_eq!(t, avatar_tone(id));
        }
    }

    fn at(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, h, min, 0).unwrap()
    }

    #[test]
    fn list_time_buckets() {
        let now = at(2026, 11, 19, 15, 0); // Thursday
        assert_eq!(list_time(&at(2026, 11, 19, 9, 5), &now), "09:05");
        assert_eq!(list_time(&at(2026, 11, 18, 23, 59), &now), "вчера");
        assert_eq!(list_time(&at(2026, 11, 16, 12, 0), &now), "пн");
        assert_eq!(list_time(&at(2026, 10, 1, 12, 0), &now), "01.10");
        assert_eq!(list_time(&at(2025, 10, 1, 12, 0), &now), "01.10.25");
    }

    #[test]
    fn day_chips() {
        let now = at(2026, 11, 19, 15, 0);
        assert_eq!(day_chip(&at(2026, 11, 19, 1, 0), &now), "Сегодня");
        assert_eq!(day_chip(&at(2026, 11, 18, 1, 0), &now), "Вчера");
        assert_eq!(day_chip(&at(2026, 3, 5, 1, 0), &now), "5 марта");
        assert_eq!(day_chip(&at(2024, 3, 5, 1, 0), &now), "5 марта 2024");
    }

    fn hm(id: i64, date: i64, sender: i64, out: bool) -> HistMsg {
        HistMsg {
            id,
            date,
            outgoing: out,
            sender_key: sender,
            sender_name: String::new(),
            text: format!("m{id}"),
            state: SendState::None,
        }
    }

    fn flags(rows: &[Row]) -> Vec<String> {
        rows.iter()
            .map(|r| match r {
                Row::Day(d) => format!("[{d}]"),
                Row::Msg { msg, first, last } => {
                    format!("{}{}{}", msg.id, if *first { "F" } else { "" }, if *last { "L" } else { "" })
                }
            })
            .collect()
    }

    #[test]
    fn series_and_day_chips() {
        let now = at(2026, 11, 19, 15, 0);
        let day1 = at(2026, 11, 18, 10, 0).timestamp();
        let day2 = at(2026, 11, 19, 10, 0).timestamp();
        let msgs = [
            hm(1, day1, 5, false),
            hm(2, day1 + 60, 5, false),
            hm(3, day1 + 120, 6, false),
            hm(4, day1 + 130, 0, true),
            hm(5, day1 + 130 + SERIES_GAP + 1, 0, true),
            hm(6, day2, 0, true),
        ];
        let rows = history_rows(&msgs, &Utc, &now);
        assert_eq!(
            flags(&rows),
            ["[Вчера]", "1F", "2L", "3FL", "4FL", "5FL", "[Сегодня]", "6FL"]
        );
    }
}
