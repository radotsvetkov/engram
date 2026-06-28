//! The tool abstraction and registry.
//!
//! A tool is a named, JSON-Schema-described action the model can call. The registry
//! collects their schemas for the model and dispatches calls back to them - the same
//! shape as Hermes's central tool registry, but with Engram's guarantees bolted in:
//! every call is auditable, and dangerous tools are **taint-gated** so an action a
//! skill/agent took *after reading untrusted content* can be refused by construction.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use engram_core::{Ledger, Taint};
use engram_gateway::{Gateway, ToolDef};
use engram_memory::Memory;
use engram_skills::Registry;
use serde_json::Value;

/// A persistent, interactive browser the agent can drive across tool calls: navigate,
/// click, type, extract, screenshot. The default [`NoBrowser`] errors with guidance; a
/// real Chrome-DevTools-Protocol session is wired in with `--features browser-cdp`.
#[async_trait]
pub trait BrowserSession: Send + Sync {
    async fn open(&self, url: &str) -> Result<String, String>;
    async fn click(&self, selector: &str) -> Result<String, String>;
    async fn type_text(&self, selector: &str, text: &str) -> Result<String, String>;
    async fn extract(&self, selector: Option<&str>) -> Result<String, String>;
    async fn screenshot(&self, path: &std::path::Path) -> Result<(), String>;
    /// Wait until `selector` exists (up to `timeout_ms`). The real CDP session overrides this;
    /// the default reports the feature is off so a SPA's late-rendered element can be awaited.
    async fn wait_for(&self, _selector: &str, _timeout_ms: u64) -> Result<String, String> {
        Err("interactive browser not enabled (build engramd with --features browser-cdp)".into())
    }
    /// Scroll the page by `dy` pixels (negative = up). Overridden by the CDP session.
    async fn scroll(&self, _dy: i64) -> Result<String, String> {
        Err("interactive browser not enabled (build engramd with --features browser-cdp)".into())
    }
}

/// Placeholder used when no interactive browser is built in.
pub struct NoBrowser;

impl NoBrowser {
    fn unavailable() -> String {
        "interactive browser not enabled (build engramd with --features browser-cdp)".into()
    }
}

#[async_trait]
impl BrowserSession for NoBrowser {
    async fn open(&self, _: &str) -> Result<String, String> {
        Err(Self::unavailable())
    }
    async fn click(&self, _: &str) -> Result<String, String> {
        Err(Self::unavailable())
    }
    async fn type_text(&self, _: &str, _: &str) -> Result<String, String> {
        Err(Self::unavailable())
    }
    async fn extract(&self, _: Option<&str>) -> Result<String, String> {
        Err(Self::unavailable())
    }
    async fn screenshot(&self, _: &std::path::Path) -> Result<(), String> {
        Err(Self::unavailable())
    }
}

/// What a tool may rely on at call time.
#[derive(Clone)]
pub struct ToolCtx {
    pub memory: Arc<Memory>,
    pub skills: Arc<Registry>,
    pub gateway: Arc<Gateway>,
    pub ledger: Arc<Ledger>,
    /// The run's current taint. `Untrusted` once any tool has read attacker-influenceable
    /// content (e.g. a web page) - the shell is refused and secret context is stripped under it.
    pub taint: Taint,
    /// The run's second provenance dimension: has it surfaced the user's private data (a memory
    /// recall, a local file read, an authenticated MCP read)? Egress is refused only when the run
    /// is BOTH `taint==Untrusted` AND `sensitive` (the lethal trifecta). Propagated into subagents
    /// via the cloned ctx, so a delegated worker can't launder an exfiltration past the gate.
    pub sensitive: bool,
    pub policy: Policy,
    /// Filesystem actions are confined to this directory.
    pub workdir: PathBuf,
    /// The model id sub-agents inherit when delegating.
    pub model: String,
    /// Delegation depth, to bound recursive subagents.
    pub depth: usize,
    /// The interactive browser session (no-op unless built with `browser-cdp`).
    pub browser: Arc<dyn BrowserSession>,
}

/// What the agent is permitted to do. Safe by default.
#[derive(Clone, Debug)]
pub struct Policy {
    /// Allow the shell tool at all (off unless explicitly enabled).
    pub allow_shell: bool,
    /// Allow writing files within the workdir.
    pub allow_write: bool,
    /// Truncate any single observation to this many bytes before feeding it back.
    pub max_obs_len: usize,
    /// Per-command / per-fetch timeout, seconds.
    pub timeout_secs: u64,
    /// Execution backend for the shell tool: `None` runs locally; `Some(image)` runs in
    /// a network-isolated `docker run` against that image (sandboxed code execution).
    pub shell_backend: Option<String>,
    /// Dry-run / planning-only: side-effecting tools are not executed; the agent is told
    /// what it *would* do, so a plan can be previewed before anything changes.
    pub dry_run: bool,
    /// Model the vision tool uses to read images. `None` = inherit the run's model (or the
    /// ENGRAM_VISION_MODEL env var). Carried on the policy because it's a per-run setting read
    /// from the live config when the run starts.
    pub vision_model: Option<String>,
    /// Default destination for the `send_message` tool when the call omits a `url`. `None` = no
    /// default (the tool then needs an explicit url or the ENGRAM_WEBHOOK_URL env var). The
    /// SSRF guard still validates whatever URL is finally used.
    pub webhook_url: Option<String>,
}

impl Default for Policy {
    fn default() -> Self {
        Policy {
            allow_shell: false,
            allow_write: true,
            max_obs_len: 6000,
            timeout_secs: 30,
            shell_backend: None,
            dry_run: false,
            vision_model: None,
            webhook_url: None,
        }
    }
}

/// One callable action.
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    /// JSON Schema for the arguments object.
    fn schema(&self) -> Value;
    /// Execute. Return the observation text on success, or an error message (which is
    /// also fed back to the model so it can recover).
    async fn run(&self, args: &Value, ctx: &ToolCtx) -> Result<String, String>;
    /// True if running this tool exposes the run to untrusted content, after which the
    /// run is tainted and egress/dangerous tools are revoked.
    fn taints(&self) -> bool {
        false
    }
    /// True if this tool can carry data *out* to an attacker-influenceable destination - a webhook
    /// (`send_message`) or a raw HTTP fetch/search (`web_fetch`/`web_search`). Egress is refused only
    /// when the run is BOTH tainted (read untrusted content) AND sensitive (read private data) - the
    /// lethal-trifecta rule - so pure web research still works while exfiltration of private data is
    /// blocked by construction. The interactive BROWSER is deliberately NOT egress: driving it to
    /// VIEW pages is research/ingress (it's SSRF-guarded and visible), so the agent can recall context
    /// AND browse the web in one run - the alternative blocked all multi-site research after a recall.
    /// Enforced centrally at the agent's dispatch boundary so every tool is covered.
    fn is_egress(&self) -> bool {
        false
    }
    /// True if this tool surfaces the user's *private/sensitive* data into the run (recalling
    /// personal memory, reading local files, an authenticated MCP/service read). Combined with
    /// `taints()`, this is what arms the no-egress gate: untrusted content alone is not enough
    /// to exfiltrate - there must also be something private in the run worth leaking.
    fn reads_sensitive(&self) -> bool {
        false
    }
    /// True if this tool changes the world outside the brain (writes files, runs shell,
    /// sends messages, drives the browser, calls an MCP server). In dry-run/preview mode
    /// these are not executed - the agent is told what it *would* do, so a plan can be
    /// inspected before anything happens. Internal, reversible writes (memory) are not
    /// side-effecting.
    fn side_effecting(&self) -> bool {
        false
    }
}

/// The set of tools available to an agent.
#[derive(Default)]
pub struct ToolRegistry {
    tools: Vec<Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self { tools: Vec::new() }
    }
    pub fn with(mut self, tool: Arc<dyn Tool>) -> Self {
        self.tools.push(tool);
        self
    }
    pub fn get(&self, name: &str) -> Option<&Arc<dyn Tool>> {
        self.tools.iter().find(|t| t.name() == name)
    }
    pub fn names(&self) -> Vec<&str> {
        self.tools.iter().map(|t| t.name()).collect()
    }
    /// Tool schemas to advertise to the model.
    pub fn defs(&self) -> Vec<ToolDef> {
        self.tools
            .iter()
            .map(|t| ToolDef {
                name: t.name().to_string(),
                description: t.description().to_string(),
                parameters: t.schema(),
            })
            .collect()
    }
}

/// Confine `rel` to `workdir`, rejecting escapes. Returns the absolute path.
pub fn confine(workdir: &std::path::Path, rel: &str) -> Result<PathBuf, String> {
    let joined = workdir.join(rel);
    // Normalise without touching the filesystem (the path may not exist yet).
    let mut out = PathBuf::new();
    for comp in joined.components() {
        use std::path::Component::*;
        match comp {
            ParentDir => {
                out.pop();
            }
            CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    if !out.starts_with(workdir) {
        return Err(format!("path '{rel}' escapes the workdir"));
    }
    // Lexical checks miss symlinks: a link *inside* the workdir can still point outside.
    // Canonicalize the deepest existing ancestor of the target and require it to remain
    // within the canonical workdir, so a symlinked escape is rejected.
    let base = std::fs::canonicalize(workdir).unwrap_or_else(|_| workdir.to_path_buf());
    let mut probe = out.as_path();
    let resolved = loop {
        match std::fs::canonicalize(probe) {
            Ok(p) => break Some(p),
            Err(_) => match probe.parent() {
                Some(parent) => probe = parent,
                None => break None,
            },
        }
    };
    if let Some(real) = resolved {
        if !real.starts_with(&base) {
            return Err(format!("path '{rel}' escapes the workdir via a symlink"));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod confine_tests {
    use super::confine;

    #[test]
    fn blocks_parent_escape_allows_inside() {
        let work = tempfile::tempdir().unwrap();
        assert!(confine(work.path(), "../etc/passwd").is_err());
        assert!(confine(work.path(), "notes/today.md").is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn blocks_symlink_escape() {
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.txt"), "top secret").unwrap();
        let work = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), work.path().join("escape")).unwrap();
        // Reading through an in-workdir symlink that points outside must be rejected.
        assert!(confine(work.path(), "escape/secret.txt").is_err());
        // A plain path inside the workdir is still fine.
        assert!(confine(work.path(), "ok.txt").is_ok());
    }
}
