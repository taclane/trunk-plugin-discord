//! Posting to channels: what the plugin's main thread hands the bot, and the task that sends it.
//! Serenity keeps to Discord's rate limits, waiting when a channel is busy.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use poise::serenity_prelude as serenity;
use serenity::{ChannelId, CreateAttachment, CreateEmbed, CreateEmbedFooter, CreateMessage, EditMessage, MessageId, Timestamp};
use tokio::sync::mpsc;
use trunk_recorder_plugin::{CallInfo, ConcludedCall, Outcome, TonePatternEvent};

use crate::state::{mhz, span, Shared};

pub const RED: u32 = 0xd83c3e;
pub const GREEN: u32 = 0x3ba55d;
pub const BLUE: u32 = 0x4f7cff;
pub const GRAY: u32 = 0x80848e;

/// Where a post goes: a channel, and the text above it.
#[derive(Clone, Debug)]
pub struct To {
    /// The server it's in (0: not known).
    pub server: u64,
    pub channel: u64,
    pub mention: String,
    pub audio: bool,
    /// Tones: play the thread's talkgroup live at once.
    pub live: bool,
}

pub enum Post {
    /// A recorded call, to each of `to`; its result goes to TRP once it's sent.
    Call { call: Box<ConcludedCall>, to: Vec<To> },
    /// A call starting, or an emergency.
    Start { call: Box<CallInfo>, emergency: bool, to: Vec<To> },
    /// Bring the decode rate messages in these channels up to date.
    Rates { channels: Vec<(u64, Vec<String>)> },
    /// A tone, for the thread feeds in `to` (their channels).
    Tone { tone: Box<TonePatternEvent>, to: Vec<To> },
    /// Direct messages: `embed`, with the recording at `file` when there is one.
    Dm { users: Vec<u64>, embed: Box<CreateEmbed>, file: Option<std::path::PathBuf> },
    /// Bring the status boards up to date.
    Boards,
    /// A day's digest, to these channels.
    Digest { day: Box<crate::digest::Day>, channels: Vec<u64> },
}

/// Where a post's footer says it's from: "Trunk Recorder Pro · clmrn-I · TG 1001" (the
/// right-click commands read it back).
pub fn footer(system: &str, talkgroup: u32) -> CreateEmbedFooter {
    CreateEmbedFooter::new(format!("Trunk Recorder Pro · {system} · TG {talkgroup}"))
}

/// The system and talkgroup a post's footer names ([`footer`]).
pub fn from_footer(text: &str) -> Option<(String, u32)> {
    let rest = text.strip_prefix("Trunk Recorder Pro · ")?;
    let (system, tg) = rest.rsplit_once(" · TG ")?;
    Some((system.to_string(), tg.trim().parse().ok()?))
}

/// Send what comes, until the bot stops.
pub async fn poster(shared: Arc<Shared>, http: Arc<serenity::Http>, cache: Arc<serenity::Cache>, mut rx: mpsc::UnboundedReceiver<Post>) {
    // The decode rate message in each channel, edited from then on.
    let mut rates: HashMap<u64, MessageId> = HashMap::new();
    // Each user's direct message channel.
    let mut dms: HashMap<u64, ChannelId> = HashMap::new();
    while let Some(p) = rx.recv().await {
        match p {
            Post::Call { call, to } => {
                let address = shared.settings().trp_address;
                let (embed, link) = call_embed(&call, &address);
                // (Its tone threads are looked up now, behind the tone that may have opened one.)
                let threads = shared.threads.posting(&call.call.short_name, call.call.talkgroup);
                let mut to = to;
                to.extend(threads.iter().map(|t| To { server: t.server, channel: t.thread, mention: String::new(), audio: t.audio, live: false }));
                if to.is_empty() {
                    shared.host.call_result(&call.path, Outcome::Skipped, "no feed takes it", "");
                    continue;
                }
                let audio = audio_file(&call).await;
                let (mut sent, mut failed) = (Vec::new(), Vec::new());
                for t in &to {
                    let thread = threads.iter().any(|th| th.thread == t.channel);
                    let mut m = CreateMessage::new().embed(embed.clone());
                    if !t.mention.is_empty() {
                        m = m.content(&t.mention);
                    }
                    if let (true, Some((bytes, name))) = (t.audio, &audio) {
                        m = m.add_file(CreateAttachment::bytes(bytes.clone(), name.clone()));
                    }
                    let result = ChannelId::new(t.channel).send_message(&http, m).await;
                    if thread {
                        match &result {
                            Ok(_) => drop(shared.threads.update(&shared.host, t.channel, |th| th.calls += 1)),
                            // (Deleted by hand: closed.)
                            Err(e) if crate::thread::gone(e) => drop(shared.threads.remove(&shared.host, t.channel)),
                            Err(_) => {}
                        }
                    }
                    match result {
                        // (A sent message doesn't say its server: the cache knows the channel's.)
                        Ok(msg) => sent.push(
                            match cache.guilds().into_iter().find(|g| {
                                cache.guild(*g).is_some_and(|g| g.channels.contains_key(&msg.channel_id) || g.threads.iter().any(|t| t.id == msg.channel_id))
                            }) {
                                Some(g) => format!("https://discord.com/channels/{g}/{}/{}", msg.channel_id, msg.id),
                                None => msg.link(),
                            },
                        ),
                        Err(e) => failed.push(format!("<#{}>: {e}", t.channel)),
                    }
                }
                shared.posted.fetch_add(sent.len() as u64, std::sync::atomic::Ordering::Relaxed);
                let url = sent.first().cloned().or(link).unwrap_or_default();
                let (outcome, message) = match (sent.len(), failed.len()) {
                    (_, 0) => (Outcome::Ok, format!("posted to {} channel{}", sent.len(), if sent.len() == 1 { "" } else { "s" })),
                    (0, _) => (Outcome::Failed, failed.join("; ")),
                    _ => (Outcome::Ok, format!("posted to {}; not to {}", sent.len(), failed.join("; "))),
                };
                shared.host.call_result(&call.path, outcome, message, url);
            }
            Post::Start { call, emergency, to } => {
                let embed = start_embed(&call, emergency);
                for t in &to {
                    let mut m = CreateMessage::new().embed(embed.clone());
                    if !t.mention.is_empty() {
                        m = m.content(&t.mention);
                    }
                    match ChannelId::new(t.channel).send_message(&http, m).await {
                        Ok(_) => drop(shared.posted.fetch_add(1, std::sync::atomic::Ordering::Relaxed)),
                        Err(e) => shared.host.warn(format!("couldn't post to channel {}: {e}", t.channel)),
                    }
                }
            }
            Post::Dm { users, embed, file } => {
                let audio = match &file {
                    Some(p) => tokio::fs::read(p)
                        .await
                        .ok()
                        .filter(|b| b.len() < 10 << 20)
                        .map(|b| (b, p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "call.m4a".into()))),
                    None => None,
                };
                for u in users {
                    let ch = match dms.get(&u) {
                        Some(c) => *c,
                        None => match serenity::UserId::new(u).create_dm_channel(&http).await {
                            Ok(c) => *dms.entry(u).or_insert(c.id),
                            Err(e) => {
                                shared.host.warn(format!("couldn't message user {u}: {e}"));
                                continue;
                            }
                        },
                    };
                    let mut m = CreateMessage::new().embed((*embed).clone());
                    if let Some((b, n)) = &audio {
                        m = m.add_file(CreateAttachment::bytes(b.clone(), n.clone()));
                    }
                    if let Err(e) = ch.send_message(&http, m).await {
                        shared.host.warn(format!("couldn't message user {u} (their DMs may be closed): {e}"));
                    }
                }
            }
            Post::Boards => {
                if let Some(r) = shared.rest() {
                    crate::board::refresh(&shared, &r, None).await;
                }
            }
            Post::Digest { day, channels } => {
                if let Some(r) = shared.rest() {
                    let c = crate::digest::components(&day, "Daily digest", RED);
                    for ch in channels {
                        if let Err(e) = r.post_v2(ch, c.clone()).await {
                            shared.host.warn(format!("couldn't post the digest in channel {ch}: {e}"));
                        }
                    }
                }
            }
            Post::Tone { tone, to } => {
                for t in &to {
                    crate::thread::tone(&shared, &http, &tone, t).await;
                }
            }
            Post::Rates { channels } => {
                for (channel, systems) in channels {
                    let embed = rates_embed(&shared, &systems);
                    let ch = ChannelId::new(channel);
                    if let Some(id) = rates.get(&channel) {
                        if ch.edit_message(&http, *id, EditMessage::new().embed(embed.clone())).await.is_ok() {
                            continue;
                        }
                    }
                    match ch.send_message(&http, CreateMessage::new().embed(embed)).await {
                        Ok(m) => drop(rates.insert(channel, m.id)),
                        Err(e) => shared.host.warn(format!("couldn't post the decode rates to channel {channel}: {e}")),
                    }
                }
            }
        }
    }
}

/// The recording to attach: the M4A when there's one, else the WAV.
async fn audio_file(call: &ConcludedCall) -> Option<(Vec<u8>, String)> {
    let path = call.files.m4a.clone().unwrap_or_else(|| call.files.wav.clone());
    let name = path.file_name()?.to_string_lossy().to_string();
    // (Discord's limit for a bot without boosts: 10 MB.)
    match tokio::time::timeout(Duration::from_secs(10), tokio::fs::read(&path)).await {
        Ok(Ok(b)) if b.len() < 10 << 20 => Some((b, name)),
        _ => None,
    }
}

fn when(unix_s: f64) -> Timestamp {
    Timestamp::from_unix_timestamp(unix_s as i64).unwrap_or_else(|_| Timestamp::now())
}

fn talkgroup_title(tg: u32, tag: &str) -> String {
    if tag.is_empty() || tag == tg.to_string() {
        format!("TG {tg}")
    } else {
        format!("{tag} ({tg})")
    }
}

fn flags(emergency: bool, encrypted: bool) -> &'static str {
    match (emergency, encrypted) {
        (true, true) => " 🚨🔒",
        (true, false) => " 🚨",
        (false, true) => " 🔒",
        _ => "",
    }
}

/// A recorded call's embed, and the link to its recording.
pub fn call_embed(c: &ConcludedCall, address: &str) -> (CreateEmbed, Option<String>) {
    let r = &c.call;
    let base = address.trim().trim_end_matches('/');
    let ext = if c.files.m4a.is_some() { "m4a" } else { "wav" };
    let link = (!base.is_empty() && !r.decrypted).then(|| format!("{base}/calls/{}.{ext}", c.path));
    let mut e = CreateEmbed::new()
        .title(format!("{}{}", talkgroup_title(r.talkgroup, &r.talkgroup_tag), flags(r.emergency, r.encrypted)))
        .color(if r.emergency { RED } else { BLUE })
        .timestamp(when(r.start_time as f64))
        .field("System", if r.site.is_empty() || r.site == r.short_name { r.short_name.clone() } else { format!("{} ({})", r.short_name, r.site) }, true)
        .field("Length", format!("{:.1} s", r.call_length), true)
        .field("Frequency", mhz(r.freq), true);
    // (A field, not the embed's url: Discord refuses a whole post over a url it doesn't like, as
    // a host without a dot, http://scanner:8091.)
    if let Some(l) = &link {
        e = e.field("Recording", format!("[play]({l})"), true);
    }
    let desc: Vec<&str> = [r.talkgroup_description.as_str(), r.talkgroup_group.as_str()].into_iter().filter(|s| !s.is_empty()).collect();
    if !desc.is_empty() {
        e = e.description(desc.join(" · "));
    }
    let mut units: Vec<String> = Vec::new();
    for s in &r.src_list {
        let u = if s.tag.is_empty() { s.src.to_string() } else { format!("{} ({})", s.tag, s.src) };
        if s.src > 0 && !units.contains(&u) {
            units.push(u);
        }
    }
    if !units.is_empty() {
        let more = units.len().saturating_sub(8);
        let mut v = units.into_iter().take(8).collect::<Vec<_>>().join(", ");
        if more > 0 {
            v += &format!(" (+{more})");
        }
        e = e.field("Radios", v, false);
    }
    if r.patched_talkgroups.len() > 1 {
        e = e.field("Patched", r.patched_talkgroups.iter().map(|t| t.to_string()).collect::<Vec<_>>().join(", "), false);
    }
    (e.footer(footer(&r.short_name, r.talkgroup)), link)
}

pub fn start_embed(c: &CallInfo, emergency: bool) -> CreateEmbed {
    let mut e = CreateEmbed::new()
        .title(format!("{}{}{}", if emergency { "Emergency: " } else { "" }, talkgroup_title(c.talkgroup, &c.talkgroup_tag), flags(c.emergency, c.encrypted)))
        .color(if emergency || c.emergency {
            RED
        } else if c.recording {
            GREEN
        } else {
            GRAY
        })
        .timestamp(when(c.start_time))
        .field("System", c.short_name.clone(), true)
        .field("Frequency", mhz(c.freq_hz), true);
    if let Some(s) = c.sources.last() {
        e = e.field("Radio", if s.tag.is_empty() { s.unit.to_string() } else { format!("{} ({})", s.tag, s.unit) }, true);
    }
    if !c.recording {
        e = e.field("Not recorded", c.reason.clone().unwrap_or_else(|| "—".into()).replace('_', " "), true);
    }
    e.footer(footer(&c.short_name, c.talkgroup))
}

/// A named tone, for a direct message.
pub fn tone_embed(t: &TonePatternEvent) -> CreateEmbed {
    let names: Vec<String> = t.pattern.signals.iter().map(|s| crate::thread::signal_name(s)).collect();
    let tg = if t.alpha_tag.is_empty() { format!("TG {}", t.talkgroup) } else { format!("{} ({})", t.alpha_tag, t.talkgroup) };
    CreateEmbed::new()
        .title(format!("🔔 {}", names.join(", ")))
        .description(format!("**{tg}** on {}\n{}", t.short_name, crate::thread::describe(&t.pattern)))
        .color(RED)
        .timestamp(when(t.time))
        .footer(footer(&t.short_name, t.talkgroup))
}

pub fn rates_embed(shared: &Shared, systems: &[String]) -> CreateEmbed {
    let rates = shared.rates.lock().unwrap().clone();
    let mut e = CreateEmbed::new().title("Control channel decode rates").color(RED).timestamp(Timestamp::now());
    let mut n = 0;
    let mut interval = 0.0;
    for (name, r) in &rates {
        if !systems.is_empty() && !systems.contains(&r.system) && !systems.contains(name) {
            continue;
        }
        n += 1;
        if n > 25 {
            break;
        }
        interval = r.interval_s;
        let (top, bottom) = r.graph(5);
        let cc = r.control_channel_hz.map_or_else(|| "searching".to_string(), mhz);
        e = e.field(name.clone(), format!("{:.1} msg/s\n```{cc}\n{top}\n{bottom}```", r.now()), true);
    }
    if n == 0 {
        return e.description("No decode rates to show.").color(GRAY);
    }
    let history = (interval * crate::state::HISTORY as f64).round() as u64;
    e.footer(CreateEmbedFooter::new(format!("{:.0} s a column · {} shown", interval * 5.0, span(history))))
}

#[cfg(test)]
mod tests {
    #[test]
    fn footers_read_back() {
        let f = super::footer("clmrn-I", 1001);
        let text = serde_json::to_value(&f).unwrap()["text"].as_str().unwrap().to_string();
        assert_eq!(super::from_footer(&text), Some(("clmrn-I".into(), 1001)));
        assert_eq!(super::from_footer("something else"), None);
    }
}
