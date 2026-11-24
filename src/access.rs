//! Who may do what in a server. Two kinds of people:
//!
//! - **Admins** set the bot up: `/setup`, feeds, boards, mutes, opening threads. Anyone
//!   with Manage Server always is one (so a server can't lock itself out), and so is each role or
//!   person in the server's **Admins**.
//! - **Users** use it: everything else (`/active`, `/recent`, `/radio`, live audio, a thread's
//!   Stop and Resume, personal alerts). Everyone, unless the server's **Users** names roles or
//!   people; then only those, and the admins.
//! - Closing or deleting a tone thread takes an admin, or Manage Threads.
//!
//! Every way in is checked the same: slash and right-click commands, buttons and menus on the
//! bot's messages, and words in the live audio chat. Commands are also shown only to whom Discord
//! would let use them by default (Manage Server, Manage Channels…), unless a server names admins of
//! its own: then the admin commands are shown to everyone, and refused here to whoever isn't one.

use poise::serenity_prelude as serenity;
use serde_json::Value;

use crate::settings::Access;

/// What something needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    User,
    Admin,
}

impl Tier {
    fn words(self) -> &'static str {
        match self {
            Tier::User => "This server lets only some roles use the bot.",
            Tier::Admin => "That's for the bot's admins in this server (Manage Server, or one of the roles set as its admins).",
        }
    }
}

/// The commands only admins may use (by name, as Discord shows them: a right-click command by its
/// label).
const ADMIN: &[&str] = &["setup", "feed", "board", "mutes", "unmute", "thread", "check", "log", "test", "Open a thread here", "Mute here"];

pub fn command_tier(name: &str) -> Tier {
    if ADMIN.contains(&name) {
        Tier::Admin
    } else {
        Tier::User
    }
}

/// What a button or menu on one of the bot's messages needs, by its ID.
pub fn component_tier(custom_id: &str) -> Tier {
    if custom_id.starts_with("trp-setup:") {
        Tier::Admin
    } else {
        Tier::User
    }
}

/// Someone in a server, as far as access goes.
pub struct Who {
    pub guild: u64,
    pub user: u64,
    pub roles: Vec<u64>,
    /// They have Manage Server (or Administrator) there.
    pub manage_guild: bool,
}

impl Who {
    /// The one pressing a button or running a command (None: not in a server).
    pub fn of(guild: Option<serenity::GuildId>, user: &serenity::User, member: Option<&serenity::Member>) -> Option<Who> {
        let guild = guild?.get();
        Some(Who {
            guild,
            user: user.id.get(),
            roles: member.map_or_else(Vec::new, |m| m.roles.iter().map(|r| r.get()).collect()),
            manage_guild: member.and_then(|m| m.permissions).is_some_and(|p| p.manage_guild()),
        })
    }

    fn listed(&self, ids: &[String]) -> bool {
        // (The server's own ID is its @everyone role.)
        ids.iter().filter_map(|x| id_of(x)).any(|id| id == self.user || id == self.guild || self.roles.contains(&id))
    }
}

/// A role's or person's ID, as typed: bare, or a mention pasted in (`<@&123>`, `<@123>`).
pub fn id_of(x: &str) -> Option<u64> {
    let x = x.trim();
    let x = x.strip_prefix("<@&").or_else(|| x.strip_prefix("<@!")).or_else(|| x.strip_prefix("<@")).and_then(|r| r.strip_suffix('>')).unwrap_or(x);
    x.parse().ok().filter(|&n| n > 0)
}

impl Access {
    pub fn admin(&self, w: &Who) -> bool {
        w.manage_guild || w.listed(&self.admins)
    }

    pub fn user(&self, w: &Who) -> bool {
        self.admin(w) || self.users.iter().all(|x| x.trim().is_empty()) || w.listed(&self.users)
    }

    /// Whether `w` may do what needs `tier`; if not, why, to tell them.
    pub fn may(&self, tier: Tier, w: &Who) -> Result<(), &'static str> {
        let ok = match tier {
            Tier::User => self.user(w),
            Tier::Admin => self.admin(w),
        };
        if ok {
            Ok(())
        } else {
            Err(tier.words())
        }
    }

    /// The server names admins of its own (so its admin commands are shown to everyone).
    pub fn names_admins(&self) -> bool {
        self.admins.iter().any(|x| !x.trim().is_empty())
    }
}

/// The commands as a server is to have them: as built, but with admins of its own, the admin
/// commands shown to everyone (the check here refuses them to whoever isn't one).
pub fn commands_for(built: &[serenity::CreateCommand], access: &Access) -> Value {
    let mut v = serde_json::to_value(built).unwrap_or(Value::Array(Vec::new()));
    if access.names_admins() {
        for c in v.as_array_mut().into_iter().flatten() {
            if c["name"].as_str().is_some_and(|n| command_tier(n) == Tier::Admin) {
                c["default_member_permissions"] = Value::Null;
            }
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn access(admins: &[&str], users: &[&str]) -> Access {
        Access { admins: admins.iter().map(|s| s.to_string()).collect(), users: users.iter().map(|s| s.to_string()).collect(), ..Default::default() }
    }

    #[test]
    fn admins_and_users_are_who_the_server_says() {
        let member = Who { guild: 1, user: 7, roles: vec![50, 60], manage_guild: false };
        let owner = Who { guild: 1, user: 8, roles: Vec::new(), manage_guild: true };
        let nobody = Who { guild: 1, user: 9, roles: Vec::new(), manage_guild: false };
        // By default: Manage Server sets it up, everyone uses it.
        let a = access(&[], &[]);
        assert!(a.admin(&owner) && !a.admin(&member) && a.user(&nobody));
        // Admins by role or by person; Manage Server still is one.
        let a = access(&["60"], &["50"]);
        assert!(a.admin(&member) && a.admin(&owner) && !a.admin(&nobody));
        assert!(a.user(&member) && !a.user(&nobody));
        assert_eq!(a.may(Tier::User, &nobody), Err(Tier::User.words()));
        assert!(access(&["9"], &["50"]).admin(&nobody), "a person, by their ID");
        assert!(access(&[], &["1"]).user(&nobody), "the server's ID is @everyone");
        assert_eq!((id_of("<@&60>"), id_of(" <@9> "), id_of("60"), id_of("@mods")), (Some(60), Some(9), Some(60), None));
    }

    #[test]
    fn a_server_with_admins_of_its_own_sees_the_admin_commands() {
        let built = vec![
            serenity::CreateCommand::new("setup").description("x").default_member_permissions(serenity::Permissions::MANAGE_GUILD),
            serenity::CreateCommand::new("active").description("x"),
        ];
        let plain = commands_for(&built, &access(&[], &[]));
        assert_eq!(plain[0]["default_member_permissions"], "32");
        let own = commands_for(&built, &access(&["60"], &[]));
        assert!(own[0]["default_member_permissions"].is_null() && own[1]["name"] == "active");
        assert_eq!((command_tier("Mute here"), command_tier("recent")), (Tier::Admin, Tier::User));
        assert_eq!((component_tier("trp-setup:voice"), component_tier("trp-live:all")), (Tier::Admin, Tier::User));
    }
}
