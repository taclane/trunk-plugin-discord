//! Live audio: one standing voice channel the bot plays talkgroups into as they're heard.
//!
//! What it plays is the **follows**: a tone thread's 🔊 Live audio button, `/live follow`, or (with
//! phrases on) words in the voice channel's chat ("follow last signal from clmrn-I", "stop live
//! audio"). One call plays at a time: an emergency first, then a tone thread's talkgroup, then the
//! rest; overlapping calls wait a little, and stale ones are dropped. The bot joins when there's
//! something followed and leaves when there isn't; the channel's status (under its name in the
//! channel list) says what's playing, and a control panel in its chat lists the follows, each
//! with a button to stop it. A tone thread's follow lasts as long as the thread posts: its panel
//! line says until when, and **⏱ +N min** gives it more. With `logSpeakers`, the chat gets a line
//! for each radio heard on a call that plays (`clmrn-I-1001-1234567 (Engine 1)`), as TRP hears it
//! (`call.update`, a new radio talking; `call.end` too, for a TRP without it):
//! posted, never edited, since the chat is everyone's. Kept in the data folder (`live.json`).
//!
//! TRP's `audio` topic gives each call's PCM as it's decoded (8 kHz, raw from the vocoder). Each
//! call is brought to one speech level as it plays (TRP's rule for its recordings), and a
//! [`LiveSource`] upsamples the mix to 48 kHz itself and hands it to songbird (which encodes and
//! encrypts it: DAVE) at the pace songbird reads it. (songbird's own resampler, at 8 kHz in, cuts
//! 2 kHz by 14 dB and 3 kHz by 46: speech through a cloth.)

use std::collections::{HashMap, VecDeque};
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use poise::serenity_prelude as serenity;
use serde::{Deserialize, Serialize};
use serenity::{ButtonStyle, ChannelId, CreateActionRow, CreateButton, CreateEmbed, CreateMessage, EditMessage, MessageId};

use crate::post::{GRAY, GREEN, RED};
use crate::state::Shared;
use trunk_recorder_plugin::Host;

const FILE: &str = "live.json";
/// The rate the mix runs at (TRP's voice audio).
pub const RATE: u32 = 8000;
/// A call starts playing with this much in hand (smooths out bursts).
const LEAD: usize = (RATE / 4) as usize;
/// A call nothing has come for in this long is let go.
const STALE: Duration = Duration::from_secs(4);
/// A call waiting this long behind another is dropped.
const TOO_LATE: Duration = Duration::from_secs(20);
/// Above this, peaks are rounded off toward full scale rather than clipped.
const KNEE: f32 = 0.7;
/// How often, while it plays, the panel is looked at and posted again if it's no longer last.
const PANEL_EVERY: Duration = Duration::from_secs(180);
/// What songbird is handed: its own rate, so it doesn't resample.
pub const OUT_RATE: u32 = 48_000;

// A call's speech level, as TRP's recordings have theirs set (trunk-core `loudness`): 20 ms
// frames; the mean power of those above −50 dBFS and within 10 dB of their mean; gain toward
// −12 dBFS, −18 to +24 dB.
const FRAME: usize = 160;
const TARGET_DB: f64 = -12.0;
const MAX_GAIN_DB: f64 = 24.0;
const MIN_GAIN_DB: f64 = -18.0;
const ABS_GATE_DB: f64 = -50.0;
const REL_GATE_DB: f64 = 10.0;
const MIN_FRAMES: usize = 10;

fn db(p: f64) -> f64 {
    10.0 * p.max(1e-20).log10()
}

/// The speech level of frames of mean power `p`, dBFS (none: too little speech to judge).
fn speech_level_db(p: &[f64]) -> Option<f64> {
    let loud: Vec<f64> = p.iter().copied().filter(|&p| db(p) > ABS_GATE_DB).collect();
    if loud.len() < MIN_FRAMES {
        return None;
    }
    let gate = db(loud.iter().sum::<f64>() / loud.len() as f64) - REL_GATE_DB;
    let kept: Vec<f64> = loud.into_iter().filter(|&p| db(p) > gate).collect();
    (kept.len() >= MIN_FRAMES).then(|| db(kept.iter().sum::<f64>() / kept.len() as f64))
}

/// 8 kHz to 48 kHz: a polyphase windowed-sinc low-pass (Kaiser), flat to 3.6 kHz, the images
/// above 4 kHz down 60 dB or more.
pub struct Upsample {
    /// Each output phase's taps.
    phases: Vec<Vec<f32>>,
    /// The last inputs, the newest last.
    hist: VecDeque<f32>,
}

impl Upsample {
    const L: usize = (OUT_RATE / RATE) as usize;
    const TAPS: usize = 48;

    pub fn new() -> Upsample {
        let n = Self::L * Self::TAPS;
        let fc = 3800.0 / OUT_RATE as f64;
        let beta = 7.0;
        let i0 = |x: f64| {
            let (mut s, mut t) = (1.0, 1.0);
            for k in 1..30 {
                t *= (x / 2.0) / k as f64;
                s += t * t;
            }
            s
        };
        let c = (n - 1) as f64 / 2.0;
        let h: Vec<f64> = (0..n)
            .map(|i| {
                let x = i as f64 - c;
                let sinc = if x == 0.0 { 2.0 * fc } else { (2.0 * std::f64::consts::PI * fc * x).sin() / (std::f64::consts::PI * x) };
                let w = i0(beta * (1.0 - (x / c).powi(2)).max(0.0).sqrt()) / i0(beta);
                // (×L: each output phase keeps the input's level.)
                sinc * w * Self::L as f64
            })
            .collect();
        let phases = (0..Self::L).map(|p| (0..Self::TAPS).map(|k| h[k * Self::L + p] as f32).collect()).collect();
        Upsample { phases, hist: std::iter::repeat_n(0.0, Self::TAPS).collect() }
    }

    /// Six out for each in.
    pub fn run(&mut self, input: &[f32], out: &mut Vec<f32>) {
        for &x in input {
            self.hist.pop_front();
            self.hist.push_back(x);
            for taps in &self.phases {
                // (Tap k meets the input k back.)
                out.push(taps.iter().zip(self.hist.iter().rev()).map(|(h, x)| h * x).sum());
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Why {
    /// A tone thread's button.
    Thread { thread: u64 },
    /// Someone asked for it.
    User { name: String },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Follow {
    /// The server whose voice channel it plays in.
    #[serde(default)]
    pub server: u64,
    pub system: String,
    pub talkgroup: u32,
    #[serde(default)]
    pub tag: String,
    pub why: Why,
}

impl Follow {
    pub fn label(&self) -> String {
        if self.tag.is_empty() {
            format!("{} TG {}", self.system, self.talkgroup)
        } else {
            format!("{} ({})", self.tag, self.talkgroup)
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
struct Saved {
    follows: Vec<Follow>,
    /// Each server's control panel: its channel and message.
    #[serde(default)]
    panels: std::collections::BTreeMap<u64, (u64, u64)>,
}

/// What's playing now.
#[derive(Clone, Debug, PartialEq)]
pub struct Playing {
    pub system: String,
    pub talkgroup: u32,
    pub tag: String,
    pub emergency: bool,
}

pub struct Live {
    saved: Mutex<Saved>,
    path: PathBuf,
    /// Each server's mix.
    mixers: Mutex<HashMap<u64, Arc<Mixer>>>,
    /// The last signal heard on each system: its talkgroup and tag, and the signal's name.
    last_signal: Mutex<HashMap<String, (u32, String, String)>>,
    /// Something changed: the voice loop looks again.
    pub changed: tokio::sync::Notify,
    /// Servers a chime is due in (a tone thread's follow started).
    chimes: Mutex<std::collections::HashSet<u64>>,
    /// Followed calls' radios (ID, name), as last told, and when.
    speakers: Mutex<HashMap<u32, (Radios, Instant)>>,
}

/// A call's radios: each one's ID and name ("" none known).
type Radios = Vec<(u32, String)>;

/// A call's radios as a chat line for each: `🎙️ **clmrn-I-1001-1234567** (Engine 1) · Fire Disp`.
fn speaker_lines(p: &Playing, who: &[(u32, String)]) -> String {
    let tg = if p.tag.is_empty() { String::new() } else { format!(" · {}", p.tag) };
    let sos = if p.emergency { " 🚨" } else { "" };
    if who.is_empty() {
        return format!("🎙️ **{}-{}** (radio not known){tg}{sos}", p.system, p.talkgroup);
    }
    who.iter()
        .map(|(u, name)| {
            let alias = if name.is_empty() { String::new() } else { format!(" ({name})") };
            format!("🎙️ **{}-{}-{u}**{alias}{tg}{sos}", p.system, p.talkgroup)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

impl Live {
    pub fn load(dir: &std::path::Path) -> Live {
        let path = dir.join(FILE);
        let saved = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        Live {
            saved: Mutex::new(saved),
            path,
            mixers: Mutex::default(),
            last_signal: Mutex::default(),
            changed: tokio::sync::Notify::new(),
            chimes: Mutex::default(),
            speakers: Mutex::default(),
        }
    }

    /// Call `call`'s radios, as TRP told them (its start, its end).
    pub fn set_speakers(&self, call: u32, who: Vec<(u32, String)>) {
        let mut s = self.speakers.lock().unwrap();
        s.retain(|_, (_, at)| at.elapsed() < Duration::from_secs(600));
        s.insert(call, (who, Instant::now()));
    }

    fn speakers(&self, call: u32) -> Vec<(u32, String)> {
        self.speakers.lock().unwrap().get(&call).map(|x| x.0.clone()).unwrap_or_default()
    }

    fn save(&self, s: &Saved) {
        if let Ok(j) = serde_json::to_vec_pretty(s) {
            let tmp = self.path.with_extension("json.new");
            let _ = std::fs::write(&tmp, j).and_then(|_| std::fs::rename(&tmp, &self.path));
        }
    }

    pub fn follows(&self) -> Vec<Follow> {
        self.saved.lock().unwrap().follows.clone()
    }

    pub fn follows_in(&self, server: u64) -> Vec<Follow> {
        self.saved.lock().unwrap().follows.iter().filter(|f| f.server == server).cloned().collect()
    }

    /// Server `server`'s mix.
    pub fn mixer(&self, server: u64) -> Arc<Mixer> {
        self.mixers.lock().unwrap().entry(server).or_default().clone()
    }

    /// The servers whose mix `system`'s `talkgroup` goes to, and how much it matters in each.
    pub fn targets(&self, system: &str, talkgroup: u32) -> Vec<(u64, u8)> {
        let s = self.saved.lock().unwrap();
        let mut t: Vec<(u64, u8)> = Vec::new();
        for f in s.follows.iter().filter(|f| f.system == system && f.talkgroup == talkgroup) {
            let p = if matches!(f.why, Why::Thread { .. }) { 2 } else { 1 };
            match t.iter_mut().find(|(g, _)| *g == f.server) {
                Some(x) => x.1 = x.1.max(p),
                None => t.push((f.server, p)),
            }
        }
        t
    }

    /// A chime is due in `server` (and now isn't).
    fn take_chime(&self, server: u64) -> bool {
        self.chimes.lock().unwrap().remove(&server)
    }

    pub fn follows_thread(&self, thread: u64) -> bool {
        self.saved.lock().unwrap().follows.iter().any(|f| f.why == Why::Thread { thread })
    }

    /// Follow it (false: it was already, for that reason).
    pub fn follow(&self, f: Follow) -> bool {
        let mut s = self.saved.lock().unwrap();
        if s.follows.contains(&f) {
            return false;
        }
        if matches!(f.why, Why::Thread { .. }) {
            self.chimes.lock().unwrap().insert(f.server);
        }
        s.follows.push(f);
        self.save(&s);
        drop(s);
        self.changed.notify_one();
        true
    }

    /// Stop following what `which` picks; what was stopped.
    pub fn unfollow(&self, which: impl Fn(&Follow) -> bool) -> Vec<Follow> {
        let mut s = self.saved.lock().unwrap();
        let (gone, kept): (Vec<Follow>, Vec<Follow>) = s.follows.drain(..).partition(|f| which(f));
        s.follows = kept;
        self.save(&s);
        drop(s);
        if !gone.is_empty() {
            self.changed.notify_one();
        }
        gone
    }

    fn panel(&self, server: u64) -> Option<(u64, u64)> {
        self.saved.lock().unwrap().panels.get(&server).copied()
    }

    fn forget_panel(&self, server: u64) {
        let mut s = self.saved.lock().unwrap();
        s.panels.remove(&server);
        self.save(&s);
    }

    fn set_panel(&self, server: u64, p: (u64, u64)) {
        let mut s = self.saved.lock().unwrap();
        s.panels.insert(server, p);
        self.save(&s);
    }

    pub fn saw_signal(&self, system: &str, talkgroup: u32, tag: &str, name: &str) {
        self.last_signal.lock().unwrap().insert(system.to_string(), (talkgroup, tag.to_string(), name.to_string()));
    }

    /// The last signal heard: on `system`, or on any (its system, talkgroup, tag, name).
    pub fn last_signal(&self, system: Option<&str>) -> Option<(String, u32, String, String)> {
        let l = self.last_signal.lock().unwrap();
        match system {
            Some(s) => l.get(s).map(|(t, tag, n)| (s.to_string(), *t, tag.clone(), n.clone())),
            // (No times kept: any system's is as good as another's.)
            None => l.iter().next().map(|(s, (t, tag, n))| (s.clone(), *t, tag.clone(), n.clone())),
        }
    }
}

// ─── The mix ──────────────────────────────────────────────────────────────

struct Pending {
    playing: Playing,
    priority: u8,
    buf: VecDeque<f32>,
    first: Instant,
    last: Instant,
    ended: bool,
    /// Its 20 ms frames' mean powers so far, and the frame being filled (its sum and count).
    powers: Vec<f64>,
    part: (f64, usize),
    /// The gain bringing its speech to [`TARGET_DB`], as measured so far.
    level: f32,
}

impl Pending {
    /// Samples it was given: their frames' powers, and the gain again (every half second, as it
    /// knows more).
    fn measure(&mut self, from: usize) {
        let before = self.powers.len();
        for &v in self.buf.range(from..) {
            self.part.0 += (v as f64) * (v as f64);
            self.part.1 += 1;
            if self.part.1 == FRAME {
                self.powers.push(self.part.0 / FRAME as f64);
                self.part = (0.0, 0);
            }
        }
        let n = self.powers.len();
        if n != before && (n == MIN_FRAMES || n / 25 != before / 25) {
            if let Some(level) = speech_level_db(&self.powers) {
                self.level = 10f64.powf((TARGET_DB - level).clamp(MIN_GAIN_DB, MAX_GAIN_DB) / 20.0) as f32;
            }
        }
    }
}

#[derive(Default)]
struct Mix {
    calls: HashMap<u32, Pending>,
    current: Option<u32>,
    /// Calls that have started playing, not yet logged.
    started: Vec<(u32, Playing)>,
}

pub struct Mixer {
    mix: Mutex<Mix>,
    /// The gain, as `f32` bits.
    gain: std::sync::atomic::AtomicU32,
}

impl Default for Mixer {
    fn default() -> Self {
        let m = Mixer { mix: Mutex::default(), gain: std::sync::atomic::AtomicU32::new(0) };
        m.set_gain(0.0);
        m
    }
}

/// `x` with peaks past [`KNEE`] rounded off, so it never passes full scale.
fn limit(x: f32) -> f32 {
    let a = x.abs();
    if a <= KNEE {
        return x;
    }
    let room = 1.0 - KNEE;
    x.signum() * (KNEE + room * ((a - KNEE) / room).tanh())
}

impl Mixer {
    /// The server's gain, dB, on each call's level.
    pub fn set_gain(&self, db: f64) {
        let g = 10f64.powf(db.clamp(-30.0, 18.0) / 20.0) as f32;
        self.gain.store(g.to_bits(), std::sync::atomic::Ordering::Relaxed);
    }

    /// A call's audio, as it comes: at `rate`, resampled (linearly) to [`RATE`].
    pub fn push(&self, call: u32, playing: Playing, priority: u8, rate: u32, samples: &[i16]) {
        let mut m = self.mix.lock().unwrap();
        let now = Instant::now();
        let p = m.calls.entry(call).or_insert_with(|| Pending {
            playing: playing.clone(),
            priority,
            buf: VecDeque::new(),
            first: now,
            last: now,
            ended: false,
            powers: Vec::new(),
            part: (0.0, 0),
            level: 1.0,
        });
        let from = p.buf.len();
        p.last = now;
        p.priority = priority + if playing.emergency { 2 } else { 0 };
        p.playing = playing;
        if rate == RATE || rate == 0 {
            p.buf.extend(samples.iter().map(|&s| s as f32 / 32768.0));
        } else {
            let step = rate as f64 / RATE as f64;
            let mut t = 0.0;
            while (t as usize) + 1 < samples.len() {
                let i = t as usize;
                let f = (t - i as f64) as f32;
                p.buf.push_back((samples[i] as f32 * (1.0 - f) + samples[i + 1] as f32 * f) / 32768.0);
                t += step;
            }
        }
        p.measure(from);
        // (Never more than a minute behind.)
        while p.buf.len() > (RATE * 60) as usize {
            p.buf.pop_front();
        }
    }

    pub fn end(&self, call: u32) {
        if let Some(p) = self.mix.lock().unwrap().calls.get_mut(&call) {
            p.ended = true;
        }
    }

    /// Forget calls of talkgroups no longer followed.
    pub fn keep(&self, followed: impl Fn(&str, u32) -> bool) {
        let mut m = self.mix.lock().unwrap();
        m.calls.retain(|_, p| followed(&p.playing.system, p.playing.talkgroup));
        if m.current.is_some_and(|c| !m.calls.contains_key(&c)) {
            m.current = None;
        }
    }

    /// The calls that started playing since it was last asked.
    fn take_started(&self) -> Vec<(u32, Playing)> {
        std::mem::take(&mut self.mix.lock().unwrap().started)
    }

    pub fn playing(&self) -> Option<Playing> {
        let m = self.mix.lock().unwrap();
        m.current.and_then(|c| m.calls.get(&c)).map(|p| p.playing.clone())
    }

    /// The next `out.len()` samples (silence when there's nothing).
    pub fn fill(&self, out: &mut [f32]) {
        let mut m = self.mix.lock().unwrap();
        let mut at = 0;
        while at < out.len() {
            if m.current.is_none() {
                // Let go of what's stale, then the most important call ready, the oldest first.
                m.calls.retain(|_, p| !(p.buf.is_empty() && (p.ended || p.last.elapsed() > STALE)) && p.last.elapsed() < TOO_LATE);
                m.current = m
                    .calls
                    .iter()
                    .filter(|(_, p)| p.buf.len() >= LEAD || (p.ended && !p.buf.is_empty()))
                    .max_by(|a, b| a.1.priority.cmp(&b.1.priority).then(b.1.first.cmp(&a.1.first)))
                    .map(|(id, _)| *id);
                if let Some(c) = m.current {
                    let p = m.calls[&c].playing.clone();
                    // (Once a call, however often it comes back after a pause.)
                    if !m.started.iter().any(|(x, _)| *x == c) {
                        m.started.push((c, p));
                    }
                    m.started.truncate(50);
                }
            }
            let Some(c) = m.current else { break };
            let p = m.calls.get_mut(&c).expect("current is held");
            while at < out.len() {
                match p.buf.pop_front() {
                    Some(s) => {
                        out[at] = s * p.level;
                        at += 1;
                    }
                    None => break,
                }
            }
            if at < out.len() {
                // Out of audio: it's over, or more is on its way.
                if p.ended || p.last.elapsed() > STALE {
                    m.calls.remove(&c);
                    m.current = None;
                    continue;
                }
                break;
            }
        }
        out[at..].iter_mut().for_each(|s| *s = 0.0);
        let g = f32::from_bits(self.gain.load(std::sync::atomic::Ordering::Relaxed));
        out[..at].iter_mut().for_each(|s| *s = limit(*s * g));
    }
}

/// The mix as a stream for songbird: f32 little-endian mono at [`RATE`], never ending. It keeps
/// at most a little ahead of real time, so what's heard is what was just said.
pub struct LiveSource {
    mixer: Arc<Mixer>,
    up: Upsample,
    start: Instant,
    made: u64,
    spare: Vec<u8>,
    /// Bytes read so far (where a reader is, if it asks).
    pos: u64,
}

impl LiveSource {
    pub fn new(mixer: Arc<Mixer>) -> LiveSource {
        LiveSource { mixer, up: Upsample::new(), start: Instant::now(), made: 0, spare: Vec::new(), pos: 0 }
    }
}

impl Read for LiveSource {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.spare.is_empty() {
            // (No more than 0.2 s ahead.)
            let due = (self.start.elapsed().as_secs_f64() * RATE as f64) as u64 + (RATE / 5) as u64;
            if self.made >= due {
                std::thread::sleep(Duration::from_millis(20));
            }
            let mut s = vec![0f32; (RATE / 50) as usize];
            self.mixer.fill(&mut s);
            self.made += s.len() as u64;
            let mut hi = Vec::with_capacity(s.len() * Upsample::L);
            self.up.run(&s, &mut hi);
            self.spare = hi.iter().flat_map(|x| x.to_le_bytes()).collect();
        }
        let n = buf.len().min(self.spare.len());
        buf[..n].copy_from_slice(&self.spare[..n]);
        self.spare.drain(..n);
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for LiveSource {
    /// Only to where it is (readers ask that way).
    fn seek(&mut self, to: SeekFrom) -> std::io::Result<u64> {
        match to {
            SeekFrom::Current(0) => Ok(self.pos),
            SeekFrom::Start(p) if p == self.pos => Ok(self.pos),
            _ => Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "live audio can't seek")),
        }
    }
}

impl songbird::input::core::io::MediaSource for LiveSource {
    fn is_seekable(&self) -> bool {
        false
    }
    fn byte_len(&self) -> Option<u64> {
        None
    }
}

// ─── The voice connection, its status and its panel ──────────────────────

/// Says why the stream ended, when songbird ends it.
struct TrackError(Host);

#[serenity::async_trait]
impl songbird::EventHandler for TrackError {
    async fn act(&self, ctx: &songbird::EventContext<'_>) -> Option<songbird::Event> {
        if let songbird::EventContext::Track(tracks) = ctx {
            for (state, _) in tracks.iter() {
                self.0.warn(format!("live audio: the stream ended: {:?}", state.playing));
            }
        }
        None
    }
}

fn start_stream(call: &mut songbird::Call, shared: &Shared, mixer: Arc<Mixer>) -> songbird::tracks::TrackHandle {
    let src = songbird::input::RawAdapter::new(LiveSource::new(mixer), OUT_RATE, 1);
    let t = call.play_input(src.into());
    let _ = t.add_event(songbird::Event::Track(songbird::TrackEvent::Error), TrackError(shared.host.clone()));
    t
}

fn status_text(p: &Option<Playing>, follows: &[Follow]) -> String {
    match p {
        Some(p) => {
            format!("{} {} · {}", if p.emergency { "🚨" } else { "🔴" }, if p.tag.is_empty() { format!("TG {}", p.talkgroup) } else { p.tag.clone() }, p.system)
        }
        None if follows.is_empty() => String::new(),
        None => format!("🔊 Following {}", follows.iter().map(Follow::label).collect::<Vec<_>>().join(", ")),
    }
}

/// How long a follow lasts: a tone thread's while it posts.
#[derive(Clone, Debug, PartialEq)]
pub enum Lasts {
    /// Until it's stopped.
    Stopped,
    /// Until this (Unix seconds), or more with ⏱; the minutes ⏱ gives.
    Until(i64, u32),
    /// Its thread kept posting, by them.
    Kept(String),
}

/// How long each of `follows` lasts.
fn lasting(shared: &Shared, follows: &[Follow]) -> Vec<Lasts> {
    let after = shared.settings().thread_timeout_s();
    follows
        .iter()
        .map(|f| match (&f.why, after) {
            (Why::Thread { thread }, Some(after)) => match shared.threads.get(*thread) {
                Some(t) if t.kept.is_some() => Lasts::Kept(t.kept.unwrap_or_default()),
                Some(t) => t.ends(after).map_or(Lasts::Stopped, |e| Lasts::Until(e as i64, (after / 60.0).round() as u32)),
                None => Lasts::Stopped,
            },
            _ => Lasts::Stopped,
        })
        .collect()
}

fn panel(p: &Option<Playing>, follows: &[Follow], lasts: &[Lasts]) -> (CreateEmbed, Vec<CreateActionRow>) {
    let mut e = CreateEmbed::new().title("🔊 Live audio").color(if p.is_some() {
        RED
    } else if follows.is_empty() {
        GRAY
    } else {
        GREEN
    });
    let now = match p {
        Some(p) => format!(
            "**Playing:** {}{} on {}",
            if p.tag.is_empty() { format!("TG {}", p.talkgroup) } else { format!("{} ({})", p.tag, p.talkgroup) },
            if p.emergency { " 🚨" } else { "" },
            p.system
        ),
        None => "**Playing:** nothing right now".to_string(),
    };
    let list = if follows.is_empty() {
        "Nothing followed. Press **🔊 Live audio** on a tone thread, or `/live follow`.".to_string()
    } else {
        follows
            .iter()
            .zip(lasts)
            .map(|(f, l)| {
                let why = match &f.why {
                    Why::Thread { thread } => format!("from <#{thread}>"),
                    Why::User { name } => format!("by {name}"),
                };
                let till = match l {
                    Lasts::Until(t, _) => format!(" · until <t:{t}:t> (<t:{t}:R>)"),
                    Lasts::Kept(who) => format!(" · kept on by {who}"),
                    Lasts::Stopped => String::new(),
                };
                format!("• {} · {} · {why}{till}", f.label(), f.system)
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    e = e.description(format!("{now}\n\n**Following**\n{list}"));
    let mut buttons: Vec<CreateButton> = Vec::new();
    for (f, l) in follows.iter().zip(lasts) {
        buttons.push(
            CreateButton::new(format!("trp-live:stop:{}:{}", f.talkgroup, f.system))
                .label(format!("Stop {}", f.label()).chars().take(80).collect::<String>())
                .style(ButtonStyle::Secondary),
        );
        if let (Why::Thread { thread }, Lasts::Until(_, min)) = (&f.why, l) {
            buttons.push(
                CreateButton::new(format!("trp-live:more:{thread}"))
                    .label(format!("⏱ +{min} min {}", f.label()).chars().take(80).collect::<String>())
                    .style(ButtonStyle::Primary),
            );
        }
    }
    // (Five rows at most, one of them Stop all.)
    buttons.truncate(20);
    let mut rows: Vec<CreateActionRow> = Vec::new();
    while !buttons.is_empty() {
        let rest = buttons.split_off(buttons.len().min(5));
        rows.push(CreateActionRow::Buttons(std::mem::replace(&mut buttons, rest)));
    }
    if !follows.is_empty() {
        rows.push(CreateActionRow::Buttons(vec![CreateButton::new("trp-live:all").label("Stop all").style(ButtonStyle::Danger)]));
    }
    (e, rows)
}

/// What the status and panel last said: the status, what's playing, the follows and how long each lasts.
type Shown = (String, Option<Playing>, Vec<Follow>, Vec<Lasts>);

/// One server's voice connection, as the loop keeps it.
#[derive(Default)]
struct Conn {
    /// The voice channel it's in.
    joined: Option<u64>,
    track: Option<songbird::tracks::TrackHandle>,
    shown: Option<Shown>,
    retry_at: Option<Instant>,
    /// When it last looked whether the panel is still the chat's last message.
    panel_looked: Option<Instant>,
    /// Calls whose radios have been posted: the radios posted, when it started, and what it is.
    logged: HashMap<u32, (Radios, Instant, Playing)>,
}

/// Keep each server's voice connection as its follows want it, and its status and panel up to date.
pub async fn run(shared: Arc<Shared>, ctx: serenity::Context) {
    let Some(manager) = songbird::get(&ctx).await else {
        return shared.host.warn("no voice support in this build");
    };
    let mut conns: HashMap<u64, Conn> = HashMap::new();
    loop {
        let _ = tokio::time::timeout(Duration::from_secs(1), shared.live.changed.notified()).await;
        // Servers no longer listed: left.
        let servers = shared.servers();
        for (g, c) in conns.iter_mut().filter(|(g, _)| !servers.iter().any(|s| s.guild() == Some(**g))) {
            if c.joined.take().is_some() {
                let _ = manager.remove(serenity::GuildId::new(*g)).await;
                c.track = None;
            }
        }
        for server in &servers {
            let Some(g) = server.guild() else { continue };
            let c = conns.entry(g).or_default();
            let guild = serenity::GuildId::new(g);
            let follows = shared.live.follows_in(g);
            let mixer = shared.live.mixer(g);
            mixer.set_gain(server.live.db());
            mixer.keep(|s, t| follows.iter().any(|f| f.system == s && f.talkgroup == t));
            let voice = server.voice();
            // Joined where it should be, or not at all.
            let want = voice.filter(|_| !follows.is_empty());
            if c.joined != want {
                if c.joined.take().is_some() {
                    let _ = manager.remove(guild).await;
                    c.track = None;
                }
                if let Some(ch) = want {
                    if c.retry_at.is_none_or(|t| Instant::now() >= t) {
                        match manager.join(guild, ChannelId::new(ch)).await {
                            Ok(call) => {
                                c.track = Some(start_stream(&mut *call.lock().await, &shared, mixer.clone()));
                                c.joined = Some(ch);
                                c.retry_at = None;
                                shared.host.info(format!("live audio: joined voice channel {ch} in server {g}"));
                            }
                            Err(e) => {
                                shared.host.warn(format!(
                                    "live audio: couldn't join voice channel {ch} in server {g} (it needs View Channel, Connect and Speak): {e}"
                                ));
                                c.retry_at = Some(Instant::now() + Duration::from_secs(30));
                            }
                        }
                    }
                }
            }
            // The stream never ends: if it has, say why and start it again.
            if let (Some(t), Some(_)) = (&c.track, c.joined) {
                let state = t.get_info().await.map(|i| i.playing);
                if !matches!(&state, Ok(songbird::tracks::PlayMode::Play) | Ok(songbird::tracks::PlayMode::Pause)) {
                    shared.host.warn(format!("live audio: the stream in server {g} stopped ({state:?}); starting it again"));
                    if let Some(call) = manager.get(guild) {
                        c.track = Some(start_stream(&mut *call.lock().await, &shared, mixer.clone()));
                    }
                }
            }
            let rest = shared.rest();
            // A chime as a tone thread's audio starts.
            if shared.live.take_chime(g) {
                if let (Some(ch), Some(r), Ok(sound)) = (c.joined, &rest, server.live.chime.trim().parse::<u64>()) {
                    if let Err(e) = r.soundboard(ch, sound, Some(g)).await {
                        shared.host.warn(format!("live audio: couldn't play the chime (Use Soundboard): {e}"));
                    }
                }
            }
            // Who's talking: as a call starts playing, a line for each radio known; as more are
            // known (its end names them all), a line for each of those.
            let started = mixer.take_started();
            if let (Some(ch), true) = (voice, server.live.log_speakers) {
                let mut lines: Vec<String> = Vec::new();
                for (call, p) in started {
                    let who = shared.live.speakers(call);
                    lines.push(speaker_lines(&p, &who));
                    c.logged.insert(call, (who, Instant::now(), p));
                }
                for (call, (shown, _, p)) in c.logged.iter_mut() {
                    let new: Radios = shared.live.speakers(*call).into_iter().filter(|r| !shown.iter().any(|s| s.0 == r.0)).collect();
                    if !new.is_empty() {
                        lines.push(speaker_lines(p, &new));
                        shown.extend(new);
                    }
                }
                c.logged.retain(|_, x| x.1.elapsed() < Duration::from_secs(180));
                // (One message for what came together, to go easy on the channel.)
                if !lines.is_empty() {
                    let m = CreateMessage::new().content(lines.join("\n")).allowed_mentions(serenity::CreateAllowedMentions::new());
                    if let Err(e) = ChannelId::new(ch).send_message(&ctx.http, m).await {
                        shared.host.debug(format!("live audio: couldn't log who's talking in voice channel {ch}'s chat: {e}"));
                    }
                }
            }
            // While it plays, the panel kept in view: anything posted under it (lines of who's
            // talking, people), and every few minutes it's posted again at the bottom, the old one
            // deleted.
            if let Some(ch) = voice.filter(|_| !follows.is_empty()) {
                if c.panel_looked.is_none_or(|t| t.elapsed() >= PANEL_EVERY) {
                    c.panel_looked = Some(Instant::now());
                    if let Some((_, pm)) = shared.live.panel(g).filter(|p| p.0 == ch) {
                        match ChannelId::new(ch).messages(&ctx.http, serenity::GetMessages::new().limit(1)).await {
                            Ok(last) if last.first().is_some_and(|m| m.id.get() != pm) => {
                                let _ = ChannelId::new(ch).delete_message(&ctx.http, MessageId::new(pm)).await;
                                shared.live.forget_panel(g);
                                c.shown = None;
                            }
                            Ok(_) => {}
                            Err(e) => shared.host.debug(format!("live audio: couldn't read voice channel {ch}'s chat (Read Message History): {e}")),
                        }
                    }
                }
            }
            // The status and the panel, when what they say changes.
            let Some(ch) = voice else { continue };
            let playing = mixer.playing();
            let status = status_text(&playing, &follows);
            let lasts = lasting(&shared, &follows);
            let now = (status.clone(), playing.clone(), follows.clone(), lasts.clone());
            if c.shown.as_ref() == Some(&now) {
                continue;
            }
            if let Some(r) = &rest {
                if c.shown.as_ref().is_none_or(|s| s.0 != status) {
                    if let Err(e) = r.voice_status(ch, &status).await {
                        shared.host.debug(format!("live audio: couldn't set the voice channel's status (Set Voice Channel Status): {e}"));
                    }
                }
            }
            let (embed, rows) = panel(&playing, &follows, &lasts);
            let http = &ctx.http;
            let edited = match shared.live.panel(g) {
                Some((pc, pm)) if pc == ch => {
                    ChannelId::new(pc).edit_message(http, MessageId::new(pm), EditMessage::new().embed(embed.clone()).components(rows.clone())).await.is_ok()
                }
                _ => false,
            };
            if !edited {
                match ChannelId::new(ch).send_message(http, CreateMessage::new().embed(embed).components(rows)).await {
                    Ok(m) => shared.live.set_panel(g, (ch, m.id.get())),
                    Err(e) => shared.host.warn(format!("live audio: couldn't post the control panel in voice channel {ch}'s chat: {e}")),
                }
            }
            c.shown = Some(now);
        }
    }
}

/// A panel button. False: it isn't one.
pub async fn button(ctx: &serenity::Context, shared: &Shared, b: &serenity::ComponentInteraction) -> Result<bool, Box<serenity::Error>> {
    let id = b.data.custom_id.as_str();
    let g = b.guild_id.map_or(0, |g| g.get());
    let who = b.member.as_ref().and_then(|m| m.nick.clone()).unwrap_or_else(|| b.user.global_name.clone().unwrap_or_else(|| b.user.name.clone()));
    if let Some(thread) = id.strip_prefix("trp-live:more:").and_then(|t| t.parse::<u64>().ok()) {
        let after = shared.settings().thread_timeout_s();
        let t = after.and_then(|after| shared.threads.update(&shared.host, thread, |t| t.extend(crate::thread::now_s(), after)));
        b.create_response(ctx, serenity::CreateInteractionResponse::Acknowledge).await?;
        if let Some(t) = t {
            shared.host.info(format!("thread {}-{} given {} min more by {who}", t.system, t.talkgroup, (after.unwrap_or(0.0) / 60.0).round()));
            crate::thread::redraw(shared, &ctx.http, &t).await;
            shared.live.changed.notify_one();
        }
        return Ok(true);
    }
    let stopped = if id == "trp-live:all" {
        shared.live.unfollow(|f| f.server == g)
    } else if let Some(rest) = id.strip_prefix("trp-live:stop:") {
        let Some((tg, system)) = rest.split_once(':') else { return Ok(false) };
        let tg: u32 = tg.parse().unwrap_or(0);
        shared.live.unfollow(|f| f.server == g && f.talkgroup == tg && f.system == system)
    } else {
        return Ok(false);
    };
    if !stopped.is_empty() {
        shared.host.info(format!("live audio: {} stopped by {who}", stopped.iter().map(Follow::label).collect::<Vec<_>>().join(", ")));
    }
    // (The loop redraws the panel; this answers the press.)
    b.create_response(ctx, serenity::CreateInteractionResponse::Acknowledge).await?;
    crate::thread::redraw_live(shared, &ctx.http, &stopped).await;
    Ok(true)
}

/// Words in the voice channel's chat (with phrases on): what to do, and what to say back.
pub fn phrase(shared: &Shared, server: u64, text: &str, who: &str) -> Option<String> {
    let t = text.trim().to_lowercase();
    let t = t.trim_start_matches("hey ").trim_start_matches("bot ").trim_end_matches(['.', '!', '?']);
    let systems: Vec<String> = shared.systems.lock().unwrap().iter().map(|s| s.short_name.clone()).collect();
    let system_in = |s: &str| systems.iter().find(|x| s.split_whitespace().any(|w| w.eq_ignore_ascii_case(x))).cloned();
    if matches!(t, "stop" | "stop live audio" | "stop audio" | "stop all" | "stop everything" | "quiet" | "silence") {
        let gone = shared.live.unfollow(|f| f.server == server);
        return Some(if gone.is_empty() {
            "Nothing was playing.".into()
        } else {
            format!("Stopped live audio ({}).", gone.iter().map(Follow::label).collect::<Vec<_>>().join(", "))
        });
    }
    if let Some(rest) = t.strip_prefix("stop ") {
        let tg: u32 = rest.split_whitespace().find_map(|w| w.trim_start_matches("tg").parse().ok())?;
        let gone = shared.live.unfollow(|f| f.server == server && f.talkgroup == tg);
        return Some(if gone.is_empty() { format!("TG {tg} wasn't followed.") } else { format!("Stopped TG {tg}.") });
    }
    if let Some(rest) = t.strip_prefix("follow ") {
        if rest.contains("signal") || rest.contains("page") || rest.contains("tone") {
            let sys = system_in(rest);
            let Some((system, tg, tag, name)) = shared.live.last_signal(sys.as_deref()) else {
                return Some(format!("No signal heard{} yet.", sys.map(|s| format!(" on {s}")).unwrap_or_default()));
            };
            shared.live.follow(Follow { server, system: system.clone(), talkgroup: tg, tag: tag.clone(), why: Why::User { name: who.to_string() } });
            return Some(format!(
                "Following {} on {system} (the last signal: {name}).",
                if tag.is_empty() { format!("TG {tg}") } else { format!("{tag} ({tg})") }
            ));
        }
        let tg: u32 = rest.split_whitespace().find_map(|w| w.trim_start_matches("tg").parse().ok())?;
        let system = system_in(rest).or_else(|| (systems.len() == 1).then(|| systems[0].clone()))?;
        shared.live.follow(Follow { server, system: system.clone(), talkgroup: tg, tag: String::new(), why: Why::User { name: who.to_string() } });
        return Some(format!("Following TG {tg} on {system}."));
    }
    if matches!(t, "status" | "what's playing" | "whats playing" | "what is playing") {
        let f = shared.live.follows_in(server);
        return Some(match shared.live.mixer(server).playing() {
            Some(p) => format!("Playing {} on {}; following {}.", if p.tag.is_empty() { format!("TG {}", p.talkgroup) } else { p.tag }, p.system, f.len()),
            None => format!("Nothing playing; following {}.", f.len()),
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn playing(tg: u32, emergency: bool) -> Playing {
        Playing { system: "s".into(), talkgroup: tg, tag: String::new(), emergency }
    }

    #[test]
    fn one_call_at_a_time_the_most_important_first() {
        let m = Mixer::default();
        m.push(1, playing(100, false), 1, RATE, &vec![1000; 4000]);
        m.push(2, playing(200, true), 1, RATE, &vec![2000; 4000]);
        let mut out = vec![0f32; 100];
        m.fill(&mut out);
        // The emergency first.
        assert_eq!(m.playing().map(|p| p.talkgroup), Some(200));
        assert!(out[0] > 0.0);
        m.end(2);
        let mut rest = vec![0f32; 4000];
        m.fill(&mut rest);
        // Then the other, straight after.
        assert_eq!(m.playing().map(|p| p.talkgroup), Some(100));
        assert!(rest[3950] > 0.0, "{}", rest[3950]);
    }

    /// Songbird takes the stream (its header parsed, its packets decoded), and the mix comes out.
    #[test]
    fn songbird_plays_the_stream() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let mixer = Arc::new(Mixer::default());
        mixer.push(1, playing(100, false), 1, RATE, &vec![8000; 8000]);
        let input: songbird::input::Input = songbird::input::RawAdapter::new(LiveSource::new(mixer.clone()), OUT_RATE, 1).into();
        let parsed = rt.block_on(input.make_playable_async(songbird::input::codecs::get_codec_registry(), songbird::input::codecs::get_probe()));
        let mut parsed = match parsed {
            Ok(songbird::input::Input::Live(songbird::input::LiveInput::Parsed(p), _)) => p,
            Ok(_) => panic!("not parsed"),
            Err(e) => panic!("songbird refused the stream: {e:?}"),
        };
        // (Packets of f32 samples: some of them the call's, not silence.)
        let mut loud = 0usize;
        for _ in 0..20 {
            let Ok(packet) = parsed.format.next_packet() else { break };
            loud += packet.data.as_chunks::<4>().0.iter().filter(|b| f32::from_le_bytes(**b).abs() > 0.2).count();
        }
        assert!(loud > 1000, "{loud}");
    }

    #[test]
    fn speakers_read_as_system_tg_unit() {
        let p = Playing { system: "clmrn-I".into(), talkgroup: 1001, tag: "Fire Disp".into(), emergency: false };
        assert_eq!(
            speaker_lines(&p, &[(1234567, "Engine 1".into()), (7654321, String::new())]),
            "🎙️ **clmrn-I-1001-1234567** (Engine 1) · Fire Disp\n🎙️ **clmrn-I-1001-7654321** · Fire Disp"
        );
        assert_eq!(speaker_lines(&p, &[]), "🎙️ **clmrn-I-1001** (radio not known) · Fire Disp");
    }

    #[test]
    fn each_call_is_logged_once_as_it_starts_playing() {
        let m = Mixer::default();
        m.push(1, playing(100, false), 1, RATE, &vec![1000; 4000]);
        let mut out = vec![0f32; 100];
        m.fill(&mut out);
        m.fill(&mut out);
        assert_eq!(m.take_started().iter().map(|(c, _)| *c).collect::<Vec<_>>(), vec![1]);
        assert!(m.take_started().is_empty());
    }

    #[test]
    fn a_call_waits_for_enough_to_play() {
        let m = Mixer::default();
        m.push(1, playing(100, false), 1, RATE, &[500; 100]);
        let mut out = vec![1f32; 50];
        m.fill(&mut out);
        assert!(out.iter().all(|&s| s == 0.0) && m.playing().is_none());
    }

    #[test]
    fn a_quiet_talker_and_a_loud_one_play_at_one_level() {
        let m = Mixer::default();
        // (−30 and −12 dBFS: both come out at −12, 0.25.)
        m.push(1, playing(1, false), 1, RATE, &[1000; 4000]);
        m.push(2, playing(2, false), 1, RATE, &[8200; 4000]);
        m.end(1);
        m.end(2);
        let mut out = vec![0f32; 8000];
        m.fill(&mut out);
        assert!(out.iter().all(|&s| (s - 0.251).abs() < 0.01), "{} {}", out[0], out[7999]);
        // Turned up past full scale: peaks rounded off, never clipped.
        let m = Mixer::default();
        m.set_gain(18.0);
        m.push(3, playing(3, false), 1, RATE, &[8200; 4000]);
        m.end(3);
        let mut out = vec![0f32; 4000];
        m.fill(&mut out);
        assert!(out.iter().all(|&s| s < 1.0 && s > 0.95), "{}", out[0]);
    }

    /// 8 kHz up to 48: speech's band kept whole (songbird's resampler cut 3 kHz by 46 dB), and
    /// the images above 4 kHz gone.
    #[test]
    fn upsampling_keeps_the_band_and_drops_the_images() {
        for hz in [300.0, 1000.0, 2000.0, 3000.0, 3400.0] {
            let x: Vec<f32> = (0..8000).map(|i| (2.0 * std::f64::consts::PI * hz * i as f64 / 8000.0).sin() as f32 * 0.5).collect();
            let mut y = Vec::new();
            Upsample::new().run(&x, &mut y);
            let tail = &y[y.len() / 2..];
            let rms = (tail.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / tail.len() as f64).sqrt();
            let gain = 20.0 * (rms / (0.5 / 2f64.sqrt())).log10();
            assert!(gain.abs() < 0.5, "{hz} Hz: {gain:.2} dB");
            // (What's left once the tone itself is taken out: its images.)
            let n = tail.len() as f64;
            let w = 2.0 * std::f64::consts::PI * hz / 48000.0;
            let (c, s) =
                tail.iter().enumerate().fold((0.0, 0.0), |(c, s), (i, v)| (c + *v as f64 * (w * i as f64).cos(), s + *v as f64 * (w * i as f64).sin()));
            let tone = 2.0 * (c * c + s * s).sqrt() / n;
            let rest = (rms * rms - tone * tone / 2.0).max(1e-20).sqrt();
            assert!(20.0 * (rest / rms).log10() < -50.0, "{hz} Hz: images {:.1} dB", 20.0 * (rest / rms).log10());
        }
    }

    #[test]
    fn other_rates_are_resampled() {
        let m = Mixer::default();
        m.push(1, playing(1, false), 1, 16000, &vec![100; 8000]);
        m.end(1);
        let mut out = vec![0f32; 5000];
        m.fill(&mut out);
        let n = out.iter().filter(|&&s| s != 0.0).count();
        assert!((3990..=4000).contains(&n), "{n}");
    }
}
