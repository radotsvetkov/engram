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

/// Guess which capabilities a task will use, from its words - drives the card's tags.
fn infer_tags(text: &str) -> Vec<String> {
    let t = text.to_lowercase();
    let mut tags = Vec::new();
    let has = |words: &[&str]| words.iter().any(|w| t.contains(w));
    if has(&["search", "google", "web", "online", "url", "http", "news", "look up"]) {
        tags.push("web".into());
    }
    if has(&["browser", "click", "navigate", "screenshot", "page", "website", "site"]) {
        tags.push("browser".into());
    }
    if has(&["file", "write", "read", "save", "folder", "directory", "csv", "pdf"]) {
        tags.push("files".into());
    }
    if has(&["run", "command", "shell", "script", "build", "install", "compile"]) {
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
        let tasks = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        TaskStore { path, tasks: Mutex::new(tasks) }
    }

    fn save(&self, tasks: &[Task]) {
        let _ = std::fs::write(&self.path, serde_json::to_vec_pretty(tasks).unwrap_or_default());
    }

    pub fn list(&self) -> Vec<Task> {
        self.tasks.lock().expect("tasks mutex").clone()
    }

    pub fn get(&self, id: &str) -> Option<Task> {
        self.tasks.lock().expect("tasks mutex").iter().find(|t| t.id == id).cloned()
    }

    pub fn create(&self, title: String, detail: String, origin: String) -> Task {
        let now = now_ms() as i64;
        let tool_tags = infer_tags(&format!("{title} {detail}"));
        let task = Task {
            id: format!("t-{now}-{}", slug(&title)),
            title,
            detail,
            status: "todo".into(),
            origin,
            tool_tags,
            schedule_id: None,
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
