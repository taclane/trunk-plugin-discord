//! Commands to see how things are: `/ping`, `/health`, `/sites`, `/about`, `/today` and `/help`
//! for users; `/check`, `/log` and `/test` for admins ([`crate::access`]). All answered to the
//! one asking only, but `/test`, whose posts are the point.

use std::sync::atomic::Ordering;
use std::time::Instant;

use poise::serenity_prelude as serenity;
use poise::CreateReply;
use serde_json::json;
use serenity::{CreateEmbed, CreateEmbedFooter, CreateMessage, Timestamp};
use trunk_recorder_plugin::{LogLine, SiteIdentity};

use crate::bot::{Ctx, Error};
use crate::post::{GRAY, GREEN, RED};
use crate::settings::LogLevel;
use crate::state::{span, Rates};

const AMBER: u32 = 0xe0a030;

/// Log lines as a code block, newest last, cut from the oldest to fit in `max` characters.
pub fn log_block(lines: &[&LogLine], max: usize) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut used = 8;
    for l in lines.iter().rev() {
        let t = chrono::DateTime::from_timestamp(l.time as i64, 0).map(|t| t.with_timezone(&chrono::Local).format("%H:%M:%S").to_string()).unwrap_or_default();
        let level = match l.level.as_str() {
            "warning" => "WARN ",
            "error" => "ERROR",
            "fatal" => "FATAL",
            "info" => "info ",
            other => other,
        };
        // (Backticks would end the block.)
        let line = format!("{t} {level} {}", l.text.replace("```", "'''"));
        let line: String = line.chars().take(300).collect();
        if used + line.len() + 1 > max {
            break;
        }
        used += line.len() + 1;
        out.push(line);
    }
    out.reverse();
    format!("```\n{}\n```", out.join("\n"))
}

/// A site's address, as its control channel states it: P25 `WACN:System:RFSS-Site` (hex) and its
/// NAC; DMR its colour code (and network, site); NXDN its RAN.
pub fn identity(protocol: &str, id: &SiteIdentity) -> String {
    let mut parts = Vec::new();
    match (id.wacn, id.sys_id, id.rfss, id.site) {
        (Some(w), Some(s), Some(r), Some(x)) if protocol == "p25" => parts.push(format!("{w:05X}:{s:03X}:{r}-{x}")),
        (_, Some(s), _, Some(x)) => parts.push(format!("System {s:X} · Site {x}")),
        _ => {}
    }
    if let Some(n) = id.nac {
        parts.push(format!("NAC {n:03X}"));
    }
    if let Some(c) = id.color_code {
        parts.push(format!("CC {c}"));
    }
    if let Some(n) = id.network {
        parts.push(format!("network {n}"));
    }
    if let Some(r) = id.ran {
        parts.push(format!("RAN {r}"));
    }
    if parts.is_empty() {
        "not stated yet".into()
    } else {
        parts.join(" · ")
    }
}

fn site_line(site: &str, r: &Rates) -> String {
    let rate = r.now();
    match (r.lost_at, r.control_channel_hz) {
        (Some(_), _) => format!("❌ **{site}**: control channel lost"),
        (None, Some(hz)) => format!(
            "{} **{site}** · {:.5} MHz · {rate:.1} msg/s · {} active ({} recording)",
            if rate > 0.0 { "✅" } else { "⚠️" },
            hz as f64 / 1e6,
            r.active,
            r.recording
        ),
        (None, None) => format!("⚠️ **{site}**: hunting for its control channel"),
    }
}

/// How long ago, said simply ("never" for none).
fn ago(t: Option<Instant>) -> String {
    t.map_or("never".into(), |t| format!("{} ago", span(t.elapsed().as_secs())))
}

/// Is the bot there, and is TRP talking to it?
#[poise::command(slash_command)]
pub async fn ping(ctx: Ctx<'_>) -> Result<(), Error> {
    let sh = &ctx.data().shared;
    let gateway = ctx.ping().await;
    let rest = match sh.rest() {
        Some(r) => {
            let t = Instant::now();
            match r.get("/users/@me").await {
                Ok(_) => format!("{} ms", t.elapsed().as_millis()),
                Err(e) => format!("failed: {e}"),
            }
        }
        None => "not logged in".into(),
    };
    let heard = *sh.heard.lock().unwrap();
    let quiet = heard.is_none_or(|t| t.elapsed().as_secs() > 60);
    let e = CreateEmbed::new()
        .title("Pong")
        .color(if quiet { AMBER } else { GREEN })
        .field("Discord gateway", if gateway.is_zero() { "measuring…".into() } else { format!("{} ms", gateway.as_millis()) }, true)
        .field("Discord API", rest, true)
        .field("TRP last heard", ago(heard), true);
    let e = if quiet { e.footer(CreateEmbedFooter::new("TRP sends a status every few seconds while it records: is recording stopped?")) } else { e };
    ctx.send(CreateReply::default().embed(e).ephemeral(true)).await?;
    Ok(())
}

/// How TRP's sites and the bot are doing
#[poise::command(slash_command)]
pub async fn health(ctx: Ctx<'_>) -> Result<(), Error> {
    let sh = &ctx.data().shared;
    let rates = sh.rates.lock().unwrap().clone();
    let mut sites: Vec<String> = rates.iter().map(|(site, r)| site_line(site, r)).collect();
    if sites.is_empty() {
        sites.push("No control channel heard yet.".into());
    }
    let lost = rates.values().filter(|r| r.lost_at.is_some()).count();
    let hunting = rates.values().filter(|r| r.lost_at.is_none() && r.now() <= 0.0).count();
    let heard = *sh.heard.lock().unwrap();
    let trp = sh.trp.lock().unwrap().clone();
    let hour_ago = chrono::Utc::now().timestamp() as f64 - 3600.0;
    let (warnings, errors) =
        sh.log.lock().unwrap().iter().filter(|l| l.time >= hour_ago).fold((0, 0), |(w, e), l| if l.level == "warning" { (w + 1, e) } else { (w, e + 1) });
    let failures = sh.failures.load(Ordering::Relaxed);
    let last_failure = sh.last_failure.lock().unwrap().clone();
    let color = if lost > 0 || errors > 0 {
        RED
    } else if hunting > 0 || warnings > 0 || failures > 0 {
        AMBER
    } else {
        GREEN
    };
    let mut bot = format!(
        "Up {} · {} posted · {} failed{}",
        span(sh.started.elapsed().as_secs()),
        sh.posted.load(Ordering::Relaxed),
        failures,
        last_failure.map_or(String::new(), |(t, why)| format!("\nLast failure {}: {}", ago(Some(t)), why.chars().take(200).collect::<String>()))
    );
    let follows = sh.live.follows().len();
    if follows > 0 {
        bot += &format!("\nLive audio: following {follows}");
    }
    let mut e = CreateEmbed::new()
        .title("Health")
        .color(color)
        .description(sites.join("\n").chars().take(4000).collect::<String>())
        .field("TRP", format!("{} {} · last heard {}", trp.name, trp.version, ago(heard)), false)
        .field(
            "TRP's log, last hour",
            format!("{warnings} warning{} · {errors} error{}", if warnings == 1 { "" } else { "s" }, if errors == 1 { "" } else { "s" }),
            true,
        )
        .field("Bot", bot, false)
        .timestamp(Timestamp::now());
    if warnings + errors > 0 {
        e = e.footer(CreateEmbedFooter::new("/log shows them (admins)"));
    }
    ctx.send(CreateReply::default().embed(e).ephemeral(true)).await?;
    Ok(())
}

/// Each site, as its control channel says it is
#[poise::command(slash_command)]
pub async fn sites(
    ctx: Ctx<'_>,
    #[description = "One system (empty: all)"]
    #[autocomplete = "crate::bot::system_names"]
    system: Option<String>,
) -> Result<(), Error> {
    let rates = ctx.data().shared.rates.lock().unwrap().clone();
    let mut e = CreateEmbed::new().title("Sites").color(GRAY);
    let mut n = 0;
    for (site, r) in rates.iter().filter(|(_, r)| system.as_ref().is_none_or(|s| s.eq_ignore_ascii_case(&r.system))).take(25) {
        let cc = r.control_channel_hz.map_or("searching".into(), |hz| format!("{:.5} MHz", hz as f64 / 1e6));
        let proto = if r.protocol.is_empty() { String::new() } else { format!("{} · ", r.protocol.to_uppercase()) };
        let name = if *site == r.system { site.clone() } else { format!("{site} ({})", r.system) };
        e = e.field(name, format!("{proto}{}\n{cc} · {:.1} msg/s", identity(&r.protocol, &r.identity), r.now()), true);
        n += 1;
    }
    if n == 0 {
        e = e.description("No control channel heard yet.");
    }
    ctx.send(CreateReply::default().embed(e).ephemeral(true)).await?;
    Ok(())
}

/// The bot's and TRP's versions, and its invite link
#[poise::command(slash_command)]
pub async fn about(ctx: Ctx<'_>) -> Result<(), Error> {
    let sh = &ctx.data().shared;
    let trp = sh.trp.lock().unwrap().clone();
    let me = *sh.me.lock().unwrap();
    let mut e = CreateEmbed::new()
        .title("Trunk Recorder Pro · Discord")
        .url(env!("CARGO_PKG_REPOSITORY"))
        .color(RED)
        .field("Plugin", env!("CARGO_PKG_VERSION"), true)
        .field("TRP", format!("{} {}", trp.name, trp.version), true)
        .field("Systems", sh.systems.lock().unwrap().len().to_string(), true);
    if let Some(id) = me {
        e = e.field("Add it to a server", format!("[Invite link]({})", crate::bot::invite_url(id)), false);
    }
    ctx.send(CreateReply::default().embed(e).ephemeral(true)).await?;
    Ok(())
}

/// Today's figures so far
#[poise::command(slash_command)]
pub async fn today(ctx: Ctx<'_>) -> Result<(), Error> {
    let sh = &ctx.data().shared;
    let poise::Context::Application(a) = ctx else { return Ok(()) };
    let (Some(rest), i) = (sh.rest(), a.interaction) else {
        ctx.send(CreateReply::default().content("The bot isn't logged in yet.").ephemeral(true)).await?;
        return Ok(());
    };
    // (A Components V2 answer: serenity's builders can't make one, so it's sent as Discord has it.)
    let components = crate::digest::components(&sh.digest.now(), "Today so far", RED);
    let body = json!({ "type": 4, "data": { "flags": crate::rest::COMPONENTS_V2 | 64, "components": components, "allowed_mentions": { "parse": [] } } });
    rest.post(&format!("/interactions/{}/{}/callback", i.id, i.token), &body).await.map_err(|e| format!("{e}"))?;
    Ok(())
}

/// What you can do with the bot here
#[poise::command(slash_command)]
pub async fn help(ctx: Ctx<'_>) -> Result<(), Error> {
    let sh = &ctx.data().shared;
    let admin = match ctx {
        poise::Context::Application(a) => crate::access::Who::of(a.interaction.guild_id, &a.interaction.user, a.interaction.member.as_deref())
            .is_none_or(|w| sh.server(w.guild).unwrap_or_default().access.admin(&w)),
        _ => false,
    };
    let mut users = Vec::new();
    let mut admins = Vec::new();
    for c in &ctx.framework().options().commands {
        let (name, what) = match &c.context_menu_name {
            Some(label) => (format!("Apps → {label}"), "on a post (right-click it)".to_string()),
            None => (format!("/{}", c.name), c.description.clone().unwrap_or_default().to_string()),
        };
        let line = format!("**{name}**: {what}");
        match crate::access::command_tier(c.context_menu_name.as_deref().unwrap_or(&c.name)) {
            crate::access::Tier::User => users.push(line),
            crate::access::Tier::Admin => admins.push(line),
        }
    }
    let mut e = CreateEmbed::new().title("Trunk Recorder Pro bot").color(RED).field("Commands", users.join("\n").chars().take(1024).collect::<String>(), false);
    if admin {
        e = e.field("For admins", admins.join("\n").chars().take(1024).collect::<String>(), false);
    }
    ctx.send(CreateReply::default().embed(e).ephemeral(true)).await?;
    Ok(())
}

/// Check the bot can see and post to every channel it's set up for here
#[poise::command(slash_command, guild_only, default_member_permissions = "MANAGE_GUILD")]
pub async fn check(ctx: Ctx<'_>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let sh = &ctx.data().shared;
    let g = ctx.guild_id().map_or(0, |g| g.get());
    let server = sh.server(g).unwrap_or_default();
    let text = match crate::bot::check(sh, &server).await {
        Ok(t) => format!("✓ {t}"),
        Err(t) => format!("✗ {t}"),
    };
    ctx.send(CreateReply::default().content(text.chars().take(2000).collect::<String>()).ephemeral(true)).await?;
    Ok(())
}

#[derive(Debug, Clone, Copy, poise::ChoiceParameter)]
pub enum Level {
    #[name = "Warnings and errors"]
    Warning,
    #[name = "Errors only"]
    Error,
}

/// TRP's latest warnings and errors
#[poise::command(slash_command, guild_only, default_member_permissions = "MANAGE_GUILD")]
pub async fn log(
    ctx: Ctx<'_>,
    #[description = "Which (empty: warnings and errors)"] level: Option<Level>,
    #[description = "How many (empty: 20)"]
    #[min = 1]
    #[max = 50]
    count: Option<u8>,
) -> Result<(), Error> {
    let at_least = match level {
        Some(Level::Error) => LogLevel::Error,
        _ => LogLevel::Warning,
    };
    let lines: Vec<LogLine> = ctx.data().shared.log.lock().unwrap().iter().filter(|l| at_least.takes(&l.level)).cloned().collect();
    let n = count.unwrap_or(20) as usize;
    let latest: Vec<&LogLine> = lines.iter().rev().take(n).rev().collect();
    let e = if latest.is_empty() {
        CreateEmbed::new().title("TRP's log").color(GREEN).description("Nothing since the plugin started.")
    } else {
        CreateEmbed::new().title(format!("TRP's log: the latest {}", latest.len())).color(AMBER).description(log_block(&latest, 4000))
    };
    ctx.send(CreateReply::default().embed(e).ephemeral(true)).await?;
    Ok(())
}

/// Send a test post to each feed of this channel, to see it's set up right
#[poise::command(slash_command, guild_only, default_member_permissions = "MANAGE_GUILD")]
pub async fn test(ctx: Ctx<'_>) -> Result<(), Error> {
    let sh = &ctx.data().shared;
    let channel = ctx.channel_id();
    let feeds: Vec<_> = sh.feeds().into_iter().filter(|f| f.channel_id() == Some(channel.get())).collect();
    if feeds.is_empty() {
        ctx.send(CreateReply::default().content("This channel has no feeds: /setup here picks what it gets.").ephemeral(true)).await?;
        return Ok(());
    }
    ctx.defer_ephemeral().await?;
    let mut said = Vec::new();
    for f in &feeds {
        let mut e = CreateEmbed::new()
            .title("Test post")
            .color(GRAY)
            .description(format!("This channel gets **{}**{}.", f.event.label(), if f.enabled { "" } else { " (switched off)" }))
            .footer(CreateEmbedFooter::new(format!("Sent by /test · feed {}", f.title())));
        if !f.mention.trim().is_empty() {
            // (Shown, not sent: a test shouldn't ping anyone.)
            e = e.field("Its posts mention", f.mention.clone(), false);
        }
        let m = CreateMessage::new().embed(e).allowed_mentions(serenity::CreateAllowedMentions::new());
        said.push(match channel.send_message(ctx, m).await {
            Ok(_) => format!("✓ {}", f.title()),
            Err(e) => format!("✗ {}: {e}", f.title()),
        });
    }
    ctx.send(CreateReply::default().content(said.join("\n")).ephemeral(true)).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_site_reads_as_its_control_channel_states_it() {
        let p25 = SiteIdentity { nac: Some(0x3a0), wacn: Some(0xbee00), sys_id: Some(0x954), rfss: Some(1), site: Some(1), ..Default::default() };
        assert_eq!(identity("p25", &p25), "BEE00:954:1-1 · NAC 3A0");
        assert_eq!(identity("dmr", &SiteIdentity { color_code: Some(1), ..Default::default() }), "CC 1");
        assert_eq!(identity("p25", &SiteIdentity::default()), "not stated yet");
    }

    #[test]
    fn the_log_block_keeps_the_newest() {
        let l = |t: f64, text: &str| LogLine { time: t, level: "warning".into(), text: text.into(), ..Default::default() };
        let lines = [l(1.0, "old"), l(2.0, "newer"), l(3.0, "newest")];
        let refs: Vec<&LogLine> = lines.iter().collect();
        let b = log_block(&refs, 8 + 22 + 21);
        assert!(b.contains("newest") && b.contains("newer") && !b.contains("old"), "{b}");
        assert!(b.starts_with("```\n") && b.ends_with("\n```"));
    }
}
