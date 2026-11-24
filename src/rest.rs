//! Discord's REST API for what serenity 0.12 can't send or read: Components V2 messages (whose
//! component types it refuses to parse), forum posts and their tags, a voice channel's status and
//! soundboard sounds. JSON in, JSON out.

use serde_json::{json, Value};

const API: &str = "https://discord.com/api/v10";
/// Message flag: its layout is Components V2 (no content or embeds).
pub const COMPONENTS_V2: u64 = 1 << 15;

#[derive(Clone)]
pub struct Rest {
    http: reqwest::Client,
    token: String,
}

#[derive(Debug)]
pub struct RestError {
    pub status: u16,
    pub code: i64,
    pub message: String,
}

impl std::fmt::Display for RestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.message, self.status)
    }
}

impl RestError {
    /// Discord doesn't have it any more (deleted by hand).
    pub fn gone(&self) -> bool {
        self.status == 404 || matches!(self.code, 10003 | 10008)
    }
}

impl Rest {
    pub fn new(token: &str) -> Rest {
        let http = reqwest::Client::builder().user_agent("DiscordBot (https://github.com/TrunkRecorder/trunk-plugin-discord, 0.1)").build().unwrap_or_default();
        Rest { http, token: token.to_string() }
    }

    async fn call(&self, method: reqwest::Method, path: &str, body: Option<&Value>) -> Result<Value, RestError> {
        // Rate limited: wait as told, a few times.
        for _ in 0..4 {
            let mut r = self.http.request(method.clone(), format!("{API}{path}")).header("Authorization", format!("Bot {}", self.token));
            if let Some(b) = body {
                r = r.json(b);
            }
            let res = r.send().await.map_err(|e| RestError { status: 0, code: 0, message: e.to_string() })?;
            let status = res.status().as_u16();
            let v: Value = res.json().await.unwrap_or(Value::Null);
            if status == 429 {
                let wait = v["retry_after"].as_f64().unwrap_or(1.0).clamp(0.05, 30.0);
                tokio::time::sleep(std::time::Duration::from_secs_f64(wait)).await;
                continue;
            }
            if (200..300).contains(&status) {
                return Ok(v);
            }
            return Err(RestError { status, code: v["code"].as_i64().unwrap_or(0), message: v["message"].as_str().unwrap_or("failed").to_string() });
        }
        Err(RestError { status: 429, code: 0, message: "rate limited".into() })
    }

    pub async fn get(&self, path: &str) -> Result<Value, RestError> {
        self.call(reqwest::Method::GET, path, None).await
    }
    pub async fn post(&self, path: &str, body: &Value) -> Result<Value, RestError> {
        self.call(reqwest::Method::POST, path, Some(body)).await
    }
    pub async fn patch(&self, path: &str, body: &Value) -> Result<Value, RestError> {
        self.call(reqwest::Method::PATCH, path, Some(body)).await
    }
    pub async fn put(&self, path: &str, body: &Value) -> Result<Value, RestError> {
        self.call(reqwest::Method::PUT, path, Some(body)).await
    }

    /// Post a Components V2 message (`components`: its top-level components); its id.
    pub async fn post_v2(&self, channel: u64, components: Value) -> Result<u64, RestError> {
        let v = self
            .post(&format!("/channels/{channel}/messages"), &json!({ "flags": COMPONENTS_V2, "components": components, "allowed_mentions": { "parse": [] } }))
            .await?;
        Ok(id(&v["id"]))
    }

    pub async fn edit_v2(&self, channel: u64, message: u64, components: Value) -> Result<(), RestError> {
        self.patch(&format!("/channels/{channel}/messages/{message}"), &json!({ "flags": COMPONENTS_V2, "components": components })).await.map(drop)
    }

    /// A voice channel's status, under its name in the channel list ("" clears it).
    pub async fn voice_status(&self, channel: u64, status: &str) -> Result<(), RestError> {
        let s: String = status.chars().take(500).collect();
        self.put(&format!("/channels/{channel}/voice-status"), &json!({ "status": s })).await.map(drop)
    }

    /// Play a soundboard sound in the voice channel the bot is in.
    pub async fn soundboard(&self, channel: u64, sound: u64, from_guild: Option<u64>) -> Result<(), RestError> {
        let mut b = json!({ "sound_id": sound.to_string() });
        if let Some(g) = from_guild {
            b["source_guild_id"] = json!(g.to_string());
        }
        self.post(&format!("/channels/{channel}/send-soundboard-sound"), &b).await.map(drop)
    }
}

/// A snowflake, as Discord sends it (a string).
pub fn id(v: &Value) -> u64 {
    v.as_str().and_then(|s| s.parse().ok()).or_else(|| v.as_u64()).unwrap_or(0)
}

// ─── Components V2 builders ───────────────────────────────────────────────

pub fn text(s: impl Into<String>) -> Value {
    json!({ "type": 10, "content": s.into() })
}

pub fn separator() -> Value {
    json!({ "type": 14, "divider": true, "spacing": 1 })
}

pub fn container(color: u32, inner: Vec<Value>) -> Value {
    json!({ "type": 17, "accent_color": color, "components": inner })
}
