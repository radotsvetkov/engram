//! The tool abstraction and registry.
//!
//! A tool is a named, JSON-Schema-described action the model can call. The registry
//! collects their schemas for the model and dispatches calls back to them - the same
//! shape as any central tool registry, but with Engram's guarantees built in:
//! every call is auditable, and dangerous tools are **taint-gated** so an action a
//! skill/agent took *after reading untrusted content* can be refused by construction.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use engram_core::{AutonomyPolicy, Ledger, ScopeCtx, Taint};
use engram_gateway::{Gateway, ToolDef};

use crate::agent::{NarrationCallback, StepCallback};
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
    /// The memory rings this run reads and writes (user ∪ project ∪ session). The `memory_recall`
    /// and `memory_remember` tools honour this so a delegated worker in a project chat can't read
    /// or write another project's memory. Propagated into subagents via the cloned ctx.
    pub scope: ScopeCtx,
    /// ORCHESTRATION PARITY (delegated subagents). The parent run's kill switch, shared token budget,
    /// live callbacks, and tool scope — carried on the ctx so a subagent that `delegate_task` builds
    /// inherits them via the cloned ctx (the sub-`Agent` is constructed from the ctx alone, so these
    /// can't be threaded as builder args). `Agent::run` seeds any of these that are `None` from the
    /// top-level Agent's own builder-set values at run entry, so the daemon keeps using the builders
    /// and the values still flow down into delegated work. Defaulting all of them to "unset" keeps
    /// every existing `ToolCtx` literal valid and behaviourally identical when delegation isn't used.
    ///
    /// The parent's kill switch: a delegated run checks it at each step boundary and stops when set,
    /// so cancelling the parent cancels its subagents too.
    pub halt: Option<Arc<std::sync::atomic::AtomicBool>>,
    /// The shared, run-wide token-spend pool (in+out tokens). One `Arc` for the whole run tree, so a
    /// delegated subagent's model calls count against the SAME budget the parent is bounded by (fan-out
    /// can't escape the cost guard). `Agent::run` uses this when present instead of a fresh per-run
    /// counter. `None` = the run makes its own local counter (unshared).
    pub spend_counter: Option<Arc<std::sync::atomic::AtomicU64>>,
    /// The run-wide token budget ceiling. Propagated so a delegated subagent honours the same ceiling
    /// (checked against the shared `spend_counter`). `None` = unbounded (still bounded by max_steps).
    pub token_budget: Option<u32>,
    /// Live progress callback (`Agent::on_step`), carried so a delegated subagent's steps are visible
    /// in the UI just like the parent's instead of running invisibly.
    pub on_step: Option<StepCallback>,
    /// Live narration callback (`Agent::on_narration`), carried so a delegated subagent's interim
    /// commentary is surfaced in the UI like the parent's.
    pub on_narration: Option<NarrationCallback>,
    /// The parent run's per-agent tool scope (the daemon's `allowed_tools`). A subagent's toolset is
    /// INTERSECTED with this in `delegate_task`, so a delegated worker can never exceed the tool
    /// permissions the parent was restricted to. `None` = no scope (the parent could use every tool),
    /// so the subagent gets the full base toolset. The daemon seeds this on the top-level ctx.
    pub allowed_tools: Option<Vec<String>>,
    /// When a durable named agent drives this run, the memory-actor tag it writes as
    /// (`agent:<name>`), letting `memory_remember` attribute a fact to the specific agent instead
    /// of the generic literal `"agent"`, so a per-agent consciousness slice can later filter to
    /// what THIS agent learned. `None` = no named agent (today's behaviour, tagged plain
    /// `"agent"`). Propagates to delegated subagents via the cloned ctx, so a worker a named
    /// agent spawns still attributes to it.
    pub agent_actor: Option<String>,
}

/// What the agent is permitted to do. Safe by default.
#[derive(Clone, Debug)]
pub struct Policy {
    /// Allow the shell tool at all (off unless explicitly enabled). Also gates *running* process
    /// skills, which execute through the same shell backend.
    pub allow_shell: bool,
    /// Allow the agent to author/improve skills (install signed skill code). On by default — this is
    /// what lets skills "pop up" from real use. Authoring is still refused on a tainted run and the
    /// declared capabilities are clamped to what the run is allowed.
    pub allow_skill_author: bool,
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
    /// EXPLICIT user approval to perform egress even on an untrusted+sensitive ("trifecta") run — the
    /// escape valve that keeps the gate from collapsing into "refuses everything". A human approves a
    /// specific blocked action and the daemon resumes the run with this set. CRITICAL boundary: ONLY
    /// the daemon sets this, and only after a real user approval; the model can *request* approval
    /// (`request_approval` tool) but can never grant its own. Every approved egress is ledgered.
    pub approved: bool,
    /// When set, scopes `approved` to a SPECIFIC destination host (the one the user actually approved),
    /// so a single "Approve once" click can't be laundered by injected content into blanket egress to
    /// any host for the rest of the run. `None` = the approval is unscoped (legacy run-wide behaviour);
    /// the hardline floor is still evaluated first either way, so an approved run can never reach a
    /// floor-listed destination. The daemon SHOULD populate this with the host of the refused action it
    /// is resuming, so the escape valve authorizes exactly that destination and nothing more.
    pub approved_dest: Option<String>,
    /// Whether a human is watching THIS run (interactive UI/HTTP) vs scheduled/unattended. Decides
    /// what happens to an egress action that isn't pre-authorized: attended → the live approval
    /// prompt (today's behaviour); unattended → stage it for async review, never block the run.
    pub attended: bool,
    /// A signed standing AUTONOMY policy for this run (from the durable agent / scheduled job). When
    /// present, the egress gate consults it instead of demanding a live human approval — allowlisted
    /// destinations proceed within budget, everything else stages. Loaded + verified out-of-band at
    /// run construction; never settable by the model or tainted content (the bypass is frozen).
    pub autonomy: Option<AutonomyPolicy>,
    /// Shared, monotonic count of egress actions consumed this run — the live half of the policy's
    /// budget. An `Arc` so delegated sub-agents share ONE pool (fan-out can't escape the budget).
    pub egress_consumed: std::sync::Arc<std::sync::atomic::AtomicU32>,
    /// Daemon-global egress allowlist (destination hosts the user approved for policy-less "default
    /// agent" runs). Consulted by the egress gate before staging a novel destination, so a run with no
    /// signed `autonomy` policy can still send to a user-approved host. A per-agent signed policy and
    /// its hardline floor are evaluated FIRST, so this can never override a floor.
    pub daemon_allowlist: Vec<String>,
}

impl Default for Policy {
    fn default() -> Self {
        Policy {
            allow_shell: false,
            allow_skill_author: true,
            allow_write: true,
            max_obs_len: 6000,
            timeout_secs: 30,
            shell_backend: None,
            dry_run: false,
            vision_model: None,
            webhook_url: None,
            approved: false,
            approved_dest: None,
            // Interactive is the safe default; the scheduler explicitly marks unattended runs.
            attended: true,
            autonomy: None,
            egress_consumed: std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0)),
            daemon_allowlist: Vec::new(),
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
    /// The destination this egress tool will ACTUALLY contact for `args`, resolved from the tool's
    /// OWN schema/precedence — a bare host for a URL, or a recipient string. The egress gate matches
    /// this against the autonomy allowlist/floor, so it must reflect where the tool truly sends, not
    /// whatever keys happen to appear in the model-authored JSON. Returning `None` means "opaque":
    /// the destination cannot be verified, so the gate must never auto-allow it (it stages/refuses).
    /// The DEFAULT is `None`, so any egress tool that doesn't override this (e.g. an MCP tool whose
    /// real recipient arg we can't know) is treated as opaque — fail-closed, which is the safe side.
    /// Takes `ctx` so a tool with a configured default (e.g. `send_message`'s webhook) can resolve
    /// the effective destination even when the call omits an explicit one.
    fn egress_dest(&self, _args: &Value, _ctx: &ToolCtx) -> Option<String> {
        None
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
    /// Keep only the tools whose name satisfies `keep`. Used at the run chokepoint to apply per-user
    /// (`disabled_tools`) and per-agent (`allowed_tools`) curation: a dropped tool isn't advertised
    /// to the model, so it simply isn't available that run. The effective set is recorded so the
    /// curation decision is auditable.
    pub fn retaining(mut self, keep: impl Fn(&str) -> bool) -> Self {
        self.tools.retain(|t| keep(t.name()));
        self
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
