//! The tool abstraction and registry.
//!
//! A tool is a named, JSON-Schema-described action the model can call. The registry
//! collects their schemas for the model and dispatches calls back to them — the same
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

/// What a tool may rely on at call time.
#[derive(Clone)]
pub struct ToolCtx {
    pub memory: Arc<Memory>,
    pub skills: Arc<Registry>,
    pub gateway: Arc<Gateway>,
    pub ledger: Arc<Ledger>,
    /// The run's current taint. `Untrusted` once any tool has read attacker-influenceable
    /// content (e.g. a web page) — dangerous tools refuse to act under it.
    pub taint: Taint,
    pub policy: Policy,
    /// Filesystem actions are confined to this directory.
    pub workdir: PathBuf,
    /// The model id sub-agents inherit when delegating.
    pub model: String,
    /// Delegation depth, to bound recursive subagents.
    pub depth: usize,
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
}

impl Default for Policy {
    fn default() -> Self {
        Policy { allow_shell: false, allow_write: true, max_obs_len: 6000, timeout_secs: 30 }
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
    Ok(out)
}
