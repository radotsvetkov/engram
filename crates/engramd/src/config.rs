//! Persisted runtime settings - the model provider, embeddings, security gates, cost
//! caps, and MCP servers. The desktop's Settings panel reads and writes this through
//! `/v1/config`; the daemon stores it as `config.json` under `ENGRAM_HOME`.
//!
//! Precedence: a `config.json` on disk wins. If there is none (a fresh install or an
//! existing env-configured deployment), the settings are seeded from the `ENGRAM_*`
//! environment so nothing changes until the user saves from the UI for the first time.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// The whole settings document.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub provider: ProviderCfg,
    pub embed: EmbedCfg,
    pub security: SecurityCfg,
    pub cost: CostCfg,
    pub channels: ChannelsCfg,
    pub mcp: Vec<McpServer>,
}

/// Outbound/inbound messaging channels the desktop's Integrations gallery configures.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ChannelsCfg {
    /// Telegram bot token (from BotFather). Empty means the Telegram bot is off.
    /// Connecting from the desktop starts the poller live; also read at boot by `telegram::spawn`.
    pub telegram_token: String,
    /// The connected bot's @username, cached from getMe for display. Public, so safe to store
    /// (unlike the token). Empty when not connected.
    #[serde(default)]
    pub telegram_username: String,
}

/// Which model backend to call, and with what credentials.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderCfg {
    /// `anthropic` | `openai` | `openrouter` | `ollama` | `mock`.
    pub kind: String,
    /// Override the backend host. Empty means "use the default for this kind".
    pub base_url: String,
    pub api_key: String,
    pub model: String,
}

impl Default for ProviderCfg {
    fn default() -> Self {
        Self { kind: "mock".into(), base_url: String::new(), api_key: String::new(), model: "claude-haiku".into() }
    }
}

/// How memories are embedded for semantic recall.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct EmbedCfg {
    /// `trigram` (offline default) | `static` (model2vec) | `gateway` (provider embeddings).
    pub kind: String,
    /// Directory of the static model2vec files, when `kind == "static"`.
    pub model_dir: String,
}

impl Default for EmbedCfg {
    fn default() -> Self {
        Self { kind: "trigram".into(), model_dir: String::new() }
    }
}

/// Local security gates.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SecurityCfg {
    /// Bearer token required on the HTTP API. Empty disables the gate.
    pub api_token: String,
    /// Shared secret required on inbound channel webhooks. Empty disables it.
    pub channel_secret: String,
    /// Whether the agent may run shell commands.
    pub allow_shell: bool,
}

/// Runaway-cost guard.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct CostCfg {
    /// Token ceiling for a single task run (input + output across the whole run).
    pub task_token_budget: u64,
}

impl Default for CostCfg {
    fn default() -> Self {
        Self { task_token_budget: 250_000 }
    }
}

/// One MCP server the agent should connect to and borrow tools from.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct McpServer {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
}

impl Config {
    pub fn path(home: &str) -> PathBuf {
        Path::new(home).join("config.json")
    }

    /// Load from `config.json`, or seed from the environment when there is none yet.
    pub fn load(home: &str) -> Self {
        match std::fs::read_to_string(Self::path(home)) {
            Ok(text) => match serde_json::from_str::<Config>(&text) {
                Ok(mut cfg) => {
                    // mcp.json is the file the agent connector reads at boot; keep the
                    // settings document in step with it if the user edited it by hand.
                    if cfg.mcp.is_empty() {
                        cfg.mcp = read_mcp_json(home);
                    }
                    cfg
                }
                Err(e) => {
                    tracing::warn!(error = %e, "config.json is invalid - falling back to environment");
                    Self::from_env(home)
                }
            },
            Err(_) => Self::from_env(home),
        }
    }

    /// Seed settings from `ENGRAM_*` so an env-configured daemon shows its real state and
    /// keeps working until the user saves from the UI.
    pub fn from_env(home: &str) -> Self {
        let mut c = Config::default();
        if let Ok(key) = std::env::var("ENGRAM_ANTHROPIC_API_KEY") {
            c.provider.kind = "anthropic".into();
            c.provider.api_key = key;
            c.provider.base_url = std::env::var("ENGRAM_LLM_BASE_URL").unwrap_or_default();
        } else if let (Ok(base), Ok(key)) =
            (std::env::var("ENGRAM_LLM_BASE_URL"), std::env::var("ENGRAM_LLM_API_KEY"))
        {
            c.provider.kind = detect_kind(&base);
            c.provider.base_url = base;
            c.provider.api_key = key;
        }
        if let Ok(m) = std::env::var("ENGRAM_MODEL") {
            c.provider.model = m;
        }
        c.embed.kind = std::env::var("ENGRAM_EMBED").unwrap_or_else(|_| "trigram".into());
        c.embed.model_dir = std::env::var("ENGRAM_STATIC_MODEL").unwrap_or_default();
        c.security.api_token = std::env::var("ENGRAM_API_TOKEN").unwrap_or_default();
        c.security.channel_secret = std::env::var("ENGRAM_CHANNEL_SECRET").unwrap_or_default();
        c.security.allow_shell = std::env::var("ENGRAM_TOOLS_SHELL").as_deref() == Ok("1");
        if let Some(b) = std::env::var("ENGRAM_TASK_TOKEN_BUDGET").ok().and_then(|v| v.parse().ok()) {
            c.cost.task_token_budget = b;
        }
        c.mcp = read_mcp_json(home);
        c
    }

    /// Persist to `config.json` (0600) and mirror the MCP list into `mcp.json` so the
    /// agent connector picks it up on the next wake.
    pub fn save(&self, home: &str) -> std::io::Result<()> {
        let text = serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".into());
        let path = Self::path(home);
        std::fs::write(&path, text)?;
        restrict(&path);
        let mcp_path = Path::new(home).join("mcp.json");
        if self.mcp.is_empty() {
            let _ = std::fs::remove_file(&mcp_path);
        } else {
            let _ = std::fs::write(&mcp_path, serde_json::to_string_pretty(&self.mcp).unwrap_or_default());
            restrict(&mcp_path);
        }
        Ok(())
    }

    /// The model id to send with requests, with a sane default.
    pub fn model(&self) -> String {
        if self.provider.model.is_empty() {
            "claude-haiku".into()
        } else {
            self.provider.model.clone()
        }
    }

    /// Build a live provider from these settings (mirrors `make_provider`'s env logic).
    pub fn build_provider(&self) -> Box<dyn engram_gateway::Provider> {
        #[cfg(feature = "http")]
        {
            let p = &self.provider;
            match p.kind.as_str() {
                "anthropic" if !p.api_key.is_empty() => {
                    return Box::new(engram_gateway::AnthropicProvider::new(p.base_url.clone(), p.api_key.clone()));
                }
                "openai" | "openrouter" | "ollama" | "http" => {
                    let base = if p.base_url.is_empty() { default_base(&p.kind) } else { p.base_url.clone() };
                    if !base.is_empty() {
                        // Ollama's OpenAI-compatible endpoint ignores the key but the client wants one.
                        let key = if p.api_key.is_empty() && p.kind == "ollama" {
                            "ollama".to_string()
                        } else {
                            p.api_key.clone()
                        };
                        return Box::new(engram_gateway::HttpProvider::new(p.kind.clone(), base, key));
                    }
                }
                _ => {}
            }
        }
        Box::new(engram_gateway::MockProvider)
    }

    /// A secrets-masked view for the UI - keys are never sent back to the browser.
    pub fn redacted(&self) -> Value {
        json!({
            "provider": {
                "kind": self.provider.kind,
                "base_url": self.provider.base_url,
                "model": self.provider.model,
                "api_key_set": !self.provider.api_key.is_empty(),
            },
            "embed": { "kind": self.embed.kind, "model_dir": self.embed.model_dir },
            "security": {
                "api_token_set": !self.security.api_token.is_empty(),
                "channel_secret_set": !self.security.channel_secret.is_empty(),
                "allow_shell": self.security.allow_shell,
            },
            "cost": { "task_token_budget": self.cost.task_token_budget },
            "channels": {
                "telegram_set": !self.channels.telegram_token.is_empty(),
                "telegram_username": self.channels.telegram_username,
            },
            "mcp": self.mcp,
        })
    }
}

/// The default host for an OpenAI-compatible backend kind.
#[cfg(feature = "http")]
fn default_base(kind: &str) -> String {
    match kind {
        "openrouter" => "https://openrouter.ai/api/v1".into(),
        "ollama" => "http://localhost:11434/v1".into(),
        _ => String::new(),
    }
}

/// Guess a backend kind from a base URL, for env migration.
fn detect_kind(base: &str) -> String {
    if base.contains("openrouter") {
        "openrouter".into()
    } else if base.contains("11434") || base.contains("ollama") {
        "ollama".into()
    } else {
        "openai".into()
    }
}

fn read_mcp_json(home: &str) -> Vec<McpServer> {
    std::fs::read_to_string(Path::new(home).join("mcp.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// Best-effort tighten file permissions to owner-only (secrets live here).
fn restrict(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    let _ = path;
}
