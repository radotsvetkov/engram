//! Durable, named, role-scoped agents - the auditable team.
//!
//! Each agent is a persisted identity: a name, a role (its system prompt / specialization), and
//! optionally its own model. Assign one to a kanban card and, when the card runs, the agent's role
//! and model drive the run AND every signed ledger entry is attributed to the agent (actor = its
//! name) - so a team of agents collaborating on a board is fully auditable, the opposite of an
//! opaque swarm. Narrow roles also cut drift and hallucination. Persisted to `<home>/agents.json`
//! (atomic, owner-only).

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn uid() -> String {
    // nanos alone can collide if two creates land in the same clock tick (coarse SystemTime, or a
    // double-submit). A process-global monotonic counter makes the id unique regardless of clock
    // granularity; the nanos prefix keeps ids distinct across daemon restarts.
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("ag{nanos:x}{n:x}")
}

/// Normalize a reasoning-effort string: only "low"/"medium"/"high" are kept, anything else (incl.
/// "auto"/"") becomes "" (the model default).
fn norm_effort(e: &str) -> String {
    match e.trim() {
        "low" | "medium" | "high" => e.trim().to_string(),
        _ => String::new(),
    }
}

/// A durable agent definition.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentDef {
    pub id: String,
    pub name: String,
    /// This agent's charter: its role / system prompt - the specialization that narrows focus
    /// (less drift, less hallucination) and leads the standing context when this agent runs a
    /// task. Named `charter` (not `persona`/`role` alone) to stay distinct from the project
    /// `brief` and the global SOUL.md persona - a different register (mandate, not voice).
    /// `alias` keeps existing `agents.json` files (written under the old `role` key) loading
    /// correctly.
    #[serde(default, alias = "role")]
    pub charter: String,
    /// The model this agent uses. Empty = the global default (right model per task).
    #[serde(default)]
    pub model: String,
    /// Optional per-agent PROVIDER, so a team can mix backends by task complexity: a cheap/fast
    /// model on one provider for a triage agent, a frontier model on another for the hard reasoning
    /// agent. Empty = use the global provider (model still overrides per agent). When set, the agent
    /// runs through its own provider/base_url/key. The key lives only in the 0600 agents.json (like
    /// the signing key) and is masked in the API.
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub base_url: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub api_key: String,
    /// Reasoning effort for this agent's runs: "" (model default) | "low" | "medium" | "high".
    /// Applied (model-awarely) when the agent runs through its own provider gateway.
    #[serde(default)]
    pub effort: String,
    /// Tool scoping for this agent: `None` = inherit all (minus the global deny-list); `Some(list)`
    /// = restrict this agent to exactly these tool names (a narrow, auditable specialist). Applied at
    /// the run chokepoint on top of the global `disabled_tools`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_tools: Option<Vec<String>>,
    /// This agent's default project scope: when set, a kanban card assigned to this agent recalls
    /// and writes into that project's ring (plus user-global) instead of user-global alone -
    /// giving a non-coding agent (a content writer, a researcher) a home to work in without
    /// needing a working directory. `None` = user-global only (today's behaviour).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home_project: Option<String>,
    /// A signed standing AUTONOMY policy: when set, this agent may egress UNATTENDED within its
    /// allowlist + budget (scheduled runs no longer need a live human to approve each send). Signed
    /// with the skill key when the user authors it; verified at run construction (a forged/unsigned
    /// policy fails closed to default-deny). `None` = no autonomous egress (today's gated behaviour).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub autonomy_policy: Option<engram_core::SignedAutonomyPolicy>,
    /// A user-chosen accent color (hex, e.g. "#45c8a8") — personality in the UI. Empty = default.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub color: String,
    /// A user-chosen emoji/glyph badge for this agent. Empty = the name's initial.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub emoji: String,
    pub created_ms: i64,
    pub updated_ms: i64,
}

pub struct AgentStore {
    path: PathBuf,
    agents: Mutex<Vec<AgentDef>>,
}

impl AgentStore {
    pub fn open(dir: &Path) -> Self {
        let path = dir.join("agents.json");
        // agents.json holds the most security-sensitive durable state (per-agent API keys and signed
        // AutonomyPolicies), so a parse failure must NOT be silently mapped to empty: that would erase
        // every durable agent and its policy on the next boot, and the first save would overwrite the
        // evidence. Mirror TaskStore/WorkspaceStore — back the bad file up to *.corrupt.json and log
        // the cause — so a half-written or hand-edited file is recoverable, not lost.
        let agents = match std::fs::read_to_string(&path) {
            Ok(s) => match serde_json::from_str::<Vec<AgentDef>>(&s) {
                Ok(a) => a,
                Err(e) => {
                    let _ = std::fs::rename(&path, dir.join("agents.corrupt.json"));
                    tracing::error!(error = %e, "agents.json was unparseable - backed it up to agents.corrupt.json and started empty");
                    Vec::new()
                }
            },
            Err(_) => Vec::new(),
        };
        Self {
            path,
            agents: Mutex::new(agents),
        }
    }

    pub fn list(&self) -> Vec<AgentDef> {
        self.agents.lock().expect("agents lock").clone()
    }

    pub fn get(&self, id: &str) -> Option<AgentDef> {
        self.agents
            .lock()
            .expect("agents lock")
            .iter()
            .find(|a| a.id == id)
            .cloned()
    }

    #[allow(clippy::too_many_arguments)]
    pub fn create(
        &self,
        name: &str,
        charter: &str,
        model: &str,
        provider: &str,
        base_url: &str,
        api_key: &str,
        effort: &str,
    ) -> AgentDef {
        let now = now_ms();
        let def = AgentDef {
            id: uid(),
            name: name.trim().to_string(),
            charter: charter.trim().to_string(),
            model: model.trim().to_string(),
            provider: provider.trim().to_string(),
            base_url: base_url.trim().to_string(),
            api_key: api_key.trim().to_string(),
            effort: norm_effort(effort),
            allowed_tools: None,
            home_project: None,
            autonomy_policy: None,
            color: String::new(),
            emoji: String::new(),
            created_ms: now,
            updated_ms: now,
        };
        let mut g = self.agents.lock().expect("agents lock");
        g.push(def.clone());
        self.persist(&g);
        def
    }

    /// Update fields by id (each `Some` is applied). Returns the updated agent. A blank `api_key`
    /// keeps the stored one (same "blank keeps it" rule as the provider key in settings).
    #[allow(clippy::too_many_arguments)]
    pub fn update(
        &self,
        id: &str,
        name: Option<&str>,
        charter: Option<&str>,
        model: Option<&str>,
        provider: Option<&str>,
        base_url: Option<&str>,
        api_key: Option<&str>,
        effort: Option<&str>,
    ) -> Option<AgentDef> {
        let mut g = self.agents.lock().expect("agents lock");
        let a = g.iter_mut().find(|a| a.id == id)?;
        if let Some(n) = name {
            a.name = n.trim().to_string();
        }
        if let Some(c) = charter {
            a.charter = c.trim().to_string();
        }
        if let Some(m) = model {
            a.model = m.trim().to_string();
        }
        if let Some(p) = provider {
            a.provider = p.trim().to_string();
        }
        if let Some(b) = base_url {
            a.base_url = b.trim().to_string();
        }
        if let Some(k) = api_key {
            let k = k.trim();
            if !k.is_empty() {
                a.api_key = k.to_string();
            }
        }
        if let Some(e) = effort {
            a.effort = norm_effort(e);
        }
        a.updated_ms = now_ms();
        let out = a.clone();
        self.persist(&g);
        Some(out)
    }

    /// Set (or clear) an agent's default project scope. `None`/blank clears it back to user-global.
    pub fn set_home_project(&self, id: &str, project: Option<&str>) -> Option<AgentDef> {
        let mut g = self.agents.lock().expect("agents lock");
        let a = g.iter_mut().find(|a| a.id == id)?;
        a.home_project = project
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(str::to_string);
        a.updated_ms = now_ms();
        let out = a.clone();
        self.persist(&g);
        Some(out)
    }

    /// Set (or clear) an agent's tool scope. `None` clears it (inherit all); `Some(list)` restricts.
    pub fn set_allowed_tools(&self, id: &str, tools: Option<Vec<String>>) -> Option<AgentDef> {
        let mut g = self.agents.lock().expect("agents lock");
        let a = g.iter_mut().find(|a| a.id == id)?;
        a.allowed_tools = tools.filter(|t| !t.is_empty());
        a.updated_ms = now_ms();
        let out = a.clone();
        self.persist(&g);
        Some(out)
    }

    /// Set an agent's UI appearance — an accent color and/or emoji badge. `None` leaves a field as-is;
    /// an empty string clears it. Personality, persisted with the agent.
    pub fn set_appearance(
        &self,
        id: &str,
        color: Option<&str>,
        emoji: Option<&str>,
    ) -> Option<AgentDef> {
        let mut g = self.agents.lock().expect("agents lock");
        let a = g.iter_mut().find(|a| a.id == id)?;
        if let Some(c) = color {
            // Keep it to a safe hex/rgb-ish token (no CSS injection into inline styles).
            let c = c.trim();
            a.color = if c.is_empty()
                || (c.len() <= 24
                    && c.chars().all(|ch| {
                        ch.is_ascii_alphanumeric()
                            || matches!(ch, '#' | '(' | ')' | ',' | '.' | '%' | ' ')
                    })) {
                c.to_string()
            } else {
                a.color.clone()
            };
        }
        if let Some(e) = emoji {
            a.emoji = e.trim().chars().take(4).collect();
        }
        a.updated_ms = now_ms();
        let out = a.clone();
        self.persist(&g);
        Some(out)
    }

    /// Set (or clear) an agent's signed standing autonomy policy. `None` revokes it (back to gated /
    /// no autonomous egress). The signed policy is persisted verbatim so it stays verifiable on load.
    pub fn set_autonomy_policy(
        &self,
        id: &str,
        policy: Option<engram_core::SignedAutonomyPolicy>,
    ) -> Option<AgentDef> {
        let mut g = self.agents.lock().expect("agents lock");
        let a = g.iter_mut().find(|a| a.id == id)?;
        a.autonomy_policy = policy;
        a.updated_ms = now_ms();
        let out = a.clone();
        self.persist(&g);
        Some(out)
    }

    pub fn delete(&self, id: &str) -> bool {
        let mut g = self.agents.lock().expect("agents lock");
        let before = g.len();
        g.retain(|a| a.id != id);
        let removed = g.len() != before;
        if removed {
            self.persist(&g);
        }
        removed
    }

    /// Atomic, owner-only write (temp + rename).
    fn persist(&self, agents: &[AgentDef]) {
        let Ok(bytes) = serde_json::to_vec_pretty(agents) else {
            return;
        };
        let tmp = self.path.with_extension("json.tmp");
        if std::fs::write(&tmp, &bytes).is_ok() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
            }
            let _ = std::fs::rename(&tmp, &self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn tmpdir() -> std::path::PathBuf {
        // An atomic counter (not just nanos) so parallel tests never share a dir even if the clock
        // resolution is coarse under load - otherwise two tests cross-contaminate one tasks/agents file.
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let n = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let d = std::env::temp_dir().join(format!(
            "engram-agents-test-{n}-{}",
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn create_get_update_delete() {
        let d = tmpdir();
        let s = AgentStore::open(&d);
        let a = s.create("Scout", "research", "claude-haiku", "", "", "", "");
        assert_eq!(a.name, "Scout");
        assert_eq!(s.list().len(), 1);
        assert_eq!(s.get(&a.id).unwrap().charter, "research");
        // each Some applies; None leaves the field
        let u = s
            .update(
                &a.id,
                Some("Scout2"),
                None,
                Some("opus"),
                None,
                None,
                None,
                None,
            )
            .unwrap();
        assert_eq!(u.name, "Scout2");
        assert_eq!(u.model, "opus");
        assert_eq!(u.charter, "research");
        assert!(s.delete(&a.id));
        assert!(s.list().is_empty());
        assert!(!s.delete(&a.id)); // already gone
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn home_project_sets_and_clears() {
        let d = tmpdir();
        let s = AgentStore::open(&d);
        let a = s.create("Writer", "drafts prose", "", "", "", "", "");
        assert_eq!(a.home_project, None);
        let u = s.set_home_project(&a.id, Some("marketing")).unwrap();
        assert_eq!(u.home_project.as_deref(), Some("marketing"));
        // A blank/None clears it back to user-global.
        let cleared = s.set_home_project(&a.id, Some("  ")).unwrap();
        assert_eq!(cleared.home_project, None);
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn ids_are_unique_in_a_tight_loop() {
        // Guards the uid() fix: nanos alone could collide; the atomic counter must keep ids distinct.
        let d = tmpdir();
        let s = AgentStore::open(&d);
        let mut ids = HashSet::new();
        for i in 0..500 {
            let a = s.create(&format!("a{i}"), "", "", "", "", "", "");
            assert!(ids.insert(a.id), "duplicate agent id generated");
        }
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn persists_and_reloads() {
        let d = tmpdir();
        {
            let s = AgentStore::open(&d);
            s.create("Persisted", "r", "m", "", "", "", "");
        }
        let s2 = AgentStore::open(&d);
        assert_eq!(s2.list().len(), 1);
        assert_eq!(s2.list()[0].name, "Persisted");
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn charter_alias_reads_a_legacy_role_key() {
        // Older agents.json files persisted this field under the key "role" - the alias must keep
        // them loading correctly after the charter rename, or every agent's charter vanishes.
        let d = tmpdir();
        std::fs::write(
            d.join("agents.json"),
            r#"[{"id":"a1","name":"Scout","role":"research","created_ms":1,"updated_ms":1}]"#,
        )
        .unwrap();
        let s = AgentStore::open(&d);
        assert_eq!(s.get("a1").unwrap().charter, "research");
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn corrupt_agents_json_is_backed_up_not_silently_erased() {
        let d = tmpdir();
        // A half-written / hand-edited file that won't parse. The OLD behavior mapped this to an
        // empty store AND then the first save overwrote the evidence — silently destroying every
        // durable agent, its API key, and its signed autonomy policy.
        std::fs::write(d.join("agents.json"), b"{ this is not valid json ][").unwrap();
        let s = AgentStore::open(&d);
        assert!(s.list().is_empty(), "starts empty on an unparseable file");
        // The bad bytes must be preserved for recovery, not discarded.
        assert!(
            d.join("agents.corrupt.json").exists(),
            "the corrupt file must be backed up to agents.corrupt.json"
        );
        std::fs::remove_dir_all(&d).ok();
    }
}
