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
    /// The LLM API key. Policy: it lives in MEMORY ONLY - read from the environment at boot and
    /// NEVER serialized to disk. `skip_serializing` keeps it out of config.json on every save, so
    /// no settings change or channel connect can ever leak it to disk; [`Config::load`] re-seeds it
    /// from the environment each boot.
    #[serde(skip_serializing)]
    pub api_key: String,
    pub model: String,
}

impl Default for ProviderCfg {
    fn default() -> Self {
        Self { kind: "mock".into(), base_url: String::new(), api_key: String::new(), model: "claude-haiku-4-5".into() }
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
        let mut cfg = match std::fs::read_to_string(Self::path(home)) {
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
        };
        // The API key is never persisted (skip_serializing), so a config.json saved after a key was
        // set comes back without it. Re-seed from the environment each boot - the env is the source
        // of truth when present; otherwise keep whatever loaded (back-compat), never wiping a key.
        if let Some(k) = env_api_key() {
            cfg.provider.api_key = k;
        } else if cfg.provider.kind == "anthropic" {
            // Also honor the standard ANTHROPIC_API_KEY (the SDK convention) for the Anthropic
            // provider, so an exported key Just Works and survives restarts under the memory-only
            // key policy. Scoped to kind == "anthropic" so it can never pollute an OpenAI/Ollama
            // config that legitimately has a different (unpersisted) key.
            if let Some(k) = std::env::var("ANTHROPIC_API_KEY").ok().filter(|s| !s.is_empty()) {
                cfg.provider.api_key = k;
            }
        }
        // Heal the dead mock-era placeholder model id. "claude-haiku" was never a valid
        // Anthropic API model - it 404s on the live endpoint ("model: claude-haiku not_found").
        // An install that saved it before this fix would keep re-sending it; remap the stored
        // copy to the real Haiku alias so the live key works without the user re-picking a model.
        if cfg.provider.model == "claude-haiku" {
            cfg.provider.model = "claude-haiku-4-5".into();
        }
        cfg
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
        } else if let Some(key) = std::env::var("ANTHROPIC_API_KEY").ok().filter(|k| !k.is_empty()) {
            // Standard Anthropic env var: an exported key brings up the real provider on a fresh
            // install (no config.json yet) with no UI step. base_url is left empty so the provider
            // uses its built-in https://api.anthropic.com/v1 default - matching the UI's anthropic
            // preset (which also sends an empty base). We deliberately do NOT adopt ANTHROPIC_BASE_URL:
            // it is a host root (no /v1), but our provider posts to "{base}/messages", so a raw host
            // would 404. Custom endpoints go through the UI or ENGRAM_LLM_BASE_URL (Engram convention).
            c.provider.kind = "anthropic".into();
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
            "claude-haiku-4-5".into()
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

/// The LLM API key from the environment (Anthropic or generic), or `None`. Memory-only by policy:
/// this is the single source of truth re-seeded on every boot, so the key is never read from disk.
fn env_api_key() -> Option<String> {
    std::env::var("ENGRAM_ANTHROPIC_API_KEY")
        .or_else(|_| std::env::var("ENGRAM_LLM_API_KEY"))
        .ok()
        .filter(|k| !k.is_empty())
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

#[cfg(test)]
mod tests {
    use super::*;

    fn tmphome() -> String {
        // Atomic counter (not just nanos) so parallel tests never share a dir under load.
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let d = std::env::temp_dir()
            .join(format!("engram-config-test-{n}-{}", SEQ.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir_all(&d).unwrap();
        d.to_string_lossy().into_owned()
    }

    // Tests that mutate process-global env vars serialize on this lock so they never race.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// RAII: set/unset an env var for the duration of a test, restoring the prior value on drop
    /// so we don't clobber a real key the dev shell has exported.
    struct EnvVarGuard {
        key: &'static str,
        prev: Option<String>,
    }
    impl EnvVarGuard {
        fn set(key: &'static str, val: &str) -> Self {
            let prev = std::env::var(key).ok();
            std::env::set_var(key, val);
            Self { key, prev }
        }
        fn unset(key: &'static str) -> Self {
            let prev = std::env::var(key).ok();
            std::env::remove_var(key);
            Self { key, prev }
        }
    }
    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            match &self.prev {
                Some(v) => std::env::set_var(self.key, v),
                None => std::env::remove_var(self.key),
            }
        }
    }

    #[test]
    fn api_key_never_persists_to_disk() {
        let _lock = ENV_LOCK.lock().unwrap();
        let home = tmphome();
        let mut c = Config::default();
        c.provider.kind = "anthropic".into();
        c.provider.api_key = "sk-ant-SUPERSECRET".into();
        c.save(&home).unwrap();
        // The key must NOT be in config.json; non-secret fields must be.
        let on_disk = std::fs::read_to_string(Config::path(&home)).unwrap();
        assert!(!on_disk.contains("SUPERSECRET"), "api_key was written to config.json");
        assert!(on_disk.contains("anthropic"), "non-secret fields should persist");
        // Reloading must not resurrect the on-disk secret. The key may be re-seeded from the
        // environment (ENGRAM_* or, for an anthropic provider, the standard ANTHROPIC_API_KEY) -
        // that is the intended memory-only-via-env policy - but never from config.json.
        let reloaded = Config::load(&home);
        assert_ne!(reloaded.provider.api_key, "sk-ant-SUPERSECRET", "api_key came back from disk");
        assert_eq!(reloaded.provider.kind, "anthropic");
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn heals_dead_haiku_placeholder_model() {
        // kind="mock" so the env-key adoption branch is skipped: the heal is the only effect,
        // making this independent of whatever ANTHROPIC_API_KEY the dev env happens to have set.
        let home = tmphome();
        let mut c = Config::default();
        c.provider.kind = "mock".into();
        c.provider.model = "claude-haiku".into(); // the dead mock-era placeholder (404s live)
        c.save(&home).unwrap();
        assert_eq!(
            Config::load(&home).provider.model,
            "claude-haiku-4-5",
            "dead placeholder must heal to the real Haiku alias"
        );
        // A real, already-valid model id must be left untouched.
        let home2 = tmphome();
        let mut c2 = Config::default();
        c2.provider.kind = "mock".into();
        c2.provider.model = "claude-opus-4-8".into();
        c2.save(&home2).unwrap();
        assert_eq!(
            Config::load(&home2).provider.model,
            "claude-opus-4-8",
            "a valid model id must not be rewritten"
        );
        std::fs::remove_dir_all(&home).ok();
        std::fs::remove_dir_all(&home2).ok();
    }

    #[test]
    fn adopts_standard_anthropic_api_key_when_no_engram_var() {
        let _lock = ENV_LOCK.lock().unwrap();
        // The ENGRAM_-prefixed vars take precedence, so clear them; set only the standard one.
        let _g1 = EnvVarGuard::unset("ENGRAM_ANTHROPIC_API_KEY");
        let _g2 = EnvVarGuard::unset("ENGRAM_LLM_API_KEY");
        let _g3 = EnvVarGuard::unset("ENGRAM_LLM_BASE_URL");
        let _g4 = EnvVarGuard::set("ANTHROPIC_API_KEY", "sk-ant-env-adopt-test");
        // No config.json in `home` -> from_env runs and should bring up the real provider.
        let home = tmphome();
        let c = Config::load(&home);
        assert_eq!(c.provider.kind, "anthropic", "standard key must select the anthropic provider");
        assert_eq!(c.provider.api_key, "sk-ant-env-adopt-test", "standard key must be adopted");
        // base_url stays empty so the provider uses its correct https://api.anthropic.com/v1 default.
        assert!(c.provider.base_url.is_empty(), "must not adopt the raw host ANTHROPIC_BASE_URL");
        assert_eq!(c.model(), "claude-haiku-4-5", "default model must be a real Anthropic id");
        std::fs::remove_dir_all(&home).ok();
    }
}
