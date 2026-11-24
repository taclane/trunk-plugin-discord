//! The plugin's settings: the bot, and the servers it works in, each with its feeds (what goes to
//! which channel) and live audio. TRP draws the form from this schema on the plugin's
//! own page, and they're set up from either end: on the page, or in Discord (an invite adds the
//! server, `/setup` and `/feed` change it), which the plugin saves to the page (`settings.save`).
//! Names are never typed: the plugin fills in each server's, channel's and feed's from Discord.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use trunk_recorder_plugin::filter::{patterns, TalkgroupFilter};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// Bot token
    ///
    /// Your bot's password for Discord. To get one: sign in at discord.com/developers/applications,
    /// press New Application and name it (that's the bot's name); open Bot on the left, press Reset
    /// Token, then Copy, and paste it here. Save, and Invite to a server appears below. Keep the
    /// token private: anyone with it can act as your bot (Reset Token again makes a new one).
    #[schemars(extend("x-secret" = true, "x-required" = true))]
    pub token: String,
    /// Invite to a server
    ///
    /// The link that adds the bot to a Discord server, with the permissions it needs. Open it,
    /// pick the server (you need Manage Server there), then run /setup in the server. Filled in
    /// once the bot has logged in.
    #[schemars(extend("readOnly" = true, "format" = "uri", "x-open" = "Invite to a server"))]
    #[serde(skip_serializing_if = "String::is_empty")]
    pub invite_link: String,
    /// Bot status
    ///
    /// Shown under the bot's name in the member list. Empty: none.
    pub status_text: String,
    /// Link recordings from
    ///
    /// TRP's address as the people reading the channel reach it (http://100.64.0.5:8091):
    /// posts and /recent link each recording. Empty: no links.
    #[schemars(extend("format" = "uri"))]
    pub trp_address: String,
    /// Servers
    ///
    /// Each server the bot is in. Inviting the bot adds its server here, and /setup and /feed in
    /// Discord change it here too; or add one by its ID and set it up on this page. Names come
    /// from Discord.
    pub servers: Vec<Server>,
    /// Live audio: phrases
    ///
    /// Take plain words in each server's live audio chat ("follow last signal from clmrn-I",
    /// "follow tg 1001", "stop live audio"). Needs the Message Content intent switched on in the
    /// Discord Developer Portal (Bot → Privileged Gateway Intents) first, or the bot can't log in.
    pub live_phrases: bool,
    /// Tone threads: stop after, min
    ///
    /// A tone thread (or forum post) stops posting calls this long after its tone was last heard
    /// (or it was last resumed), so none posts on for good. The tone again, or Resume, starts it
    /// again. 0: they post until someone presses Stop.
    #[schemars(range(min = 0.0), extend("default" = 30))]
    pub thread_timeout_min: f64,

    // ─── Before 0.2: folded into what replaced them ───
    /// (`trpAddress`, as it was called.)
    #[schemars(skip)]
    #[serde(skip_serializing_if = "String::is_empty")]
    pub recorder_address: String,
    // One server's, before servers: folded into `servers`.
    #[schemars(skip)]
    #[serde(skip_serializing_if = "String::is_empty")]
    pub server: String,
    #[schemars(skip)]
    #[serde(skip_serializing_if = "String::is_empty")]
    pub live_channel: String,
    #[schemars(skip)]
    #[serde(skip_serializing_if = "is_zero")]
    pub live_gain_db: f64,
    #[schemars(skip)]
    #[serde(skip_serializing_if = "String::is_empty")]
    pub live_chime: String,
    #[schemars(skip)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub feeds: Vec<Feed>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            token: String::new(),
            invite_link: String::new(),
            status_text: "Archive the Airwaves".into(),
            trp_address: String::new(),
            recorder_address: String::new(),
            servers: Vec::new(),
            live_phrases: false,
            thread_timeout_min: 30.0,
            server: String::new(),
            live_channel: String::new(),
            live_gain_db: 0.0,
            live_chime: String::new(),
            feeds: Vec::new(),
        }
    }
}

impl Settings {
    /// How long a tone thread posts after its tone, seconds (none: until stopped).
    pub fn thread_timeout_s(&self) -> Option<f64> {
        (self.thread_timeout_min > 0.0).then_some(self.thread_timeout_min * 60.0)
    }
}

fn is_zero(x: &f64) -> bool {
    *x == 0.0
}

/// Server
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", default)]
#[schemars(extend(
    "x-title" = "name",
    "x-actions" = [{ "id": "check", "label": "Check access", "hint": "Is the bot in this server, can it see its channels, and has it the permissions it needs?" }]
))]
pub struct Server {
    /// Server
    ///
    /// Its name in Discord, filled in by the plugin.
    #[schemars(extend("readOnly" = true))]
    #[serde(skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// Server ID
    ///
    /// Right-click the server, Copy Server ID (Developer Mode on: User Settings → Advanced).
    #[schemars(extend("x-required" = true))]
    pub id: String,
    /// Live audio
    ///
    /// Followed talkgroups played into a voice channel as they're heard (a thread's 🔊 Live audio
    /// button, /live follow).
    pub live: LiveAudio,
    /// Feeds
    ///
    /// What the bot posts in this server, and where. Feeds can also be added in Discord (/feed
    /// add in a channel): those are kept by the plugin, and /feed list shows both.
    pub feeds: Vec<Feed>,
}

/// A server's live audio.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", default)]
pub struct LiveAudio {
    /// Voice channel ID
    ///
    /// The voice channel it plays in (right-click it, Copy Channel ID). Empty: no live audio in
    /// this server.
    pub channel: String,
    /// Voice channel
    ///
    /// Its name in Discord, filled in by the plugin.
    #[schemars(extend("readOnly" = true))]
    #[serde(skip_serializing_if = "String::is_empty")]
    pub channel_name: String,
    /// Volume adjust, %
    ///
    /// Louder or quieter than usual: +100% is twice as loud, −100% half. 0: as loud as the
    /// recordings (each call brought to one level, as TRP does for them).
    /// Takes effect at once.
    #[schemars(range(min = -300.0, max = 300.0), extend("x-slider" = true, "multipleOf" = 25.0))]
    pub volume: f64,
    /// Before 0.3: the gain in dB, folded into `volume`.
    #[schemars(skip)]
    #[serde(skip_serializing)]
    pub gain_db: f64,
    /// Chime
    ///
    /// A soundboard sound's ID, played as a tone thread's audio starts (right-click the sound,
    /// Copy ID). Empty: none.
    pub chime: String,
    /// Log who's talking
    ///
    /// Each call that plays gets a line in the voice channel's chat naming its radios
    /// (clmrn-I-1001-1234567 (Engine 1)).
    pub log_speakers: bool,
}

impl Default for LiveAudio {
    fn default() -> Self {
        LiveAudio { channel: String::new(), channel_name: String::new(), volume: 0.0, gain_db: 0.0, chime: String::new(), log_speakers: true }
    }
}

/// Each 25% of volume, in dB: +100% is +6 dB (twice the amplitude), ±300% ±18 dB.
pub const DB_PER_STEP: f64 = 1.5;

impl LiveAudio {
    /// The volume adjust as a gain, dB.
    pub fn db(&self) -> f64 {
        self.volume.clamp(-300.0, 300.0) / 25.0 * DB_PER_STEP
    }
}

impl Server {
    pub fn guild(&self) -> Option<u64> {
        self.id.trim().parse().ok().filter(|&g| g != 0)
    }

    pub fn voice(&self) -> Option<u64> {
        self.live.channel.trim().parse().ok().filter(|&c| c != 0)
    }
}

/// Feed
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", default)]
#[schemars(extend(
    "x-title" = "name",
    "x-summary" = ["{event}", "{systems|any system}", "TG {talkgroups}", "except {exceptTalkgroups}"]
))]
pub struct Feed {
    /// Feed
    ///
    /// Named by the plugin after its channel and what it posts (#dispatch call): /feed in
    /// Discord goes by it.
    #[schemars(extend("readOnly" = true))]
    pub name: String,
    /// The server it's in (its ID): its place in `servers` says, for one there; one added in
    /// Discord keeps it here.
    #[schemars(skip)]
    #[serde(skip_serializing_if = "String::is_empty")]
    pub server: String,
    /// On
    pub enabled: bool,
    /// Channel ID
    ///
    /// Right-click the channel, Copy Channel ID (Developer Mode on).
    #[schemars(extend("x-required" = true))]
    pub channel: String,
    /// What
    ///
    /// Recorded calls: each as it finishes, with its recording. Calls as they start: recorded or
    /// not, a live log. Emergencies: each emergency call once, as soon as it's flagged. Decode
    /// rates: the control channels', in one message kept up to date. Tones: a tone heard (any,
    /// or one of the Signals picked) opens a thread for its talkgroup (SYSTEM-TG), and every call
    /// on the talkgroup posts there until someone presses Stop on its header (or Delete); in a forum
    /// channel, each is a post, tagged with its system, signal and state. Daily digest: the
    /// day's figures, at midnight.
    pub event: Event,
    /// Systems
    ///
    /// Empty: any.
    #[schemars(extend("x-systems" = true))]
    pub systems: Vec<String>,
    /// Signals
    ///
    /// Which (empty: any signal of the systems'). Signals are named in Setup (each system's Tones).
    #[schemars(extend("x-signals" = true, "x-when" = { "event": "signal" }))]
    pub signals: Vec<String>,
    /// Kinds of tone
    ///
    /// Empty: any.
    #[schemars(extend("x-when" = { "event": "tone" }))]
    pub patterns: Vec<crate::tones::Pattern>,
    /// Tones
    ///
    /// Particular tones, typed in: a page from a paging list that hasn't been named yet, say.
    /// Empty: any tone of the kinds above. (Tones heard and named are signals: "Signals".)
    #[schemars(extend("x-tones" = true, "x-when" = { "event": "tone" }))]
    pub tones: Vec<crate::tones::ToneSpec>,
    /// Talkgroups
    ///
    /// Numbers, ranges (1000-1099) and patterns (507*). Empty: any.
    #[serde(deserialize_with = "patterns")]
    #[schemars(extend("x-when" = { "event": ["call", "start", "emergency", "signal", "tone"] }))]
    pub talkgroups: Vec<String>,
    /// Except talkgroups
    #[serde(deserialize_with = "patterns")]
    #[schemars(extend("x-when" = { "event": ["call", "start", "emergency", "signal", "tone"] }))]
    pub except_talkgroups: Vec<String>,
    /// Attach the recording
    #[schemars(extend("x-when" = { "event": ["call", "signal", "tone"] }))]
    pub audio: bool,
    /// Live audio at once
    ///
    /// Each thread it opens plays its talkgroup in the server's live audio straight away, as if
    /// 🔊 Live audio were pressed, until the thread stops posting (its time up, or Stop).
    #[schemars(extend("x-when" = { "event": ["signal", "tone"] }))]
    pub live: bool,
    /// Shortest call, s
    ///
    /// Calls shorter than this aren't posted. 0: all.
    #[schemars(range(min = 0.0))]
    #[schemars(extend("x-when" = { "event": "call" }))]
    pub min_length_s: f64,
    /// Message
    ///
    /// Text above each post: mentions go here (@everyone, <@user id>, <@&role id>).
    pub mention: String,
}

impl Default for Feed {
    fn default() -> Self {
        Feed {
            name: String::new(),
            server: String::new(),
            enabled: true,
            channel: String::new(),
            event: Event::Call,
            systems: Vec::new(),
            signals: Vec::new(),
            patterns: Vec::new(),
            tones: Vec::new(),
            talkgroups: Vec::new(),
            except_talkgroups: Vec::new(),
            audio: true,
            live: false,
            min_length_s: 0.0,
            mention: String::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Event {
    /// Recorded calls
    #[default]
    Call,
    /// Calls as they start
    Start,
    /// Emergencies
    Emergency,
    /// Decode rates
    Rates,
    /// Signals: a thread each
    Signal,
    /// Tones: a thread each
    Tone,
    /// Daily digest
    Digest,
}

impl Event {
    /// One word for it, in a feed's name.
    pub fn word(self) -> &'static str {
        match self {
            Event::Call => "call",
            Event::Start => "start",
            Event::Emergency => "emergency",
            Event::Rates => "rates",
            Event::Signal => "signal",
            Event::Tone => "tone",
            Event::Digest => "digest",
        }
    }

    /// It opens a thread for each talkgroup it's heard on.
    pub fn threads(self) -> bool {
        matches!(self, Event::Signal | Event::Tone)
    }

    pub fn label(self) -> &'static str {
        match self {
            Event::Call => "recorded calls",
            Event::Start => "calls as they start",
            Event::Emergency => "emergencies",
            Event::Rates => "decode rates",
            Event::Signal => "signals (a thread for each talkgroup)",
            Event::Tone => "tones (a thread for each talkgroup)",
            Event::Digest => "the daily digest",
        }
    }
}

impl Feed {
    /// The channel, as a number (none: not set, or not a Discord ID).
    pub fn channel_id(&self) -> Option<u64> {
        self.channel.trim().parse().ok().filter(|&id| id != 0)
    }

    /// It takes what's heard on `system`'s `talkgroup`.
    pub fn picks(&self, system: &str, talkgroup: u32) -> bool {
        self.enabled
            && (self.systems.is_empty() || self.systems.iter().any(|s| s == system))
            && TalkgroupFilter::new(&self.talkgroups, &self.except_talkgroups).passes(talkgroup)
    }

    /// It takes a tone that is these `signals` (`system:ID`): any, when it names none. A tone
    /// not named is no signal.
    pub fn picks_signals(&self, signals: &[String]) -> bool {
        !signals.is_empty() && (self.signals.is_empty() || self.signals.iter().any(|w| signals.iter().any(|s| same_signal(s, w))))
    }

    /// It opens a thread for tone `p` (as the notify plugin's rules: Signals, the named ones it
    /// picks; Tones, any of its kinds and typed tones, named or not).
    pub fn picks_tone(&self, p: &trunk_recorder_plugin::TonePattern) -> bool {
        match self.event {
            Event::Signal => self.picks_signals(&p.signals),
            Event::Tone => {
                (self.patterns.is_empty() || self.patterns.iter().any(|k| k.name() == crate::tones::kind(p)))
                    && (self.tones.is_empty() || self.tones.iter().any(|t| crate::tones::tone_is(t, p)))
            }
            _ => false,
        }
    }

    /// What it's called: its name, or (none given) what it posts and where (`<#id>`, which
    /// Discord shows as the channel's name).
    pub fn title(&self) -> String {
        if self.name.trim().is_empty() {
            format!("{} → <#{}>", self.event.label(), self.channel.trim())
        } else {
            self.name.clone()
        }
    }

    /// Who it is, among others: its name, else its channel and kind.
    fn key(&self) -> String {
        if self.name.trim().is_empty() {
            format!("{}:{:?}", self.channel.trim(), self.event)
        } else {
            self.name.trim().to_lowercase()
        }
    }

    /// One line on what it is, for /feed list.
    pub fn describe(&self) -> String {
        let mut s = if self.name.trim().is_empty() { self.event.label().to_string() } else { format!("**{}**: {}", self.name, self.event.label()) };
        if !self.systems.is_empty() {
            s += &format!(" on {}", self.systems.join(", "));
        }
        if self.event == Event::Signal && !self.signals.is_empty() {
            s += &format!(" ({})", self.signals.iter().map(|x| crate::thread::signal_name(x)).collect::<Vec<_>>().join(", "));
        }
        if self.event == Event::Tone && !self.patterns.is_empty() {
            s += &format!(" ({})", self.patterns.iter().map(|k| k.words()).collect::<Vec<_>>().join(", "));
        }
        if self.event == Event::Tone && !self.tones.is_empty() {
            s += &format!(", {} typed tone{}", self.tones.len(), if self.tones.len() == 1 { "" } else { "s" });
        }
        if self.event.threads() && self.live {
            s += ", 🔊 live at once";
        }
        if !matches!(self.event, Event::Rates | Event::Digest) {
            let f = TalkgroupFilter::new(&self.talkgroups, &self.except_talkgroups);
            if !f.is_empty() {
                s += &format!(", {}", f.describe());
            }
        }
        if self.event == Event::Call {
            if !self.audio {
                s += ", no recording";
            }
            if self.min_length_s > 0.0 {
                s += &format!(", {} s and longer", self.min_length_s);
            }
        }
        if !self.enabled {
            s += " (off)";
        }
        s + &format!(" → <#{}>", self.channel)
    }
}

/// Two signals are one: their IDs match, and their systems too when both name one (`county:PD_PAGE`
/// is `PD_PAGE`, but not `city:PD_PAGE`).
pub fn same_signal(a: &str, b: &str) -> bool {
    let split = |s: &str| s.trim().rsplit_once(':').map_or((None, s.trim().to_string()), |(sys, id)| (Some(sys.to_string()), id.to_string()));
    let ((sa, ia), (sb, ib)) = (split(a), split(b));
    ia.eq_ignore_ascii_case(&ib)
        && match (sa, sb) {
            (Some(x), Some(y)) => x == y,
            _ => true,
        }
}

impl Settings {
    /// As they're used: what came before servers folded into one (whose ID, when that had none,
    /// is the one server the bot is in, once it's known), and each server's feeds marked with it.
    pub fn normalized(mut self, only_guild: Option<u64>) -> Settings {
        let old_address = std::mem::take(&mut self.recorder_address);
        if self.trp_address.trim().is_empty() {
            self.trp_address = old_address;
        }
        if !self.feeds.is_empty() || !self.live_channel.trim().is_empty() || !self.server.trim().is_empty() {
            let old = Server {
                name: "Discord".into(),
                id: std::mem::take(&mut self.server),
                live: LiveAudio {
                    channel: std::mem::take(&mut self.live_channel),
                    gain_db: std::mem::take(&mut self.live_gain_db),
                    chime: std::mem::take(&mut self.live_chime),
                    ..Default::default()
                },
                feeds: std::mem::take(&mut self.feeds),
            };
            // Into the server it was: the one with its ID, or, having none, the bot's one server
            // (or the only one listed). Its feeds that are there already by name aren't added again.
            let at = self.servers.iter().position(|s| s.id.trim() == old.id.trim()).or_else(|| {
                old.id.trim().is_empty().then(|| self.servers.iter().position(|s| only_guild.is_some_and(|g| s.guild() == Some(g)))).flatten().or((self
                    .servers
                    .len()
                    == 1
                    && old.id.trim().is_empty())
                .then_some(0))
            });
            match at {
                Some(i) => {
                    let s = &mut self.servers[i];
                    for f in old.feeds {
                        if !s.feeds.iter().any(|x| x.key() == f.key()) {
                            s.feeds.push(f);
                        }
                    }
                    if s.voice().is_none() && !old.live.channel.trim().is_empty() {
                        s.live = old.live;
                    }
                }
                None => self.servers.push(old),
            }
        }
        if let Some(g) = only_guild {
            for s in self.servers.iter_mut().filter(|s| s.id.trim().is_empty()) {
                s.id = g.to_string();
            }
        }
        for s in &mut self.servers {
            // (Before 0.3, a tone feed naming signals was what's now Signals.)
            for f in s.feeds.iter_mut().filter(|f| f.event == Event::Tone && !f.signals.is_empty()) {
                f.event = Event::Signal;
            }
            // (A gain in dB, from before: the nearest step of volume.)
            let db = std::mem::take(&mut s.live.gain_db);
            if db != 0.0 && s.live.volume == 0.0 {
                s.live.volume = (db / DB_PER_STEP).round().clamp(-12.0, 12.0) * 25.0;
            }
            let id = s.id.trim().to_string();
            for f in &mut s.feeds {
                f.server = id.clone();
            }
        }
        self
    }

    /// Every server's feeds (each marked with its server).
    pub fn all_feeds(&self) -> Vec<Feed> {
        self.servers.iter().flat_map(|s| s.feeds.iter().cloned()).collect()
    }

    pub fn server(&self, guild: u64) -> Option<&Server> {
        self.servers.iter().find(|s| s.guild() == Some(guild))
    }

    /// The servers' IDs (where the commands are registered).
    pub fn guilds(&self) -> Vec<u64> {
        let mut g: Vec<u64> = Vec::new();
        for id in self.servers.iter().filter_map(Server::guild) {
            if !g.contains(&id) {
                g.push(id);
            }
        }
        g
    }

    /// Fill in the names from Discord: each server's (`guild`), its voice channel's and each
    /// feed's (`channel`: a channel's name, without #), feeds being named after their channel and
    /// what they post, a number after those that would share one. What isn't known (the bot can't
    /// see it) keeps the name it has. Whether any changed.
    pub fn named(&mut self, invite: Option<String>, guild: impl Fn(u64) -> Option<String>, channel: impl Fn(u64, u64) -> Option<String>) -> bool {
        let before = self.clone();
        if let Some(i) = invite {
            self.invite_link = i;
        }
        let mut taken: Vec<String> = Vec::new();
        for s in &mut self.servers {
            let Some(g) = s.guild() else { continue };
            if let Some(n) = guild(g) {
                s.name = n;
            }
            match s.voice() {
                Some(v) => {
                    if let Some(n) = channel(g, v) {
                        s.live.channel_name = n;
                    }
                }
                None => s.live.channel_name.clear(),
            }
            for f in &mut s.feeds {
                let Some(c) = f.channel_id() else { continue };
                let base = match channel(g, c) {
                    Some(n) => format!("#{n} {}", f.event.word()),
                    None if !f.name.trim().is_empty() => f.name.trim().trim_end_matches(|ch: char| ch.is_ascii_digit()).trim_end().to_string(),
                    None => format!("channel {c} {}", f.event.word()),
                };
                let name = std::iter::once(base.clone())
                    .chain((2..).map(|i| format!("{base} {i}")))
                    .find(|n| !taken.iter().any(|t| t.eq_ignore_ascii_case(n)))
                    .unwrap();
                taken.push(name.clone());
                f.name = name;
            }
        }
        *self != before
    }

    /// They still have what came before servers (folded into them as they're used).
    pub fn has_old(&self) -> bool {
        !self.recorder_address.is_empty()
            || !self.server.is_empty()
            || !self.live_channel.is_empty()
            || self.live_gain_db != 0.0
            || !self.live_chime.is_empty()
            || !self.feeds.is_empty()
            || self.servers.iter().any(|s| s.live.gain_db != 0.0 || s.feeds.iter().any(|f| f.event == Event::Tone && !f.signals.is_empty()))
    }

    /// What they're saved to TRP as (`settings.save`): the servers, and nulls for what
    /// came before servers (folded into them).
    pub fn to_save(&self) -> serde_json::Map<String, serde_json::Value> {
        let mut servers = self.servers.clone();
        for f in servers.iter_mut().flat_map(|s| s.feeds.iter_mut()) {
            f.server.clear();
        }
        let mut m = serde_json::Map::new();
        m.insert("servers".into(), serde_json::to_value(&servers).unwrap_or_default());
        m.insert("trpAddress".into(), self.trp_address.clone().into());
        m.insert("inviteLink".into(), if self.invite_link.is_empty() { serde_json::Value::Null } else { self.invite_link.clone().into() });
        for k in ["recorderAddress", "server", "liveChannel", "liveGainDb", "liveChime", "feeds"] {
            m.insert(k.into(), serde_json::Value::Null);
        }
        m
    }

    /// What's wrong with them, for the settings form and /feed add.
    pub fn check(&self) -> Result<(), String> {
        let check_feeds = |feeds: &[Feed]| -> Result<(), String> {
            // (One being filled in, its channel not given yet, is just not used.)
            for f in feeds.iter().filter(|f| !f.channel.trim().is_empty()) {
                if f.channel_id().is_none() {
                    return Err(format!("feed {:?}: its channel isn't a channel ID ({:?})", f.name, f.channel));
                }
                TalkgroupFilter::parse(&f.talkgroups, &f.except_talkgroups).map_err(|e| format!("feed {}: {e}", f.title()))?;
            }
            Ok(())
        };
        check_feeds(&self.feeds)?;
        for s in &self.servers {
            let what = format!("server {}", s.id.trim());
            if !s.id.trim().is_empty() && s.guild().is_none() {
                return Err(format!("{what}: its ID isn't a server ID ({:?})", s.id));
            }
            if !s.live.channel.trim().is_empty() && s.voice().is_none() {
                return Err(format!("{what}: the live audio channel isn't a channel ID ({:?})", s.live.channel));
            }
            check_feeds(&s.feeds).map_err(|e| format!("{what}: {e}"))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_feed_picks_by_system_and_talkgroup() {
        let f = Feed { systems: vec!["county".into()], talkgroups: vec!["100-199".into()], except_talkgroups: vec!["150".into()], ..Default::default() };
        assert!(f.picks("county", 120));
        assert!(!f.picks("county", 150));
        assert!(!f.picks("city", 120));
        assert!(!Feed { enabled: false, ..f }.picks("county", 120));
    }

    #[test]
    fn signals_match_with_or_without_their_system() {
        let f = Feed { event: Event::Signal, signals: vec!["county:PW_CWID".into()], ..Default::default() };
        assert!(f.picks_signals(&["PW_CWID".into()]));
        assert!(f.picks_signals(&["county:PW_CWID".into()]));
        assert!(!f.picks_signals(&["city:PW_CWID".into()]));
        assert!(!f.picks_signals(&["PD_PAGE".into()]));
        assert!(!f.picks_signals(&[]));
        assert!(Feed { signals: vec!["pw_cwid".into()], ..f }.picks_signals(&["county:PW_CWID".into()]));
    }

    /// As the notify plugin's rules: Signals take named tones only; Tones any tone, named or not,
    /// of their kinds.
    #[test]
    fn signals_feeds_take_named_tones_and_tones_feeds_any() {
        let unnamed = trunk_recorder_plugin::TonePattern { pattern: "two_tone".into(), tones: vec![781.25, 875.0], ..Default::default() };
        let named = trunk_recorder_plugin::TonePattern { signals: vec!["PD_PAGE".into()], ..unnamed.clone() };
        let signals = Feed { event: Event::Signal, ..Default::default() };
        assert!(!signals.picks_tone(&unnamed) && signals.picks_tone(&named));
        let tones = Feed { event: Event::Tone, ..Default::default() };
        assert!(tones.picks_tone(&unnamed) && tones.picks_tone(&named));
        let longs = Feed { patterns: vec![crate::tones::Pattern::Long], ..tones.clone() };
        assert!(!longs.picks_tone(&unnamed));
        let typed = Feed { tones: vec![crate::tones::ToneSpec { tones_hz: vec![781.25, 875.0], ..Default::default() }], ..tones };
        assert!(typed.picks_tone(&unnamed));
        assert!(!Feed { event: Event::Call, ..Default::default() }.picks_tone(&named));
    }

    /// A tone feed from before that names signals is a Signals feed; one naming none, any tone.
    #[test]
    fn old_tone_feeds_naming_signals_become_signals_feeds() {
        let old: Settings = serde_json::from_value(serde_json::json!({ "servers": [{ "id": "1", "feeds": [
            { "channel": "9", "event": "tone", "signals": ["county:PD_PAGE"] },
            { "channel": "8", "event": "tone", "systems": ["CLMRN"] } ] }] }))
        .unwrap();
        assert!(old.has_old());
        let s = old.normalized(None);
        assert_eq!(s.servers[0].feeds.iter().map(|f| f.event).collect::<Vec<_>>(), vec![Event::Signal, Event::Tone]);
        assert!(!s.has_old());
    }

    /// One server's settings from before servers become a server of their own, its feeds
    /// marked with it; with no ID, the bot's one server's.
    #[test]
    fn the_old_settings_become_a_server() {
        let old: Settings = serde_json::from_value(
            serde_json::json!({ "token": "t", "server": "", "liveChannel": "55", "feeds": [{ "name": "Feedz", "channel": "9", "event": "tone" }] }),
        )
        .unwrap();
        let s = old.normalized(Some(42));
        assert_eq!(s.servers.len(), 1);
        assert_eq!((s.servers[0].id.as_str(), s.servers[0].voice()), ("42", Some(55)));
        assert_eq!(s.all_feeds()[0].server, "42");
        assert!(s.feeds.is_empty() && s.live_channel.is_empty());
        // (Saved again, only the new shape.)
        let v = serde_json::to_value(&s).unwrap();
        assert!(v.get("feeds").is_none() && v.get("server").is_none(), "{v}");
    }

    /// Old settings left beside a server added on the page fold into it, not into a second one.
    #[test]
    fn old_settings_join_the_server_added_beside_them() {
        let both: Settings = serde_json::from_value(serde_json::json!({
            "server": "", "feeds": [{ "name": "Feedz", "channel": "9" }, { "name": "Old", "channel": "8" }],
            "servers": [{ "name": "Test Server", "id": "42", "feeds": [{ "name": "feedz", "channel": "9" }] }]
        }))
        .unwrap();
        let s = both.normalized(Some(42));
        assert_eq!(s.servers.len(), 1);
        assert_eq!(s.servers[0].feeds.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), ["feedz", "Old"]);
    }

    /// A feed half filled in on the page (no channel yet) doesn't stop the rest being taken.
    #[test]
    fn a_feed_being_filled_in_is_left_out() {
        let s = Settings {
            servers: vec![Server {
                id: "1".into(),
                feeds: vec![Feed::default(), Feed { name: "x".into(), channel: "9".into(), ..Default::default() }],
                ..Default::default()
            }],
            ..Default::default()
        };
        assert!(s.check().is_ok());
        let bad = Settings {
            servers: vec![Server { id: "1".into(), feeds: vec![Feed { channel: "#general".into(), ..Default::default() }], ..Default::default() }],
            ..Default::default()
        };
        assert!(bad.check().is_err());
        let twice =
            Settings { servers: vec![Server { id: "1".into(), ..Default::default() }, Server { id: "1".into(), ..Default::default() }], ..Default::default() };
        assert_eq!(twice.guilds(), [1]);
    }

    /// Names come from Discord: the server's, its voice channel's, each feed's after its channel
    /// and kind (numbered when two would share one); one the bot can't see keeps its own.
    #[test]
    fn a_gain_in_db_becomes_the_nearest_volume() {
        let old: Settings = serde_json::from_value(serde_json::json!({ "servers": [{ "id": "1", "live": { "channel": "5", "gainDb": 6.4 } }] })).unwrap();
        assert!(old.has_old());
        let s = old.normalized(None);
        assert_eq!((s.servers[0].live.volume, s.servers[0].live.db()), (100.0, 6.0));
        assert!(!s.has_old() && !serde_json::to_string(&s.to_save()).unwrap().contains("gainDb"));
    }

    #[test]
    fn names_are_filled_in_from_discord() {
        let feed = |ch: &str, e: Event| Feed { channel: ch.into(), event: e, ..Default::default() };
        let mut s = Settings {
            servers: vec![Server {
                id: "1".into(),
                live: LiveAudio { channel: "50".into(), ..Default::default() },
                feeds: vec![
                    feed("9", Event::Call),
                    feed("9", Event::Call),
                    feed("9", Event::Tone),
                    Feed { name: "#gone call".into(), ..feed("8", Event::Call) },
                ],
                ..Default::default()
            }],
            ..Default::default()
        };
        let guild = |g: u64| (g == 1).then(|| "Test Server".to_string());
        let channel = |_: u64, c: u64| match c {
            9 => Some("dispatch".to_string()),
            50 => Some("General".to_string()),
            _ => None,
        };
        assert!(s.named(Some("https://discord.com/oauth2/authorize?client_id=1".into()), guild, channel));
        assert_eq!((s.servers[0].name.as_str(), s.servers[0].live.channel_name.as_str()), ("Test Server", "General"));
        let names: Vec<&str> = s.servers[0].feeds.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["#dispatch call", "#dispatch call 2", "#dispatch tone", "#gone call"]);
        assert!(!s.named(None, guild, channel), "nothing new: nothing changed");
        assert!(s.invite_link.ends_with("client_id=1"));
        let old: Settings = serde_json::from_value(serde_json::json!({ "recorderAddress": "http://scanner:8091" })).unwrap();
        assert!(old.has_old() && old.clone().normalized(None).trp_address == "http://scanner:8091");
        assert_eq!(old.normalized(None).to_save()["recorderAddress"], serde_json::Value::Null, "saved under its new name");
        let saved = s.to_save();
        assert!(saved["feeds"].is_null() && saved["servers"][0]["feeds"][0].get("server").is_none(), "{saved:?}");
    }
}
