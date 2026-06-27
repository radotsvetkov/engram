//! Durable, named, role-scoped agents - the auditable team (the Hermes model).
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
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

fn uid() -> String {
    // nanos alone can collide if two creates land in the same clock tick (coarse SystemTime, or a
    // double-submit). A process-global monotonic counter makes the id unique regardless of clock
    // granularity; the nanos prefix keeps ids distinct across daemon restarts.
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("ag{nanos:x}{n:x}")
}

/// A durable agent definition.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentDef {
    pub id: String,
    pub name: String,
    /// The role / system prompt - the specialization that narrows focus (less drift, less
    /// hallucination) and leads the standing context when this agent runs a task.
    #[serde(default)]
    pub role: String,
    /// The model this agent uses. Empty = the global default (right model per task).
    #[serde(default)]
    pub model: String,
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
        let agents = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<Vec<AgentDef>>(&s).ok())
            .unwrap_or_default();
        Self { path, agents: Mutex::new(agents) }
    }

    pub fn list(&self) -> Vec<AgentDef> {
        self.agents.lock().expect("agents lock").clone()
    }

    pub fn get(&self, id: &str) -> Option<AgentDef> {
        self.agents.lock().expect("agents lock").iter().find(|a| a.id == id).cloned()
    }

    pub fn create(&self, name: &str, role: &str, model: &str) -> AgentDef {
        let now = now_ms();
        let def = AgentDef {
            id: uid(),
            name: name.trim().to_string(),
            role: role.trim().to_string(),
            model: model.trim().to_string(),
            created_ms: now,
            updated_ms: now,
        };
        let mut g = self.agents.lock().expect("agents lock");
        g.push(def.clone());
        self.persist(&g);
        def
    }

    /// Update fields by id (each `Some` is applied). Returns the updated agent.
    pub fn update(
        &self,
        id: &str,
        name: Option<&str>,
        role: Option<&str>,
        model: Option<&str>,
    ) -> Option<AgentDef> {
        let mut g = self.agents.lock().expect("agents lock");
        let a = g.iter_mut().find(|a| a.id == id)?;
        if let Some(n) = name {
            a.name = n.trim().to_string();
        }
        if let Some(r) = role {
            a.role = r.trim().to_string();
        }
        if let Some(m) = model {
            a.model = m.trim().to_string();
        }
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
        let Ok(bytes) = serde_json::to_vec_pretty(agents) else { return };
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
