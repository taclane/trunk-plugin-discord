//! The bot: logged in on a runtime of its own, its slash commands, and the poster.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use poise::serenity_prelude as serenity;
use poise::CreateReply;
use serenity::{CreateEmbed, CreateEmbedFooter, Timestamp};
use trunk_recorder_plugin::{CallsQuery, State};

use crate::menus::{self, feed_edit};
use crate::post::{self, BLUE, GRAY, GREEN, RED};
use crate::settings::{Event, Feed, Settings};
use crate::state::{mhz, span, Shared};

pub struct Data {
    pub shared: Arc<Shared>,
}
pub type Error = Box<dyn std::error::Error + Send + Sync>;
pub type Ctx<'a> = poise::Context<'a, Data, Error>;

/// Send messages, embed links, attach files, read history (to edit the rates message), view
/// channels; for tone threads, create public threads, send in them and manage them (to delete);
/// manage channels (a forum's tags); for live audio, connect, speak, use the soundboard and set
/// the voice channel's status.
const PERMISSIONS: u64 = 1024 | 2048 | 16384 | 32768 | 65536 | (1 << 4) | (1 << 20) | (1 << 21) | (1 << 34) | (1 << 35) | (1 << 38) | (1 << 42) | (1 << 48);

/// A bot running: stop it with [`Bot::stop`].
/// Every command the bot has.
pub fn commands() -> Vec<poise::Command<Data, Error>> {
    vec![
        info(),
        rates(),
        active(),
        recent(),
        systems(),
        threads(),
        feed(),
        menus::radio(),
        menus::tg(),
        menus::mutes(),
        menus::unmute(),
        menus::live(),
        menus::board(),
        menus::notify(),
        menus::thread(),
        menus::menu_follow(),
        menus::menu_thread(),
        menus::menu_mute(),
        menus::menu_recent(),
        crate::setup::setup(),
    ]
}

/// Before every command: whoever runs it may ([`crate::access`]); if not, they're told why.
async fn command_check(ctx: Ctx<'_>) -> Result<bool, Error> {
    let poise::Context::Application(a) = ctx else { return Ok(true) };
    let i = a.interaction;
    let Some(who) = crate::access::Who::of(i.guild_id, &i.user, i.member.as_deref()) else { return Ok(true) };
    let server = ctx.data().shared.server(who.guild).unwrap_or_default();
    let name = ctx.command().context_menu_name.as_deref().unwrap_or(&ctx.command().name).to_string();
    let root = ctx.parent_commands().first().map_or(name, |p| p.name.to_string());
    match server.access.may(crate::access::command_tier(&root), &who) {
        Ok(()) => Ok(true),
        Err(why) => {
            ctx.send(CreateReply::default().content(why).ephemeral(true)).await?;
            Ok(false)
        }
    }
}

/// What registering the commands in server `g` sends: the client, the path and the commands as
/// its access has them shown.
pub fn registration(sh: &Shared, g: u64) -> Result<(crate::rest::Rest, String, serde_json::Value), String> {
    let (Some(rest), Some(me)) = (sh.rest(), *sh.me.lock().unwrap()) else { return Err("not logged in".into()) };
    let access = sh.server(g).unwrap_or_default().access;
    let body = crate::access::commands_for(&sh.commands.lock().unwrap(), &access);
    Ok((rest, format!("/applications/{me}/guilds/{g}/commands"), body))
}

/// Register the commands in server `g`, as its access has them shown (one bulk overwrite).
pub async fn register_in(sh: &Shared, g: u64) -> Result<(), String> {
    let (rest, path, body) = registration(sh, g)?;
    rest.put(&path, &body).await.map(|_| ()).map_err(|e| format!("{e:?}"))
}

pub struct Bot {
    shards: Arc<serenity::ShardManager>,
    shared: Arc<Shared>,
}

impl Bot {
    pub fn start(rt: &tokio::runtime::Runtime, shared: Arc<Shared>, settings: &Settings) -> Result<Bot, String> {
        let token = settings.token.trim().to_string();
        serenity::validate_token(&token)
            .map_err(|_| "the bot token isn't one (Discord Developer Portal → your application → Bot → Reset Token)".to_string())?;
        let (sh, sh_stop) = (shared.clone(), shared.clone());
        let token_for_rest = token.clone();
        // Voice for live audio; with phrases, the voice channel's chat too (a privileged intent:
        // switched on in the Developer Portal, or Discord refuses the login).
        let mut intents = serenity::GatewayIntents::GUILDS | serenity::GatewayIntents::GUILD_VOICE_STATES;
        if settings.live_phrases {
            intents |= serenity::GatewayIntents::GUILD_MESSAGES | serenity::GatewayIntents::MESSAGE_CONTENT;
        }
        let framework = poise::Framework::builder()
            .options(poise::FrameworkOptions {
                commands: commands(),
                command_check: Some(|ctx| Box::pin(command_check(ctx))),
                on_error: |e| Box::pin(on_error(e)),
                event_handler: |fw, event| Box::pin(on_event(fw.serenity_context, event, fw.user_data)),
                ..Default::default()
            })
            .setup(move |ctx, ready, framework| {
                Box::pin(async move {
                    *sh.rest.lock().unwrap() = Some(crate::rest::Rest::new(&token_for_rest));
                    *sh.me.lock().unwrap() = Some(ready.user.id.get());
                    *sh.only_guild.lock().unwrap() = (ready.guilds.len() == 1).then(|| ready.guilds[0].id.get());
                    *sh.cache.lock().unwrap() = Some(ctx.cache.clone());
                    // Every server it's in is one of its own (in the settings: set up there with
                    // /setup, or on the page); those on the page too, which it should be in.
                    let joined: Vec<u64> = ready.guilds.iter().map(|g| g.id.get()).collect();
                    let added = sh.edit(|s| {
                        for g in &joined {
                            if s.server(*g).is_none() {
                                s.servers.push(crate::settings::Server { id: g.to_string(), ..Default::default() });
                            }
                        }
                        Ok(String::new())
                    });
                    if let Err(e) = added {
                        sh.host.warn(format!("couldn't add the servers it's in to the settings: {e}"));
                    }
                    let mut servers: Vec<serenity::GuildId> = ready.guilds.iter().map(|g| g.id).collect();
                    for g in sh.settings().guilds().into_iter().map(serenity::GuildId::new) {
                        if !servers.contains(&g) {
                            sh.host.warn(format!("server {g} is on the plugin's page, but the bot isn't in it: open Invite to a server on the plugin's page"));
                            servers.push(g);
                        }
                    }
                    *sh.commands.lock().unwrap() = poise::builtins::create_application_commands(&framework.options().commands);
                    register(ctx, ready, &framework.options().commands, &servers, &sh).await?;
                    tokio::spawn(crate::live::run(sh.clone(), ctx.clone()));
                    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
                    *sh.posts.lock().unwrap() = Some(tx);
                    tokio::spawn(post::poster(sh.clone(), ctx.http.clone(), ctx.cache.clone(), rx));
                    let id = ready.user.id;
                    sh.host.info(format!(
                        "logged in as {}; invite it: https://discord.com/oauth2/authorize?client_id={id}&scope=bot+applications.commands&permissions={PERMISSIONS}",
                        ready.user.name
                    ));
                    sh.host.status(State::Ok, format!("logged in as {}", ready.user.name));
                    tokio::spawn(keep_presence(sh.clone(), ctx.clone()));
                    tokio::spawn(crate::names::keep_status(sh.clone(), ctx.cache.clone()));
                    tokio::spawn(crate::thread::keep_timeouts(sh.clone(), ctx.http.clone()));
                    Ok(Data { shared: sh })
                })
            })
            .build();
        let client = rt.block_on(std::future::IntoFuture::into_future(songbird::SerenityInit::register_songbird(
            serenity::ClientBuilder::new(&token, intents).framework(framework),
        )));
        let mut client = client.map_err(|e| format!("couldn't make the bot: {e}"))?;
        let shards = client.shard_manager.clone();
        rt.spawn(async move {
            // (Stopped on purpose, it ends Ok, and the bot after it may be posting already.)
            if let Err(e) = client.start().await {
                *shared.posts.lock().unwrap() = None;
                shared.host.status(State::Error, format!("the bot stopped: {e}"));
            }
        });
        Ok(Bot { shards, shared: sh_stop })
    }

    pub fn stop(self, rt: &tokio::runtime::Runtime) {
        *self.shared.posts.lock().unwrap() = None;
        let shards = self.shards;
        let _ = rt.block_on(async move { tokio::time::timeout(Duration::from_secs(5), shards.shutdown_all()).await });
    }
}

/// Register the commands, in the one server or everywhere, and remove every other command the
/// bot has: an older build's, the old Trunk Recorder bot's (/ping, /helpy), those of a server it
/// used to be set to. Each set is a bulk overwrite, so what's left is exactly these.
async fn register(
    ctx: &serenity::Context,
    ready: &serenity::Ready,
    commands: &[poise::Command<Data, Error>],
    servers: &[serenity::GuildId],
    sh: &Shared,
) -> Result<(), Error> {
    let ours = poise::builtins::create_application_commands(commands);
    // (As Discord knows them: a right-click command by its label.)
    let names: Vec<String> = commands.iter().map(|c| c.context_menu_name.as_deref().unwrap_or(&c.name).to_string()).collect();
    let mut gone: Vec<String> = Vec::new();
    let note = |had: Vec<serenity::Command>, keeps: bool, gone: &mut Vec<String>| {
        gone.extend(had.into_iter().filter(|c| !keeps || !names.contains(&c.name)).map(|c| format!("/{}", c.name)));
    };
    let global = serenity::Command::get_global_commands(ctx).await.unwrap_or_default();
    note(global, servers.is_empty(), &mut gone);
    serenity::Command::set_global_commands(ctx, if servers.is_empty() { ours.clone() } else { Vec::new() }).await?;
    for g in ready.guilds.iter().map(|g| g.id) {
        let mine = servers.contains(&g);
        let had = g.get_commands(ctx).await.unwrap_or_default();
        if had.is_empty() && !mine {
            continue;
        }
        note(had, mine, &mut gone);
        let set = if mine { register_in(sh, g.get()).await } else { g.set_commands(ctx, Vec::new()).await.map(|_| ()).map_err(|e| e.to_string()) };
        if let Err(e) = set {
            sh.host.warn(format!("couldn't set the commands in server {g}: {e}"));
        }
    }
    gone.sort();
    gone.dedup();
    let place = match servers {
        [] => "in every server (new ones can take a while to show)".to_string(),
        [g] => format!("in server {g}"),
        many => format!("in servers {}", many.iter().map(|g| g.to_string()).collect::<Vec<_>>().join(", ")),
    };
    sh.host.info(format!("commands registered {}{}", place, if gone.is_empty() { String::new() } else { format!("; removed old ones: {}", gone.join(" ")) }));
    Ok(())
}

/// Buttons pressed (the threads' headers, the live audio panel), and words in the live audio
/// channel's chat (with phrases on).
async fn on_event(ctx: &serenity::Context, event: &serenity::FullEvent, data: &Data) -> Result<(), Error> {
    let sh = &data.shared;
    match event {
        // Invited to a server: it's one of its own from now on, its commands there at once.
        serenity::FullEvent::GuildCreate { guild, is_new } => {
            let (g, name) = (guild.id.get(), guild.name.clone());
            if let Err(e) = sh.edit_server(g, |_| Ok(String::new())) {
                sh.host.warn(format!("couldn't add server {name} to the settings: {e}"));
            }
            if *is_new == Some(true) {
                sh.host.info(format!("joined server {name} ({g})"));
                if let Err(e) = register_in(sh, g).await {
                    sh.host.warn(format!("couldn't register the commands in {name}: {e}"));
                }
                if let Some(ch) = guild.system_channel_id {
                    let hello = CreateEmbed::new()
                        .title("Trunk Recorder is here")
                        .color(RED)
                        .description("Someone with **Manage Server** can run `/setup` to pick a voice channel for live audio and what to post where. `/info` and `/rates` work for everyone.");
                    let _ = ch.send_message(ctx, serenity::CreateMessage::new().embed(hello)).await;
                }
            }
        }
        // Removed from one (not just an outage): its setup stays, to invite it back to, or to
        // remove on the page.
        serenity::FullEvent::GuildDelete { incomplete, .. } if !incomplete.unavailable => {
            let g = incomplete.id.get();
            sh.host.info(format!("left server {g}: its setup stays in the settings (remove it on the page if it's for good)"));
        }
        // Renamed (or made, or gone): the names in the settings follow.
        serenity::FullEvent::GuildUpdate { .. }
        | serenity::FullEvent::ChannelUpdate { .. }
        | serenity::FullEvent::ChannelCreate { .. }
        | serenity::FullEvent::ChannelDelete { .. } => {
            sh.renamed.notify_one();
        }
        serenity::FullEvent::InteractionCreate { interaction: serenity::Interaction::Component(b) } => {
            if let Some(who) = crate::access::Who::of(b.guild_id, &b.user, b.member.as_ref()) {
                let server = sh.server(who.guild).unwrap_or_default();
                if let Err(why) = server.access.may(crate::access::component_tier(&b.data.custom_id), &who) {
                    let say = serenity::CreateInteractionResponseMessage::new().content(why).ephemeral(true);
                    let _ = b.create_response(ctx, serenity::CreateInteractionResponse::Message(say)).await;
                    return Ok(());
                }
            }
            let handled = match crate::thread::button(ctx, sh, b).await {
                Ok(true) => Ok(true),
                Ok(false) => match crate::live::button(ctx, sh, b).await {
                    Ok(false) => crate::setup::component(ctx, sh, b).await,
                    other => other,
                },
                Err(e) => Err(e),
            };
            if let Err(e) = handled {
                sh.host.warn(format!("a button: {e}"));
            }
        }
        serenity::FullEvent::Message { new_message: m } if !m.author.bot => {
            let s = sh.settings();
            let g = m.guild_id.map_or(0, |g| g.get());
            let roles: Vec<u64> = m.member.as_ref().map_or_else(Vec::new, |p| p.roles.iter().map(|r| r.get()).collect());
            let who = crate::access::Who { guild: g, user: m.author.id.get(), roles, manage_guild: false };
            let may = sh.server(g).is_some_and(|x| x.access.user(&who));
            if s.live_phrases && may && sh.server(g).and_then(|x| x.voice()) == Some(m.channel_id.get()) {
                let who = m.author.global_name.clone().unwrap_or_else(|| m.author.name.clone());
                if let Some(answer) = crate::live::phrase(sh, g, &m.content, &who) {
                    sh.host.info(format!("live audio, {who}: {:?} → {answer}", m.content));
                    let _ = m.reply(ctx, answer).await;
                }
            }
        }
        _ => {}
    }
    Ok(())
}

async fn on_error(e: poise::FrameworkError<'_, Data, Error>) {
    match e {
        poise::FrameworkError::Command { error, ctx, .. } => {
            ctx.data().shared.host.warn(format!("/{}: {error}", ctx.command().qualified_name));
            let _ = ctx.send(CreateReply::default().content(format!("That didn't work: {error}")).ephemeral(true)).await;
        }
        poise::FrameworkError::Setup { error, .. } => eprintln!("the bot couldn't start: {error}"),
        other => {
            let _ = poise::builtins::on_error(other).await;
        }
    }
}

// ─── Autocomplete ─────────────────────────────────────────────────────────

pub(crate) fn choices(names: Vec<String>) -> serenity::CreateAutocompleteResponse {
    serenity::CreateAutocompleteResponse::new().set_choices(names.into_iter().map(|n| serenity::AutocompleteChoice::new(n.clone(), n)).collect())
}

pub(crate) async fn system_names(ctx: Ctx<'_>, partial: &str) -> serenity::CreateAutocompleteResponse {
    // (A list, commas between: the last one is completed, those before it kept.)
    let (before, last) = partial.rsplit_once(',').map_or(("", partial), |(b, l)| (b, l));
    let p = last.trim().to_lowercase();
    let prefix = if before.is_empty() { String::new() } else { format!("{},", before.trim_end_matches(',')) };
    let names: Vec<String> = ctx
        .data()
        .shared
        .systems
        .lock()
        .unwrap()
        .iter()
        .map(|s| s.short_name.clone())
        .filter(|n| n.to_lowercase().contains(&p))
        .take(25)
        .map(|n| format!("{prefix}{n}"))
        .collect();
    choices(names)
}

pub(crate) async fn feed_names(ctx: Ctx<'_>, partial: &str) -> serenity::CreateAutocompleteResponse {
    let p = partial.to_lowercase();
    let here = ctx.channel_id().get().to_string();
    let guild = ctx.guild_id().map(|g| g.get().to_string()).unwrap_or_default();
    let mut feeds: Vec<Feed> = ctx.data().shared.feeds().into_iter().filter(|f| f.server == guild).collect();
    // This channel's first.
    feeds.sort_by_key(|f| f.channel != here);
    let names: Vec<String> = feeds.into_iter().map(|f| f.name).filter(|n| n.to_lowercase().contains(&p)).take(25).collect();
    choices(names)
}

// ─── Everyone's ───────────────────────────────────────────────────────────

/// What TRP is up to
#[poise::command(slash_command)]
async fn info(ctx: Ctx<'_>) -> Result<(), Error> {
    let sh = &ctx.data().shared;
    let systems = sh.systems.lock().unwrap().len();
    let (active, recording) = {
        let a = sh.active.lock().unwrap();
        (a.len(), a.values().filter(|c| c.recording).count())
    };
    let feeds = sh.feeds().iter().filter(|f| f.enabled).count();
    let e = CreateEmbed::new()
        .title("Trunk Recorder Pro")
        .color(RED)
        .field("Up", span(sh.started.elapsed().as_secs()), true)
        .field("Calls", sh.calls.load(Ordering::Relaxed).to_string(), true)
        .field("Posted", sh.posted.load(Ordering::Relaxed).to_string(), true)
        .field("Systems", systems.to_string(), true)
        .field("Active", format!("{active} ({recording} recording)"), true)
        .field("Feeds", feeds.to_string(), true)
        .timestamp(Timestamp::now());
    ctx.send(CreateReply::default().embed(e)).await?;
    Ok(())
}

/// Control channel decode rates
#[poise::command(slash_command)]
async fn rates(
    ctx: Ctx<'_>,
    #[description = "One system (empty: all)"]
    #[autocomplete = "system_names"]
    system: Option<String>,
) -> Result<(), Error> {
    let systems: Vec<String> = system.into_iter().collect();
    let e = post::rates_embed(&ctx.data().shared, &systems);
    ctx.send(CreateReply::default().embed(e)).await?;
    Ok(())
}

/// Calls going on now
#[poise::command(slash_command)]
async fn active(
    ctx: Ctx<'_>,
    #[description = "One system (empty: all)"]
    #[autocomplete = "system_names"]
    system: Option<String>,
) -> Result<(), Error> {
    let mut calls: Vec<_> = ctx.data().shared.active.lock().unwrap().values().filter(|c| system.as_ref().is_none_or(|s| *s == c.short_name)).cloned().collect();
    calls.sort_by(|a, b| a.start_time.total_cmp(&b.start_time));
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0);
    let mut e = CreateEmbed::new().title("Active calls").color(GREEN).timestamp(Timestamp::now());
    for c in calls.iter().take(25) {
        let title = format!(
            "{}{}{}",
            if c.talkgroup_tag.is_empty() { format!("TG {}", c.talkgroup) } else { format!("{} ({})", c.talkgroup_tag, c.talkgroup) },
            if c.emergency { " 🚨" } else { "" },
            if c.encrypted { " 🔒" } else { "" }
        );
        let mut v = format!("{} · {}\n{} s", c.short_name, mhz(c.freq_hz), (now - c.start_time).max(0.0).round());
        v += &if c.recording {
            "\n🔴 Recording".to_string()
        } else {
            format!("\n👂 Not recorded: {}", c.reason.clone().unwrap_or_default().replace('_', " "))
        };
        if let Some(s) = c.sources.last() {
            v += &format!("\nRadio {}{}", s.unit, if s.tag.is_empty() { String::new() } else { format!(" ({})", s.tag) });
        }
        if c.patched_talkgroups.len() > 1 {
            v += &format!("\nPatched {}", c.patched_talkgroups.iter().map(|t| t.to_string()).collect::<Vec<_>>().join(", "));
        }
        e = e.field(title, v, true);
    }
    e = match calls.len() {
        0 => e.description("No calls at the moment.").color(GRAY),
        n => e.footer(CreateEmbedFooter::new(format!(
            "{n} active call{}{}",
            if n == 1 { "" } else { "s" },
            system.map(|s| format!(" on {s}")).unwrap_or_default()
        ))),
    };
    ctx.send(CreateReply::default().embed(e)).await?;
    Ok(())
}

/// The latest recorded calls
#[poise::command(slash_command)]
async fn recent(
    ctx: Ctx<'_>,
    #[description = "Talkgroup"] talkgroup: Option<u32>,
    #[description = "System"]
    #[autocomplete = "system_names"]
    system: Option<String>,
    #[description = "Radio ID"] radio: Option<i64>,
    #[description = "How many (10)"]
    #[min = 1]
    #[max = 20]
    count: Option<usize>,
) -> Result<(), Error> {
    ctx.defer().await?;
    let sh = &ctx.data().shared;
    let q = CallsQuery { system: system.clone(), talkgroup, unit: radio, limit: Some(count.unwrap_or(10)), ..Default::default() };
    let page = tokio::time::timeout(Duration::from_secs(15), sh.ask_calls(q)).await.map_err(|_| "TRP didn't answer")??;
    let base = sh.settings().trp_address.trim().trim_end_matches('/').to_string();
    let mut lines = Vec::new();
    for c in &page.entries {
        let r = &c.record;
        let tg = if r.talkgroup_tag.is_empty() { format!("TG {}", r.talkgroup) } else { format!("{} ({})", r.talkgroup_tag, r.talkgroup) };
        let mut l = format!("<t:{}:t> **{tg}** · {} · {:.0} s{}", r.start_time, r.short_name, r.call_length, if r.emergency { " 🚨" } else { "" });
        if c.audio && !base.is_empty() && !r.decrypted {
            // (TRP serves the WAV for an .m4a it doesn't have.)
            l += &format!(" · [play]({base}/calls/{}.m4a)", c.path);
        }
        lines.push(l);
    }
    let mut e = CreateEmbed::new().title("Recent calls").color(BLUE);
    e = if lines.is_empty() { e.description("None found.").color(GRAY) } else { e.description(lines.join("\n")) };
    if let Some(err) = page.error {
        e = e.footer(CreateEmbedFooter::new(err));
    }
    ctx.send(CreateReply::default().embed(e)).await?;
    Ok(())
}

/// TRP's systems
#[poise::command(slash_command)]
async fn systems(ctx: Ctx<'_>) -> Result<(), Error> {
    let sh = &ctx.data().shared;
    let systems = sh.systems.lock().unwrap().clone();
    let rates = sh.rates.lock().unwrap().clone();
    let mut e = CreateEmbed::new().title("Systems").color(RED);
    for s in systems.iter().take(25) {
        let mut v = s.kind.to_uppercase();
        if !s.name.is_empty() {
            v = format!("{} · {v}", s.name);
        }
        if s.sites.len() > 1 {
            v += &format!(" · {} sites", s.sites.len());
        }
        for (site, r) in rates.iter().filter(|(_, r)| r.system == s.short_name) {
            v += &format!("\n{}{:.1} msg/s", if *site == s.short_name { String::new() } else { format!("{site}: ") }, r.now());
        }
        e = e.field(s.short_name.clone(), v, true);
    }
    ctx.send(CreateReply::default().embed(e)).await?;
    Ok(())
}

/// The tone threads open now
#[poise::command(slash_command)]
async fn threads(ctx: Ctx<'_>) -> Result<(), Error> {
    let open = ctx.data().shared.threads.all();
    let lines: Vec<String> = open
        .iter()
        .map(|t| {
            let state = match &t.stopped {
                Some(who) => format!("stopped by {who}"),
                None => "posting".to_string(),
            };
            format!("<#{}> · {} · <t:{}:R> · {} call{} · {state}", t.thread, t.title, t.time as i64, t.calls, if t.calls == 1 { "" } else { "s" })
        })
        .collect();
    let text = if lines.is_empty() { "No tone threads are open.".to_string() } else { lines.join("\n").chars().take(4000).collect() };
    ctx.send(CreateReply::default().embed(CreateEmbed::new().title("Tone threads").description(text).color(RED)).ephemeral(true)).await?;
    Ok(())
}

// ─── Feeds (Manage Channels) ──────────────────────────────────────────────

#[derive(Clone, Copy, Debug, poise::ChoiceParameter)]
enum What {
    #[name = "Recorded calls"]
    Call,
    #[name = "Calls as they start"]
    Start,
    #[name = "Emergencies"]
    Emergency,
    #[name = "Decode rates"]
    Rates,
    #[name = "Signals: named tones (a thread for each talkgroup)"]
    Signal,
    #[name = "Tones: any tone heard (a thread for each talkgroup)"]
    Tone,
    #[name = "Daily digest (at midnight)"]
    Digest,
}

impl From<What> for Event {
    fn from(w: What) -> Event {
        match w {
            What::Call => Event::Call,
            What::Start => Event::Start,
            What::Emergency => Event::Emergency,
            What::Rates => Event::Rates,
            What::Signal => Event::Signal,
            What::Tone => Event::Tone,
            What::Digest => Event::Digest,
        }
    }
}

/// What the bot posts, and where
#[poise::command(
    slash_command,
    guild_only,
    default_member_permissions = "MANAGE_CHANNELS",
    subcommands("feed_add", "feed_list", "feed_remove", "feed_on", "feed_off", "feed_edit"),
    subcommand_required
)]
async fn feed(_: Ctx<'_>) -> Result<(), Error> {
    Ok(())
}

fn list(s: &str) -> Vec<String> {
    s.split([',', ' ']).map(str::trim).filter(|x| !x.is_empty()).map(String::from).collect()
}

/// Post something to a channel (this one unless you pick another)
#[poise::command(slash_command, rename = "add")]
#[allow(clippy::too_many_arguments)]
async fn feed_add(
    ctx: Ctx<'_>,
    #[description = "What to post"] what: What,
    #[description = "Systems, by short name, commas between (empty: all)"]
    #[autocomplete = "system_names"]
    systems: Option<String>,
    #[description = "Talkgroups: 101, 1000-1099, 507* (empty: all)"] talkgroups: Option<String>,
    #[description = "But not these talkgroups"] except: Option<String>,
    #[description = "Signals: which, by ID, commas between (empty: any signal)"] signals: Option<String>,
    #[description = "Attach recordings (recorded calls; yes)"] audio: Option<bool>,
    #[description = "Tones: play each thread's talkgroup in live audio at once (no)"] live: Option<bool>,
    #[description = "Shortest call to post, s (recorded calls)"]
    #[min = 0.0]
    min_length: Option<f64>,
    #[description = "Text above each post: @role mentions go here"] mention: Option<String>,
    #[description = "The channel (this one)"]
    #[channel_types("Text", "News", "PublicThread", "PrivateThread")]
    channel: Option<serenity::Channel>,
) -> Result<(), Error> {
    let channel = channel.map(|c| c.id()).unwrap_or(ctx.channel_id());
    change(ctx, move |feeds| {
        let event: Event = what.into();
        // (Named by the plugin, after its channel and what it posts.)
        let f = Feed {
            channel: channel.get().to_string(),
            event,
            systems: systems.as_deref().map(list).unwrap_or_default(),
            signals: signals.as_deref().map(list).unwrap_or_default(),
            talkgroups: talkgroups.as_deref().map(list).unwrap_or_default(),
            except_talkgroups: except.as_deref().map(list).unwrap_or_default(),
            audio: audio.unwrap_or(true),
            live: live.unwrap_or(false),
            min_length_s: min_length.unwrap_or(0.0),
            mention: mention.unwrap_or_default(),
            ..Default::default()
        };
        let said = format!("Added {}", f.describe());
        feeds.push(f);
        Ok(said)
    })
    .await
}

/// The feeds (this channel's, or all)
#[poise::command(slash_command, rename = "list")]
async fn feed_list(ctx: Ctx<'_>, #[description = "Every channel's (no: this one's)"] all: Option<bool>) -> Result<(), Error> {
    let here = ctx.channel_id().get().to_string();
    let all = all.unwrap_or(false);
    let sh = &ctx.data().shared;
    // (This server's alone: another's are none of its business.)
    let guild = ctx.guild_id().map(|g| g.get().to_string()).unwrap_or_default();
    let feeds: Vec<String> = sh.feeds().into_iter().filter(|f| f.server == guild).filter(|f| all || f.channel == here).map(|f| f.describe()).collect();
    let text = match (feeds.is_empty(), all) {
        (true, false) => "No feeds post here. `/feed add` adds one; `/feed list all:True` shows every channel's.".to_string(),
        (true, true) => "No feeds yet. `/feed add` in a channel adds one.".to_string(),
        _ => feeds.join("\n"),
    };
    // (A description holds 4096 characters.)
    let text: String = text.chars().take(4000).collect();
    ctx.send(CreateReply::default().embed(CreateEmbed::new().title("Feeds").description(text).color(BLUE)).ephemeral(true)).await?;
    Ok(())
}

/// Remove a feed
#[poise::command(slash_command, rename = "remove")]
async fn feed_remove(
    ctx: Ctx<'_>,
    #[description = "Which"]
    #[autocomplete = "feed_names"]
    name: String,
) -> Result<(), Error> {
    change(ctx, move |feeds| {
        let at = find(feeds, &name)?;
        let f = feeds.remove(at);
        Ok(format!("Removed {}", f.describe()))
    })
    .await
}

/// Switch a feed on
#[poise::command(slash_command, rename = "on")]
async fn feed_on(
    ctx: Ctx<'_>,
    #[description = "Which"]
    #[autocomplete = "feed_names"]
    name: String,
) -> Result<(), Error> {
    switch(ctx, name, true).await
}

/// Switch a feed off (kept, to switch on again)
#[poise::command(slash_command, rename = "off")]
async fn feed_off(
    ctx: Ctx<'_>,
    #[description = "Which"]
    #[autocomplete = "feed_names"]
    name: String,
) -> Result<(), Error> {
    switch(ctx, name, false).await
}

async fn switch(ctx: Ctx<'_>, name: String, on: bool) -> Result<(), Error> {
    change(ctx, move |feeds| {
        let at = find(feeds, &name)?;
        let f = &mut feeds[at];
        f.enabled = on;
        Ok(format!("Switched {}: {}", if on { "on" } else { "off" }, f.describe()))
    })
    .await
}

/// Where feed `name` is among a server's; Err: it isn't one.
pub(crate) fn find(feeds: &[Feed], name: &str) -> Result<usize, String> {
    feeds.iter().position(|f| f.name.eq_ignore_ascii_case(name.trim())).ok_or_else(|| format!("No feed here is called {name:?}."))
}

/// Change this server's feeds with `f`, saved to the settings (and so TRP's page); say
/// what it said, or why not.
async fn change(ctx: Ctx<'_>, f: impl FnOnce(&mut Vec<Feed>) -> Result<String, String>) -> Result<(), Error> {
    let sh = ctx.data().shared.clone();
    let g = ctx.guild_id().map_or(0, |g| g.get());
    match sh.edit_server(g, |s| f(&mut s.feeds)) {
        Ok(said) => {
            sh.host.info(format!("{} (by {})", said.replace("**", ""), ctx.author().name));
            ctx.send(CreateReply::default().content(said)).await?;
        }
        Err(e) => {
            ctx.send(CreateReply::default().content(e).ephemeral(true)).await?;
        }
    }
    Ok(())
}

// ─── The plugin page's buttons ────────────────────────────────────────────

/// The bot's user ID: known once it has logged in, else read from the token (its first part).
fn bot_id(shared: &Shared) -> Option<u64> {
    if let Some(id) = *shared.me.lock().unwrap() {
        return Some(id);
    }
    let first = shared.settings().token.trim().split('.').next()?.to_string();
    // (Base64 of the ID's digits, without padding.)
    let mut bits = 0u32;
    let mut n = 0;
    let mut out = Vec::new();
    for c in first.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => break,
        } as u32;
        bits = bits << 6 | v;
        n += 6;
        if n >= 8 {
            n -= 8;
            out.push((bits >> n) as u8);
        }
    }
    String::from_utf8(out).ok()?.parse().ok()
}

/// The bot's status line (`statusText`), set again whenever it's changed.
async fn keep_presence(sh: std::sync::Arc<Shared>, ctx: serenity::Context) {
    let mut shown: Option<String> = None;
    loop {
        let text = sh.settings().status_text.trim().to_string();
        if shown.as_ref() != Some(&text) {
            ctx.set_activity((!text.is_empty()).then(|| serenity::ActivityData::custom(text.clone())));
            shown = Some(text);
        }
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    }
}

pub fn invite_url(id: u64) -> String {
    format!("https://discord.com/oauth2/authorize?client_id={id}&scope=bot+applications.commands&permissions={PERMISSIONS}")
}

/// Names for permission bits, for Check access.
const NEEDS: &[(u32, &str)] = &[
    (10, "View Channel"),
    (11, "Send Messages"),
    (14, "Embed Links"),
    (15, "Attach Files"),
    (16, "Read Message History"),
    (35, "Create Public Threads"),
    (38, "Send Messages in Threads"),
    (34, "Manage Threads"),
];
const VOICE_NEEDS: &[(u32, &str)] = &[(20, "Connect"), (21, "Speak"), (48, "Set Voice Channel Status")];

/// A button on the plugin's page: Check access (a server). (Invite link, from before 0.3: the
/// link, now a field of its own.)
pub async fn action(shared: &Shared, a: trunk_recorder_plugin::Action, reply: trunk_recorder_plugin::Reply) {
    match a.action.as_str() {
        "invite" => match bot_id(shared) {
            Some(id) => reply.ok(invite_url(id)),
            None => reply.fail("Set the bot token first (and save): the link is made from it."),
        },
        "check" => {
            let server: crate::settings::Server = serde_json::from_value(a.value).unwrap_or_default();
            match check(shared, &server).await {
                Ok(said) => reply.ok(said),
                Err(said) => reply.fail(said),
            }
        }
        other => reply.fail(format!("no such button: {other}")),
    }
}

/// Permissions `base` (the bot's roles') in channel `c`: its overwrites, @everyone's, then its
/// roles', then the bot's own.
fn in_channel(base: u64, c: &serde_json::Value, g: u64, roles: &[u64], me: u64) -> u64 {
    let ow: Vec<(u64, u64, u64)> = c["permission_overwrites"]
        .as_array()
        .map(|o| {
            o.iter()
                .map(|x| {
                    (
                        crate::rest::id(&x["id"]),
                        x["allow"].as_str().and_then(|v| v.parse().ok()).unwrap_or(0),
                        x["deny"].as_str().and_then(|v| v.parse().ok()).unwrap_or(0),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let mut p = base;
    if let Some((_, a, d)) = ow.iter().find(|(id, _, _)| *id == g) {
        p = (p & !d) | a;
    }
    let (a, d) = ow.iter().filter(|(id, _, _)| roles.contains(id)).fold((0, 0), |(a, d), x| (a | x.1, d | x.2));
    p = (p & !d) | a;
    if let Some((_, a, d)) = ow.iter().find(|(id, _, _)| *id == me) {
        p = (p & !d) | a;
    }
    p
}

/// Is the bot in `s`, can it see the channels its feeds and live audio use, has it what it
/// needs? Ok: all's well (what it found); Err: what's wrong.
pub(crate) async fn check(shared: &Shared, s: &crate::settings::Server) -> Result<String, String> {
    let rest = shared.rest().ok_or("The bot isn't logged in yet (is the plugin on, and the token right?).")?;
    let g = s.guild().ok_or("Fill in the server ID first.")?;
    let me = (*shared.me.lock().unwrap()).ok_or("The bot isn't logged in yet.")?;
    let invite = invite_url(me);
    let guild = match rest.get(&format!("/guilds/{g}")).await {
        Ok(v) => v,
        Err(e) if matches!(e.status, 403 | 404) => return Err(format!("The bot isn't in this server: invite it with {invite}")),
        Err(e) => return Err(format!("Couldn't ask Discord: {e}")),
    };
    let name = guild["name"].as_str().unwrap_or("?").to_string();
    // What its roles (and @everyone) let it do, server-wide.
    let member = rest.get(&format!("/guilds/{g}/members/{me}")).await.map_err(|e| format!("Couldn't read the bot's roles: {e}"))?;
    let mine: Vec<u64> = member["roles"].as_array().map(|r| r.iter().map(crate::rest::id).collect()).unwrap_or_default();
    let roles = guild["roles"].as_array().cloned().unwrap_or_default();
    let mut perms: u64 = 0;
    for r in &roles {
        let id = crate::rest::id(&r["id"]);
        if id == g || mine.contains(&id) {
            perms |= r["permissions"].as_str().and_then(|p| p.parse().ok()).unwrap_or(0);
        }
    }
    let admin = perms & (1 << 3) != 0;
    let mut needs: Vec<(u32, &str)> = NEEDS.to_vec();
    if s.voice().is_some() {
        needs.extend_from_slice(VOICE_NEEDS);
        if !s.live.chime.trim().is_empty() {
            needs.push((42, "Use Soundboard"));
        }
    }
    let mut problems: Vec<String> = Vec::new();
    // The channels it uses: each there, in this server, and seen.
    let mut channels: Vec<(String, u64)> = s
        .feeds
        .iter()
        .filter_map(|f| Some((if f.name.trim().is_empty() { f.event.label().to_string() } else { format!("feed {:?}", f.name) }, f.channel_id()?)))
        .collect();
    if let Some(v) = s.voice() {
        channels.push(("live audio".into(), v));
    }
    let mut once: Vec<(String, u64)> = Vec::new();
    for (what, ch) in channels {
        match once.iter_mut().find(|(_, c)| *c == ch) {
            Some((w, _)) => *w += &format!(", {what}"),
            None => once.push((what, ch)),
        }
    }
    let channels = once;
    let mut seen: Vec<String> = Vec::new();
    for (what, ch) in &channels {
        match rest.get(&format!("/channels/{ch}")).await {
            Ok(c) if crate::rest::id(&c["guild_id"]) != g => problems.push(format!("{what}: channel {ch} is in another server")),
            Ok(c) => {
                let n = c["name"].as_str().unwrap_or("?");
                let n = if matches!(c["type"].as_u64(), Some(2 | 13)) { format!("🔊 {n}") } else { format!("#{n}") };
                seen.push(format!("{n} ({what})"));
                // A forum: what the bot may do there, its own permissions over the role's.
                if c["type"].as_u64() == Some(15) && !admin {
                    let p = in_channel(perms, &c, g, &mine, me);
                    let mut lacks: Vec<&str> = [(11, "Create Posts"), (38, "Send Messages in Posts"), (34, "Manage Posts (to close them)")]
                        .iter()
                        .filter(|(b, _)| p & (1u64 << b) == 0)
                        .map(|(_, n)| *n)
                        .collect();
                    let tags: Vec<String> = c["available_tags"]
                        .as_array()
                        .map(|t| t.iter().filter_map(|x| x["name"].as_str().map(str::to_lowercase)).collect())
                        .unwrap_or_default();
                    let has_tags = ["active", "stopped", "closed"].iter().all(|t| tags.iter().any(|x| x == t));
                    if p & (1 << 4) == 0 && !has_tags {
                        lacks.push("Manage Channel (to add its tags)");
                    }
                    if !lacks.is_empty() {
                        problems.push(format!(
                            "forum {n}: the bot lacks {} there. Allow them for the bot in the forum's Edit Channel → Permissions, or invite it again with the link on the plugin's page{}",
                            lacks.join(", "),
                            if has_tags || p & (1 << 4) != 0 { "" } else { " (or make tags named Active, Stopped and Closed yourself: then it only applies them)" }
                        ));
                    }
                }
            }
            Err(e) if e.status == 404 => problems.push(format!("{what}: there's no channel {ch}")),
            Err(e) if e.status == 403 => problems.push(format!("{what}: the bot can't see channel {ch} (give its role View Channel there)")),
            Err(e) => problems.push(format!("{what}: couldn't look at channel {ch}: {e}")),
        }
    }
    if !admin {
        let missing: Vec<&str> = needs.iter().filter(|(b, _)| perms & (1u64 << b) == 0).map(|(_, n)| *n).collect();
        if !missing.is_empty() {
            problems.insert(0, format!("its role lacks {} (invite it again with {invite}, or add them to its role)", missing.join(", ")));
        }
    }
    if problems.is_empty() {
        let list = if seen.is_empty() { "no channels set yet".to_string() } else { seen.join(", ") };
        Ok(format!("In {name}: {list}. It can see them all and has every permission it needs."))
    } else {
        Err(format!("In {name}: {}.", problems.join("; ")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The invite link works before the bot has logged in: its ID is the token's first part.
    #[test]
    fn the_bot_id_comes_from_the_token() {
        let (host, _) = trunk_recorder_plugin::testing::capture();
        let settings = Settings { token: ["MTIzNDU2Nzg5MDEyMzQ1Njc4", "Gx1234", &"x".repeat(38)].join("."), ..Default::default() };
        let sh = Shared::new(host, settings, Vec::new(), trunk_recorder_plugin::testing::temp_dir("discord-botid"));
        assert_eq!(bot_id(&sh), Some(123456789012345678));
        assert!(invite_url(1).contains("permissions=286199443999760"));
    }
}
