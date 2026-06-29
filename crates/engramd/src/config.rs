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
    /// Per-modality model overrides (vision / image / TTS / STT). Empty = the built-in default.
    #[serde(default)]
    pub media: MediaCfg,
    /// Interactive-browser settings (Chrome path + CDP port). Applied at boot.
    #[serde(default)]
    pub browser: BrowserCfg,
    /// Web-search provider keys/URL (Tavily / Brave / SearXNG). Injected into the tool environment.
    #[serde(default)]
    pub web: WebCfg,
}

/// Web/data API configuration. The keys here are injected into the daemon environment at boot and on
/// save, where the search tool and the flight_search skill read them. All are free, no-credit-card
/// options. Surfaced in Settings › Keys & security; secret keys are masked in the redacted UI view.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct WebCfg {
    /// Tavily API key (free tier, no credit card — tavily.com). Saved to disk; masked in the UI view.
    pub tavily_api_key: String,
    /// Brave Search API key (api.search.brave.com). Saved to disk; masked in the UI view.
    pub brave_api_key: String,
    /// SearXNG instance base URL, e.g. https://searx.example.org (keyless). A URL, so not masked.
    pub searxng_url: String,
    /// Travelpayouts data-API token (free affiliate token, no credit card — travelpayouts.com). Lets
    /// the `flight_search` skill return real cheapest fares + booking links instead of scraping. Masked.
    pub travelpayouts_token: String,
}

/// Model overrides for the non-text modalities. Each empty string keeps the built-in default
/// (or the matching ENGRAM_* env var). Surfaced in Settings > Gateways.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MediaCfg {
    /// Model the vision tool uses to read images. Empty = inherit the run's primary model.
    pub vision_model: String,
    /// Image-generation model. Empty = `gpt-image-1`.
    pub image_model: String,
    /// Text-to-speech model. Empty = `tts-1`.
    pub tts_model: String,
    /// Speech-to-text model. Empty = `whisper-1`.
    pub stt_model: String,
}

/// Interactive-browser configuration. The session is built once at boot, so changes are
/// "restart to apply". Surfaced in Settings > Advanced.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BrowserCfg {
    /// Explicit Chrome/Chromium binary path. Empty = auto-detect (or the ENGRAM_CHROME env var).
    pub chrome_path: String,
    /// Chrome DevTools Protocol port. 0 (the default) = fall through to ENGRAM_CDP_PORT, then 9222.
    pub cdp_port: u16,
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
    /// Default outbound webhook URL for the `send_message` tool (e.g. a Slack/Discord/Mattermost
    /// incoming webhook). Empty = no default. A Slack-style URL embeds a secret, so it is masked in
    /// the redacted API view (only presence is reported) and validated by the SSRF guard at use.
    #[serde(default)]
    pub webhook_url: String,
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
    /// Reasoning effort applied to model calls: "" (model default) | "low" | "medium" | "high".
    /// Mapped model-awarely by the gateway (OpenAI `reasoning_effort` / Claude extended-thinking),
    /// so it is a no-op on models that do not support it.
    #[serde(default)]
    pub effort: String,
}

impl Default for ProviderCfg {
    fn default() -> Self {
        Self {
            kind: "mock".into(),
            base_url: String::new(),
            api_key: String::new(),
            model: "claude-haiku-4-5".into(),
            effort: String::new(),
        }
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
        Self {
            kind: "trigram".into(),
            model_dir: String::new(),
        }
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
    /// How shell commands are isolated: "" / "local" (host), "docker", or "ssh". Anything other
    /// than docker/ssh runs on the host.
    #[serde(default)]
    pub shell_backend: String,
    /// The target for the chosen backend: the Docker image (docker) or the user@host (ssh).
    #[serde(default)]
    pub shell_target: String,
    /// Run each task in its own throwaway git worktree (when the workspace is a git repo), so
    /// concurrent agents can't clobber each other's files. Off by default. Mirrors ENGRAM_WORKTREES.
    #[serde(default)]
    pub enable_worktree_isolation: bool,
    /// Built-in tools the user has turned OFF — a deny-list of tool names (e.g. "web_fetch",
    /// "send_message"). A disabled tool is dropped from the toolset (built-ins and MCP at the run
    /// chokepoint, and also for delegated subagents via the global deny-list), so it is simply never
    /// advertised to the model that run.
    #[serde(default)]
    pub disabled_tools: Vec<String>,
    /// Turn OFF the agent's ability to author/improve skills. `false` (the default) = authoring is
    /// ON, so skills pop up from real use. Inverted so the zero value keeps the on-by-default posture.
    #[serde(default)]
    pub disable_skill_author: bool,
    /// AUTONOMOUS distillation: after a task, the model reflects on whether the work yields a reusable
    /// program and, if so, proposes one (installed inactive). OFF by default — it puts one extra model
    /// call on the per-task path, so it's strictly opt-in to protect the zero-idle/low-cost posture.
    /// Enabling it also turns on the skill-sleep prune that retires proposed-but-never-adopted skills.
    #[serde(default)]
    pub auto_distill_skills: bool,
}

/// Resolve a (backend, target) pair into the policy's shell-backend string the agent expects:
/// `None` = run on the host, `Some("<image>")` = a Docker sandbox, `Some("ssh:<host>")` = SSH.
pub fn resolve_shell_backend(backend: &str, target: &str) -> Option<String> {
    match backend.trim() {
        "docker" => Some(if target.trim().is_empty() {
            "alpine".to_string()
        } else {
            target.trim().to_string()
        }),
        "ssh" if !target.trim().is_empty() => Some(format!("ssh:{}", target.trim())),
        _ => None,
    }
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
        Self {
            // Generous enough for a real multi-tool research task to finish (the old 250k cut sprawling
            // trip-planning/research runs off mid-way); still a runaway-cost ceiling (~$1.80 at $3/M).
            // Users can raise it in Settings › Cost, and a hit budget now ends with a useful summary.
            task_token_budget: 600_000,
        }
    }
}

/// One MCP server the agent should connect to and borrow tools from.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct McpServer {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// Per-server environment variables (API keys, tokens) passed ONLY to this subprocess - so an
    /// authenticated server (GitHub/Slack/Postgres) is configurable without polluting the daemon's
    /// global env. Stored in config.json but masked in the redacted view sent to the browser.
    #[serde(default)]
    pub env: std::collections::BTreeMap<String, String>,
    /// Working directory for the subprocess (e.g. a repo root for a git/filesystem server).
    #[serde(default)]
    pub cwd: String,
    /// A first-party server you trust: its reads no longer taint/sensitise the run (default false,
    /// i.e. treated as untrusted+sensitive so it can't launder content into an egress-capable run).
    #[serde(default)]
    pub trusted: bool,
}

impl Config {
    pub fn path(home: &str) -> PathBuf {
        Path::new(home).join("config.json")
    }

    /// Load from `config.json`, or seed from the environment when there is none yet.
    pub fn load(home: &str) -> Self {
        Self::load_inner(home, true)
    }

    /// Like [`load`](Self::load) but does NOT touch the OS keyring. Reading the keyring can pop a
    /// blocking macOS Keychain password prompt (the app is adhoc-signed), which previously stalled
    /// the daemon BEFORE it bound the HTTP port — so the desktop webview hit a dead URL and showed a
    /// white screen. The daemon now loads with this, binds + serves immediately, then reads the
    /// keyring in the background and hot-swaps the provider in. Env keys are still applied here.
    pub fn load_no_keychain(home: &str) -> Self {
        Self::load_inner(home, false)
    }

    fn load_inner(home: &str, read_keychain: bool) -> Self {
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
        // The API key is kept OUT of config.json (skip_serializing) so that document stays
        // shareable/backup-able. Re-seed it each boot, in precedence order:
        //   1. the environment (ENGRAM_* / ANTHROPIC_API_KEY) - the source of truth on a VPS;
        //   2. the local secret store (OS keyring when built with `keyring`, else a 0600 file
        //      next to the Ed25519 signing key) - so a key typed into the desktop UI survives
        //      idle-sleep, restart, and reboot instead of silently degrading to the offline mock.
        if let Some(k) = env_api_key() {
            cfg.provider.api_key = k;
        } else if cfg.provider.kind == "anthropic" {
            // Also honor the standard ANTHROPIC_API_KEY (the SDK convention) for the Anthropic
            // provider. Scoped to kind == "anthropic" so it can never pollute an OpenAI/Ollama
            // config that legitimately has a different key.
            if let Some(k) = std::env::var("ANTHROPIC_API_KEY")
                .ok()
                .filter(|s| !s.is_empty())
            {
                cfg.provider.api_key = k;
            }
        }
        if read_keychain && cfg.provider.api_key.is_empty() {
            if let Some(k) = read_secret_key(home) {
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
        } else if let (Ok(base), Ok(key)) = (
            std::env::var("ENGRAM_LLM_BASE_URL"),
            std::env::var("ENGRAM_LLM_API_KEY"),
        ) {
            c.provider.kind = detect_kind(&base);
            c.provider.base_url = base;
            c.provider.api_key = key;
        } else if let Some(key) = std::env::var("ANTHROPIC_API_KEY")
            .ok()
            .filter(|k| !k.is_empty())
        {
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
        if let Some(b) = std::env::var("ENGRAM_TASK_TOKEN_BUDGET")
            .ok()
            .and_then(|v| v.parse().ok())
        {
            c.cost.task_token_budget = b;
        }
        // Seed the rest of the env-configurable settings so a fresh env-only deploy shows its real
        // state in the UI (otherwise the Settings panels would report these as unset while the
        // ENGRAM_* fallbacks silently drive behaviour). All still fall back to the env var at the
        // use-site once a config.json exists, so this only changes what the UI displays.
        c.security.enable_worktree_isolation =
            std::env::var("ENGRAM_WORKTREES").as_deref() == Ok("1");
        match std::env::var("ENGRAM_SHELL_BACKEND").as_deref() {
            Ok("docker") => {
                c.security.shell_backend = "docker".into();
                c.security.shell_target = std::env::var("ENGRAM_DOCKER_IMAGE").unwrap_or_default();
            }
            Ok("ssh") => {
                c.security.shell_backend = "ssh".into();
                c.security.shell_target = std::env::var("ENGRAM_SSH_HOST").unwrap_or_default();
            }
            _ => {}
        }
        c.media.vision_model = std::env::var("ENGRAM_VISION_MODEL").unwrap_or_default();
        c.media.image_model = std::env::var("ENGRAM_IMAGE_MODEL").unwrap_or_default();
        c.media.tts_model = std::env::var("ENGRAM_TTS_MODEL").unwrap_or_default();
        c.media.stt_model = std::env::var("ENGRAM_STT_MODEL").unwrap_or_default();
        c.browser.chrome_path = std::env::var("ENGRAM_CHROME").unwrap_or_default();
        c.browser.cdp_port = std::env::var("ENGRAM_CDP_PORT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        c.channels.webhook_url = std::env::var("ENGRAM_WEBHOOK_URL").unwrap_or_default();
        c.mcp = read_mcp_json(home);
        c
    }

    /// Persist to `config.json` (0600) and mirror the MCP list into `mcp.json` so the
    /// agent connector picks it up on the next wake. The API key is written to the local
    /// secret store (NOT config.json) so it survives a restart without leaking into backups.
    pub fn save(&self, home: &str) -> std::io::Result<()> {
        let text = serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".into());
        let path = Self::path(home);
        // config.json + mcp.json can carry per-server MCP env secrets, so create them owner-only
        // atomically (no chmod-after-write window where they're briefly group/other-readable).
        write_owner_only(&path, text.as_bytes())?;
        let mcp_path = Path::new(home).join("mcp.json");
        if self.mcp.is_empty() {
            let _ = std::fs::remove_file(&mcp_path);
        } else {
            let _ = write_owner_only(
                &mcp_path,
                serde_json::to_string_pretty(&self.mcp)
                    .unwrap_or_default()
                    .as_bytes(),
            );
        }
        // Persist (or clear) the API key in the local secret store. Best-effort: a failure here
        // must not lose the user's other settings - the key simply won't survive the next restart.
        write_secret_key(home, &self.provider.api_key);
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
    ///
    /// Anthropic uses its native Messages transport; every other non-mock kind is treated as an
    /// OpenAI-compatible HTTP backend (OpenAI, OpenRouter, Groq, DeepSeek, Mistral, Together, xAI,
    /// Perplexity, Gemini's OpenAI endpoint, and local Ollama / LM Studio / vLLM / llama.cpp). The
    /// base URL comes from the saved config, else a built-in default for that kind, so a user can
    /// pick a provider and have it work without hunting down the endpoint.
    pub fn build_provider(&self) -> Box<dyn engram_gateway::Provider> {
        build_provider_from(
            &self.provider.kind,
            &self.provider.base_url,
            &self.provider.api_key,
            &self.media,
        )
    }

    /// A secrets-masked view for the UI - keys are never sent back to the browser.
    pub fn redacted(&self) -> Value {
        json!({
            "provider": {
                "kind": self.provider.kind,
                "base_url": self.provider.base_url,
                "model": self.provider.model,
                "api_key_set": !self.provider.api_key.is_empty(),
                "effort": self.provider.effort,
            },
            "embed": { "kind": self.embed.kind, "model_dir": self.embed.model_dir },
            "security": {
                "api_token_set": !self.security.api_token.is_empty(),
                "channel_secret_set": !self.security.channel_secret.is_empty(),
                "allow_shell": self.security.allow_shell,
                "shell_backend": self.security.shell_backend,
                "shell_target": self.security.shell_target,
                "enable_worktree_isolation": self.security.enable_worktree_isolation,
                "disabled_tools": self.security.disabled_tools,
                "disable_skill_author": self.security.disable_skill_author,
                "auto_distill_skills": self.security.auto_distill_skills,
            },
            "cost": { "task_token_budget": self.cost.task_token_budget },
            "channels": {
                "telegram_set": !self.channels.telegram_token.is_empty(),
                "telegram_username": self.channels.telegram_username,
                // A Slack-style webhook URL embeds a secret, so report presence only - never the URL.
                "webhook_url_set": !self.channels.webhook_url.is_empty(),
            },
            "media": {
                "vision_model": self.media.vision_model,
                "image_model": self.media.image_model,
                "tts_model": self.media.tts_model,
                "stt_model": self.media.stt_model,
            },
            "browser": {
                "chrome_path": self.browser.chrome_path,
                "cdp_port": self.browser.cdp_port,
            },
            "web": {
                // Mask the secret keys (presence only); the SearXNG URL is not a secret.
                "tavily_key_set": !self.web.tavily_api_key.is_empty(),
                "brave_key_set": !self.web.brave_api_key.is_empty(),
                "searxng_url": self.web.searxng_url,
                "travelpayouts_set": !self.web.travelpayouts_token.is_empty(),
            },
            // Mask per-server env VALUES (they hold secrets) - the UI shows which keys are set,
            // never their values, exactly like the provider api_key.
            "mcp": self.mcp.iter().map(|m| json!({
                "name": m.name,
                "command": m.command,
                "args": m.args,
                "cwd": m.cwd,
                "trusted": m.trusted,
                "env": m.env.keys().map(|k| (k.clone(), json!("\u{2022}\u{2022}\u{2022}"))).collect::<serde_json::Map<_,_>>(),
            })).collect::<Vec<_>>(),
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

/// The default endpoint for a known OpenAI-compatible backend kind. Empty for an unknown kind
/// (the caller then falls back to the mock), so a typo can never silently hit the wrong host.
#[cfg(feature = "http")]
fn default_base(kind: &str) -> String {
    match kind {
        "openai" => "https://api.openai.com/v1",
        "openrouter" => "https://openrouter.ai/api/v1",
        "ollama" => "http://localhost:11434/v1",
        "groq" => "https://api.groq.com/openai/v1",
        "deepseek" => "https://api.deepseek.com",
        "mistral" => "https://api.mistral.ai/v1",
        "together" => "https://api.together.xyz/v1",
        "xai" => "https://api.x.ai/v1",
        "perplexity" => "https://api.perplexity.ai",
        // Google's OpenAI-compatible Gemini endpoint.
        "gemini" => "https://generativelanguage.googleapis.com/v1beta/openai",
        "lmstudio" => "http://localhost:1234/v1",
        "vllm" => "http://localhost:8000/v1",
        "llamacpp" => "http://localhost:8080/v1",
        _ => "",
    }
    .into()
}

/// Local OpenAI-compatible servers that need no API key (the client still wants a placeholder).
#[cfg(feature = "http")]
fn is_local_backend(kind: &str) -> bool {
    matches!(kind, "ollama" | "lmstudio" | "vllm" | "llamacpp")
}

/// Build a provider from explicit (kind, base_url, api_key) - shared by the global config and the
/// per-agent provider routing (so a team can mix backends/models by task complexity). Falls back
/// to the offline mock for an unknown kind or a missing required key, never a wrong-host call.
pub fn build_provider_from(
    kind: &str,
    base_url: &str,
    api_key: &str,
    media: &MediaCfg,
) -> Box<dyn engram_gateway::Provider> {
    #[cfg(feature = "http")]
    {
        match kind {
            "mock" | "" => {}
            "anthropic" => {
                if !api_key.is_empty() {
                    return Box::new(engram_gateway::AnthropicProvider::new(
                        base_url.to_string(),
                        api_key.to_string(),
                    ));
                }
            }
            kind => {
                let base = if base_url.is_empty() {
                    default_base(kind)
                } else {
                    base_url.to_string()
                };
                if !base.is_empty() {
                    let key = if api_key.is_empty() && is_local_backend(kind) {
                        "local".to_string()
                    } else {
                        api_key.to_string()
                    };
                    return Box::new(
                        engram_gateway::HttpProvider::new(kind.to_string(), base, key).with_media(
                            media.image_model.clone(),
                            media.tts_model.clone(),
                            media.stt_model.clone(),
                        ),
                    );
                }
            }
        }
    }
    #[cfg(not(feature = "http"))]
    {
        let _ = (kind, base_url, api_key, media);
    }
    Box::new(engram_gateway::MockProvider)
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

// --- Local secret store for the API key --------------------------------------------------
// The key survives restarts without ever entering config.json (so configs stay shareable).
// With the `keyring` feature it lives in the OS secret store (Keychain / libsecret / Credential
// Manager); otherwise it lives in a 0600 file beside the Ed25519 signing key - the same posture
// the signing key already uses, so this is not a new disk-exposure surface.

fn secret_path(home: &str) -> PathBuf {
    Path::new(home).join("secret.key")
}

#[cfg(feature = "keyring")]
fn keyring_entry(home: &str) -> Option<keyring::Entry> {
    // Scope the entry to this ENGRAM_HOME so multiple profiles don't collide.
    keyring::Entry::new("engram", &format!("provider_api_key:{home}")).ok()
}

/// Whether to use the OS keyring for the API key. **Opt-in** (default off): the 0600 `secret.key`
/// file is the default store because it never pops a macOS Keychain authorization prompt. The
/// adhoc-signed desktop app can't persist a Keychain ACL ("Always Allow"), so the keyring otherwise
/// re-prompts for the login password on EVERY launch. Power users can set `ENGRAM_USE_KEYCHAIN=1`.
#[cfg(feature = "keyring")]
fn use_keychain() -> bool {
    std::env::var("ENGRAM_USE_KEYCHAIN")
        .map(|v| !v.is_empty() && v != "0")
        .unwrap_or(false)
}

/// Read the persisted API key. The 0600 file is the default store and is read FIRST — it's
/// prompt-free. Only if the file is empty do we consult the OS keyring (an existing/opt-in install);
/// when found outside keychain mode we migrate it to the file so future launches never touch the
/// Keychain again (the one-time prompt for that read happens OFF the startup path).
pub(crate) fn read_secret_key(home: &str) -> Option<String> {
    if let Some(k) = std::fs::read_to_string(secret_path(home))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    {
        return Some(k);
    }
    #[cfg(feature = "keyring")]
    {
        if let Some(entry) = keyring_entry(home) {
            if let Ok(k) = entry.get_password() {
                if !k.is_empty() {
                    // Migrate to the prompt-free file unless the user explicitly wants the keyring.
                    if !use_keychain() {
                        let _ = write_owner_only(&secret_path(home), k.as_bytes());
                    }
                    return Some(k);
                }
            }
        }
    }
    None
}

/// Persist (non-empty) or clear (empty) the API key. Default: the 0600 file (no Keychain prompt,
/// ever). Only with `ENGRAM_USE_KEYCHAIN=1` does it go to the OS keyring. Best-effort.
fn write_secret_key(home: &str, key: &str) {
    #[cfg(feature = "keyring")]
    if use_keychain() {
        if let Some(entry) = keyring_entry(home) {
            if key.is_empty() {
                let _ = entry.delete_credential();
                let _ = std::fs::remove_file(secret_path(home));
                return;
            }
            match entry.set_password(key) {
                Ok(()) => {
                    let _ = std::fs::remove_file(secret_path(home));
                    return;
                }
                Err(e) => {
                    tracing::warn!(error = %e, "OS keyring write failed - falling back to the 0600 secret file");
                }
            }
        }
    }
    let path = secret_path(home);
    if key.is_empty() {
        let _ = std::fs::remove_file(&path);
    } else {
        // Create owner-only ATOMICALLY (no fs::write-then-chmod TOCTOU window where the secret is
        // briefly group/other-readable).
        let _ = write_owner_only(&path, key.as_bytes());
    }
}

/// Write `bytes` to `path`, truncating, with mode 0600 set at creation time (Unix) so the file is
/// never momentarily world/group-readable. On non-Unix this is a plain create+write.
fn write_owner_only(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    #[cfg(unix)]
    let mut f = {
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?
    };
    #[cfg(not(unix))]
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)?;
    f.write_all(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_backend_resolves_to_the_policy_string() {
        // Host (empty / anything not docker|ssh) => no isolation.
        assert_eq!(resolve_shell_backend("", ""), None);
        assert_eq!(resolve_shell_backend("local", "whatever"), None);
        // Docker => the image (default alpine when blank).
        assert_eq!(resolve_shell_backend("docker", ""), Some("alpine".into()));
        assert_eq!(
            resolve_shell_backend("docker", "ubuntu:24.04"),
            Some("ubuntu:24.04".into())
        );
        // SSH => ssh:<host>, but only when a host is given (a blank target can't be a sandbox).
        assert_eq!(
            resolve_shell_backend("ssh", "deploy@10.0.0.5"),
            Some("ssh:deploy@10.0.0.5".into())
        );
        assert_eq!(resolve_shell_backend("ssh", ""), None);
        // Whitespace is trimmed so a stray space doesn't change the meaning.
        assert_eq!(
            resolve_shell_backend("  docker ", "  alpine "),
            Some("alpine".into())
        );
    }

    fn tmphome() -> String {
        // Atomic counter (not just nanos) so parallel tests never share a dir under load.
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let n = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let d = std::env::temp_dir().join(format!(
            "engram-config-test-{n}-{}",
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
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
    fn api_key_stays_out_of_config_but_survives_restart_via_secret_store() {
        let _lock = ENV_LOCK.lock().unwrap();
        // Isolate from any real key the dev shell exports, so the only source is the secret store.
        let _g1 = EnvVarGuard::unset("ENGRAM_ANTHROPIC_API_KEY");
        let _g2 = EnvVarGuard::unset("ENGRAM_LLM_API_KEY");
        let _g3 = EnvVarGuard::unset("ANTHROPIC_API_KEY");
        let home = tmphome();
        let mut c = Config::default();
        c.provider.kind = "anthropic".into();
        c.provider.api_key = "sk-ant-SUPERSECRET".into();
        c.save(&home).unwrap();
        // The key must NEVER be in config.json (so configs stay shareable); other fields persist.
        let on_disk = std::fs::read_to_string(Config::path(&home)).unwrap();
        assert!(
            !on_disk.contains("SUPERSECRET"),
            "api_key must not be written to config.json"
        );
        assert!(
            on_disk.contains("anthropic"),
            "non-secret fields should persist"
        );
        // It MUST come back on reload from the local secret store - the fix for "key lost on
        // restart". (With the default build that's the 0600 secret.key file; verify its mode.)
        let reloaded = Config::load(&home);
        assert_eq!(
            reloaded.provider.api_key, "sk-ant-SUPERSECRET",
            "key must survive a restart"
        );
        assert_eq!(reloaded.provider.kind, "anthropic");
        #[cfg(all(unix, not(feature = "keyring")))]
        {
            use std::os::unix::fs::PermissionsExt;
            let p = std::path::Path::new(&home).join("secret.key");
            let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "the secret file must be owner-only");
        }
        // Clearing the key wipes it from the store (no lingering secret on disk).
        let mut cleared = Config::load(&home);
        cleared.provider.api_key = String::new();
        cleared.save(&home).unwrap();
        assert!(
            Config::load(&home).provider.api_key.is_empty(),
            "cleared key must not resurrect"
        );
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
        assert_eq!(
            c.provider.kind, "anthropic",
            "standard key must select the anthropic provider"
        );
        assert_eq!(
            c.provider.api_key, "sk-ant-env-adopt-test",
            "standard key must be adopted"
        );
        // base_url stays empty so the provider uses its correct https://api.anthropic.com/v1 default.
        assert!(
            c.provider.base_url.is_empty(),
            "must not adopt the raw host ANTHROPIC_BASE_URL"
        );
        assert_eq!(
            c.model(),
            "claude-haiku-4-5",
            "default model must be a real Anthropic id"
        );
        std::fs::remove_dir_all(&home).ok();
    }

    // The provider router is the connectivity surface: every known OpenAI-compatible kind must
    // build a live HTTP provider (id == kind), and an unknown kind must fall back to the mock
    // rather than silently posting to the wrong host. Only meaningful with the `http` feature.
    #[cfg(feature = "http")]
    #[test]
    fn build_provider_routes_known_kinds_and_falls_back_safely() {
        let mk = |kind: &str, key: &str| {
            let mut c = Config::default();
            c.provider.kind = kind.into();
            c.provider.api_key = key.into();
            c.build_provider().id().to_string()
        };
        // Cloud OpenAI-compatible backends (with a key) route to their own id.
        for kind in [
            "openai",
            "openrouter",
            "groq",
            "deepseek",
            "mistral",
            "together",
            "xai",
            "perplexity",
            "gemini",
        ] {
            assert_eq!(
                mk(kind, "sk-test"),
                kind,
                "{kind} must build a live HTTP provider"
            );
        }
        // Local backends need no key.
        for kind in ["ollama", "lmstudio", "vllm", "llamacpp"] {
            assert_eq!(mk(kind, ""), kind, "{kind} must build without a key");
        }
        // Anthropic with a key uses its native transport.
        assert_eq!(mk("anthropic", "sk-ant-x"), "anthropic");
        // Anthropic with NO key, an unknown kind, and the mock all fall back to the mock - never
        // a wrong-host call.
        assert_eq!(
            mk("anthropic", ""),
            "mock",
            "anthropic without a key must not go live"
        );
        assert_eq!(
            mk("totally-unknown", "k"),
            "mock",
            "an unknown kind must fall back to mock"
        );
        assert_eq!(mk("mock", ""), "mock");
    }
}
