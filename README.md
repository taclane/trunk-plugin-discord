# Discord

A [Trunk Recorder Pro](https://github.com/TrunkRecorder/trunk-recorder-pro) plugin: a Discord bot
that posts what TRP hears to your server's channels, plays talkgroups live in a voice channel, and
answers slash commands. It needs Trunk Recorder Pro 0.5.0 or later.

- **Feeds** post one kind of thing to one channel: recorded calls with their audio, calls as they
  start, emergencies, decode rates, a daily digest, TRP's warnings, control channels lost and
  found.
- **Tone threads** open a thread (or forum post) for each incident when a page or other tone is
  heard on a talkgroup, and post that talkgroup's calls in it until the incident's over.
- **Live audio** plays followed talkgroups in a voice channel, about a second behind the air, and
  logs who's talking.
- **Commands** for anyone you allow: what's on now, recent calls, a radio's or talkgroup's
  history, the sites' health. Admins set the bot up from Discord itself.

Everything is set up either on the plugin's page in TRP or from Discord with `/setup`: it's one
set of settings, and a change at either end shows at the other at once.

## Setting up

**In TRP (once):**

1. Install the plugin from the plugin store, and open its page.
2. At [discord.com/developers/applications](https://discord.com/developers/applications), press
   **New Application** and name it. On its **Bot** page, press **Reset Token** and copy the
   token. (Leave the privileged intents off, unless you want [phrases](#live-audio).)
3. Paste the token into **Bot token** and save. Once the bot has logged in, **Invite to a
   server** fills in: open it.
4. Pick your server (you need Manage Server there). The bot joins, adds the server to the
   plugin's **Servers**, and posts a welcome with a **Set up** button.

**In Discord:** someone with Manage Server presses **Set up** on the welcome, or runs `/setup` in
a channel. It opens a panel only they see, in three pages:

- **This channel:** what the channel gets. Each pick is a feed; unpick one and it goes.
  **Send a test post** checks it can post there, and **Check access** looks over every channel
  the bot's set up for.
- **Live audio:** the voice channel to play in, its volume, and whether this channel's tone
  threads start playing by themselves.
- **Access:** the roles or people who are the bot's admins, and those who may use it.

Run `/setup` in each channel you want to post to. `/feed edit` narrows a feed to some systems or
talkgroups, and `/help` lists every command.

**Back in TRP:** **Servers** shows each server with what `/setup` and `/feed` set, with names
filled in from Discord. A server can also be added there by its ID and set up on the page alone.
The plugin's status says what's wrong, such as a server the bot isn't in or a channel it can't
see.

## Feeds

| Kind | Posts |
| --- | --- |
| Recorded calls | each call as it's recorded, with its audio attached and a link to it in TRP |
| Calls as they start | each call as it starts, recorded or not: a live log |
| Emergencies | each emergency, once, as it's flagged |
| Signals | a tone thread for each talkgroup a named tone (a signal) is heard on |
| Tones | a tone thread for each talkgroup any tone is heard on |
| Decode rates | the control channels' decode rates, in one message kept up to date, with a graph |
| Daily digest | the day's figures at midnight: calls, airtime, the busiest talkgroups and hours |
| Control channels lost and found | a post when a site's control channel is lost for 30 s, and when it's back |
| TRP's warnings and errors | TRP's log lines as they're logged, at the level picked (info, warnings, errors) |

A feed can be narrowed to some systems and talkgroups (`101`, `1000-1099`, `507*`, and ones to
leave out), can skip short calls, and can put a mention above each post (`@here`, a role).
Two feeds posting the same call to one channel post it once.

## Tone threads

When a Signals or Tones feed hears its tone on a talkgroup, a thread opens in its channel named
for the talkgroup (`clmrn-I-1001`), from a header saying what was heard. Every recorded call on
that talkgroup then posts in the thread.

- **What a feed takes** follows the notification plugin's rules. A Signals feed takes the signals
  it picks (or any signal). A Tones feed takes any tone, named or not, of the **Kinds of tone** it
  picks (two-tone, DTMF, KNOX…) or matching the **Tones** typed into it.
- **It stops by itself.** A thread posts for **Tone threads: stop after, min** (30 by default; 0
  turns this off) after its tone was last heard. The same tone again within that time (more
  callouts for the same incident) posts in the same thread and restarts its time. Heard after
  its time is up, it's a new incident: the old thread is closed and a new one opened. The header
  says when it stops.
- **Buttons on the header:** **Stop posting** and **Resume posting**; **⏱ Keep posting** takes it
  off the timer for as long as an incident runs, and **⏱ Let it time out** puts it back;
  **🔊 Live audio** plays the talkgroup live; **Delete** removes it.
- **In a forum:** give the feed a forum's channel ID and each incident becomes a post, tagged
  with its system, its signal and Active, Stopped or Closed. **Close** archives it.
- `/threads` lists the threads open now, and `/thread` opens one for a talkgroup by hand.

## Live audio

Pick a voice channel on `/setup`'s **Live audio** page (or set **Live audio → Voice channel ID**
on TRP's page). The bot joins while something is followed and leaves when nothing is. One call
plays at a time: emergencies first, then talkgroups followed from tone threads, then the rest.

- **Follow a talkgroup:** **🔊 Live audio** on a tone thread, right-click a post → **Apps →
  Follow live**, or `/live follow`. `/live follow-signal` follows the talkgroup of the last
  signal heard; `/live stop`, `/live stop-all` and `/live status` do what they say.
- **Tone threads can start playing by themselves:** a Signals or Tones feed with **Live audio at
  once** on follows each thread it opens. It keeps to the thread's timer.
- **The voice channel's chat** holds a control panel with a Stop button for each follow. While
  audio plays, the panel is posted again at the bottom whenever other messages have pushed it up.
  A follow from a tone thread says until when it plays, and **⏱ +30 min** gives it more time.
- **Who's talking:** with **Log who's talking** on (the default), each radio heard on a call that
  plays gets a line in that chat as it keys up: `clmrn-I-1001-1234567 (Engine 1) · Fire Disp`.
- **Level:** each call plays at one level, as TRP's recordings do. **Volume adjust** moves it in
  25% steps, from −300% to +300%, and takes effect at once.
- **Chime:** a soundboard sound played as a tone thread's audio starts.
- **Phrases** (optional): people type plain words in the voice channel's chat ("follow tg 1001",
  "follow last signal from clmrn-I", "stop live audio", "status"). It needs the **Message
  Content** intent switched on in the Developer Portal first, or the bot can't log in.

The channel's status line, under its name in the channel list, says what's playing.

## Who may use it

Each server has two lists, set on `/setup`'s **Access** page or under **Who may use the bot** on
its row on TRP's page (by ID there: right-click a role or person, Copy ID).

- **Admins** set the bot up: `/setup`, `/feed`, `/board`, mutes, `/check`, `/log`, `/test`,
  opening threads. Anyone with Manage Server always counts as one, so a server can't lock itself
  out. None picked: only them.
- **Users** use everything else: the other commands, live audio, the buttons on its posts and
  the phrases. None picked: everyone in the server. Pick roles, and only those roles (and the
  admins) may.

Closing or deleting a tone thread takes an admin, or Manage Threads.

The same check covers slash commands, right-click commands, buttons, menus and live audio's
phrases. When a server picks admins of its own, its admin commands are shown to everyone in
Discord's command list, and the bot refuses them to anyone who isn't one. Discord's own
per-command settings (Server Settings → Integrations) still apply on top.

## Commands

All answers but posts are seen only by whoever asked.

| Command | Who | What |
| --- | --- | --- |
| `/help` | users | the commands you may use here |
| `/info` | users | uptime, calls, posts, systems, calls on the air now |
| `/active [system]` | users | calls going on now |
| `/recent [talkgroup] [system] [radio] [count]` | users | the latest recorded calls, with links |
| `/radio id`, `/tg number` | users | a radio's latest calls; a talkgroup's card, its latest calls and today's count |
| `/rates [system]`, `/systems`, `/sites [system]` | users | decode rates; TRP's systems; each site's identity and control channel |
| `/health` | users | every site's control channel, TRP's recent warnings, and how the bot's doing |
| `/ping` | users | the bot's and Discord's latency, and when TRP was last heard from |
| `/today` | users | today's figures so far |
| `/about` | users | the plugin's and TRP's versions, and the invite link |
| `/threads` | users | the tone threads open now |
| `/live …` | users | follow and stop talkgroups in the voice channel |
| `/notify me`, `/notify list`, `/notify remove` | users | direct messages for a talkgroup's calls, a signal, or emergencies |
| **Apps → Follow live**, **Recent calls** | users | on a post (right-click it) |
| `/setup [volume] [chime]` | admins | the setup panel |
| `/feed add`, `list`, `edit`, `remove`, `on`, `off` | admins | this channel's feeds, one by one |
| `/board add`, `list`, `remove` | admins | a message kept up to date in place (below) |
| `/thread`, **Apps → Open a thread here** | admins | a tone thread for a talkgroup, by hand |
| **Apps → Mute here**, `/mutes`, `/unmute` | admins | keep a talkgroup out of a channel |
| `/check` | admins | can the bot see and post to every channel it's set up for here, and what's missing |
| `/log [level] [count]` | admins | TRP's latest warnings and errors |
| `/test` | admins | a test post to each of this channel's feeds (mentioning no one) |

**Boards** (`/board add`) are messages edited in place every minute, so a channel can be a
dashboard that doesn't scroll: the systems (decode rates and graphs, control channels, calls and
recorders), calls going on now, open tone threads, or today so far.

## Permissions

The invite link asks for all of these:

- **Every feed:** View Channel, Send Messages, Embed Links, Attach Files, Read Message History.
- **Threads:** Create Public Threads, Send Messages in Threads, Manage Threads (Close and Delete).
- **Forums:** Manage Channels (to add the forum's tags).
- **Live audio:** Connect, Speak, Use Soundboard, Set Voice Channel Status.

## What gets posted

The bot posts what TRP records: talkgroup and radio names as TRP knows them (including talker
aliases heard on the air), call audio, and links to TRP's own pages when **Link recordings from**
is set. Anyone who can read a channel can see what's posted in it, so pick the channels, and what
they get, with that in mind.

## Settings

- **Bot token**, **Bot status** (the line under its name), **Link recordings from** (TRP's address
  as your server's members reach it) and **Live audio: phrases** are for the whole bot.
- **Tone threads: stop after, min** is how long a tone thread posts after its tone.
- **Servers**: one entry per Discord server, with its **Who may use the bot**, **Live audio** and
  **Feeds**. Servers and channels are entered by ID; the plugin fills in their names from
  Discord and follows renames. **Check access** on a server says what the bot can't see or do
  there.

The slash commands are registered in each server listed (with none listed, everywhere).

## Building

```sh
cargo build --release
```

`cargo test` runs the tests. Releases are built for Linux (x86-64, ARM64), macOS and Windows by
the release workflow when a `v<version>` tag is pushed.

## License

GPL-3.0-or-later.
