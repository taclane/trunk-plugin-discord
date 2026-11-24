//! The day so far: calls by talkgroup, emergencies, signals heard, calls by hour. Kept in the data
//! folder (`today.json`), so a restart doesn't lose the day; at local midnight the day's digest
//! goes to the digest feeds and a new day starts.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

use chrono::{Local, Timelike};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::rest;

const FILE: &str = "today.json";

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Talkgroup {
    pub system: String,
    pub talkgroup: u32,
    pub tag: String,
    pub calls: u32,
    pub seconds: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Day {
    /// Local date, YYYY-MM-DD.
    pub date: String,
    pub calls: u32,
    pub seconds: f64,
    /// By `system:tg`.
    pub talkgroups: BTreeMap<String, Talkgroup>,
    pub emergencies: u32,
    /// Signals heard (their names), and how often.
    pub signals: BTreeMap<String, u32>,
    pub hours: Vec<u32>,
}

pub struct Digest {
    day: Mutex<Day>,
    path: PathBuf,
}

pub fn today() -> String {
    Local::now().format("%Y-%m-%d").to_string()
}

impl Digest {
    pub fn load(dir: &std::path::Path) -> Digest {
        let path = dir.join(FILE);
        let day = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_else(|| Day {
            date: today(),
            hours: vec![0; 24],
            ..Default::default()
        });
        Digest { day: Mutex::new(day), path }
    }

    pub fn save(&self) {
        let d = self.day.lock().unwrap().clone();
        if let Ok(j) = serde_json::to_vec(&d) {
            let tmp = self.path.with_extension("json.new");
            let _ = std::fs::write(&tmp, j).and_then(|_| std::fs::rename(&tmp, &self.path));
        }
    }

    pub fn now(&self) -> Day {
        self.day.lock().unwrap().clone()
    }

    /// A new day: the one that ended, to post.
    pub fn roll(&self) -> Option<Day> {
        let mut d = self.day.lock().unwrap();
        if d.date == today() {
            return None;
        }
        let old = std::mem::replace(&mut *d, Day { date: today(), hours: vec![0; 24], ..Default::default() });
        Some(old)
    }

    pub fn call(&self, system: &str, talkgroup: u32, tag: &str, seconds: f64, emergency: bool) {
        let mut d = self.day.lock().unwrap();
        d.calls += 1;
        d.seconds += seconds;
        if emergency {
            d.emergencies += 1;
        }
        let h = Local::now().hour() as usize;
        if d.hours.len() < 24 {
            d.hours.resize(24, 0);
        }
        d.hours[h] += 1;
        let t = d.talkgroups.entry(format!("{system}:{talkgroup}")).or_insert_with(|| Talkgroup { system: system.into(), talkgroup, ..Default::default() });
        t.calls += 1;
        t.seconds += seconds;
        if !tag.is_empty() {
            t.tag = tag.to_string();
        }
    }

    pub fn signal(&self, name: &str) {
        *self.day.lock().unwrap().signals.entry(name.to_string()).or_default() += 1;
    }
}

/// A day's figures as Components V2 (a board, or the digest).
pub fn components(d: &Day, title: &str, color: u32) -> Value {
    let mut tgs: Vec<&Talkgroup> = d.talkgroups.values().collect();
    tgs.sort_by(|a, b| b.calls.cmp(&a.calls).then(b.seconds.total_cmp(&a.seconds)));
    let top: Vec<String> = tgs
        .iter()
        .take(10)
        .enumerate()
        .map(|(i, t)| {
            let name = if t.tag.is_empty() { format!("TG {}", t.talkgroup) } else { format!("{} ({})", t.tag, t.talkgroup) };
            format!("{}. **{name}** · {} · {} call{} · {}", i + 1, t.system, t.calls, if t.calls == 1 { "" } else { "s" }, crate::state::span(t.seconds as u64))
        })
        .collect();
    let busiest =
        d.hours.iter().enumerate().max_by_key(|(_, n)| **n).filter(|(_, n)| **n > 0).map(|(h, n)| format!("{h:02}:00–{:02}:00 ({n} calls)", (h + 1) % 24));
    let mut inner = vec![
        rest::text(format!("## {title}\n{}", d.date)),
        rest::separator(),
        rest::text(format!(
            "**{}** calls · **{}** on the air · **{}** emergenc{}{}",
            d.calls,
            crate::state::span(d.seconds as u64),
            d.emergencies,
            if d.emergencies == 1 { "y" } else { "ies" },
            busiest.map(|b| format!("\nBusiest hour: {b}")).unwrap_or_default()
        )),
    ];
    if !top.is_empty() {
        inner.push(rest::separator());
        inner.push(rest::text(format!("### Busiest talkgroups\n{}", top.join("\n"))));
    }
    if !d.signals.is_empty() {
        let mut s: Vec<(&String, &u32)> = d.signals.iter().collect();
        s.sort_by(|a, b| b.1.cmp(a.1));
        inner.push(rest::separator());
        inner.push(rest::text(format!("### Signals heard\n{}", s.iter().take(15).map(|(n, c)| format!("{n} ×{c}")).collect::<Vec<_>>().join(" · "))));
    }
    inner.push(rest::text(format!("-# Updated <t:{}:R>", chrono::Utc::now().timestamp())));
    serde_json::json!([rest::container(color, inner)])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_day_counts_and_survives() {
        let dir = trunk_recorder_plugin::testing::temp_dir("discord-digest");
        let d = Digest::load(&dir);
        d.call("clmrn-I", 1001, "Ops", 4.0, false);
        d.call("clmrn-I", 1001, "Ops", 6.0, true);
        d.signal("STATION 1");
        d.save();
        let again = Digest::load(&dir).now();
        assert_eq!((again.calls, again.emergencies, again.talkgroups["clmrn-I:1001"].calls), (2, 1, 2));
        let c = components(&again, "Today", 0).to_string();
        assert!(c.contains("Ops (1001)") && c.contains("STATION 1 ×1"), "{c}");
    }
}
