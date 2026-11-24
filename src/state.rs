//! What the bot knows, shared between the plugin's main thread (which keeps it up to date) and the
//! bot's (which answers commands from it). Copies, never the events themselves held open.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Instant;

use poise::serenity_prelude as serenity;
use tokio::sync::{mpsc, oneshot};
use trunk_recorder_plugin::{CallInfo, CallsPage, Host, HostInfo, LogLine, SiteIdentity, SystemInfo};

use crate::settings::{Feed, Server, Settings};

/// Talkgroups muted by channel, in the plugin's data folder.
const MUTES: &str = "mutes.json";
/// Each system's short name by its id, as last seen: to follow a rename.
const SYSTEMS: &str = "systems.json";

/// How long TRP has to answer a save before the plugin says it can't save.
const SAVE_ANSWER: std::time::Duration = std::time::Duration::from_secs(10);

/// TRP's warnings and errors kept, for /log.
pub const LOG_KEEP: usize = 200;

/// Decode rate samples kept per system (a status every few seconds).
pub const HISTORY: usize = 120;

pub struct Shared {
    pub host: Host,
    pub settings: Mutex<Settings>,
    pub systems: Mutex<Vec<SystemInfo>>,
    /// Calls going on now, by id.
    pub active: Mutex<HashMap<u32, CallInfo>>,
    /// By site name (a system of one site: its short name).
    pub rates: Mutex<BTreeMap<String, Rates>>,
    pub started: Instant,
    pub calls: AtomicU64,
    pub posted: AtomicU64,
    /// Answers TRP owes the bot, by request number.
    pub pages: Mutex<HashMap<u64, oneshot::Sender<CallsPage>>>,
    pub data_dir: PathBuf,
    /// Tone threads open now.
    pub threads: crate::thread::Threads,
    pub live: crate::live::Live,
    pub boards: crate::board::Boards,
    pub digest: crate::digest::Digest,
    pub alerts: crate::alerts::Alerts,
    /// Saves sent to TRP and not answered yet (request, when).
    pub saving: Mutex<Vec<(u64, Instant)>>,
    /// It's been said that TRP doesn't save the plugin's settings.
    pub cant_save: std::sync::atomic::AtomicBool,
    /// What the bot sees of Discord (set once it logs in): the names it fills in.
    pub cache: Mutex<Option<std::sync::Arc<serenity::Cache>>>,
    /// New settings, or a channel or server renamed: names to fill in now, not at the next look.
    pub renamed: tokio::sync::Notify,
    /// Talkgroups muted in a channel ("Mute here"): by channel, `system:tg`.
    pub mutes: Mutex<BTreeMap<u64, Vec<String>>>,
    /// Discord's REST API, for what serenity can't do (set once the bot logs in).
    pub rest: Mutex<Option<crate::rest::Rest>>,
    /// The bot's own user ID, and the server it's in when it's in only one (set once it logs in).
    pub me: Mutex<Option<u64>>,
    pub only_guild: Mutex<Option<u64>>,
    /// To the bot's poster, while a bot runs.
    pub posts: Mutex<Option<mpsc::UnboundedSender<crate::post::Post>>>,
    /// When TRP last sent anything (a status, a call): to tell a quiet link from a quiet air.
    pub heard: Mutex<Option<Instant>>,
    /// TRP's latest warnings and errors (`log`), for /log.
    pub log: Mutex<VecDeque<LogLine>>,
    /// Lines for the Log feeds, posted at the next status.
    pub log_pending: Mutex<Vec<LogLine>>,
    /// Posts that failed, and the latest one's why (and when).
    pub failures: AtomicU64,
    pub last_failure: Mutex<Option<(Instant, String)>>,
    /// The TRP it runs in: its name and version.
    pub trp: Mutex<HostInfo>,
    /// The commands as built, to register in each server with its access ([`crate::access::commands_for`]).
    pub commands: Mutex<Vec<serenity::CreateCommand>>,
}

impl Shared {
    pub fn new(host: Host, settings: Settings, systems: Vec<SystemInfo>, data_dir: PathBuf) -> Shared {
        let threads = crate::thread::Threads::load(&data_dir, &host);
        let mutes = std::fs::read(data_dir.join(MUTES)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        let sh = Shared {
            host,
            settings: Mutex::new(settings),
            systems: Mutex::new(systems),
            active: Mutex::default(),
            rates: Mutex::default(),
            started: Instant::now(),
            calls: AtomicU64::new(0),
            posted: AtomicU64::new(0),
            pages: Mutex::default(),
            threads,
            live: crate::live::Live::load(&data_dir),
            boards: crate::board::Boards::load(&data_dir),
            digest: crate::digest::Digest::load(&data_dir),
            alerts: crate::alerts::Alerts::load(&data_dir),
            mutes: Mutex::new(mutes),
            saving: Mutex::default(),
            cant_save: Default::default(),
            cache: Mutex::default(),
            renamed: tokio::sync::Notify::new(),
            rest: Mutex::new(None),
            me: Mutex::new(None),
            only_guild: Mutex::new(None),
            data_dir,
            posts: Mutex::new(None),
            commands: Mutex::default(),
            heard: Mutex::default(),
            log: Mutex::default(),
            log_pending: Mutex::default(),
            failures: AtomicU64::new(0),
            last_failure: Mutex::default(),
            trp: Mutex::default(),
        };
        sh.follow_renames();
        sh
    }

    /// What's kept by a system's short name (alerts, mutes, boards) follows its renames: TRP
    /// knows it by its id, the same through them.
    pub fn follow_renames(&self) {
        let path = self.data_dir.join(SYSTEMS);
        let mut known: BTreeMap<u32, String> = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        let now: Vec<(u32, String)> = self.systems.lock().unwrap().iter().filter(|s| s.id != 0).map(|s| (s.id, s.short_name.clone())).collect();
        let mut changed = false;
        for (id, name) in now {
            match known.insert(id, name.clone()) {
                Some(old) if old == name => continue,
                Some(old) => {
                    self.host.info(format!("system {old} is now {name}: its alerts, mutes and boards follow it"));
                    self.alerts.rename(&old, &name);
                    self.boards.rename(&old, &name);
                    self.rename_mutes(&old, &name);
                }
                None => {}
            }
            changed = true;
        }
        if changed {
            if let Ok(j) = serde_json::to_vec_pretty(&known) {
                let tmp = path.with_extension("json.new");
                let _ = std::fs::write(&tmp, j).and_then(|_| std::fs::rename(&tmp, &path));
            }
        }
    }

    fn rename_mutes(&self, old: &str, new: &str) {
        let mut m = self.mutes.lock().unwrap();
        let prefix = format!("{old}:");
        let mut changed = false;
        for x in m.values_mut().flatten() {
            if let Some(tg) = x.strip_prefix(&prefix) {
                *x = format!("{new}:{tg}");
                changed = true;
            }
        }
        if changed {
            Self::save_mutes(&self.data_dir, &m);
        }
    }

    fn save_mutes(dir: &std::path::Path, m: &BTreeMap<u64, Vec<String>>) {
        if let Ok(j) = serde_json::to_vec_pretty(m) {
            let tmp = dir.join(format!("{MUTES}.new"));
            let _ = std::fs::write(&tmp, j).and_then(|_| std::fs::rename(&tmp, dir.join(MUTES)));
        }
    }

    pub fn settings(&self) -> Settings {
        let only = *self.only_guild.lock().unwrap();
        self.settings.lock().unwrap().clone().normalized(only)
    }

    pub fn post(&self, p: crate::post::Post) {
        if let Some(tx) = self.posts.lock().unwrap().as_ref() {
            let _ = tx.send(p);
        }
    }

    pub fn count_call(&self) {
        self.calls.fetch_add(1, Ordering::Relaxed);
    }

    /// Ask TRP for recorded calls; the page comes back through [`Shared::pages`].
    pub fn ask_calls(&self, q: trunk_recorder_plugin::CallsQuery) -> oneshot::Receiver<CallsPage> {
        let (tx, rx) = oneshot::channel();
        // (Held while asking: the answer can't come before it's waited for.)
        let mut pages = self.pages.lock().unwrap();
        let request = self.host.calls(q);
        pages.insert(request, tx);
        rx
    }

    /// Every server.
    pub fn servers(&self) -> Vec<Server> {
        self.settings().servers
    }

    pub fn server(&self, guild: u64) -> Option<Server> {
        self.servers().into_iter().find(|s| s.guild() == Some(guild))
    }

    /// Change the settings with `f` (as they're used), fill in the names, and save them to the
    /// TRP, which shows them on the page and sends them back. `f`'s Err, or settings that
    /// don't check: nothing changed, and why. What `f` said.
    pub fn edit(&self, f: impl FnOnce(&mut Settings) -> Result<String, String>) -> Result<String, String> {
        let only = *self.only_guild.lock().unwrap();
        let mut cur = self.settings.lock().unwrap();
        let was = cur.clone().normalized(only);
        let mut s = was.clone();
        let said = f(&mut s)?;
        if let Some(cache) = self.cache.lock().unwrap().clone() {
            crate::names::fill(&mut s, &cache);
        }
        s.check()?;
        if s.to_save() != was.to_save() {
            *cur = s.clone();
            drop(cur);
            self.save(&s);
            self.live.changed.notify_one();
            // A server's admins named or cleared: its commands are shown to them now.
            let shown = |x: &Settings, g: u64| x.server(g).is_some_and(|v| v.access.names_admins());
            let regs: Vec<u64> = s.guilds().into_iter().filter(|&g| shown(&was, g) != shown(&s, g)).collect();
            if let (false, Ok(rt)) = (regs.is_empty(), tokio::runtime::Handle::try_current()) {
                for g in regs {
                    let Ok((rest, path, body)) = crate::bot::registration(self, g) else { continue };
                    let host = self.host.clone();
                    rt.spawn(async move {
                        if let Err(e) = rest.put(&path, &body).await {
                            host.warn(format!("couldn't show the commands to server {g}'s admins: {e:?}"));
                        }
                    });
                }
            }
        }
        Ok(said)
    }

    /// Change server `guild` with `f` (adding it, when it isn't one yet).
    pub fn edit_server(&self, guild: u64, f: impl FnOnce(&mut Server) -> Result<String, String>) -> Result<String, String> {
        self.edit(|s| {
            let at = match s.servers.iter().position(|x| x.guild() == Some(guild)) {
                Some(at) => at,
                None => {
                    s.servers.push(Server { id: guild.to_string(), ..Default::default() });
                    s.servers.len() - 1
                }
            };
            f(&mut s.servers[at])
        })
    }

    /// Fill in names that changed in Discord (a channel renamed), saving them.
    pub fn rename(&self) {
        if let Err(e) = self.edit(|_| Ok(String::new())) {
            self.host.warn(format!("couldn't fill in the names: {e}"));
        }
    }

    fn save(&self, s: &Settings) -> u64 {
        let request = self.host.save_settings(&s.to_save());
        self.saving.lock().unwrap().push((request, Instant::now()));
        request
    }

    /// TRP answered save `request`.
    pub fn saved(&self, request: u64, ok: bool, message: &str) {
        self.saving.lock().unwrap().retain(|(r, _)| *r != request);
        if !ok {
            self.host.warn(format!("TRP didn't save the settings: {message}"));
        }
    }

    /// A save TRP hasn't answered in time: it can't save them (it's older than
    /// settings.save). Said once.
    pub fn check_saves(&self) {
        let late = self.saving.lock().unwrap().iter().any(|(_, t)| t.elapsed() > SAVE_ANSWER);
        if late && !self.cant_save.swap(true, Ordering::Relaxed) {
            self.host.warn(
                "this version of TRP doesn't save plugins' settings (it needs settings.save): what's set in Discord lasts only until the plugin restarts",
            );
        }
    }

    /// TRP sent something.
    pub fn heard(&self) {
        *self.heard.lock().unwrap() = Some(Instant::now());
    }

    /// A post failed: why, to the log and /health.
    pub fn failed(&self, why: String) {
        self.failures.fetch_add(1, Ordering::Relaxed);
        self.host.warn(why.clone());
        *self.last_failure.lock().unwrap() = Some((Instant::now(), why));
    }

    /// A line TRP logged: kept for /log (warnings and worse), and for the Log feeds that take it.
    pub fn logged(&self, line: LogLine) {
        let wants = self.feeds().iter().any(|f| f.enabled && f.event == crate::settings::Event::Log && f.log_level.takes(&line.level));
        if wants {
            let mut p = self.log_pending.lock().unwrap();
            // (A burst: the latest are what matter.)
            if p.len() >= 100 {
                p.remove(0);
            }
            p.push(line.clone());
        }
        if crate::settings::LogLevel::Warning.takes(&line.level) {
            let mut l = self.log.lock().unwrap();
            if l.len() >= LOG_KEEP {
                l.pop_front();
            }
            l.push_back(line);
        }
    }

    pub fn rest(&self) -> Option<crate::rest::Rest> {
        self.rest.lock().unwrap().clone()
    }

    /// `system`'s `talkgroup` is muted in `channel`.
    pub fn muted(&self, channel: u64, system: &str, talkgroup: u32) -> bool {
        self.mutes.lock().unwrap().get(&channel).is_some_and(|m| m.iter().any(|x| *x == format!("{system}:{talkgroup}")))
    }

    /// Mute (or unmute) `system`'s `talkgroup` in `channel`: whether it changed.
    pub fn mute(&self, channel: u64, system: &str, talkgroup: u32, on: bool) -> bool {
        let key = format!("{system}:{talkgroup}");
        let mut m = self.mutes.lock().unwrap();
        let l = m.entry(channel).or_default();
        let had = l.contains(&key);
        match (on, had) {
            (true, false) => l.push(key),
            (false, true) => l.retain(|x| *x != key),
            _ => return false,
        }
        m.retain(|_, l| !l.is_empty());
        Self::save_mutes(&self.data_dir, &m);
        true
    }

    /// Every server's feeds.
    pub fn feeds(&self) -> Vec<Feed> {
        self.settings().all_feeds()
    }
}

/// A system's control channel decode rates, latest last.
#[derive(Clone, Debug, Default)]
pub struct Rates {
    /// The system it's a site of (its short name).
    pub system: String,
    pub control_channel_hz: Option<u64>,
    pub history: VecDeque<f64>,
    /// Calls on it now, and those being recorded.
    pub active: u32,
    pub recording: u32,
    /// Seconds between the last two statuses.
    pub interval_s: f64,
    pub last: Option<f64>,
    /// What its control channel says it is, and its protocol (`p25`…).
    pub identity: SiteIdentity,
    pub protocol: String,
    /// When its control channel was lost (none: it has one), and whether a Health feed was told.
    pub lost_at: Option<f64>,
    pub said_lost: bool,
}

impl Rates {
    pub fn push(&mut self, rate: f64, at: f64) {
        if let Some(last) = self.last {
            self.interval_s = (at - last).max(0.0);
        }
        self.last = Some(at);
        self.history.push_back(rate);
        while self.history.len() > HISTORY {
            self.history.pop_front();
        }
    }

    pub fn now(&self) -> f64 {
        self.history.back().copied().unwrap_or(0.0)
    }

    /// Two rows of braille, oldest left: each character two columns of `per` samples' average,
    /// each column four dots on each row (0 to 40 messages a second, 5 a dot). Columns not yet
    /// heard are blank.
    pub fn graph(&self, per: usize) -> (String, String) {
        // Braille with `l` dots up the left column and `r` up the right, 0 to 4 each.
        const LEFT: [u32; 4] = [0x40, 0x04, 0x02, 0x01];
        const RIGHT: [u32; 4] = [0x80, 0x20, 0x10, 0x08];
        let cell = |l: usize, r: usize| {
            let bits = LEFT[..l].iter().sum::<u32>() + RIGHT[..r].iter().sum::<u32>();
            char::from_u32(0x2800 + bits).unwrap_or(' ')
        };
        let columns = HISTORY / per;
        let pad = HISTORY - self.history.len();
        let avg = |c: usize| -> Option<f64> {
            let (from, to) = (c * per, (c + 1) * per);
            let got: Vec<f64> = (from..to).filter(|&i| i >= pad).map(|i| self.history[i - pad]).collect();
            (!got.is_empty()).then(|| got.iter().sum::<f64>() / got.len() as f64)
        };
        let dots = |c: usize| avg(c).map_or(0, |a| ((a / 5.0).round() as usize).min(8));
        let (mut top, mut bottom) = (String::new(), String::new());
        for c in (0..columns).step_by(2) {
            let (l, r) = (dots(c), dots(c + 1));
            top.push(cell(l.saturating_sub(4), r.saturating_sub(4)));
            bottom.push(cell(l.min(4), r.min(4)));
        }
        (top, bottom)
    }
}

/// MHz, six places.
pub fn mhz(hz: u64) -> String {
    format!("{:.6} MHz", hz as f64 / 1e6)
}

/// "1 h 05 m", "4 m 10 s", "12 s".
pub fn span(s: u64) -> String {
    match s {
        0..60 => format!("{s} s"),
        60..3600 => format!("{} m {:02} s", s / 60, s % 60),
        3600..86400 => format!("{} h {:02} m", s / 3600, s % 3600 / 60),
        _ => format!("{} d {} h", s / 86400, s % 86400 / 3600),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_graph_fills_from_the_right() {
        let mut r = Rates::default();
        assert_eq!(r.graph(5), ("\u{2800}".repeat(12), "\u{2800}".repeat(12)));
        for i in 0..10 {
            r.push(40.0, i as f64);
        }
        let (top, bottom) = r.graph(5);
        // The newest two columns full: 8 dots each.
        assert_eq!(top.chars().last(), Some('⣿'));
        assert_eq!(bottom.chars().last(), Some('⣿'));
        assert_eq!(top.chars().filter(|&c| c == '\u{2800}').count(), 11);
    }

    /// A system renamed in TRP keeps its alerts, mutes and boards: they're found by its id.
    #[test]
    fn a_renamed_system_keeps_what_was_kept_by_its_name() {
        let dir = trunk_recorder_plugin::testing::temp_dir("discord-rename");
        std::fs::write(dir.join(SYSTEMS), r#"{"7":"county"}"#).unwrap();
        std::fs::write(dir.join(MUTES), r#"{"9":["county:5","city:5"]}"#).unwrap();
        std::fs::write(dir.join("alerts.json"), r#"[{"user":1,"system":"county","talkgroup":5}]"#).unwrap();
        let (host, _) = trunk_recorder_plugin::testing::capture();
        let systems = vec![
            SystemInfo { id: 7, short_name: "county-p25".into(), ..Default::default() },
            SystemInfo { id: 8, short_name: "city".into(), ..Default::default() },
        ];
        let sh = Shared::new(host, Settings::default(), systems, dir.clone());
        assert!(sh.muted(9, "county-p25", 5) && sh.muted(9, "city", 5) && !sh.muted(9, "county", 5));
        assert_eq!(sh.alerts.all()[0].system, "county-p25");
        let known: BTreeMap<u32, String> = serde_json::from_slice(&std::fs::read(dir.join(SYSTEMS)).unwrap()).unwrap();
        assert_eq!(known.get(&7).map(String::as_str), Some("county-p25"));
        assert_eq!(known.get(&8).map(String::as_str), Some("city"));
    }

    /// What Discord changes is saved to the settings (settings.save), one save per change.
    #[test]
    fn discord_changes_are_saved_to_the_settings() {
        let dir = trunk_recorder_plugin::testing::temp_dir("discord-edit");
        let (host, out) = trunk_recorder_plugin::testing::capture();
        let sh = Shared::new(host, Settings::default(), Vec::new(), dir.clone());
        let saves = || {
            out.output()
                .messages
                .into_iter()
                .filter_map(|m| if let trunk_recorder_plugin::PluginMessage::SettingsSave { request, settings } = m { Some((request, settings)) } else { None })
                .collect::<Vec<_>>()
        };
        let r = sh.edit_server(42, |s| {
            s.live.channel = "55".into();
            s.feeds.push(Feed { channel: "9".into(), event: crate::settings::Event::Tone, ..Default::default() });
            Ok(String::new())
        });
        assert!(r.is_ok());
        let s = sh.settings();
        assert_eq!((s.servers.len(), s.servers[0].voice(), s.servers[0].feeds.len()), (1, Some(55), 1));
        let (request, saved) = saves().pop().expect("saved");
        assert_eq!(saved["servers"][0]["feeds"][0]["channel"], "9");
        sh.saved(request, true, "");
        // A change from Discord: one more save; an Err, none.
        assert!(sh.edit_server(42, |s| Ok(std::mem::take(&mut s.live.channel))).is_ok());
        assert!(sh.edit_server(42, |_| Err("no".into())).is_err());
        assert_eq!(saves().len(), 2);
        assert_eq!(sh.server(42).unwrap().voice(), None);
    }

    #[test]
    fn spans_read_well() {
        assert_eq!(span(12), "12 s");
        assert_eq!(span(250), "4 m 10 s");
        assert_eq!(span(3900), "1 h 05 m");
    }
}
