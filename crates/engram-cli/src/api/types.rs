//! Wire types for the engramd HTTP API.
//!
//! These mirror the JSON the daemon actually emits (verified against a live
//! `engramd` on `127.0.0.1:8088`). Everything is deliberately tolerant — most
//! fields are `Option` or `#[serde(default)]` and unknown fields are ignored —
//! so a slightly older or newer daemon never makes the client fall over.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// `GET /health`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Health {
    #[serde(default)]
    pub ok: bool,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub offline: bool,
}

/// `GET /v1/meter`
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Meter {
    #[serde(default)]
    pub tokens_in: u64,
    #[serde(default)]
    pub tokens_out: u64,
    #[serde(default)]
    pub cost_usd: f64,
    #[serde(default)]
    pub calls: u64,
}

/// `GET /v1/ledger/verify`
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LedgerVerify {
    #[serde(default)]
    pub ok: bool,
    #[serde(default)]
    pub entries: u64,
    /// Present only when verification fails.
    #[serde(default)]
    pub bad_seq: Option<u64>,
}

/// `GET /v1/memory/stats`
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MemoryStats {
    #[serde(default)]
    pub total: u64,
    #[serde(default)]
    pub by_region: BTreeMap<String, u64>,
    #[serde(default)]
    pub by_tier: BTreeMap<String, u64>,
}

/// One row from `GET /v1/memory/recent` (and the shape inside recall hits).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MemRecord {
    #[serde(default)]
    pub id: i64,
    #[serde(default)]
    pub region: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub importance: f32,
    #[serde(default)]
    pub tier: String,
    #[serde(default)]
    pub taint: String,
    #[serde(default)]
    pub source: Option<Value>,
    #[serde(default)]
    pub created_ms: i64,
    #[serde(default)]
    pub last_access_ms: i64,
    #[serde(default)]
    pub access_count: u64,
    #[serde(default)]
    pub ledger_seq: u64,
    /// Present on a grounded-reflection fact: `{"reflection": true, "source_ids": [...],
    /// "source_seqs": [...]}`. Null/absent for an ordinary, directly-witnessed fact.
    #[serde(default)]
    pub metadata: Value,
}

/// One row from `GET /v1/supersessions` - a detected-but-unconfirmed contradiction, never applied
/// until a human accepts or rejects it via `POST /v1/supersessions/{id}/resolve`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PendingSupersession {
    #[serde(default)]
    pub id: i64,
    #[serde(default)]
    pub old_id: i64,
    #[serde(default)]
    pub candidate_text: String,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub region: String,
    #[serde(default)]
    pub scope_kind: String,
    #[serde(default)]
    pub scope_id: String,
    #[serde(default)]
    pub actor: String,
    #[serde(default)]
    pub created_ms: i64,
}

/// One recall hit from `GET /v1/recall`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RecallHit {
    #[serde(default)]
    pub record: MemRecord,
    #[serde(default)]
    pub score: f32,
    /// Rank in the keyword (BM25/FTS5) arm, if it carried the hit.
    #[serde(default)]
    pub keyword_rank: Option<u32>,
    /// Rank in the semantic (vector cosine) arm, if it carried the hit.
    #[serde(default)]
    pub semantic_rank: Option<u32>,
}

impl RecallHit {
    /// A short label of which arm(s) surfaced this hit, for the "why" ribbon.
    pub fn arm(&self) -> &'static str {
        match (self.keyword_rank.is_some(), self.semantic_rank.is_some()) {
            (true, true) => "both",
            (true, false) => "keyword",
            (false, true) => "semantic",
            (false, false) => "—",
        }
    }
}

/// A grounding memory surfaced under a chat answer.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct RecalledRef {
    #[serde(default)]
    pub id: i64,
    #[serde(default)]
    pub region: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub score: f32,
}

/// One step in an agent run (the glass-box trace unit).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StepRecord {
    #[serde(default)]
    pub index: usize,
    #[serde(default)]
    pub tool: String,
    #[serde(default)]
    pub args: Value,
    #[serde(default)]
    pub observation: String,
    #[serde(default)]
    pub ok: bool,
    #[serde(default)]
    pub ledger_seq: u64,
    #[serde(default)]
    pub ledger_hash: String,
}

/// The receipt a finished run carries (`task.run`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TaskRun {
    #[serde(default)]
    pub answer: String,
    #[serde(default)]
    pub steps: Vec<StepRecord>,
    #[serde(default)]
    pub stopped: String,
    #[serde(default)]
    pub tokens_in: u64,
    #[serde(default)]
    pub tokens_out: u64,
    #[serde(default)]
    pub cost_usd: f64,
    #[serde(default)]
    pub ledger_head_hash: String,
    #[serde(default)]
    pub started_ms: i64,
    #[serde(default)]
    pub finished_ms: i64,
    #[serde(default)]
    pub output_files: Vec<String>,
}

/// `GET /v1/tasks` element.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Task {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub detail: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub origin: String,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub progress: Option<String>,
    #[serde(default)]
    pub created_ms: i64,
    #[serde(default)]
    pub updated_ms: i64,
    #[serde(default)]
    pub handoffs: Vec<Value>,
    #[serde(default)]
    pub run: Option<TaskRun>,
}

impl Task {
    /// Normalised status, defaulting to `todo` when the field is absent.
    pub fn status_or_todo(&self) -> &str {
        if self.status.is_empty() {
            "todo"
        } else {
            &self.status
        }
    }
}

/// `GET /v1/skills` element.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Skill {
    #[serde(default)]
    pub id: String,
    /// The active version — the daemon sends an explicit `null` for a PROPOSED
    /// (distilled-but-not-adopted) skill, so this must be an Option: a bare u64
    /// made the whole `/v1/skills` payload fail to decode the moment one
    /// proposal existed, which blanked the entire skills list client-side.
    #[serde(default)]
    pub active: Option<u64>,
    #[serde(default)]
    pub versions: Vec<u64>,
    #[serde(default)]
    pub runs: u64,
    #[serde(default)]
    pub runtime: String,
    #[serde(default)]
    pub interpreter: Option<String>,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub when_to_use: Option<String>,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub enabled: bool,
    /// Distilled/authored but not yet activated — adoptable via `/adopt`.
    #[serde(default)]
    pub proposed: bool,
    #[serde(default)]
    pub learn: Vec<Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SkillsResp {
    #[serde(default)]
    pub skills: Vec<Skill>,
}

/// `POST /v1/skills/{id}/run`
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SkillRun {
    #[serde(default)]
    pub output: String,
    #[serde(default)]
    pub fuel_used: u64,
    #[serde(default)]
    pub host_calls: u64,
    #[serde(default)]
    pub duration_us: u64,
    #[serde(default)]
    pub logs: Vec<String>,
}

/// One line of the distilled self-model (`GET /v1/consciousness`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConsciousnessLine {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub region: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub source: Value,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Consciousness {
    #[serde(default)]
    pub version: u64,
    #[serde(default)]
    pub distilled_at_ms: i64,
    #[serde(default)]
    pub lines: Vec<ConsciousnessLine>,
}

/// `GET /v1/schedule` element.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ScheduleJob {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub created_ms: i64,
    #[serde(default)]
    pub last_fire_ms: Option<i64>,
    #[serde(default)]
    pub next_fire_ms: Option<i64>,
    #[serde(default)]
    pub last_task_id: Option<String>,
    #[serde(default)]
    pub payload: Value,
    #[serde(default)]
    pub recurrence: Value,
}

/// `POST /v1/schedule/preview`
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SchedulePreview {
    #[serde(default)]
    pub ok: bool,
    #[serde(default)]
    pub next_fire_ms: Option<i64>,
    #[serde(default)]
    pub recurrence: Value,
    #[serde(default)]
    pub error: Option<String>,
}

/// `GET /v1/ledger/tail` element.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LedgerEntry {
    #[serde(default)]
    pub seq: u64,
    #[serde(default)]
    pub ts_ms: i64,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub actor: String,
    #[serde(default)]
    pub hash: String,
    #[serde(default)]
    pub prev: String,
    #[serde(default)]
    pub payload: Value,
}

/// `GET /v1/autonomy/report`
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AutonomyReport {
    #[serde(default)]
    pub totals: AutonomyTotals,
    #[serde(default)]
    pub one_time_approvals: u64,
    #[serde(default)]
    pub scopes: Vec<Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AutonomyTotals {
    #[serde(default)]
    pub autonomous_sends: u64,
    #[serde(default)]
    pub staged: u64,
    #[serde(default)]
    pub refused: u64,
    #[serde(default)]
    pub allowlisted: u64,
    #[serde(default)]
    pub denied: u64,
}

/// `GET /v1/egress/pending`
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EgressPending {
    #[serde(default)]
    pub pending: Vec<EgressItem>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EgressItem {
    #[serde(default)]
    pub scope: String,
    #[serde(default)]
    pub dest: String,
    #[serde(default)]
    pub tool: String,
    #[serde(default)]
    pub reason: String,
}

/// `GET /v1/tools` element.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolInfo {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub disabled: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolsResp {
    #[serde(default)]
    pub tools: Vec<ToolInfo>,
    #[serde(default)]
    pub disable_skill_author: bool,
}

/// `GET /v1/sessions` element.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionMeta {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub project_id: String,
    #[serde(default)]
    pub fav: bool,
    #[serde(default)]
    pub messages: u64,
    #[serde(default)]
    pub created_ms: i64,
    #[serde(default)]
    pub updated_ms: i64,
}

/// One message inside `GET /v1/sessions/{id}`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionMsg {
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub recalled: Vec<String>,
    #[serde(default)]
    pub recalled_refs: Vec<RecalledRef>,
    #[serde(default)]
    pub learned: Vec<String>,
    #[serde(default)]
    pub ts_ms: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionDetail {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub messages: Vec<SessionMsg>,
}

/// `GET /v1/projects` element.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Project {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub persona: String,
    #[serde(default)]
    pub created_ms: i64,
    /// The working directory the project's agent operates in (None = shared daemon workdir).
    #[serde(default)]
    pub workdir: Option<String>,
}

/// The slice of `GET /v1/config` the client surfaces. The full object is also
/// kept verbatim as a [`Value`] for views that want to render everything.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub provider_id: String,
    #[serde(default)]
    pub model_in_use: String,
    #[serde(default)]
    pub http_enabled: bool,
    #[serde(default)]
    pub browser_enabled: bool,
    #[serde(default)]
    pub keyring_enabled: bool,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub provider: Value,
    #[serde(default)]
    pub security: Value,
    #[serde(default)]
    pub web: Value,
    #[serde(default)]
    pub embed: Value,
    #[serde(default)]
    pub channels: Value,
    #[serde(default)]
    pub cost: Value,
    #[serde(default)]
    pub media: Value,
    #[serde(default)]
    pub mcp: Value,
}

/// The terminal `done` payload of a streamed chat turn.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConverseDone {
    #[serde(default)]
    pub reply: String,
    #[serde(default)]
    pub recalled: Vec<String>,
    #[serde(default)]
    pub recalled_refs: Vec<RecalledRef>,
    #[serde(default)]
    pub learned: Vec<String>,
    #[serde(default)]
    pub steps: Vec<StepRecord>,
}

/// `POST /v1/agent` one-shot response.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentResp {
    #[serde(default)]
    pub answer: String,
    #[serde(default)]
    pub stopped: String,
    #[serde(default)]
    pub steps: Vec<StepRecord>,
    #[serde(default)]
    pub tokens_in: u64,
    #[serde(default)]
    pub tokens_out: u64,
    #[serde(default)]
    pub cost_usd: f64,
}
