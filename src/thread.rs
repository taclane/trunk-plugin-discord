//! Tone threads. A tone (any, or one of the system's signals a feed picks) on a talkgroup opens a thread under its
//! feed's channel, named `SYSTEM-TG` (`clmrn-I-1001`), started from a header saying what was
//! heard. From then on every recorded call on that talkgroup posts in the thread, until someone
//! presses Stop on the header or its time is up (Resume starts it again). The tone heard again
//! within its time (more callouts for one incident) posts there and gives it more time; once its
//! time is up, it's a new incident: the old thread is closed and a new one opened. Delete
//! ends it: the thread goes, and the header says who ended it. Open threads are kept in the data
//! folder (`threads.json`), so they and their buttons outlast a restart.

use std::path::PathBuf;
use std::sync::Mutex;

use poise::serenity_prelude as serenity;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use serenity::{
    AutoArchiveDuration, ButtonStyle, ChannelId, ComponentInteraction, CreateActionRow, CreateButton, CreateEmbed, CreateEmbedFooter,
    CreateInteractionResponse, CreateInteractionResponseMessage, CreateMessage, CreateThread, EditMessage, MessageId, Timestamp,
};
use trunk_recorder_plugin::{Host, TonePattern, TonePatternEvent};

use crate::post::{GRAY, RED};
use crate::state::Shared;

const FILE: &str = "threads.json";
/// Buttons' ids: `trp-thread:<stop|resume|delete>:<thread id>`.
const PREFIX: &str = "trp-thread";

/// An open tone thread.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Thread {
    /// The server it's in.
    #[serde(default)]
    pub server: u64,
    /// The thread (a channel of its own).
    pub thread: u64,
    /// The channel it's in, and the header it was started from there.
    pub channel: u64,
    pub header: u64,
    pub system: String,
    pub talkgroup: u32,
    /// The talkgroup's alpha tag.
    #[serde(default)]
    pub tag: String,
    /// The header's title and text, to draw it again.
    pub title: String,
    pub text: String,
    /// Unix seconds the tone was heard.
    pub time: f64,
    pub audio: bool,
    /// Who stopped its posts (none: posting).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stopped: Option<String>,
    /// It stopped because its time ran out (`threadTimeoutMin`), not by a press.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub timed_out: bool,
    /// Who's keeping it posting past the timeout (someone's watching it): no timeout while set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kept: Option<String>,
    /// Unix seconds it last started posting: its tone heard (again), or Resume pressed. 0: `time`.
    #[serde(default)]
    pub since: f64,
    #[serde(default)]
    pub calls: u64,
    /// It's a forum's post (`channel` the forum), tagged with `tags`.
    #[serde(default)]
    pub forum: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Tags>,
}

pub struct Threads {
    list: Mutex<Vec<Thread>>,
    path: PathBuf,
}

impl Threads {
    pub fn load(dir: &std::path::Path, host: &Host) -> Threads {
        let path = dir.join(FILE);
        let list = match std::fs::read(&path) {
            Ok(b) => serde_json::from_slice(&b).unwrap_or_else(|e| {
                host.warn(format!("{FILE} unreadable ({e}): its tone threads' buttons won't work"));
                Vec::new()
            }),
            Err(_) => Vec::new(),
        };
        Threads { list: Mutex::new(list), path }
    }

    fn save(&self, list: &[Thread]) -> Result<(), String> {
        let tmp = self.path.with_extension("json.new");
        let json = serde_json::to_vec_pretty(list).map_err(|e| e.to_string())?;
        if let Some(d) = self.path.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        std::fs::write(&tmp, json).and_then(|_| std::fs::rename(&tmp, &self.path)).map_err(|e| format!("couldn't save the tone threads: {e}"))
    }

    /// Change the list with `f`, and save it.
    fn change<T>(&self, host: &Host, f: impl FnOnce(&mut Vec<Thread>) -> T) -> T {
        let mut list = self.list.lock().unwrap();
        let r = f(&mut list);
        if let Err(e) = self.save(&list) {
            host.warn(e);
        }
        r
    }

    pub fn all(&self) -> Vec<Thread> {
        self.list.lock().unwrap().clone()
    }

    /// The thread `id`.
    pub fn get(&self, id: u64) -> Option<Thread> {
        self.list.lock().unwrap().iter().find(|t| t.thread == id).cloned()
    }

    /// The open thread in `channel` for `system`'s `talkgroup`.
    fn in_channel(&self, channel: u64, system: &str, talkgroup: u32) -> Option<Thread> {
        self.list.lock().unwrap().iter().find(|t| t.channel == channel && t.system == system && t.talkgroup == talkgroup).cloned()
    }

    /// The threads `system`'s `talkgroup`'s calls post in now (not stopped).
    pub fn posting(&self, system: &str, talkgroup: u32) -> Vec<Thread> {
        self.list.lock().unwrap().iter().filter(|t| t.system == system && t.talkgroup == talkgroup && t.stopped.is_none()).cloned().collect()
    }

    /// Any thread, stopped or not, is open for `system`'s `talkgroup`.
    pub fn any_for(&self, system: &str, talkgroup: u32) -> bool {
        self.list.lock().unwrap().iter().any(|t| t.system == system && t.talkgroup == talkgroup)
    }

    pub fn update(&self, host: &Host, id: u64, f: impl FnOnce(&mut Thread)) -> Option<Thread> {
        self.change(host, |list| {
            let t = list.iter_mut().find(|t| t.thread == id)?;
            f(t);
            Some(t.clone())
        })
    }

    pub fn remove(&self, host: &Host, id: u64) -> Option<Thread> {
        self.change(host, |list| {
            let at = list.iter().position(|t| t.thread == id)?;
            Some(list.remove(at))
        })
    }

    fn add(&self, host: &Host, t: Thread) {
        self.change(host, |list| list.push(t));
    }

    /// Stop those posting for longer than `after` seconds by `now`: the ones stopped.
    fn time_out(&self, host: &Host, now: f64, after: f64) -> Vec<Thread> {
        self.change(host, |list| {
            let mut out = Vec::new();
            for t in list.iter_mut().filter(|t| t.stopped.is_none() && t.kept.is_none() && now - t.since.max(t.time) >= after) {
                t.stopped = Some(String::new());
                t.timed_out = true;
                out.push(t.clone());
            }
            out
        })
    }
}

impl Thread {
    /// It posts again from `now` (its tone again, Resume): its time counts from then.
    fn go(&mut self, now: f64) {
        self.stopped = None;
        self.timed_out = false;
        self.since = now;
    }

    /// When it stops posting, timing out after `after` seconds (none: it's stopped, or kept).
    pub fn ends(&self, after: f64) -> Option<f64> {
        (self.stopped.is_none() && self.kept.is_none()).then(|| self.since.max(self.time) + after)
    }

    /// It's past its time at `now`: timed out, or stopped by hand longer ago than `after` (none:
    /// no timeout, so never).
    pub fn past(&self, now: f64, after: Option<f64>) -> bool {
        self.stopped.is_some() && (self.timed_out || after.is_some_and(|a| self.kept.is_none() && now - self.since.max(self.time) >= a))
    }

    /// `after` seconds more before it times out (from now, if its time is already up).
    pub fn extend(&mut self, now: f64, after: f64) {
        self.since = (self.since.max(self.time) + after).max(now);
    }
}

pub fn now_s() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

/// Stop tone threads whose time is up (`threadTimeoutMin`), every half minute, while the bot runs.
pub async fn keep_timeouts(shared: std::sync::Arc<Shared>, http: std::sync::Arc<serenity::Http>) {
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        let Some(after) = shared.settings().thread_timeout_s() else { continue };
        for t in shared.threads.time_out(&shared.host, now_s(), after) {
            shared.host.info(format!("thread {}-{} stopped: its {} min were up", t.system, t.talkgroup, (after / 60.0).round()));
            shared.live.unfollow(|f| f.why == crate::live::Why::Thread { thread: t.thread });
            redraw(&shared, &http, &t).await;
        }
    }
}

/// A signal's id (`clmrn-I:STATION_1`) as people read it (`STATION 1`).
pub fn signal_name(id: &str) -> String {
    id.rsplit_once(':').map_or(id, |(_, n)| n).replace('_', " ")
}

/// A tone pattern for a person: "Two-tone 849.6 → 1292.9 Hz (Quick Call II)".
pub fn describe(p: &TonePattern) -> String {
    let hz = |t: &[f64]| t.iter().map(|f| format!("{f:.1}")).collect::<Vec<_>>().join(" → ") + " Hz";
    let mut s = match p.pattern.as_str() {
        "two_tone" => format!("Two-tone {}", hz(&p.tones)),
        "long" => format!("Long tone {}", hz(&p.tones)),
        "hi_low" => format!("Hi-low {}", hz(&p.tones)),
        "pulsed" => format!("Pulsed tone {}", hz(&p.tones)),
        "sequence" => format!("Tone sequence {}", hz(&p.tones)),
        "digits" => format!("{} {}", if p.knox { "KNOX" } else { "DTMF" }, p.digits),
        "morse" => format!("Morse {}", p.digits),
        other => format!("{} {}", other.replace('_', " "), hz(&p.tones)),
    };
    if !p.format.is_empty() {
        s += &format!(" ({})", p.format);
    }
    s
}

/// Where a thread's header is: the channel's message it was started from, or (a forum post) the
/// post's first message.
fn header_at(t: &Thread) -> (ChannelId, MessageId) {
    if t.forum {
        (ChannelId::new(t.thread), MessageId::new(t.thread))
    } else {
        (ChannelId::new(t.channel), MessageId::new(t.header))
    }
}

/// The header: what was heard, and whether its calls post (and play live).
fn header(shared: &Shared, t: &Thread, ended: Option<&str>) -> CreateEmbed {
    let footer = match (ended, &t.stopped) {
        (Some(how), _) => format!("{how} after {} call{}.", t.calls, if t.calls == 1 { "" } else { "s" }),
        (None, Some(_)) if t.timed_out => {
            let min = shared.settings().thread_timeout_min.round();
            format!("Stopped {min} min after the last tone: calls aren't posted in the thread. Resume posts them here again; the tone again opens a new one.")
        }
        (None, Some(who)) => format!("Stopped by {who}: calls aren't posted in the thread. Resume, or the tone again within its time, posts them here again."),
        (None, None) if t.kept.is_some() => format!(
            "Kept posting by {}: every call on this talkgroup posts in the thread until it's stopped, with no time limit.",
            t.kept.as_deref().unwrap_or_default()
        ),
        (None, None) => match shared.settings().thread_timeout_s() {
            Some(after) => format!(
                "Every call on this talkgroup posts in the thread, until it's stopped or {} min pass without the tone. The tone again gives it {0} min more.",
                (after / 60.0).round()
            ),
            None => "Every call on this talkgroup posts in the thread until it's stopped.".to_string(),
        },
    };
    let mut text = t.text.clone();
    if ended.is_none() {
        if let Some(end) = shared.settings().thread_timeout_s().and_then(|after| t.ends(after)) {
            let end = end as i64;
            text += &format!("\n⏱ Posts until <t:{end}:t> (<t:{end}:R>)");
        }
    }
    if ended.is_none() && shared.live.follows_thread(t.thread) {
        if let Some(v) = shared.server(t.server).and_then(|s| s.voice()) {
            text += &format!("\n🔊 **Live** in <#{v}>");
        }
    }
    CreateEmbed::new()
        .title(&t.title)
        .description(text)
        .color(if ended.is_some() || t.stopped.is_some() { GRAY } else { RED })
        .timestamp(Timestamp::from_unix_timestamp(t.time as i64).unwrap_or_else(|_| Timestamp::now()))
        .footer(CreateEmbedFooter::new(footer))
}

fn buttons(shared: &Shared, t: &Thread) -> Vec<CreateActionRow> {
    let mut row = vec![match t.stopped {
        None => CreateButton::new(format!("{PREFIX}:stop:{}", t.thread)).label("Stop posting").style(ButtonStyle::Secondary),
        Some(_) => CreateButton::new(format!("{PREFIX}:resume:{}", t.thread)).label("Resume posting").style(ButtonStyle::Success),
    }];
    if t.stopped.is_none() && shared.settings().thread_timeout_s().is_some() {
        row.push(match t.kept {
            None => CreateButton::new(format!("{PREFIX}:keep:{}", t.thread)).label("⏱ Keep posting").style(ButtonStyle::Secondary),
            Some(_) => CreateButton::new(format!("{PREFIX}:unkeep:{}", t.thread)).label("⏱ Let it time out").style(ButtonStyle::Secondary),
        });
    }
    if shared.server(t.server).and_then(|s| s.voice()).is_some() {
        row.push(if shared.live.follows_thread(t.thread) {
            CreateButton::new(format!("{PREFIX}:live:{}", t.thread)).label("🔇 Stop live audio").style(ButtonStyle::Secondary)
        } else {
            CreateButton::new(format!("{PREFIX}:live:{}", t.thread)).label("🔊 Live audio").style(ButtonStyle::Primary)
        });
    }
    if t.forum {
        row.push(CreateButton::new(format!("{PREFIX}:close:{}", t.thread)).label("Close").style(ButtonStyle::Secondary));
    }
    row.push(
        CreateButton::new(format!("{PREFIX}:delete:{}", t.thread)).label(if t.forum { "Delete post" } else { "Delete thread" }).style(ButtonStyle::Danger),
    );
    vec![CreateActionRow::Buttons(row)]
}

/// Draw a thread's header again (its state changed).
pub async fn redraw(shared: &Shared, http: &serenity::Http, t: &Thread) {
    let (ch, m) = header_at(t);
    let _ = ch.edit_message(http, m, EditMessage::new().embed(header(shared, t, None)).components(buttons(shared, t))).await;
    if t.forum {
        set_tags(shared, t).await;
    }
}

/// Live audio stopped elsewhere (the voice channel's panel): the headers of those from threads.
pub async fn redraw_live(shared: &Shared, http: &serenity::Http, stopped: &[crate::live::Follow]) {
    for f in stopped {
        if let crate::live::Why::Thread { thread } = f.why {
            if let Some(t) = shared.threads.get(thread) {
                redraw(shared, http, &t).await;
            }
        }
    }
}

/// Discord no longer has it (the channel or message was deleted by hand).
pub fn gone(e: &serenity::Error) -> bool {
    matches!(e, serenity::Error::Http(serenity::HttpError::UnsuccessfulRequest(r)) if matches!(r.error.code, 10003 | 10008))
}

// ─── Forum posts ──────────────────────────────────────────────────────────

/// A forum's tags the bot uses, by name.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Tags {
    pub active: u64,
    pub stopped: u64,
    pub closed: u64,
    /// The system's and the signal's.
    pub about: Vec<u64>,
}

/// Tags `names` in forum `forum`, made when it hasn't them (that takes Manage Channels): their
/// ids, in order (0: none could be had).
async fn forum_tags(rest: &crate::rest::Rest, forum: u64, names: &[String]) -> Result<Vec<u64>, String> {
    let ch = rest.get(&format!("/channels/{forum}")).await.map_err(|e| e.to_string())?;
    let mut tags: Vec<Value> = ch["available_tags"].as_array().cloned().unwrap_or_default();
    let has = |tags: &[Value], n: &str| tags.iter().any(|t| t["name"].as_str().is_some_and(|x| x.eq_ignore_ascii_case(n)));
    let missing: Vec<&String> = names.iter().filter(|n| !has(&tags, n)).collect();
    if !missing.is_empty() {
        // (A forum has at most 20.)
        for n in missing.into_iter().take(20usize.saturating_sub(tags.len())) {
            tags.push(json!({ "name": n.chars().take(20).collect::<String>() }));
        }
        let v = rest.patch(&format!("/channels/{forum}"), &json!({ "available_tags": tags })).await.map_err(|e| {
            format!(
                "couldn't add tags to forum {forum}: allow the bot Manage Channel in the forum's permissions (or make tags named {} yourself): {e}",
                names.join(", ")
            )
        })?;
        tags = v["available_tags"].as_array().cloned().unwrap_or_default();
    }
    Ok(names
        .iter()
        .map(|n| {
            tags.iter()
                .find(|t| t["name"].as_str().is_some_and(|x| x.eq_ignore_ascii_case(&n.chars().take(20).collect::<String>())))
                .map_or(0, |t| crate::rest::id(&t["id"]))
        })
        .collect())
}

fn applied(t: &Thread, closed: bool) -> Vec<String> {
    let Some(tags) = &t.tags else { return Vec::new() };
    let state = if closed {
        tags.closed
    } else if t.stopped.is_some() {
        tags.stopped
    } else {
        tags.active
    };
    std::iter::once(state).chain(tags.about.iter().copied()).filter(|&i| i != 0).take(5).map(|i| i.to_string()).collect()
}

async fn set_tags(shared: &Shared, t: &Thread) {
    if let Some(rest) = shared.rest() {
        if let Err(e) = rest.patch(&format!("/channels/{}", t.thread), &json!({ "applied_tags": applied(t, false) })).await {
            shared.host.debug(format!("couldn't tag forum post {}: {e}", t.thread));
        }
    }
}

// ─── Opening ──────────────────────────────────────────────────────────────

/// A thread to open (or the word to give an open one).
pub struct Opening {
    /// The server it's in.
    pub server: u64,
    pub system: String,
    pub talkgroup: u32,
    /// The talkgroup's alpha tag ("" none).
    pub tag: String,
    pub title: String,
    pub text: String,
    /// For a tag on a forum post: the signal's name.
    pub signal: String,
    pub time: f64,
    pub mention: String,
    pub audio: bool,
    /// Play its talkgroup in the server's live audio at once.
    pub live: bool,
}

/// Follow thread `t`'s talkgroup in its server's live audio (there being a voice channel).
fn go_live(shared: &Shared, t: &Thread) {
    if shared.server(t.server).and_then(|s| s.voice()).is_some()
        && shared.live.follow(crate::live::Follow {
            server: t.server,
            system: t.system.clone(),
            talkgroup: t.talkgroup,
            tag: t.tag.clone(),
            why: crate::live::Why::Thread { thread: t.thread },
        })
    {
        shared.host.info(format!("live audio for {}-{} on: its feed plays it live", t.system, t.talkgroup));
    }
}

/// Open a thread in `channel` (a text channel's, or a forum's post) for `o`; or, one being open
/// for that talkgroup there, say so in it, and post again if it was stopped. The thread.
pub async fn open(shared: &Shared, http: &serenity::Http, channel: u64, o: Opening) -> Option<Thread> {
    let host = &shared.host;
    // One past its time (the timeout since its last tone or resume, not kept): over. The tone
    // again is a new incident. Within it (more callouts for the same incident), it posts there.
    let after = shared.settings().thread_timeout_s();
    if let Some(t) = shared.threads.in_channel(channel, &o.system, o.talkgroup).filter(|t| t.past(now_s(), after)) {
        close_for_new(shared, http, &t).await;
    }
    if let Some(t) = shared.threads.in_channel(channel, &o.system, o.talkgroup) {
        let again = CreateEmbed::new().title(format!("{} again", o.title)).description(&o.text).color(RED).timestamp(Timestamp::now());
        match ChannelId::new(t.thread).send_message(http, CreateMessage::new().embed(again)).await {
            Ok(_) => {
                // (Its time counts from this tone, so the header says so.)
                let t = shared.threads.update(host, t.thread, |t| t.go(o.time.max(now_s())))?;
                if o.live {
                    go_live(shared, &t);
                }
                redraw(shared, http, &t).await;
                return Some(t);
            }
            // (Deleted by hand: a new one.)
            Err(e) if gone(&e) => drop(shared.threads.remove(host, t.thread)),
            Err(e) => {
                host.warn(format!("couldn't post in thread {}: {e}", t.thread));
                return None;
            }
        }
    }
    let tg = if o.tag.is_empty() { format!("TG {}", o.talkgroup) } else { format!("{} ({})", o.tag, o.talkgroup) };
    let mut t = Thread {
        server: o.server,
        thread: 0,
        channel,
        header: 0,
        system: o.system.clone(),
        talkgroup: o.talkgroup,
        tag: o.tag.clone(),
        title: o.title.clone(),
        text: o.text.clone(),
        time: o.time,
        audio: o.audio,
        stopped: None,
        timed_out: false,
        kept: None,
        since: o.time,
        calls: 0,
        forum: false,
        tags: None,
    };
    let name = format!("{}-{}", o.system, o.talkgroup);
    let rest = shared.rest();
    let forum = match &rest {
        Some(r) => r.get(&format!("/channels/{channel}")).await.ok().is_some_and(|c| c["type"].as_u64() == Some(15)),
        None => false,
    };
    if forum {
        let r = rest.expect("forum is known through it");
        let mut names = vec!["Active".to_string(), "Stopped".to_string(), "Closed".to_string(), o.system.clone()];
        if !o.signal.is_empty() {
            names.push(o.signal.clone());
        }
        let ids = match forum_tags(&r, channel, &names).await {
            Ok(ids) => ids,
            Err(e) => {
                host.warn(e);
                vec![0; names.len()]
            }
        };
        t.forum = true;
        t.tags = Some(Tags { active: ids[0], stopped: ids[1], closed: ids[2], about: ids[3..].to_vec() });
        let mut message = json!({ "embeds": [header(shared, &t, None)], "allowed_mentions": { "parse": ["everyone", "roles", "users"] } });
        if !o.mention.is_empty() {
            message["content"] = json!(o.mention);
        }
        let body = json!({ "name": format!("{name} · {tg}").chars().take(100).collect::<String>(), "auto_archive_duration": 1440, "applied_tags": applied(&t, false), "message": message });
        match r.post(&format!("/channels/{channel}/threads"), &body).await {
            Ok(v) => {
                t.thread = crate::rest::id(&v["id"]);
                t.header = t.thread;
                shared.threads.add(host, t.clone());
                if o.live {
                    go_live(shared, &t);
                }
                redraw(shared, http, &t).await;
                host.info(format!("forum post {name} opened in channel {channel}: {}", o.title));
                Some(t)
            }
            Err(e) => {
                host.warn(format!("couldn't open a forum post in channel {channel} (it needs Send Messages): {e}"));
                None
            }
        }
    } else {
        let ch = ChannelId::new(channel);
        let mut m = CreateMessage::new().embed(header(shared, &t, None));
        if !o.mention.is_empty() {
            m = m.content(&o.mention);
        }
        let msg = match ch.send_message(http, m).await {
            Ok(m) => m,
            Err(e) => {
                host.warn(format!("couldn't post a thread's header in channel {channel}: {e}"));
                return None;
            }
        };
        match ch.create_thread_from_message(http, msg.id, CreateThread::new(&name).auto_archive_duration(AutoArchiveDuration::OneDay)).await {
            Ok(th) => {
                t.thread = th.id.get();
                t.header = msg.id.get();
                shared.threads.add(host, t.clone());
                if o.live {
                    go_live(shared, &t);
                }
                redraw(shared, http, &t).await;
                host.info(format!("thread {name} opened in channel {channel}: {}", o.title));
                Some(t)
            }
            Err(e) => {
                host.warn(format!("couldn't open a thread in channel {channel} (it needs Create Public Threads): {e}"));
                None
            }
        }
    }
}

/// Close stopped thread `t`, its tone heard again: its header says so, it's archived (a forum
/// post tagged Closed), and it's forgotten.
async fn close_for_new(shared: &Shared, http: &serenity::Http, t: &Thread) {
    let how = format!("Closed: the tone came again <t:{}:t>, after its time was up, and opened a new one", now_s() as i64);
    let (ch, m) = header_at(t);
    let _ = ch.edit_message(http, m, EditMessage::new().embed(header(shared, t, Some(&how))).components(Vec::new())).await;
    if let Some(r) = shared.rest() {
        let body = if t.forum { json!({ "applied_tags": applied(t, true), "archived": true }) } else { json!({ "archived": true }) };
        if let Err(e) = r.patch(&format!("/channels/{}", t.thread), &body).await {
            shared.host.debug(format!("couldn't archive thread {}: {e}", t.thread));
        }
    }
    shared.threads.remove(&shared.host, t.thread);
    shared.live.unfollow(|f| f.why == crate::live::Why::Thread { thread: t.thread });
    shared.host.info(format!("thread {}-{} closed: its tone came again after its time was up", t.system, t.talkgroup));
}

/// A tone for a thread feed in `channel`.
pub async fn tone(shared: &Shared, http: &serenity::Http, tone: &TonePatternEvent, to: &crate::post::To) {
    let (server, channel) = (to.server, to.channel);
    let names: Vec<String> = tone.pattern.signals.iter().map(|s| signal_name(s)).collect();
    let text = describe(&tone.pattern);
    let radio = match (tone.pattern.src, tone.src_tag.as_str()) {
        (0, _) => String::new(),
        (u, "") => format!("\nRadio {u}"),
        (u, tag) => format!("\nRadio {tag} ({u})"),
    };
    let tg = if tone.alpha_tag.is_empty() { format!("TG {}", tone.talkgroup) } else { format!("{} ({})", tone.alpha_tag, tone.talkgroup) };
    let site = if tone.site.is_empty() || tone.site == tone.short_name { tone.short_name.clone() } else { format!("{} ({})", tone.short_name, tone.site) };
    let o = Opening {
        server,
        system: tone.short_name.clone(),
        talkgroup: tone.talkgroup,
        tag: tone.alpha_tag.clone(),
        // (A tone not named: what it was.)
        title: if names.is_empty() { format!("🔔 {text}") } else { format!("🔔 {}", names.join(", ")) },
        text: format!("**{tg}** on {site}\n{text}{radio}"),
        signal: names.first().cloned().unwrap_or_default(),
        time: tone.time,
        mention: to.mention.clone(),
        audio: to.audio,
        live: to.live,
    };
    open(shared, http, channel, o).await;
}

/// A button on a header was pressed. False: it isn't one of these.
pub async fn button(ctx: &serenity::Context, shared: &Shared, b: &ComponentInteraction) -> Result<bool, Box<serenity::Error>> {
    let mut parts = b.data.custom_id.splitn(3, ':');
    let (Some(PREFIX), Some(action), Some(Ok(id))) = (parts.next(), parts.next(), parts.next().map(str::parse::<u64>)) else {
        return Ok(false);
    };
    let host = &shared.host;
    let who = b.member.as_ref().and_then(|m| m.nick.clone()).unwrap_or_else(|| b.user.global_name.clone().unwrap_or_else(|| b.user.name.clone()));
    let say = |text: &str| CreateInteractionResponse::Message(CreateInteractionResponseMessage::new().content(text).ephemeral(true));
    let Some(t) = shared.threads.get(id) else {
        b.create_response(ctx, CreateInteractionResponse::UpdateMessage(CreateInteractionResponseMessage::new().components(Vec::new()))).await?;
        return Ok(true);
    };
    let update = |t: &Thread, ended: Option<&str>| {
        let m = CreateInteractionResponseMessage::new().embed(header(shared, t, ended));
        CreateInteractionResponse::UpdateMessage(if ended.is_some() { m.components(Vec::new()) } else { m.components(buttons(shared, t)) })
    };
    let may_end = b.member.as_ref().and_then(|m| m.permissions).is_some_and(|p| p.manage_threads());
    match action {
        "stop" | "resume" => {
            let stop = action == "stop";
            let Some(t) = shared.threads.update(host, id, |t| {
                if stop {
                    t.stopped = Some(who.clone());
                    t.timed_out = false;
                    t.kept = None;
                } else {
                    t.go(now_s());
                }
            }) else {
                return Ok(true);
            };
            host.info(format!("thread {}-{} {} by {who}", t.system, t.talkgroup, if action == "stop" { "stopped" } else { "resumed" }));
            // (Its live audio stops with it.)
            if stop {
                shared.live.unfollow(|f| f.why == crate::live::Why::Thread { thread: id });
            }
            b.create_response(ctx, update(&t, None)).await?;
            if t.forum {
                set_tags(shared, &t).await;
            }
        }
        "keep" | "unkeep" => {
            let keep = action == "keep";
            let Some(t) = shared.threads.update(host, id, |t| {
                t.kept = keep.then(|| who.clone());
                // (Let go: its time counts from now.)
                if !keep {
                    t.since = now_s();
                }
            }) else {
                return Ok(true);
            };
            host.info(format!("thread {}-{} {} by {who}", t.system, t.talkgroup, if keep { "kept past its timeout" } else { "back on its timeout" }));
            b.create_response(ctx, update(&t, None)).await?;
        }
        "live" => {
            let why = crate::live::Why::Thread { thread: id };
            let on = if shared.live.follows_thread(id) {
                shared.live.unfollow(|f| f.why == why);
                false
            } else {
                let server = b.guild_id.map_or(t.server, |g| g.get());
                shared.live.follow(crate::live::Follow { server, system: t.system.clone(), talkgroup: t.talkgroup, tag: t.tag.clone(), why })
            };
            host.info(format!("live audio for {}-{} {} by {who}", t.system, t.talkgroup, if on { "on" } else { "off" }));
            b.create_response(ctx, update(&t, None)).await?;
        }
        "close" | "delete" => {
            if !may_end {
                b.create_response(ctx, say("Ending a thread takes Manage Threads. Stop posting is open to everyone.")).await?;
                return Ok(true);
            }
            let how = if action == "close" { format!("Closed by {who}") } else { format!("Ended by {who}") };
            if action == "close" {
                // (The header first: an archived post's messages can't be edited.)
                b.create_response(ctx, update(&t, Some(&how))).await?;
                if let Some(r) = shared.rest() {
                    let _ = r.patch(&format!("/channels/{id}"), &json!({ "applied_tags": applied(&t, true), "archived": true, "locked": true })).await;
                }
            } else {
                match ChannelId::new(t.thread).delete(ctx).await {
                    Ok(_) => {}
                    Err(e) if gone(&e) => {}
                    Err(e) => {
                        b.create_response(ctx, say(&format!("Couldn't delete it (the bot needs Manage Threads): {e}"))).await?;
                        return Ok(true);
                    }
                }
                // (A forum post's header went with it.)
                if !t.forum {
                    b.create_response(ctx, update(&t, Some(&how))).await?;
                }
            }
            shared.threads.remove(host, id);
            shared.live.unfollow(|f| f.why == crate::live::Why::Thread { thread: id });
            host.info(format!("thread {}-{} {} by {who}", t.system, t.talkgroup, if action == "close" { "closed" } else { "deleted" }));
        }
        _ => return Ok(false),
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn threads_time_out() {
        let dir = trunk_recorder_plugin::testing::temp_dir("discord-timeout");
        let (host, _) = trunk_recorder_plugin::testing::capture();
        let a = Threads::load(&dir, &host);
        let t = Thread {
            server: 1,
            thread: 9,
            channel: 1,
            header: 2,
            system: "clmrn-I".into(),
            talkgroup: 1001,
            tag: String::new(),
            title: "t".into(),
            text: String::new(),
            time: 1000.0,
            audio: true,
            stopped: None,
            timed_out: false,
            kept: None,
            since: 0.0,
            calls: 0,
            forum: false,
            tags: None,
        };
        a.add(&host, t);
        assert!(a.time_out(&host, 1000.0 + 1799.0, 1800.0).is_empty());
        assert_eq!(a.time_out(&host, 1000.0 + 1800.0, 1800.0).len(), 1);
        assert!(a.posting("clmrn-I", 1001).is_empty() && a.get(9).unwrap().timed_out);
        a.update(&host, 9, |t| {
            t.go(5000.0);
            t.kept = Some("nick".into());
        });
        assert!(a.time_out(&host, 5000.0 + 99999.0, 1800.0).is_empty());
        a.update(&host, 9, |t| t.kept = None);
        assert!(a.time_out(&host, 5000.0 + 60.0, 1800.0).is_empty() && a.posting("clmrn-I", 1001).len() == 1);
        // Stopped by hand within its time: still the incident's; once timed out, past it.
        a.update(&host, 9, |t| t.stopped = Some("nick".into()));
        assert!(!a.get(9).unwrap().past(5000.0 + 60.0, Some(1800.0)));
        assert!(a.get(9).unwrap().past(5000.0 + 1800.0, Some(1800.0)) && !a.get(9).unwrap().past(1e9, None));
        a.update(&host, 9, |t| t.go(5000.0));
        // A second tone: its time counts from that one.
        a.update(&host, 9, |t| t.go(6000.0));
        assert_eq!(a.get(9).unwrap().ends(1800.0), Some(7800.0));
        assert!(a.time_out(&host, 7799.0, 1800.0).is_empty());
        // Extended: 30 min more on what it had, or from now once that's past.
        a.update(&host, 9, |t| t.extend(7000.0, 1800.0));
        assert_eq!(a.get(9).unwrap().ends(1800.0), Some(9600.0));
        a.update(&host, 9, |t| t.extend(20000.0, 1800.0));
        assert_eq!(a.get(9).unwrap().ends(1800.0), Some(21800.0));
    }

    #[test]
    fn tones_read_well() {
        let p = TonePattern { pattern: "two_tone".into(), tones: vec![849.63, 1292.87], format: "Quick Call II".into(), ..Default::default() };
        assert_eq!(describe(&p), "Two-tone 849.6 → 1292.9 Hz (Quick Call II)");
    }

    #[test]
    fn signals_read_as_names() {
        assert_eq!(signal_name("clmrn-I:STATION_1"), "STATION 1");
        assert_eq!(signal_name("PAGE"), "PAGE");
    }

    #[test]
    fn threads_outlast_a_restart() {
        let dir = trunk_recorder_plugin::testing::temp_dir("discord-threads");
        let (host, _) = trunk_recorder_plugin::testing::capture();
        let t = Thread {
            server: 1,
            thread: 9,
            channel: 1,
            header: 2,
            system: "clmrn-I".into(),
            talkgroup: 1001,
            tag: String::new(),
            title: "t".into(),
            text: String::new(),
            time: 0.0,
            audio: true,
            stopped: None,
            timed_out: false,
            kept: None,
            since: 0.0,
            calls: 0,
            forum: false,
            tags: None,
        };
        let a = Threads::load(&dir, &host);
        a.add(&host, t.clone());
        assert_eq!(a.posting("clmrn-I", 1001).len(), 1);
        a.update(&host, 9, |t| t.stopped = Some("nick".into()));
        let b = Threads::load(&dir, &host);
        assert!(b.posting("clmrn-I", 1001).is_empty() && b.any_for("clmrn-I", 1001));
        b.remove(&host, 9);
        assert!(Threads::load(&dir, &host).all().is_empty());
    }
}
