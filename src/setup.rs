//! `/setup`: a server's guided setup, in Discord. A panel only its caller sees: the voice channel
//! for live audio (a dropdown of the server's), what the channel it's run in gets (a dropdown of
//! the kinds of feed: picked, a feed for it; unpicked, gone), and buttons to switch live audio off
//! and to check the bot's access. Saved to the settings as it's changed, so TRP's page
//! shows it at once, and it can be changed there too.

use poise::serenity_prelude as serenity;
use poise::CreateReply;
use serenity::{
    ButtonStyle, ChannelType, ComponentInteraction, ComponentInteractionDataKind, CreateActionRow, CreateButton, CreateEmbed, CreateEmbedFooter,
    CreateInteractionResponse, CreateInteractionResponseMessage, CreateSelectMenu, CreateSelectMenuKind, CreateSelectMenuOption,
};

use crate::bot::{Ctx, Error};
use crate::post::{GRAY, GREEN};
use crate::settings::{Event, Feed};
use crate::state::Shared;

const PREFIX: &str = "trp-setup";

/// The kinds of feed a channel can get, in the dropdown's order.
const KINDS: [(Event, &str); 7] = [
    (Event::Call, "Each recorded call, with its recording"),
    (Event::Start, "Each call as it starts: a live log"),
    (Event::Emergency, "Each emergency, once, as it's flagged"),
    (Event::Signal, "A thread for each talkgroup a named tone (signal) is heard on"),
    (Event::Tone, "A thread for each talkgroup any tone is heard on"),
    (Event::Rates, "The control channels' decode rates, one message kept current"),
    (Event::Digest, "The day's figures, at midnight"),
];

fn value(e: Event) -> &'static str {
    match e {
        Event::Call => "call",
        Event::Start => "start",
        Event::Emergency => "emergency",
        Event::Signal => "signal",
        Event::Tone => "tone",
        Event::Rates => "rates",
        Event::Digest => "digest",
    }
}

/// The panel for server `g`, run in `channel`.
fn panel(sh: &Shared, cache: &serenity::Cache, g: u64, channel: u64) -> (CreateEmbed, Vec<CreateActionRow>) {
    let server = sh.server(g).unwrap_or_default();
    let name = crate::names::guild(cache, g).unwrap_or_else(|| "this server".into());
    let live = match server.voice() {
        Some(v) => format!("🔊 <#{v}>"),
        None => "Off. Pick a voice channel below to play followed talkgroups there.".into(),
    };
    let here: Vec<Feed> = server.feeds.iter().filter(|f| f.channel_id() == Some(channel)).cloned().collect();
    let posts = if here.is_empty() {
        "Nothing yet. Pick below what it should get.".to_string()
    } else {
        here.iter().map(|f| format!("• {}", f.describe())).collect::<Vec<_>>().join("\n")
    };
    let e = CreateEmbed::new()
        .title(format!("Set up Trunk Recorder in {name}"))
        .color(if server.voice().is_some() || !here.is_empty() { GREEN } else { GRAY })
        .field("Live audio", live, false)
        .field(format!("<#{channel}> gets"), posts, false)
        .footer(CreateEmbedFooter::new(
            "Changes are saved as you make them. Run /setup in another channel to set that one up. /feed edit narrows a feed to some talkgroups.",
        ));
    let mut rows = Vec::new();
    let voice = CreateSelectMenuKind::Channel {
        channel_types: Some(vec![ChannelType::Voice, ChannelType::Stage]),
        default_channels: server.voice().map(|v| vec![serenity::ChannelId::new(v)]),
    };
    rows.push(CreateActionRow::SelectMenu(
        CreateSelectMenu::new(format!("{PREFIX}:voice"), voice).placeholder("Live audio: pick a voice channel").min_values(1).max_values(1),
    ));
    let options: Vec<CreateSelectMenuOption> = KINDS
        .iter()
        .map(|(e, d)| CreateSelectMenuOption::new(capital(e.label()), value(*e)).description(*d).default_selection(here.iter().any(|f| f.event == *e)))
        .collect();
    rows.push(CreateActionRow::SelectMenu(
        CreateSelectMenu::new(format!("{PREFIX}:post:{channel}"), CreateSelectMenuKind::String { options })
            .placeholder("What this channel gets")
            .min_values(0)
            .max_values(KINDS.len() as u8),
    ));
    if server.voice().is_some() {
        let options: Vec<CreateSelectMenuOption> = (-12..=12)
            .rev()
            .map(|step| {
                let pct = step * 25;
                let label = if step == 0 { "Volume: usual".to_string() } else { format!("Volume: {pct:+}%") };
                CreateSelectMenuOption::new(label, pct.to_string()).default_selection((server.live.volume - pct as f64).abs() < 0.5)
            })
            .collect();
        rows.push(CreateActionRow::SelectMenu(
            CreateSelectMenu::new(format!("{PREFIX}:volume"), CreateSelectMenuKind::String { options })
                .placeholder("Live audio volume")
                .min_values(1)
                .max_values(1),
        ));
    }
    let mut buttons = vec![CreateButton::new(format!("{PREFIX}:check")).label("Check access").style(ButtonStyle::Secondary)];
    if server.voice().is_some() {
        buttons.insert(0, CreateButton::new(format!("{PREFIX}:liveoff")).label("Live audio off").style(ButtonStyle::Secondary));
        if let Some(f) = here.iter().find(|f| f.event.threads()) {
            let (label, style) =
                if f.live { ("🔊 Tone threads play live: on", ButtonStyle::Success) } else { ("🔊 Tone threads play live: off", ButtonStyle::Secondary) };
            buttons.insert(0, CreateButton::new(format!("{PREFIX}:tonelive:{channel}")).label(label).style(style));
        }
    }
    rows.push(CreateActionRow::Buttons(buttons));
    (e, rows)
}

fn capital(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

/// Set up the bot in this server: live audio, and what this channel gets
#[poise::command(slash_command, guild_only, default_member_permissions = "MANAGE_GUILD")]
pub async fn setup(
    ctx: Ctx<'_>,
    #[description = "Live audio volume adjust, % (0: usual; +100: twice as loud)"]
    #[min = -300]
    #[max = 300]
    volume: Option<i64>,
    #[description = "Live audio chime: a soundboard sound's ID (right-click it, Copy ID); 0: none"] chime: Option<String>,
) -> Result<(), Error> {
    let sh = ctx.data().shared.clone();
    let g = ctx.guild_id().map_or(0, |g| g.get());
    let mut said = Vec::new();
    if volume.is_some() || chime.is_some() {
        let r = sh.edit_server(g, |s| {
            if let Some(x) = volume {
                // (In steps of 25.)
                s.live.volume = ((x as f64 / 25.0).round() * 25.0).clamp(-300.0, 300.0);
                said.push(format!("Live audio volume: {:+}%.", s.live.volume));
            }
            if let Some(c) = &chime {
                s.live.chime = if c.trim() == "0" { String::new() } else { c.trim().to_string() };
                said.push(if s.live.chime.is_empty() { "No chime.".to_string() } else { "Chime set.".to_string() });
            }
            Ok(String::new())
        });
        if let Err(e) = r {
            said = vec![format!("Not saved: {e}")];
        }
    }
    let (e, rows) = panel(&sh, &ctx.serenity_context().cache, g, ctx.channel_id().get());
    ctx.send(CreateReply::default().content(said.join(" ")).embed(e).components(rows).ephemeral(true)).await?;
    Ok(())
}

/// A press on a setup panel. False: it isn't one.
pub async fn component(ctx: &serenity::Context, sh: &Shared, b: &ComponentInteraction) -> Result<bool, Box<serenity::Error>> {
    let id = b.data.custom_id.clone();
    let Some(rest) = id.strip_prefix(&format!("{PREFIX}:")) else { return Ok(false) };
    let g = b.guild_id.map_or(0, |g| g.get());
    let say = |text: String| CreateInteractionResponse::Message(CreateInteractionResponseMessage::new().content(text).ephemeral(true));
    // (Only admins get here: see bot::on_event.)
    let who = b.user.global_name.clone().unwrap_or_else(|| b.user.name.clone());
    let mut channel = b.channel_id.get();
    match (rest.split(':').next().unwrap_or(""), &b.data.kind) {
        ("voice", ComponentInteractionDataKind::ChannelSelect { values }) => {
            if let Some(v) = values.first() {
                let _ = sh.edit_server(g, |s| {
                    s.live.channel = v.get().to_string();
                    Ok(String::new())
                });
                sh.host.info(format!("live audio in server {g}: voice channel {v}, by {who}"));
            }
        }
        ("post", ComponentInteractionDataKind::StringSelect { values }) => {
            channel = rest.split(':').nth(1).and_then(|c| c.parse().ok()).unwrap_or(channel);
            let want: Vec<Event> = KINDS.iter().map(|(e, _)| *e).filter(|e| values.iter().any(|v| v == value(*e))).collect();
            let result = sh.edit_server(g, move |s| {
                let feeds = &mut s.feeds;
                let before = feeds.len();
                // Those of this channel no longer picked go; those picked and missing come.
                feeds.retain(|f| f.channel_id() != Some(channel) || want.contains(&f.event));
                let removed = before - feeds.len();
                let mut added = 0;
                for e in &want {
                    if !feeds.iter().any(|f| f.channel_id() == Some(channel) && f.event == *e) {
                        // (Named by the plugin, after the channel and what it posts.)
                        feeds.push(Feed { channel: channel.to_string(), event: *e, ..Default::default() });
                        added += 1;
                    }
                }
                Ok(format!("{added} added, {removed} removed"))
            });
            match result {
                Ok(n) => sh.host.info(format!("feeds in channel {channel}: {n}, by {who}")),
                Err(e) => {
                    b.create_response(ctx, say(format!("Not saved: {e}"))).await?;
                    return Ok(true);
                }
            }
        }
        ("volume", ComponentInteractionDataKind::StringSelect { values }) => {
            if let Some(pct) = values.first().and_then(|v| v.parse::<f64>().ok()) {
                let _ = sh.edit_server(g, |s| {
                    s.live.volume = pct.clamp(-300.0, 300.0);
                    Ok(String::new())
                });
                sh.host.info(format!("live audio in server {g}: volume {pct:+}%, by {who}"));
            }
        }
        ("tonelive", _) => {
            channel = rest.split(':').nth(1).and_then(|c| c.parse().ok()).unwrap_or(channel);
            let r = sh.edit_server(g, move |s| {
                let on = !s.feeds.iter().any(|f| f.channel_id() == Some(channel) && f.event.threads() && f.live);
                s.feeds.iter_mut().filter(|f| f.channel_id() == Some(channel) && f.event.threads()).for_each(|f| f.live = on);
                Ok(if on { "on" } else { "off" }.to_string())
            });
            match r {
                Ok(on) => sh.host.info(format!("tone threads in channel {channel} play live: {on}, by {who}")),
                Err(e) => {
                    b.create_response(ctx, say(format!("Not saved: {e}"))).await?;
                    return Ok(true);
                }
            }
        }
        ("liveoff", _) => {
            let _ = sh.edit_server(g, |s| {
                s.live.channel.clear();
                Ok(String::new())
            });
            sh.live.unfollow(|f| f.server == g);
            sh.host.info(format!("live audio in server {g}: off, by {who}"));
        }
        ("check", _) => {
            let server = sh.server(g).unwrap_or_default();
            let text = match crate::bot::check(sh, &server).await {
                Ok(t) => format!("✓ {t}"),
                Err(t) => format!("✗ {t}"),
            };
            b.create_response(ctx, say(text)).await?;
            return Ok(true);
        }
        _ => return Ok(false),
    }
    let (e, rows) = panel(sh, &ctx.cache, g, channel);
    b.create_response(ctx, CreateInteractionResponse::UpdateMessage(CreateInteractionResponseMessage::new().embed(e).components(rows))).await?;
    Ok(true)
}
