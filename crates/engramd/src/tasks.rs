//! Tasks - the unit of work behind the Kanban board.
//!
//! A task is something the user wants done, captured from chat or typed directly. It
//! moves through todo → doing → done, can be run by the agent (which attaches the
//! answer and the tools it used), and persists as plain JSON in the brain dir. This is
//! the model the desktop's board, chat input, and scheduler all sit on top of.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use engram_agent::StepRecord;
use engram_core::now_ms;
use serde::{Deserialize, Serialize};

/// A glass-box record of one agent run on a task: the answer, every step verbatim, and
/// the trust/cost facts (tokens, cost, and the signed ledger head pinned at finish).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskRun {
    pub answer: String,
    pub steps: Vec<StepRecord>,
    pub stopped: String,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub cost_usd: f64,
    pub ledger_head_hash: String,
    pub started_ms: i64,
    pub finished_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub detail: String,
    /// "todo" | "doing" | "done" | "failed" | "scheduled".
    pub status: String,
    #[serde(default = "default_origin")]
    pub origin: String,
    #[serde(default)]
    pub tool_tags: Vec<String>,
    #[serde(default)]
    pub schedule_id: Option<String>,
    /// The durable agent (by id) assigned to run this card - the auditable team. None = the
    /// default agent. When set, the run adopts the agent's role + model and signs as that actor.
    #[serde(default)]
    pub agent: Option<String>,
    /// The hand-off trail: each time the card passes between agents, with the note explaining why.
    /// Makes a multi-agent collaboration on one card auditable end to end.
    #[serde(default)]
    pub handoffs: Vec<Handoff>,
    pub created_ms: i64,
    pub updated_ms: i64,
    /// Live progress while running, e.g. "step 3 · web_search".
    #[serde(default)]
    pub progress: Option<String>,
    #[serde(default)]
    pub run: Option<TaskRun>,
}

fn default_origin() -> String {
    "manual".into()
}

/// One pass of a card between agents.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Handoff {
    /// The agent the card came from ("" / "Default" when previously unassigned).
    pub from: String,
    /// The agent it was handed to.
    pub to: String,
    /// Why - the note that makes the collaboration legible.
    #[serde(default)]
    pub note: String,
    pub ts_ms: i64,
}

/// Guess which capabilities a task will use, from its words - drives the card's tags.
fn infer_tags(text: &str) -> Vec<String> {
    let t = text.to_lowercase();
    let mut tags = Vec::new();
    let has = |words: &[&str]| words.iter().any(|w| t.contains(w));
    if has(&[
        "search", "google", "web", "online", "url", "http", "news", "look up",
    ]) {
        tags.push("web".into());
    }
    if has(&[
        "browser",
        "click",
        "navigate",
        "screenshot",
        "page",
        "website",
        "site",
    ]) {
        tags.push("browser".into());
    }
    if has(&[
        "file",
        "write",
        "read",
        "save",
        "folder",
        "directory",
        "csv",
        "pdf",
    ]) {
        tags.push("files".into());
    }
    if has(&[
        "run", "command", "shell", "script", "build", "install", "compile",
    ]) {
        tags.push("shell".into());
    }
    tags
}

pub struct TaskStore {
    path: PathBuf,
    tasks: Mutex<Vec<Task>>,
}

impl TaskStore {
    pub fn open(dir: &Path) -> Self {
        let path = dir.join("tasks.json");
        // Back up an unparseable file rather than silently overwriting it with an empty default.
        let tasks = match std::fs::read(&path) {
            Ok(b) => match serde_json::from_slice(&b) {
                Ok(t) => t,
                Err(e) => {
                    let _ = std::fs::rename(&path, dir.join("tasks.corrupt.json"));
                    tracing::error!(error = %e, "tasks.json was unparseable - backed it up and started fresh");
                    Vec::new()
                }
            },
            Err(_) => Vec::new(),
        };
        TaskStore {
            path,
            tasks: Mutex::new(tasks),
        }
    }

    /// Write atomically (temp + rename) and owner-only.
    fn save(&self, tasks: &[Task]) {
        let bytes = serde_json::to_vec_pretty(tasks).unwrap_or_default();
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

    pub fn list(&self) -> Vec<Task> {
        self.tasks.lock().expect("tasks mutex").clone()
    }

    pub fn get(&self, id: &str) -> Option<Task> {
        self.tasks
            .lock()
            .expect("tasks mutex")
            .iter()
            .find(|t| t.id == id)
            .cloned()
    }

    pub fn create(&self, title: String, detail: String, origin: String) -> Task {
        let now = now_ms() as i64;
        let tool_tags = infer_tags(&format!("{title} {detail}"));
        // A process-wide counter guarantees a unique id even when several cards are created in the
        // same millisecond with the same title - e.g. a mission's parallel subtask cards, which
        // would otherwise collide on `t-{now}-{slug}` and corrupt each other's runs.
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let task = Task {
            id: format!("t-{now}-{seq}-{}", slug(&title)),
            title,
            detail,
            status: "todo".into(),
            origin,
            tool_tags,
            schedule_id: None,
            agent: None,
            handoffs: Vec::new(),
            created_ms: now,
            updated_ms: now,
            progress: None,
            run: None,
        };
        let mut t = self.tasks.lock().expect("tasks mutex");
        t.insert(0, task.clone());
        self.save(&t);
        task
    }

    pub fn update(
        &self,
        id: &str,
        status: Option<String>,
        title: Option<String>,
        detail: Option<String>,
    ) -> Option<Task> {
        let mut t = self.tasks.lock().expect("tasks mutex");
        let task = t.iter_mut().find(|x| x.id == id)?;
        if let Some(s) = status {
            task.status = s;
        }
        if let Some(ti) = title {
            task.title = ti;
        }
        if let Some(d) = detail {
            task.detail = d;
        }
        task.updated_ms = now_ms() as i64;
        let out = task.clone();
        self.save(&t);
        Some(out)
    }

    /// Assign (or clear, with `None`) the durable agent that runs this card.
    pub fn set_agent(&self, id: &str, agent: Option<String>) -> Option<Task> {
        let mut t = self.tasks.lock().expect("tasks mutex");
        let task = t.iter_mut().find(|x| x.id == id)?;
        task.agent = agent;
        task.updated_ms = now_ms() as i64;
        let out = task.clone();
        self.save(&t);
        Some(out)
    }

    /// Hand a card to another agent: reassign it and append the hand-off (with its note) to the trail.
    pub fn handoff(
        &self,
        id: &str,
        to_agent: Option<String>,
        from_name: &str,
        to_name: &str,
        note: &str,
    ) -> Option<Task> {
        let mut t = self.tasks.lock().expect("tasks mutex");
        let task = t.iter_mut().find(|x| x.id == id)?;
        task.agent = to_agent;
        let now = now_ms() as i64;
        task.handoffs.push(Handoff {
            from: from_name.to_string(),
            to: to_name.to_string(),
            note: note.to_string(),
            ts_ms: now,
        });
        task.updated_ms = now;
        let out = task.clone();
        self.save(&t);
        Some(out)
    }

    /// Atomically claim a task for running: transition it to "doing" only if it isn't
    /// already running. Returns false if it was already "doing", so a second concurrent
    /// run (double-click, HTTP racing the scheduler) is rejected rather than duplicated.
    pub fn try_begin(&self, id: &str) -> bool {
        let mut t = self.tasks.lock().expect("tasks mutex");
        let Some(task) = t.iter_mut().find(|x| x.id == id) else {
            return false;
        };
        if task.status == "doing" {
            return false;
        }
        task.status = "doing".into();
        task.updated_ms = now_ms() as i64;
        self.save(&t);
        true
    }

    /// Update the live progress label of a running task (cheap; not persisted to disk
    /// every tick - it is transient and overwritten at finish).
    pub fn set_progress(&self, id: &str, progress: String) {
        let mut t = self.tasks.lock().expect("tasks mutex");
        if let Some(task) = t.iter_mut().find(|x| x.id == id) {
            task.progress = Some(progress);
        }
    }

    /// Attach a run to the task and set its final status ("done" or "failed").
    pub fn finish(&self, id: &str, run: TaskRun, status: &str) -> Option<Task> {
        let mut t = self.tasks.lock().expect("tasks mutex");
        let task = t.iter_mut().find(|x| x.id == id)?;
        task.run = Some(run);
        task.status = status.to_string();
        task.progress = None;
        task.updated_ms = now_ms() as i64;
        let out = task.clone();
        self.save(&t);
        Some(out)
    }

    pub fn remove(&self, id: &str) -> bool {
        let mut t = self.tasks.lock().expect("tasks mutex");
        let before = t.len();
        t.retain(|x| x.id != id);
        let changed = t.len() != before;
        if changed {
            self.save(&t);
        }
        changed
    }
}

fn slug(s: &str) -> String {
    let s: String = s
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    let s = s.trim_matches('-').replace("--", "-");
    let out: String = s.chars().take(16).collect();
    if out.is_empty() {
        "task".into()
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir() -> std::path::PathBuf {
        // Atomic counter (not just nanos) so parallel tests never share a dir under load.
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let n = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let d = std::env::temp_dir().join(format!(
            "engram-tasks-test-{n}-{}",
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn set_agent_assigns_and_clears() {
        let d = tmpdir();
        let s = TaskStore::open(&d);
        let t = s.create("do a thing".into(), String::new(), "test".into());
        assert_eq!(
            s.set_agent(&t.id, Some("ag1".into()))
                .unwrap()
                .agent
                .as_deref(),
            Some("ag1")
        );
        assert!(s.set_agent(&t.id, None).unwrap().agent.is_none());
        assert!(s.set_agent("nope", Some("x".into())).is_none());
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn handoff_reassigns_and_appends_to_the_trail() {
        let d = tmpdir();
        let s = TaskStore::open(&d);
        let t = s.create("investigate".into(), String::new(), "test".into());
        s.set_agent(&t.id, Some("scout-id".into()));
        let after = s
            .handoff(
                &t.id,
                Some("rev-id".into()),
                "Scout",
                "Reviewer",
                "found it, verify",
            )
            .unwrap();
        assert_eq!(after.agent.as_deref(), Some("rev-id"));
        assert_eq!(after.handoffs.len(), 1);
        assert_eq!(after.handoffs[0].from, "Scout");
        assert_eq!(after.handoffs[0].to, "Reviewer");
        assert_eq!(after.handoffs[0].note, "found it, verify");
        // a second hand-off appends rather than replacing, and can clear the agent
        let after2 = s
            .handoff(&t.id, None, "Reviewer", "Default agent", "done")
            .unwrap();
        assert_eq!(after2.handoffs.len(), 2);
        assert!(after2.agent.is_none());
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn handoff_trail_persists_across_reload() {
        let d = tmpdir();
        let id = {
            let s = TaskStore::open(&d);
            let t = s.create("x".into(), String::new(), "test".into());
            s.handoff(&t.id, Some("a".into()), "", "A", "note");
            t.id
        };
        let s2 = TaskStore::open(&d);
        let t = s2.get(&id).unwrap();
        assert_eq!(t.handoffs.len(), 1);
        assert_eq!(t.handoffs[0].to, "A");
        std::fs::remove_dir_all(&d).ok();
    }
}
