//! Status boards: one message each, edited in place every minute rather than posted again, so a
//! channel can be a dashboard that never scrolls. Drawn with Components V2 (sent through
//! [`crate::rest`]). Kept in the data folder (`boards.json`): after a restart the same messages
//! are edited. A board whose message was deleted by hand is dropped.

use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::post::{BLUE, GREEN, RED};
use crate::rest::{self, Rest};
use crate::state::{mhz, Shared};

const FILE: &str = "boards.json";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, poise::ChoiceParameter)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    #[name = "Systems (decode rates, calls, recorders)"]
    Systems,
    #[name = "Calls going on now"]
    Active,
    #[name = "Open tone threads"]
    Threads,
    #[name = "Today so far"]
    Today,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Systems => "systems",
            Kind::Active => "active calls",
            Kind::Threads => "tone threads",
            Kind::Today => "today so far",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Board {
    pub name: String,
    pub kind: Kind,
    pub channel: u64,
    pub message: u64,
    /// Only these systems (empty: all).
    #[serde(default)]
    pub systems: Vec<String>,
}

pub struct Boards {
    list: Mutex<Vec<Board>>,
    path: PathBuf,
}

impl Boards {
    pub fn load(dir: &std::path::Path) -> Boards {
        let path = dir.join(FILE);
        let list = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        Boards { list: Mutex::new(list), path }
    }

    /// A system renamed: the boards of it follow it.
    pub fn rename(&self, old: &str, new: &str) {
        let mut l = self.list.lock().unwrap();
        let mut changed = false;
        for x in l.iter_mut().flat_map(|b| b.systems.iter_mut()).filter(|x| *x == old) {
            *x = new.to_string();
            changed = true;
        }
        if changed {
            self.save(&l);
        }
    }

    fn save(&self, list: &[Board]) {
        if let Ok(j) = serde_json::to_vec_pretty(list) {
            let tmp = self.path.with_extension("json.new");
            let _ = std::fs::write(&tmp, j).and_then(|_| std::fs::rename(&tmp, &self.path));
        }
    }

    pub fn all(&self) -> Vec<Board> {
        self.list.lock().unwrap().clone()
    }

    pub fn add(&self, b: Board) {
        let mut l = self.list.lock().unwrap();
        l.push(b);
        self.save(&l);
    }

    pub fn remove(&self, name: &str) -> Option<Board> {
        let mut l = self.list.lock().unwrap();
        let at = l.iter().position(|b| b.name.eq_ignore_ascii_case(name.trim()))?;
        let b = l.remove(at);
        self.save(&l);
        Some(b)
    }

    fn drop_message(&self, message: u64) {
        let mut l = self.list.lock().unwrap();
        l.retain(|b| b.message != message);
        self.save(&l);
    }
}

fn now_s() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

fn updated() -> Value {
    rest::text(format!("-# Updated <t:{}:R>", now_s() as i64))
}

/// What board `b` shows now.
pub fn draw(shared: &Shared, kind: Kind, systems: &[String]) -> Value {
    let picks = |s: &str| systems.is_empty() || systems.iter().any(|x| x == s);
    match kind {
        Kind::Systems => {
            let rates = shared.rates.lock().unwrap().clone();
            let mut inner = vec![rest::text("## Systems"), rest::separator()];
            let mut any = false;
            for (site, r) in rates.iter().filter(|(_, r)| picks(&r.system)) {
                any = true;
                let (top, bottom) = r.graph(5);
                let cc = r.control_channel_hz.map_or_else(|| "searching".to_string(), mhz);
                let name = if *site == r.system { site.clone() } else { format!("{} · {site}", r.system) };
                inner.push(rest::text(format!(
                    "**{name}** · {:.1} msg/s · {cc}\n{} active · {} recording\n```\n{top}\n{bottom}\n```",
                    r.now(),
                    r.active,
                    r.recording
                )));
            }
            if !any {
                inner.push(rest::text("No systems heard yet."));
            }
            inner.push(updated());
            json!([rest::container(RED, inner)])
        }
        Kind::Active => {
            let mut calls: Vec<_> = shared.active.lock().unwrap().values().filter(|c| picks(&c.short_name)).cloned().collect();
            calls.sort_by(|a, b| a.start_time.total_cmp(&b.start_time));
            let lines: Vec<String> = calls
                .iter()
                .take(30)
                .map(|c| {
                    let tg = if c.talkgroup_tag.is_empty() { format!("TG {}", c.talkgroup) } else { format!("{} ({})", c.talkgroup_tag, c.talkgroup) };
                    let state = if c.recording { "🔴".to_string() } else { format!("👂 {}", c.reason.clone().unwrap_or_default().replace('_', " ")) };
                    format!(
                        "{state} **{tg}**{}{} · {} · since <t:{}:R>",
                        if c.emergency { " 🚨" } else { "" },
                        if c.encrypted { " 🔒" } else { "" },
                        c.short_name,
                        c.start_time as i64
                    )
                })
                .collect();
            let body = if lines.is_empty() { "No calls at the moment.".to_string() } else { lines.join("\n") };
            json!([rest::container(GREEN, vec![rest::text(format!("## Calls now ({})", calls.len())), rest::separator(), rest::text(body), updated()])])
        }
        Kind::Threads => {
            let open: Vec<_> = shared.threads.all().into_iter().filter(|t| picks(&t.system)).collect();
            let lines: Vec<String> = open
                .iter()
                .map(|t| {
                    let state = if t.stopped.is_some() { "⏸️" } else { "▶️" };
                    let live = if shared.live.follows_thread(t.thread) { " 🔊" } else { "" };
                    format!("{state}{live} <#{}> · {} · <t:{}:R> · {} call{}", t.thread, t.title, t.time as i64, t.calls, if t.calls == 1 { "" } else { "s" })
                })
                .collect();
            let body = if lines.is_empty() { "No tone threads are open.".to_string() } else { lines.join("\n") };
            json!([rest::container(RED, vec![rest::text("## Tone threads"), rest::separator(), rest::text(body), updated()])])
        }
        Kind::Today => crate::digest::components(&shared.digest.now(), "Today so far", BLUE),
    }
}

/// Bring every board up to date (or `only` one).
pub async fn refresh(shared: &Shared, rest: &Rest, only: Option<u64>) {
    for b in shared.boards.all().into_iter().filter(|b| only.is_none_or(|m| m == b.message)) {
        let c = draw(shared, b.kind, &b.systems);
        match rest.edit_v2(b.channel, b.message, c).await {
            Ok(()) => {}
            Err(e) if e.gone() => {
                shared.host.info(format!("board {:?} was deleted in Discord: dropped", b.name));
                shared.boards.drop_message(b.message);
            }
            Err(e) => shared.host.warn(format!("couldn't update board {:?}: {e}", b.name)),
        }
    }
}
