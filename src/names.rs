//! Names from Discord: servers and channels are set up by ID, and called by their names (as the
//! cache has them, which Discord keeps up to date: a channel renamed is renamed here). The names
//! are filled into the settings (shown on the page, never typed), and the plugin's status says,
//! for each server, what its feeds post to and where its live audio plays, by name; and what's
//! wrong (a server the bot isn't in, a channel it can't see).

use std::sync::Arc;
use std::time::Duration;

use poise::serenity_prelude as serenity;
use serenity::{ChannelId, ChannelType, GuildId};
use trunk_recorder_plugin::State;

use crate::state::Shared;

/// A server's name, if the bot is in it.
pub fn guild(cache: &serenity::Cache, g: u64) -> Option<String> {
    cache.guild(GuildId::new(g)).map(|x| x.name.clone())
}

/// A channel's name as people know it ("#dispatch", "🔊 General"), if the bot can see it.
pub fn channel(cache: &serenity::Cache, g: u64, ch: u64) -> Option<String> {
    let guild = cache.guild(GuildId::new(g))?;
    let id = ChannelId::new(ch);
    if let Some(c) = guild.channels.get(&id) {
        return Some(match c.kind {
            ChannelType::Voice | ChannelType::Stage => format!("🔊 {}", c.name),
            ChannelType::Forum => format!("💬 {}", c.name),
            _ => format!("#{}", c.name),
        });
    }
    guild.threads.iter().find(|t| t.id == id).map(|t| format!("🧵 {}", t.name))
}

/// A channel's own name (no #), if the bot can see it.
fn bare(cache: &serenity::Cache, g: u64, ch: u64) -> Option<String> {
    let guild = cache.guild(GuildId::new(g))?;
    let id = ChannelId::new(ch);
    guild.channels.get(&id).map(|c| c.name.clone()).or_else(|| guild.threads.iter().find(|t| t.id == id).map(|t| t.name.clone()))
}

/// Fill in `s`'s names from what the bot sees ([`crate::settings::Settings::named`]), and who
/// each server's access names.
pub fn fill(s: &mut crate::settings::Settings, cache: &serenity::Cache) -> bool {
    let mut changed = s.named(Some(crate::bot::invite_url(cache.current_user().id.get())), |g| guild(cache, g), |g, ch| bare(cache, g, ch));
    for server in &mut s.servers {
        let Some(g) = server.guild() else { continue };
        let a = &mut server.access;
        let (admins, users) = (who(cache, g, &a.admins), who(cache, g, &a.users));
        if a.admin_names != admins || a.user_names != users {
            (a.admin_names, a.user_names) = (admins, users);
            changed = true;
        }
    }
    changed
}

/// Roles and people by name ("@Dispatch, @alex"), as listed by ID; one the bot can't see by its ID.
pub fn who(cache: &serenity::Cache, g: u64, ids: &[String]) -> String {
    let guild = cache.guild(GuildId::new(g));
    ids.iter()
        .filter_map(|x| crate::access::id_of(x))
        .map(|id| {
            if id == g {
                return "@everyone".to_string();
            }
            let role = guild.as_ref().and_then(|x| x.roles.get(&serenity::RoleId::new(id)).map(|r| format!("@{}", r.name)));
            let member = || guild.as_ref().and_then(|x| x.members.get(&serenity::UserId::new(id)).map(|m| format!("@{}", m.display_name())));
            let user = || cache.user(serenity::UserId::new(id)).map(|u| format!("@{}", u.global_name.clone().unwrap_or_else(|| u.name.clone())));
            role.or_else(member).or_else(user).unwrap_or_else(|| id.to_string())
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// The status: each server by name, its feeds' channels and its live audio; or what's wrong.
pub fn status(shared: &Shared, cache: &serenity::Cache) -> (State, String) {
    let me = cache.current_user().name.clone();
    let servers = shared.servers();
    if servers.is_empty() {
        return (State::Warning, format!("logged in as {me}, in no server yet: open Invite to a server below and add it to yours, then run /setup there"));
    }
    let mut parts = Vec::new();
    let mut wrong = false;
    for s in &servers {
        let Some(g) = s.guild() else { continue };
        let Some(name) = guild(cache, g) else {
            wrong = true;
            parts.push(format!("server {g}: the bot isn't in it (Invite to a server)"));
            continue;
        };
        let feeds: Vec<_> = s.feeds.iter().filter(|f| f.enabled && f.channel_id().is_some()).collect();
        let mut chans: Vec<String> = Vec::new();
        for f in &feeds {
            let ch = f.channel_id().unwrap_or(0);
            let n = channel(cache, g, ch).unwrap_or_else(|| {
                wrong = true;
                format!("{ch} (can't see it)")
            });
            if !chans.contains(&n) {
                chans.push(n);
            }
        }
        let mut line = format!("{name}: {} feed{}", feeds.len(), if feeds.len() == 1 { "" } else { "s" });
        if !chans.is_empty() {
            line += &format!(" → {}", chans.join(", "));
        }
        if let Some(v) = s.voice() {
            let n = channel(cache, g, v).unwrap_or_else(|| {
                wrong = true;
                format!("{v} (can't see it)")
            });
            line += &format!(" · live audio in {n}");
        }
        parts.push(line);
    }
    (if wrong { State::Warning } else { State::Ok }, format!("logged in as {me} · {}", parts.join(" | ")))
}

/// Keep the names and the status as they are (a rename, a server joined, settings changed),
/// while the bot runs; and say so if TRP doesn't save what Discord changes.
pub async fn keep_status(shared: Arc<Shared>, cache: Arc<serenity::Cache>) {
    let mut said: Option<(State, String)> = None;
    // (The servers' channels come just after login.)
    tokio::time::sleep(Duration::from_secs(3)).await;
    loop {
        if shared.posts.lock().unwrap().is_none() {
            return;
        }
        shared.rename();
        shared.check_saves();
        let now = status(&shared, &cache);
        if said.as_ref() != Some(&now) {
            shared.host.status(now.0, now.1.clone());
            said = Some(now);
        }
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(15)) => {}
            _ = shared.renamed.notified() => {}
        }
    }
}
