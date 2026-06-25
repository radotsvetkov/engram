//! Tasks — the unit of work behind the Kanban board.
//!
//! A task is something the user wants done, captured from chat or typed directly. It
//! moves through todo → doing → done, can be run by the agent (which attaches the
//! answer and the tools it used), and persists as plain JSON in the brain dir. This is
//! the model the desktop's board, chat input, and scheduler all sit on top of.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use engram_core::now_ms;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LastRun {
    pub answer: String,
    pub tools: Vec<String>,
    pub ts_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub detail: String,
    /// "todo" | "doing" | "done".
    pub status: String,
    pub created_ms: i64,
    pub updated_ms: i64,
    #[serde(default)]
    pub last_run: Option<LastRun>,
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

    pub fn create(&self, title: String, detail: String) -> Task {
        let now = now_ms() as i64;
        let task = Task {
            id: format!("t-{now}-{}", slug(&title)),
            title,
            detail,
            status: "todo".into(),
            created_ms: now,
            updated_ms: now,
            last_run: None,
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

    /// Record an agent run on the task and mark it done.
    pub fn finish(&self, id: &str, run: LastRun) -> Option<Task> {
        let mut t = self.tasks.lock().expect("tasks mutex");
        let task = t.iter_mut().find(|x| x.id == id)?;
        task.last_run = Some(run);
        task.status = "done".into();
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
