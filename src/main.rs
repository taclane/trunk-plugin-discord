//! Discord: a bot that posts what TRP hears to channels (recorded calls with their
//! recordings, calls as they start, emergencies, decode rates) and answers slash commands. Its
//! feeds (what goes where) come from its settings page and from `/feed` in Discord, which it keeps
//! in its data folder.

mod access;
mod alerts;
mod board;
mod bot;
mod digest;
mod live;
mod menus;
mod names;
mod post;
mod rest;
mod settings;
mod setup;
mod state;
mod thread;
mod tones;

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use trunk_recorder_plugin::{
    format, topic, AudioChunk, CallInfo, CallsPage, ConcludedCall, Host, Manifest, NoConfig, Outcome, Page, Plugin, Setup, State, Status, TonePatternEvent,
};

use crate::post::{Post, To};
use crate::settings::{Event, Settings};
use crate::state::Shared;

/// How often a rates feed's message, and the status boards, are brought up to date.
const RATES_EVERY: Duration = Duration::from_secs(60);

struct Discord {
    shared: Arc<Shared>,
    rt: tokio::runtime::Runtime,
    bot: Option<bot::Bot>,
    /// Calls an emergency post has gone out for (once each).
    emergencies: HashSet<u32>,
    rates_at: Option<Instant>,
}

impl Discord {
    /// Where an event on `system`'s `talkgroup` goes, for feeds of `event`.
    fn to(&self, event: Event, system: &str, talkgroup: u32, length_s: Option<f64>) -> Vec<To> {
        let feeds = self.shared.feeds();
        let mut to: Vec<To> = Vec::new();
        for f in feeds.iter().filter(|f| f.event == event && f.picks(system, talkgroup)) {
            if length_s.is_some_and(|l| l < f.min_length_s) {
                continue;
            }
            let Some(channel) = f.channel_id() else { continue };
            if self.shared.muted(channel, system, talkgroup) {
                continue;
            }
            // (Two feeds into one channel: one post.)
            if let Some(t) = to.iter_mut().find(|t| t.channel == channel) {
                t.audio |= f.audio;
                continue;
            }
            to.push(To { server: f.server.parse().unwrap_or(0), channel, mention: f.mention.clone(), audio: f.audio, live: false });
        }
        to
    }

    /// A followed call's radios, for the live audio chat's log.
    fn speakers(&self, c: &CallInfo) {
        if self.shared.live.targets(&c.short_name, c.talkgroup).is_empty() {
            return;
        }
        let mut who: Vec<(u32, String)> = Vec::new();
        for s in c.sources.iter().filter(|s| s.unit != 0) {
            if !who.iter().any(|(u, _)| *u == s.unit) {
                who.push((s.unit, s.tag.clone()));
            }
        }
        self.shared.live.set_speakers(c.id, who);
    }

    fn emergency(&mut self, c: &CallInfo) {
        if !c.emergency || !self.emergencies.insert(c.id) {
            return;
        }
        let to = self.to(Event::Emergency, &c.short_name, c.talkgroup, None);
        if !to.is_empty() {
            self.shared.post(Post::Start { call: Box::new(c.clone()), emergency: true, to });
        }
        let users: Vec<u64> = self.shared.alerts.all().into_iter().filter(|a| a.takes_emergency(&c.short_name, c.talkgroup)).map(|a| a.user).collect();
        if !users.is_empty() {
            self.shared.post(Post::Dm { users, embed: Box::new(post::start_embed(c, true)), file: None });
        }
    }

    /// (Re)start the bot with the settings as they are.
    fn restart(&mut self) {
        if let Some(b) = self.bot.take() {
            b.stop(&self.rt);
        }
        let s = self.shared.settings();
        self.shared.host.status(State::Warning, "logging in to Discord");
        match bot::Bot::start(&self.rt, self.shared.clone(), &s) {
            Ok(b) => self.bot = Some(b),
            Err(e) => self.shared.host.status(State::Error, e),
        }
    }
}

impl Plugin for Discord {
    type Config = Settings;
    type SystemConfig = NoConfig;

    fn manifest() -> Manifest {
        Manifest {
            name: "Discord".into(),
            subscribe: vec![
                topic::CALL_START.into(),
                topic::CALL_UPDATE.into(),
                topic::CALL_END.into(),
                topic::CALL_CONCLUDED.into(),
                topic::STATUS.into(),
                topic::TONE_PATTERN.into(),
                topic::AUDIO.into(),
            ],
            audio_formats: vec![format::M4A.into()],
            page: Some(Page { title: "Discord".into() }),
            live_settings: true,
            ..trunk_recorder_plugin::manifest!()
        }
    }

    fn start(host: Host, setup: Setup<Settings, NoConfig>) -> Result<Self, String> {
        setup.config.check()?;
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("discord")
            .enable_all()
            .build()
            .map_err(|e| format!("no runtime: {e}"))?;
        let systems = setup
            .systems
            .iter()
            .map(|s| trunk_recorder_plugin::SystemInfo {
                index: s.index,
                id: s.id,
                short_name: s.short_name.clone(),
                kind: s.kind.clone(),
                name: s.name.clone(),
                sites: s.sites.iter().map(|x| trunk_recorder_plugin::SiteInfo { id: x.id, name: x.name.clone(), ..Default::default() }).collect(),
                ..Default::default()
            })
            .collect();
        let shared = Arc::new(Shared::new(host, setup.config, systems, setup.data_dir.clone()));
        let mut d = Discord { shared, rt, bot: None, emergencies: HashSet::new(), rates_at: None };
        d.restart();
        Ok(d)
    }

    fn settings(&mut self, setup: Setup<Settings, NoConfig>) -> Result<(), String> {
        setup.config.check()?;
        // Its own saves come back this way, each before its answer: with more than one still
        // unanswered, this is an older one, and what it has now is newer.
        let pending = self.shared.saving.lock().unwrap().len();
        if pending > 1 {
            return Ok(());
        }
        let old = std::mem::replace(&mut *self.shared.settings.lock().unwrap(), setup.config.clone());
        // (A new login, or other servers to register the commands in, or the intents phrases
        // need, restart it; feeds and live audio take effect at once. The servers it added itself
        // have their commands already.)
        let (old, new) = (old.normalized(None), setup.config.clone().normalized(None));
        if old.token != new.token || old.live_phrases != new.live_phrases || (pending == 0 && old.guilds() != new.guilds()) {
            self.restart();
        } else {
            // A server's admins named or cleared: its admin commands are shown to others now.
            let shown = |s: &Settings, g: u64| s.server(g).is_some_and(|x| x.access.names_admins());
            for g in new.guilds().into_iter().filter(|&g| shown(&old, g) != shown(&new, g)) {
                let sh = self.shared.clone();
                self.rt.spawn(async move {
                    if let Err(e) = bot::register_in(&sh, g).await {
                        sh.host.warn(format!("couldn't show the commands to server {g}'s admins: {e}"));
                    }
                });
            }
        }
        self.rates_at = None;
        self.shared.live.changed.notify_one();
        // (An ID typed in on the page: its name, now.)
        self.shared.renamed.notify_one();
        Ok(())
    }

    fn settings_saved(&mut self, request: u64, ok: bool, message: String) {
        self.shared.saved(request, ok, &message);
    }

    fn action(&mut self, a: trunk_recorder_plugin::Action, reply: trunk_recorder_plugin::Reply) {
        let shared = self.shared.clone();
        self.rt.spawn(async move { bot::action(&shared, a, reply).await });
    }

    fn calls(&mut self, request: u64, page: CallsPage) {
        if let Some(tx) = self.shared.pages.lock().unwrap().remove(&request) {
            let _ = tx.send(page);
        }
    }

    fn call_start(&mut self, c: CallInfo) {
        self.shared.count_call();
        let to = self.to(Event::Start, &c.short_name, c.talkgroup, None);
        if !to.is_empty() {
            self.shared.post(Post::Start { call: Box::new(c.clone()), emergency: false, to });
        }
        self.emergency(&c);
        self.speakers(&c);
        self.shared.active.lock().unwrap().insert(c.id, c);
    }

    /// Another radio talking (TRP's word of it as it happens): the live audio chat's log hears it.
    fn call_update(&mut self, c: CallInfo) {
        self.speakers(&c);
        if let Some(a) = self.shared.active.lock().unwrap().get_mut(&c.id) {
            *a = c;
        }
    }

    fn call_end(&mut self, c: CallInfo) {
        self.emergency(&c);
        self.speakers(&c);
        self.emergencies.remove(&c.id);
        self.shared.active.lock().unwrap().remove(&c.id);
        for f in self.shared.live.follows().iter().filter(|f| f.system == c.short_name && f.talkgroup == c.talkgroup) {
            self.shared.live.mixer(f.server).end(c.id);
        }
    }

    fn audio(&mut self, a: AudioChunk) {
        let targets = self.shared.live.targets(&a.short_name, a.talkgroup);
        if targets.is_empty() {
            return;
        }
        let (tag, emergency) = self.shared.active.lock().unwrap().get(&a.call_id).map(|c| (c.talkgroup_tag.clone(), c.emergency)).unwrap_or_default();
        let playing = live::Playing { system: a.short_name.clone(), talkgroup: a.talkgroup, tag, emergency };
        let samples = a.samples();
        for (server, priority) in targets {
            self.shared.live.mixer(server).push(a.call_id, playing.clone(), priority, a.sample_rate, &samples);
        }
    }

    fn tone_pattern(&mut self, t: TonePatternEvent) {
        for s in &t.pattern.signals {
            let name = thread::signal_name(s);
            self.shared.digest.signal(&name);
            self.shared.live.saw_signal(&t.short_name, t.talkgroup, &t.alpha_tag, &name);
        }
        let users: Vec<u64> = self.shared.alerts.all().into_iter().filter(|a| a.takes_tone(&t.short_name, &t.pattern.signals)).map(|a| a.user).collect();
        if !users.is_empty() {
            self.shared.post(Post::Dm { users, embed: Box::new(post::tone_embed(&t)), file: None });
        }
        let feeds = self.shared.feeds();
        let mut to: Vec<To> = Vec::new();
        for f in feeds.iter().filter(|f| f.event.threads() && f.picks(&t.short_name, t.talkgroup) && f.picks_tone(&t.pattern)) {
            let Some(channel) = f.channel_id() else { continue };
            // (Two feeds into one channel: one thread, live if either says so.)
            match to.iter_mut().find(|x| x.channel == channel) {
                Some(x) => x.live |= f.live,
                None => to.push(To { server: f.server.parse().unwrap_or(0), channel, mention: f.mention.clone(), audio: f.audio, live: f.live }),
            }
        }
        if !to.is_empty() {
            self.shared.post(Post::Tone { tone: Box::new(t), to });
        }
    }

    fn call_concluded(&mut self, c: ConcludedCall) {
        let r = &c.call;
        self.shared.digest.call(&r.short_name, r.talkgroup, &r.talkgroup_tag, r.call_length, r.emergency);
        let alerts: Vec<alerts::Alert> = self.shared.alerts.all().into_iter().filter(|a| a.takes_call(&r.short_name, r.talkgroup, r.emergency)).collect();
        if !alerts.is_empty() {
            let (embed, _) = post::call_embed(&c, &self.shared.settings().trp_address);
            let file = c.files.m4a.clone().or_else(|| Some(c.files.wav.clone()));
            let users: Vec<u64> = alerts.iter().map(|a| a.user).collect();
            self.shared.post(Post::Dm { users, embed: Box::new(embed), file });
        }
        let (system, tg) = (c.call.short_name.as_str(), c.call.talkgroup);
        let to = self.to(Event::Call, system, tg, Some(c.call.call_length));
        // A tone thread may take it: one open, or one a tone ahead of it in the queue is opening.
        let threads = self.shared.threads.any_for(system, tg) || self.shared.feeds().iter().any(|f| f.event.threads() && f.picks(system, tg));
        // (Without a bot, nothing is posted: the call isn't held for it.)
        if (to.is_empty() && !threads) || self.shared.posts.lock().unwrap().is_none() {
            let why = if to.is_empty() && !threads { "no feed takes it" } else { "the bot isn't logged in" };
            self.shared.host.call_result(&c.path, Outcome::Skipped, why, "");
            return;
        }
        self.shared.post(Post::Call { call: Box::new(c), to });
    }

    fn status(&mut self, s: Status) {
        {
            let mut rates = self.shared.rates.lock().unwrap();
            for sys in s.systems.iter().filter(|x| x.scope.control_channel_hz.is_some() || x.decode_rate > 0.0) {
                let at = &sys.scope;
                let site = if at.site_name.is_empty() { &at.short_name } else { &at.site_name };
                let r = rates.entry(site.clone()).or_default();
                r.system = at.short_name.clone();
                r.control_channel_hz = at.control_channel_hz;
                r.active = sys.active_calls;
                r.recording = sys.recording;
                r.push(sys.decode_rate, s.time);
            }
        }
        if self.rates_at.is_some_and(|t| t.elapsed() < RATES_EVERY) {
            return;
        }
        self.rates_at = Some(Instant::now());
        // The day: saved, and posted once it's over.
        self.shared.digest.save();
        if let Some(day) = self.shared.digest.roll() {
            let channels: Vec<u64> = self.shared.feeds().iter().filter(|f| f.enabled && f.event == Event::Digest).filter_map(|f| f.channel_id()).collect();
            if !channels.is_empty() && day.calls > 0 {
                self.shared.post(Post::Digest { day: Box::new(day), channels });
            }
        }
        if !self.shared.boards.all().is_empty() {
            self.shared.post(Post::Boards);
        }
        let channels: Vec<(u64, Vec<String>)> =
            self.shared.feeds().iter().filter(|f| f.enabled && f.event == Event::Rates).filter_map(|f| Some((f.channel_id()?, f.systems.clone()))).collect();
        if !channels.is_empty() {
            self.shared.post(Post::Rates { channels });
        }
    }

    fn shutdown(&mut self, _grace: Duration) {
        self.shared.digest.save();
        if let Some(b) = self.bot.take() {
            b.stop(&self.rt);
        }
    }
}

fn main() {
    trunk_recorder_plugin::run::<Discord>();
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use trunk_recorder_plugin::testing;
    use trunk_recorder_plugin::{CallRecord, HostMessage, PluginMessage};

    fn concluded(tg: u32) -> HostMessage {
        HostMessage::CallConcluded(ConcludedCall {
            path: format!("county/2026/11/3/{tg}-1"),
            call: CallRecord { short_name: "county".into(), talkgroup: tg, call_length: 4.0, ..Default::default() },
            ..Default::default()
        })
    }

    /// Without a bot logged in (no token here), every call is answered at once: skipped, so the
    /// application never holds its files for this plugin.
    #[test]
    fn calls_are_answered_without_a_bot() {
        let dir = testing::temp_dir("discord-nobot");
        let settings =
            json!({ "token": "", "servers": [{ "id": "1", "feeds": [{ "name": "fire", "channel": "123456789012345678", "talkgroups": ["100-199"] }] }] });
        let out = testing::run::<Discord>([HostMessage::Hello(testing::hello(&dir, settings)), concluded(150), concluded(250)]);
        let results: Vec<(String, Outcome, String)> = out
            .messages
            .iter()
            .filter_map(|m| match m {
                PluginMessage::CallResult { path, outcome, message, .. } => Some((path.clone(), *outcome, message.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(
            results,
            [
                ("county/2026/11/3/150-1".into(), Outcome::Skipped, "the bot isn't logged in".into()),
                ("county/2026/11/3/250-1".into(), Outcome::Skipped, "no feed takes it".into())
            ]
        );
        assert!(
            out.messages.iter().any(|m| matches!(m, PluginMessage::Status { state: State::Error, message } if message.contains("token"))),
            "{:?}",
            out.messages
        );
    }

    #[test]
    fn bad_feeds_are_refused() {
        let dir = testing::temp_dir("discord-bad");
        let settings = json!({ "servers": [{ "id": "1", "feeds": [{ "name": "x", "channel": "#general" }] }] });
        let out = testing::run::<Discord>([HostMessage::Hello(testing::hello(&dir, settings))]);
        assert!(
            out.messages.iter().any(|m| matches!(m, PluginMessage::Status { state: State::Error, message } if message.contains("channel ID"))),
            "{:?}",
            out.messages
        );
    }
}
