# Discord

A [Trunk Recorder Pro](https://github.com/TrunkRecorder/trunk-recorder-pro) plugin: a Discord bot
that posts what TRP hears to channels, and takes slash commands. It needs Trunk Recorder Pro 0.5.0
or later.

It posts **feeds**. Each one sends one kind of thing to one channel:

- **Recorded calls** as each one finishes, with its recording attached and linked
- **Calls as they start**, recorded or not, as a live log
- **Emergencies**, each one once, as soon as it's flagged
- **Decode rates** for the control channels, in one message that's kept up to date (with the braille
  graph from the old Trunk Recorder bot)

- **Signals** and **Tones**: a thread opens for a talkgroup (`clmrn-I-1001`) when a tone is
  heard on it. The same rules as the notify plugin's: a **Signals** feed takes named tones (the
  signals it picks, or any signal), a **Tones** feed takes any tone, named or not, of the
  **Kinds of tone** it picks (two-tone, DTMF, …) or matching the **Tones** typed into it. It starts from a header in the channel saying what was heard, and every
  recorded call on that talkgroup posts in it. **Stop posting** on the header stops the posts and
  **Resume posting** brings them back. So an unwatched thread doesn't post forever, it stops by
  itself **Tone threads: stop after, min** (30 by default; 0 turns this off) after its tone was
  last heard or it was last resumed. The tone heard again within that time (more callouts for
  the same incident) posts in the same thread and gives it the full time again, even if someone
  pressed Stop; heard once its time is up (and nobody kept it posting), it's a new incident: the
  old thread (or forum post) is closed and a new one opened. The header says when it stops.
  **⏱ Keep posting** takes it off that timer for as long as an incident runs, and
  **⏱ Let it time out** puts it back on, counting from then.
  **Delete thread** (needs Manage Threads) deletes the thread and marks the header as ended.
  `/threads` lists the threads that are open.

A feed can be narrowed to certain systems and talkgroups (`101`, `1000-1099`, `507*`, plus ones
to leave out). It can skip short calls, and it can put a mention above each post.

Feeds can be set on the plugin's own page in TRP, or from Discord with `/setup` and
`/feed`. They're one list, in the plugin's settings: whatever is changed in Discord shows on the
page at once, and the page's changes apply in Discord at once.

## Settings

- **Bot token**, **Link recordings from** and **Live audio: phrases** apply to the whole bot.
  **Invite link** answers with the link that adds the bot to a server, with every permission it
  needs.
- **Servers**: one entry per Discord server, each with its ID, its **Live audio** (voice channel,
  volume, chime) and its **Feeds**. Inviting the bot adds its server here. Servers and channels are
  entered by ID only: the plugin fills in their names (and each feed's, after its channel and
  what it posts) from Discord, and follows renames. The plugin's status names each server, its feeds' channels and
  its live audio channel, and says when the bot isn't in a server or can't see a channel. A feed
  name is optional; without one, a feed is called after what it posts and where. The slash commands are registered in each listed server. With
  none listed, they're registered globally. **Check access** on a server says whether the bot is
  in it, whether it can see every channel its feeds and live audio use, and which permissions
  it's missing.
- Settings from 0.1 (a single server, with feeds at the top level) are read as one server. Saving
  the page writes them in the new shape.

## Live audio

Under **Servers** on the plugin's page, set the server's **Live audio → Voice channel ID**. The
bot then plays followed talkgroups
into that voice channel as they're heard, about a second behind the air. It joins while
something is followed and leaves when nothing is. One call plays at a time: emergencies first,
then talkgroups followed from tone threads, then the rest.

- On a tone thread's header, **🔊 Live audio** follows that talkgroup until it's pressed again or
  the thread stops posting (its time up, Stop posting, or deleted).
- A tones feed with **Live audio at once** on (or the **🔊 Tone threads play live** button
  in `/setup`) follows each thread it opens straight away, without anyone pressing anything. It
  keeps to the thread's timer too.
- `/live follow`, `/live follow-signal` (the talkgroup of the last signal heard), `/live stop`,
  `/live stop-all` and `/live status` work too.
- The voice channel's chat holds a control panel, kept up to date, with a Stop button for each
  follow and one to stop them all. While audio is playing, the bot looks every 3 minutes whether
  anything has been posted under the panel, and if so posts it again at the bottom (the old one
  deleted), so its buttons don't scroll out of reach. A follow from a tone thread says until when it plays, and
  **⏱ +30 min** gives it (and its thread) 30 minutes more.
- The channel's status line, under its name in the channel list, says what's playing.
- **Live audio → Log who's talking** (on unless switched off) puts a line in the voice channel's
  chat for each radio heard on a call that plays: `clmrn-I-1001-1234567 (Engine 1) · Fire Disp`.
  The radio TRP knows at the call's start is posted as the call starts playing, and each radio
  that keys up after it as TRP hears it (TRP's `call.update`; a TRP without it, as the call ends). Lines are only ever added, never edited, since the chat is everyone's.
- **Live audio: phrases** lets people type plain words in that chat: "follow last signal from
  clmrn-I", "follow tg 1001", "stop 1001", "stop live audio", "status". It needs the Message
  Content intent switched on in the Developer Portal first.
- **Live audio → Chime** plays a soundboard sound as a tone thread's audio starts.
- Each call plays at one level, as TRP's recordings do (a quiet talker brought up, a loud one
  down), with peaks rounded off rather than clipped, and the whole of the radio's voice band
  (the bot upsamples to 48 kHz itself: songbird's own resampler muffled it).
  **Live audio → Volume adjust** moves it from there in 25% steps, from −300% to +300% (+100% is
  twice as loud), and takes effect at once. `/setup` has the same as a dropdown.

Voice in Discord is end-to-end encrypted (DAVE). songbird 0.6 handles that.

## Boards

`/board add` puts a message in the channel that's edited in place every minute instead of being
posted again, so a channel can be a dashboard that doesn't scroll. There are four kinds:
systems (decode rates and graphs, control channels, calls and recorders), calls going on now,
open tone threads, and today so far. Boards survive restarts, and one deleted by hand is dropped.

## More

- **Tone threads in a forum.** Give a Signals or Tones feed a forum's channel ID (nothing else
  to set: the bot sees it's a forum) and each incident
  becomes a post, tagged with its system, signal and Active, Stopped or Closed. Close archives it
  and keeps it, Delete removes it.
- **Right-click a post → Apps:** Follow live, Open a thread here, Mute here (`/mutes` and
  `/unmute` to manage them) and Recent calls.
- **Lookups:** `/radio <id>` for a radio's calls, and `/tg <number>` for a talkgroup's card with
  its latest calls and today's count. `/thread` opens a thread for a talkgroup by hand.
- **`/feed edit`** opens a form for a feed's systems, talkgroups, signals and mention.
- **Daily digest:** a feed of that kind gets the day's figures at midnight.
- **Personal alerts:** `/notify me` sends you a DM for a talkgroup's calls, a signal or
  emergencies. `/notify list` and `/notify remove` manage them.

## Permissions

The invite link the bot logs asks for all of these:

- **Every feed:** View Channel, Send Messages, Embed Links, Attach Files, Read Message History.
- **Threads:** Create Public Threads, Send Messages in Threads, Manage Threads (Close and Delete).
- **Forums:** Manage Channels (to add the forum's tags).
- **Live audio:** Connect, Speak, Use Soundboard, Set Voice Channel Status.

## Who may use it

Each server has two lists, under **Who may use the bot** on its row on TRP's page. Each one
takes roles or people, by ID: right-click one and pick Copy ID, or paste a mention.

- **Admins** set the bot up: `/setup`, `/feed`, `/board`, mutes, opening threads. Anyone with
  Manage Server always counts as one, so a server can't lock itself out. Empty: only them.
- **Users** use it: every other command, live audio, and the buttons on its posts. Empty:
  everyone in the server. When it names roles, only those roles and the admins may.

Closing or deleting a tone thread takes an admin, or Manage Threads.

The same check covers slash commands, right-click commands, buttons, menus and the words typed in
live audio's chat. When a server names admins of its own, its admin commands are shown to
everyone in Discord's command list, and the bot refuses them to anyone who isn't an admin.
Discord's own per-command settings (Server Settings → Integrations) still apply on top of these.

## Commands

| Command | Who | What |
| --- | --- | --- |
| `/info` | users | uptime, calls, posts, systems, active calls |
| `/rates [system]` | users | control channel decode rates |
| `/active [system]` | users | calls going on now |
| `/recent [talkgroup] [system] [radio] [count]` | users | the latest recorded calls, with links |
| `/systems` | users | TRP's systems |
| `/threads` | users | the tone threads open now |
| `/feed add what …` | admins | post something to this channel (or another) |
| `/feed list [all]` | admins | this channel's feeds, or every channel's |
| `/feed remove name`, `/feed on name`, `/feed off name` | admins | |


## Setting up

**In TRP (once):**
1. Install the plugin.
2. At [discord.com/developers/applications](https://discord.com/developers/applications), make a
   New Application. On its **Bot** page, press **Reset Token** and copy the token.
3. Paste the token into the plugin's page and save. Press **Invite link** and open the link it
   gives you.
4. Pick your server (you need Manage Server there). The bot joins, registers its commands there
   straight away, and posts a short welcome.

**In the server:** someone with Manage Server runs `/setup`. That opens a panel only they see:
- a dropdown of the server's voice channels, for live audio
- a dropdown of what the channel `/setup` was run in gets: recorded calls, calls as they start,
  emergencies, tone threads, decode rates, the daily digest
- **Live audio off** and **Check access** buttons, and, with a tones feed in the channel,
  **🔊 Tone threads play live**

Changes are saved to the plugin's settings as they're made, so TRP's page shows them.
Run `/setup` in each channel you want to post to. `/feed edit` narrows a feed to
some systems or talkgroups. Volume and chime are options on `/setup`.

**Back in TRP:** the page's **Servers** list has each server the bot is in, with what
`/setup` and `/feed` set, by name. Change it there or in Discord: either end shows the other's
changes. A server can also be added there by its ID and set up on the page alone. The plugin's
status says what's wrong, such as a channel the bot can't see.

