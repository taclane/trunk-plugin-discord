//! The commands beyond the basics: right-click (Apps) menus on a post, lookups (/radio, /tg),
//! mutes, live audio (/live), status boards (/board), personal alerts (/notify), a thread opened
//! by hand (/thread) and the feed editing form (/feed edit).

use std::time::Duration;

use poise::serenity_prelude as serenity;
use poise::CreateReply;
use serenity::{CreateEmbed, CreateEmbedFooter};
use trunk_recorder_plugin::{CallsPage, CallsQuery};

use crate::bot::{feed_names, system_names, Ctx, Data, Error};
use crate::live::{Follow, Why};
use crate::post::{BLUE, GRAY, GREEN};

fn server(ctx: &Ctx<'_>) -> u64 {
    ctx.guild_id().map_or(0, |g| g.get())
}

fn who(ctx: &Ctx<'_>) -> String {
    ctx.author().global_name.clone().unwrap_or_else(|| ctx.author().name.clone())
}

fn tg_name(tag: &str, tg: u32) -> String {
    if tag.is_empty() {
        format!("TG {tg}")
    } else {
        format!("{tag} ({tg})")
    }
}

/// A page of calls as lines, newest first, with links to play them.
pub fn lines(ctx: &Ctx<'_>, page: &CallsPage) -> Vec<String> {
    let base = ctx.data().shared.settings().trp_address.trim().trim_end_matches('/').to_string();
    page.entries
        .iter()
        .map(|c| {
            let r = &c.record;
            let mut l = format!(
                "<t:{}:f> **{}** · {} · {:.0} s{}",
                r.start_time,
                tg_name(&r.talkgroup_tag, r.talkgroup),
                r.short_name,
                r.call_length,
                if r.emergency { " 🚨" } else { "" }
            );
            if c.audio && !base.is_empty() && !r.decrypted {
                l += &format!(" · [play]({base}/calls/{}.m4a)", c.path);
            }
            l
        })
        .collect()
}

async fn ask(ctx: &Ctx<'_>, q: CallsQuery) -> Result<CallsPage, Error> {
    let rx = ctx.data().shared.ask_calls(q);
    Ok(tokio::time::timeout(Duration::from_secs(15), rx).await.map_err(|_| "TRP didn't answer")??)
}

fn page_embed(title: String, lines: Vec<String>, page: &CallsPage) -> CreateEmbed {
    let mut text = lines.join("\n");
    text.truncate(text.char_indices().nth(4000).map_or(text.len(), |(i, _)| i));
    let mut e = CreateEmbed::new().title(title).color(BLUE);
    e = if text.is_empty() { e.description("No calls found.").color(GRAY) } else { e.description(text) };
    if let Some(err) = &page.error {
        e = e.footer(CreateEmbedFooter::new(err));
    }
    e
}

/// What a post is about: its system, talkgroup and tag (a call's, a header's).
fn about(ctx: &Ctx<'_>, msg: &serenity::Message) -> Option<(String, u32, String)> {
    if let Some(t) = ctx.data().shared.threads.all().into_iter().find(|t| t.header == msg.id.get() || (t.forum && t.thread == msg.channel_id.get())) {
        return Some((t.system, t.talkgroup, t.tag));
    }
    let e = msg.embeds.first()?;
    let (system, tg) = crate::post::from_footer(&e.footer.as_ref()?.text)?;
    // (A call's title: "Tag (1001)", maybe with flags after it.)
    let title = e.title.clone().unwrap_or_default();
    let tag = title.split(&format!(" ({tg})")).next().filter(|t| *t != title).unwrap_or("").trim_start_matches("Emergency: ").to_string();
    Some((system, tg, tag))
}

const NOT_OURS: &str = "That isn't one of the bot's call posts.";

// ─── Right-click menus ────────────────────────────────────────────────────

/// Follow this talkgroup in the live audio channel
#[poise::command(context_menu_command = "Follow live", guild_only)]
pub async fn menu_follow(ctx: Ctx<'_>, msg: serenity::Message) -> Result<(), Error> {
    let Some((system, tg, tag)) = about(&ctx, &msg) else {
        ctx.send(CreateReply::default().content(NOT_OURS).ephemeral(true)).await?;
        return Ok(());
    };
    follow(ctx, system, tg, tag).await
}

/// Open a thread for this talkgroup here
#[poise::command(context_menu_command = "Open a thread here", guild_only, default_member_permissions = "MANAGE_THREADS")]
pub async fn menu_thread(ctx: Ctx<'_>, msg: serenity::Message) -> Result<(), Error> {
    let Some((system, tg, tag)) = about(&ctx, &msg) else {
        ctx.send(CreateReply::default().content(NOT_OURS).ephemeral(true)).await?;
        return Ok(());
    };
    open_thread(ctx, system, tg, tag).await
}

/// Stop this talkgroup's posts in this channel
#[poise::command(context_menu_command = "Mute here", guild_only, default_member_permissions = "MANAGE_CHANNELS")]
pub async fn menu_mute(ctx: Ctx<'_>, msg: serenity::Message) -> Result<(), Error> {
    let Some((system, tg, tag)) = about(&ctx, &msg) else {
        ctx.send(CreateReply::default().content(NOT_OURS).ephemeral(true)).await?;
        return Ok(());
    };
    let changed = ctx.data().shared.mute(ctx.channel_id().get(), &system, tg, true);
    let text = if changed { format!("Muted {} on {system} here. `/unmute` brings it back.", tg_name(&tag, tg)) } else { "It's muted here already.".into() };
    ctx.send(CreateReply::default().content(text).ephemeral(true)).await?;
    Ok(())
}

/// The latest calls on this talkgroup
#[poise::command(context_menu_command = "Recent calls", guild_only)]
pub async fn menu_recent(ctx: Ctx<'_>, msg: serenity::Message) -> Result<(), Error> {
    let Some((system, tg, tag)) = about(&ctx, &msg) else {
        ctx.send(CreateReply::default().content(NOT_OURS).ephemeral(true)).await?;
        return Ok(());
    };
    ctx.defer_ephemeral().await?;
    let page = ask(&ctx, CallsQuery { system: Some(system.clone()), talkgroup: Some(tg), limit: Some(10), ..Default::default() }).await?;
    let l = lines(&ctx, &page);
    ctx.send(CreateReply::default().embed(page_embed(format!("{} on {system}", tg_name(&tag, tg)), l, &page)).ephemeral(true)).await?;
    Ok(())
}

// ─── Lookups ──────────────────────────────────────────────────────────────

/// A radio's latest calls
#[poise::command(slash_command)]
pub async fn radio(
    ctx: Ctx<'_>,
    #[description = "Radio ID"] id: i64,
    #[description = "System"]
    #[autocomplete = "system_names"]
    system: Option<String>,
    #[description = "How many (10)"]
    #[min = 1]
    #[max = 25]
    count: Option<usize>,
) -> Result<(), Error> {
    ctx.defer().await?;
    let page = ask(&ctx, CallsQuery { system: system.clone(), unit: Some(id), limit: Some(count.unwrap_or(10)), ..Default::default() }).await?;
    // Its name, as its calls have it.
    let name = page.entries.iter().flat_map(|c| c.record.src_list.iter()).find(|s| s.src == id && !s.tag.is_empty()).map(|s| s.tag.clone());
    let title = match name {
        Some(n) => format!("Radio {n} ({id})"),
        None => format!("Radio {id}"),
    };
    let l = lines(&ctx, &page);
    ctx.send(CreateReply::default().embed(page_embed(title, l, &page))).await?;
    Ok(())
}

/// A talkgroup: what it is, and its latest calls
#[poise::command(slash_command)]
pub async fn tg(
    ctx: Ctx<'_>,
    #[description = "Talkgroup"] talkgroup: u32,
    #[description = "System"]
    #[autocomplete = "system_names"]
    system: Option<String>,
) -> Result<(), Error> {
    ctx.defer().await?;
    let page = ask(&ctx, CallsQuery { system: system.clone(), talkgroup: Some(talkgroup), limit: Some(10), ..Default::default() }).await?;
    let first = page.entries.first().map(|c| c.record.clone()).unwrap_or_default();
    let mut e = page_embed(tg_name(&first.talkgroup_tag, talkgroup), lines(&ctx, &page), &page);
    let about: Vec<&str> = [first.talkgroup_description.as_str(), first.talkgroup_group.as_str()].into_iter().filter(|s| !s.is_empty()).collect();
    if !about.is_empty() {
        e = e.field("About", about.join(" · "), false);
    }
    let day = ctx.data().shared.digest.now();
    let today: Vec<String> = day
        .talkgroups
        .values()
        .filter(|t| t.talkgroup == talkgroup && system.as_ref().is_none_or(|s| *s == t.system))
        .map(|t| format!("{} call{} on {} ({})", t.calls, if t.calls == 1 { "" } else { "s" }, t.system, crate::state::span(t.seconds as u64)))
        .collect();
    if !today.is_empty() {
        e = e.field("Today", today.join("\n"), false);
    }
    let g = server(&ctx);
    let live = ctx.data().shared.live.follows_in(g).iter().any(|f| f.talkgroup == talkgroup && system.as_ref().is_none_or(|s| *s == f.system));
    if live {
        e = e.field("Live", "🔊 followed in the live audio channel", false);
    }
    ctx.send(CreateReply::default().embed(e)).await?;
    Ok(())
}

// ─── Mutes ────────────────────────────────────────────────────────────────

/// Talkgroups muted in this channel
#[poise::command(slash_command, guild_only, default_member_permissions = "MANAGE_CHANNELS")]
pub async fn mutes(ctx: Ctx<'_>) -> Result<(), Error> {
    let m = ctx.data().shared.mutes.lock().unwrap().get(&ctx.channel_id().get()).cloned().unwrap_or_default();
    let text = if m.is_empty() {
        "Nothing is muted here. Right-click a post → Apps → Mute here mutes its talkgroup.".to_string()
    } else {
        m.iter().map(|k| format!("• {}", k.replace(':', " TG "))).collect::<Vec<_>>().join("\n")
    };
    ctx.send(CreateReply::default().content(text).ephemeral(true)).await?;
    Ok(())
}

/// Post a muted talkgroup here again
#[poise::command(slash_command, guild_only, default_member_permissions = "MANAGE_CHANNELS")]
pub async fn unmute(
    ctx: Ctx<'_>,
    #[description = "Talkgroup"] talkgroup: u32,
    #[description = "System"]
    #[autocomplete = "system_names"]
    system: String,
) -> Result<(), Error> {
    let changed = ctx.data().shared.mute(ctx.channel_id().get(), &system, talkgroup, false);
    let text = if changed { format!("TG {talkgroup} on {system} posts here again.") } else { "It wasn't muted here.".into() };
    ctx.send(CreateReply::default().content(text).ephemeral(true)).await?;
    Ok(())
}

// ─── Live audio ───────────────────────────────────────────────────────────

async fn follow(ctx: Ctx<'_>, system: String, tg: u32, tag: String) -> Result<(), Error> {
    let sh = &ctx.data().shared;
    let g = server(&ctx);
    let Some(voice) = sh.server(g).and_then(|s| s.voice()) else {
        ctx.send(
            CreateReply::default()
                .content("Live audio is off in this server: someone with Manage Server picks its voice channel with `/setup`.")
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    };
    let new = sh.live.follow(Follow { server: g, system: system.clone(), talkgroup: tg, tag: tag.clone(), why: Why::User { name: who(&ctx) } });
    let text =
        if new { format!("🔊 Following {} on {system} in <#{voice}>.", tg_name(&tag, tg)) } else { format!("{} is followed already.", tg_name(&tag, tg)) };
    sh.host.info(format!("live audio: {system} TG {tg} followed by {}", who(&ctx)));
    ctx.send(CreateReply::default().content(text)).await?;
    Ok(())
}

/// Live audio in the voice channel
#[poise::command(slash_command, guild_only, subcommands("live_follow", "live_follow_signal", "live_stop", "live_stop_all", "live_status"), subcommand_required)]
pub async fn live(_: Ctx<'_>) -> Result<(), Error> {
    Ok(())
}

/// Play a talkgroup live
#[poise::command(slash_command, rename = "follow")]
pub async fn live_follow(
    ctx: Ctx<'_>,
    #[description = "Talkgroup"] talkgroup: u32,
    #[description = "System"]
    #[autocomplete = "system_names"]
    system: String,
) -> Result<(), Error> {
    follow(ctx, system, talkgroup, String::new()).await
}

/// Play the talkgroup of the last signal heard
#[poise::command(slash_command, rename = "follow-signal")]
pub async fn live_follow_signal(
    ctx: Ctx<'_>,
    #[description = "Its system (empty: any)"]
    #[autocomplete = "system_names"]
    system: Option<String>,
) -> Result<(), Error> {
    match ctx.data().shared.live.last_signal(system.as_deref()) {
        Some((system, tg, tag, _)) => follow(ctx, system, tg, tag).await,
        None => {
            ctx.send(CreateReply::default().content("No signal has been heard since the bot started.").ephemeral(true)).await?;
            Ok(())
        }
    }
}

/// Stop playing a talkgroup
#[poise::command(slash_command, rename = "stop")]
pub async fn live_stop(ctx: Ctx<'_>, #[description = "Talkgroup"] talkgroup: u32) -> Result<(), Error> {
    let sh = &ctx.data().shared;
    let g = server(&ctx);
    let gone = sh.live.unfollow(|f| f.server == g && f.talkgroup == talkgroup);
    crate::thread::redraw_live(sh, &ctx.serenity_context().http, &gone).await;
    let text = if gone.is_empty() { format!("TG {talkgroup} wasn't followed.") } else { format!("🔇 Stopped TG {talkgroup}.") };
    ctx.send(CreateReply::default().content(text)).await?;
    Ok(())
}

/// Stop all live audio
#[poise::command(slash_command, rename = "stop-all")]
pub async fn live_stop_all(ctx: Ctx<'_>) -> Result<(), Error> {
    let sh = &ctx.data().shared;
    let g = server(&ctx);
    let gone = sh.live.unfollow(|f| f.server == g);
    crate::thread::redraw_live(sh, &ctx.serenity_context().http, &gone).await;
    ctx.send(CreateReply::default().content(if gone.is_empty() { "Nothing was followed." } else { "🔇 Stopped all live audio." })).await?;
    Ok(())
}

/// What's playing and followed
#[poise::command(slash_command, rename = "status")]
pub async fn live_status(ctx: Ctx<'_>) -> Result<(), Error> {
    let sh = &ctx.data().shared;
    let g = server(&ctx);
    let f = sh.live.follows_in(g);
    let playing = match sh.live.mixer(g).playing() {
        Some(p) => format!("Playing **{}** on {}", tg_name(&p.tag, p.talkgroup), p.system),
        None => "Nothing playing right now".into(),
    };
    let list =
        if f.is_empty() { "Nothing followed.".into() } else { f.iter().map(|x| format!("• {} on {}", x.label(), x.system)).collect::<Vec<_>>().join("\n") };
    ctx.send(CreateReply::default().embed(CreateEmbed::new().title("🔊 Live audio").description(format!("{playing}\n\n{list}")).color(GREEN)).ephemeral(true))
        .await?;
    Ok(())
}

// ─── Boards ───────────────────────────────────────────────────────────────

async fn board_names(ctx: Ctx<'_>, partial: &str) -> serenity::CreateAutocompleteResponse {
    let p = partial.to_lowercase();
    crate::bot::choices(ctx.data().shared.boards.all().into_iter().map(|b| b.name).filter(|n| n.to_lowercase().contains(&p)).take(25).collect())
}

/// Status boards: messages kept up to date in place
#[poise::command(
    slash_command,
    guild_only,
    default_member_permissions = "MANAGE_CHANNELS",
    subcommands("board_add", "board_list", "board_remove"),
    subcommand_required
)]
pub async fn board(_: Ctx<'_>) -> Result<(), Error> {
    Ok(())
}

/// Put a board in this channel
#[poise::command(slash_command, rename = "add")]
pub async fn board_add(
    ctx: Ctx<'_>,
    #[description = "What it shows"] kind: crate::board::Kind,
    #[description = "Only these systems, commas between (empty: all)"]
    #[autocomplete = "system_names"]
    systems: Option<String>,
    #[description = "Its name (to remove it later)"] name: Option<String>,
) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let sh = ctx.data().shared.clone();
    let Some(rest) = sh.rest() else { return Err("the bot isn't logged in".into()) };
    let systems: Vec<String> = systems.unwrap_or_default().split([',', ' ']).filter(|s| !s.is_empty()).map(String::from).collect();
    let taken: Vec<String> = sh.boards.all().into_iter().map(|b| b.name.to_lowercase()).collect();
    let base = name.unwrap_or_else(|| kind.label().to_string());
    let name = std::iter::once(base.clone()).chain((2..).map(|i| format!("{base} {i}"))).find(|n| !taken.contains(&n.to_lowercase())).unwrap_or(base);
    let channel = ctx.channel_id().get();
    let message = rest.post_v2(channel, crate::board::draw(&sh, kind, &systems)).await.map_err(|e| format!("couldn't post it: {e}"))?;
    sh.boards.add(crate::board::Board { name: name.clone(), kind, channel, message, systems });
    sh.host.info(format!("board {name:?} ({}) added in channel {channel} by {}", kind.label(), who(&ctx)));
    ctx.send(CreateReply::default().content(format!("Added board **{name}**: it's brought up to date every minute. Pin it to keep it at hand."))).await?;
    Ok(())
}

/// The boards
#[poise::command(slash_command, rename = "list")]
pub async fn board_list(ctx: Ctx<'_>) -> Result<(), Error> {
    let b = ctx.data().shared.boards.all();
    let text = if b.is_empty() {
        "No boards. `/board add` puts one in a channel.".to_string()
    } else {
        b.iter().map(|b| format!("• **{}**: {} in <#{}>", b.name, b.kind.label(), b.channel)).collect::<Vec<_>>().join("\n")
    };
    ctx.send(CreateReply::default().content(text).ephemeral(true)).await?;
    Ok(())
}

/// Remove a board (its message too)
#[poise::command(slash_command, rename = "remove")]
pub async fn board_remove(
    ctx: Ctx<'_>,
    #[description = "Which"]
    #[autocomplete = "board_names"]
    name: String,
) -> Result<(), Error> {
    let sh = &ctx.data().shared;
    let Some(b) = sh.boards.remove(&name) else {
        ctx.send(CreateReply::default().content(format!("No board is called {name:?}.")).ephemeral(true)).await?;
        return Ok(());
    };
    let _ = serenity::ChannelId::new(b.channel).delete_message(ctx, serenity::MessageId::new(b.message)).await;
    ctx.send(CreateReply::default().content(format!("Removed board **{}**.", b.name)).ephemeral(true)).await?;
    Ok(())
}

// ─── Personal alerts ──────────────────────────────────────────────────────

async fn my_alerts(ctx: Ctx<'_>, partial: &str) -> serenity::CreateAutocompleteResponse {
    let p = partial.to_lowercase();
    crate::bot::choices(
        ctx.data().shared.alerts.of(ctx.author().id.get()).iter().map(|a| a.describe()).filter(|d| d.to_lowercase().contains(&p)).take(25).collect(),
    )
}

/// Direct messages for what you care about
#[poise::command(slash_command, subcommands("notify_me", "notify_list", "notify_remove"), subcommand_required)]
pub async fn notify(_: Ctx<'_>) -> Result<(), Error> {
    Ok(())
}

/// Get a DM: a talkgroup's calls, a signal, or emergencies
#[poise::command(slash_command, rename = "me")]
pub async fn notify_me(
    ctx: Ctx<'_>,
    #[description = "A talkgroup's calls"] talkgroup: Option<u32>,
    #[description = "A signal heard, by ID (STATION_1)"] signal: Option<String>,
    #[description = "Only emergencies (on that talkgroup, or any)"] emergency: Option<bool>,
    #[description = "On this system (empty: any)"]
    #[autocomplete = "system_names"]
    system: Option<String>,
    #[description = "Attach recordings (yes)"] audio: Option<bool>,
) -> Result<(), Error> {
    let a = crate::alerts::Alert {
        user: ctx.author().id.get(),
        system: system.unwrap_or_default(),
        talkgroup,
        signal: signal.unwrap_or_default().trim().to_string(),
        emergency: emergency.unwrap_or(false),
        audio: audio.unwrap_or(true),
    };
    if a.talkgroup.is_none() && a.signal.is_empty() && !a.emergency {
        ctx.send(CreateReply::default().content("Say what for: a talkgroup, a signal, or emergency:True.").ephemeral(true)).await?;
        return Ok(());
    }
    let text = match ctx.data().shared.alerts.add(a.clone()) {
        Ok(()) => format!("You'll get a DM for {}. (Your DMs need to be open to this server's members.)", a.describe()),
        Err(e) => e,
    };
    ctx.send(CreateReply::default().content(text).ephemeral(true)).await?;
    Ok(())
}

/// Your alerts
#[poise::command(slash_command, rename = "list")]
pub async fn notify_list(ctx: Ctx<'_>) -> Result<(), Error> {
    let a = ctx.data().shared.alerts.of(ctx.author().id.get());
    let text = if a.is_empty() {
        "You have no alerts. `/notify me` adds one.".to_string()
    } else {
        a.iter().map(|a| format!("• {}", a.describe())).collect::<Vec<_>>().join("\n")
    };
    ctx.send(CreateReply::default().content(text).ephemeral(true)).await?;
    Ok(())
}

/// Remove one of your alerts
#[poise::command(slash_command, rename = "remove")]
pub async fn notify_remove(
    ctx: Ctx<'_>,
    #[description = "Which"]
    #[autocomplete = "my_alerts"]
    what: String,
) -> Result<(), Error> {
    let text = match ctx.data().shared.alerts.remove(ctx.author().id.get(), &what) {
        Ok(a) => format!("Removed: {}.", a.describe()),
        Err(e) => e,
    };
    ctx.send(CreateReply::default().content(text).ephemeral(true)).await?;
    Ok(())
}

// ─── A thread by hand ─────────────────────────────────────────────────────

async fn open_thread(ctx: Ctx<'_>, system: String, tg: u32, tag: String) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let sh = ctx.data().shared.clone();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0);
    let o = crate::thread::Opening {
        server: server(&ctx),
        system: system.clone(),
        talkgroup: tg,
        tag: tag.clone(),
        title: format!("🧵 {}", tg_name(&tag, tg)),
        text: format!("**{}** on {system}\nOpened by {}", tg_name(&tag, tg), who(&ctx)),
        signal: String::new(),
        time: now,
        mention: String::new(),
        audio: true,
        live: false,
    };
    let text = match crate::thread::open(&sh, &ctx.serenity_context().http, ctx.channel_id().get(), o).await {
        Some(t) => format!("Its calls post in <#{}> until it's stopped.", t.thread),
        None => "Couldn't open it here (the bot needs Create Public Threads; a thread can't hold one).".into(),
    };
    ctx.send(CreateReply::default().content(text).ephemeral(true)).await?;
    Ok(())
}

/// Open a thread here for a talkgroup: its calls post there until it's stopped
#[poise::command(slash_command, guild_only, default_member_permissions = "MANAGE_THREADS")]
pub async fn thread(
    ctx: Ctx<'_>,
    #[description = "Talkgroup"] talkgroup: u32,
    #[description = "System"]
    #[autocomplete = "system_names"]
    system: String,
) -> Result<(), Error> {
    open_thread(ctx, system, talkgroup, String::new()).await
}

// ─── The feed form ────────────────────────────────────────────────────────

#[derive(Debug, Default, poise::Modal)]
#[name = "Edit feed"]
pub struct FeedForm {
    #[name = "Systems (commas between; empty: all)"]
    systems: Option<String>,
    #[name = "Talkgroups (101, 1000-1099, 507*)"]
    talkgroups: Option<String>,
    #[name = "Except talkgroups"]
    except: Option<String>,
    #[name = "Signals, for tones (empty: any tone)"]
    signals: Option<String>,
    #[name = "Text above each post (mentions)"]
    mention: Option<String>,
}

fn list(s: Option<String>) -> Vec<String> {
    s.unwrap_or_default().split([',', ' ']).map(str::trim).filter(|x| !x.is_empty()).map(String::from).collect()
}

/// Edit a feed's filters and message in a form
#[poise::command(slash_command, rename = "edit")]
pub async fn feed_edit(
    ctx: poise::ApplicationContext<'_, Data, Error>,
    #[description = "Which"]
    #[autocomplete = "feed_names"]
    name: String,
) -> Result<(), Error> {
    let sh = ctx.data().shared.clone();
    let g = ctx.guild_id().map_or(0, |g| g.get());
    let Some(f) = sh.server(g).and_then(|s| s.feeds.into_iter().find(|f| f.name.eq_ignore_ascii_case(name.trim()))) else {
        poise::send_application_reply(ctx, CreateReply::default().content(format!("No feed here is called {name:?}.")).ephemeral(true)).await?;
        return Ok(());
    };
    let join = |v: &[String]| (!v.is_empty()).then(|| v.join(", "));
    let defaults = FeedForm {
        systems: join(&f.systems),
        talkgroups: join(&f.talkgroups),
        except: join(&f.except_talkgroups),
        signals: join(&f.signals),
        mention: (!f.mention.is_empty()).then(|| f.mention.clone()),
    };
    let Some(form) = poise::Modal::execute_with_defaults(ctx, defaults).await? else { return Ok(()) };
    let form: FeedForm = form;
    let fname = f.name.clone();
    let result = sh.edit_server(g, move |s| {
        let f = s.feeds.iter_mut().find(|x| x.name == fname).ok_or("It's gone.")?;
        f.systems = list(form.systems);
        f.talkgroups = list(form.talkgroups);
        f.except_talkgroups = list(form.except);
        f.signals = list(form.signals);
        f.mention = form.mention.unwrap_or_default();
        Ok(format!("Changed {}", f.describe()))
    });
    let text = match result {
        Ok(t) => t,
        Err(e) => format!("Not changed: {e}"),
    };
    poise::send_application_reply(ctx, CreateReply::default().content(text)).await?;
    Ok(())
}
