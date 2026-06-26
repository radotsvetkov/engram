//! Projects and chat sessions - the workspace the desktop sidebar sits on.
//!
//! A project is a named world of work; a session is one chat thread inside it, holding its
//! messages. Both persist as plain JSON in the brain dir, so the sidebar survives a reload and
//! is shared by every client pointed at this daemon (not stranded in one browser's localStorage).
//! The agent's *memory* still lives separately in the brain - these are just the visible threads.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use engram_core::now_ms;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub created_ms: u64,
    /// Standing instructions for this project's chats - what gives a project its own voice.
    #[serde(default)]
    pub persona: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Msg {
    pub role: String, // "user" | "engram"
    pub text: String,
    #[serde(default)]
    pub recalled: Vec<String>,
    #[serde(default)]
    pub learned: Vec<String>,
    pub ts_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub project_id: String,
    pub title: String,
    #[serde(default)]
    pub fav: bool,
    pub created_ms: u64,
    pub updated_ms: u64,
    #[serde(default)]
    pub messages: Vec<Msg>,
}

/// A session without its messages - the lightweight shape the sidebar lists.
#[derive(Debug, Clone, Serialize)]
pub struct SessionMeta {
    pub id: String,
    pub project_id: String,
    pub title: String,
    pub fav: bool,
    pub created_ms: u64,
    pub updated_ms: u64,
    pub messages: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Data {
    projects: Vec<Project>,
    sessions: Vec<Session>,
}

pub struct WorkspaceStore {
    path: PathBuf,
    data: Mutex<Data>,
}

static SEQ: AtomicU64 = AtomicU64::new(1);
fn new_id(prefix: &str) -> String {
    format!("{prefix}-{}-{}", now_ms(), SEQ.fetch_add(1, Ordering::Relaxed))
}

impl WorkspaceStore {
    pub fn open(dir: &Path) -> Self {
        let path = dir.join("workspace.json");
        // Never silently discard a workspace: if the file is present but unparseable, back it
        // up rather than overwriting it with an empty default and losing every project/session.
        let mut data: Data = match std::fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice(&bytes) {
                Ok(d) => d,
                Err(e) => {
                    let backup = dir.join("workspace.corrupt.json");
                    let _ = std::fs::rename(&path, &backup);
                    tracing::error!(error = %e, backup = %backup.display(), "workspace.json was unparseable - backed it up and started fresh");
                    Data::default()
                }
            },
            Err(_) => Data::default(),
        };
        if data.projects.is_empty() {
            data.projects.push(Project { id: "personal".into(), name: "Personal".into(), created_ms: now_ms(), persona: String::new() });
        }
        let store = WorkspaceStore { path, data: Mutex::new(data) };
        store.persist();
        store
    }

    /// Write atomically (temp file + rename) and owner-only, so a crash mid-write can't leave a
    /// half-written file that boot would discard, and chat content isn't world-readable.
    fn persist(&self) {
        let bytes = {
            let d = self.data.lock().expect("workspace mutex");
            serde_json::to_vec_pretty(&*d).unwrap_or_default()
        };
        let tmp = self.path.with_extension("json.tmp");
        if std::fs::write(&tmp, &bytes).is_ok() {
            restrict(&tmp);
            let _ = std::fs::rename(&tmp, &self.path);
        }
    }
    /// Whether a project id exists - used to keep sessions from being orphaned.
    fn has_project(&self, id: &str) -> bool {
        self.data.lock().expect("ws").projects.iter().any(|p| p.id == id)
    }

    // --- projects ---
    pub fn projects(&self) -> Vec<Project> {
        self.data.lock().expect("ws").projects.clone()
    }
    pub fn create_project(&self, name: String) -> Project {
        let p = Project { id: new_id("p"), name, created_ms: now_ms(), persona: String::new() };
        self.data.lock().expect("ws").projects.push(p.clone());
        self.persist();
        p
    }
    pub fn update_project(&self, id: &str, name: Option<String>, persona: Option<String>) -> Option<Project> {
        let out = {
            let mut d = self.data.lock().expect("ws");
            d.projects.iter_mut().find(|p| p.id == id).map(|p| {
                if let Some(n) = name {
                    p.name = n;
                }
                if let Some(per) = persona {
                    p.persona = per;
                }
                p.clone()
            })
        };
        if out.is_some() {
            self.persist();
        }
        out
    }
    /// The standing-instructions persona for the project that owns a session (if any).
    pub fn persona_for_session(&self, session_id: &str) -> Option<String> {
        let d = self.data.lock().expect("ws");
        let pid = d.sessions.iter().find(|s| s.id == session_id).map(|s| s.project_id.clone())?;
        d.projects
            .iter()
            .find(|p| p.id == pid)
            .map(|p| p.persona.clone())
            .filter(|p| !p.trim().is_empty())
    }
    /// Delete a project and its sessions. Refuses to remove the last project.
    pub fn delete_project(&self, id: &str) -> bool {
        let ok = {
            let mut d = self.data.lock().expect("ws");
            if d.projects.len() <= 1 || !d.projects.iter().any(|p| p.id == id) {
                false
            } else {
                d.projects.retain(|p| p.id != id);
                d.sessions.retain(|s| s.project_id != id);
                true
            }
        };
        if ok {
            self.persist();
        }
        ok
    }

    // --- sessions ---
    pub fn sessions_meta(&self, project: &str) -> Vec<SessionMeta> {
        let d = self.data.lock().expect("ws");
        let mut v: Vec<SessionMeta> = d
            .sessions
            .iter()
            .filter(|s| s.project_id == project)
            .map(|s| SessionMeta {
                id: s.id.clone(),
                project_id: s.project_id.clone(),
                title: s.title.clone(),
                fav: s.fav,
                created_ms: s.created_ms,
                updated_ms: s.updated_ms,
                messages: s.messages.len(),
            })
            .collect();
        v.sort_by_key(|s| (std::cmp::Reverse(s.updated_ms), std::cmp::Reverse(s.created_ms))); // most-recent first, stable on ties
        v
    }
    pub fn session(&self, id: &str) -> Option<Session> {
        self.data.lock().expect("ws").sessions.iter().find(|s| s.id == id).cloned()
    }
    pub fn create_session(&self, project_id: String, title: Option<String>) -> Session {
        // Keep sessions from being orphaned under a project that doesn't exist.
        let project_id = if self.has_project(&project_id) {
            project_id
        } else {
            self.data.lock().expect("ws").projects.first().map(|p| p.id.clone()).unwrap_or_else(|| "personal".into())
        };
        let now = now_ms();
        let s = Session {
            id: new_id("s"),
            project_id,
            title: title.unwrap_or_else(|| "New chat".into()),
            fav: false,
            created_ms: now,
            updated_ms: now,
            messages: Vec::new(),
        };
        self.data.lock().expect("ws").sessions.insert(0, s.clone());
        self.persist();
        s
    }
    pub fn update_session(
        &self,
        id: &str,
        title: Option<String>,
        fav: Option<bool>,
        project_id: Option<String>,
    ) -> Option<Session> {
        let project_id = project_id.filter(|p| self.has_project(p)); // ignore a re-parent to a missing project
        let out = {
            let mut d = self.data.lock().expect("ws");
            d.sessions.iter_mut().find(|s| s.id == id).map(|s| {
                if let Some(t) = title {
                    s.title = t;
                }
                if let Some(f) = fav {
                    s.fav = f;
                }
                if let Some(p) = project_id {
                    s.project_id = p;
                }
                s.updated_ms = now_ms();
                s.clone()
            })
        };
        if out.is_some() {
            self.persist();
        }
        out
    }
    pub fn delete_session(&self, id: &str) -> bool {
        let ok = {
            let mut d = self.data.lock().expect("ws");
            let before = d.sessions.len();
            d.sessions.retain(|s| s.id != id);
            d.sessions.len() != before
        };
        if ok {
            self.persist();
        }
        ok
    }
    /// Append a completed turn (the user message and Engram's reply) to a session, titling the
    /// session from the first message. Returns false if the session no longer exists.
    pub fn append_turn(
        &self,
        id: &str,
        user_text: &str,
        reply: &str,
        recalled: Vec<String>,
        learned: Vec<String>,
    ) -> bool {
        let now = now_ms();
        let ok = {
            let mut d = self.data.lock().expect("ws");
            match d.sessions.iter_mut().find(|s| s.id == id) {
                Some(s) => {
                    if s.title.is_empty() || s.title == "New chat" {
                        let t: String = user_text.trim().chars().take(42).collect();
                        s.title = if t.is_empty() { "New chat".into() } else { t };
                    }
                    s.messages.push(Msg { role: "user".into(), text: user_text.into(), recalled: vec![], learned: vec![], ts_ms: now });
                    s.messages.push(Msg { role: "engram".into(), text: reply.into(), recalled, learned, ts_ms: now });
                    s.updated_ms = now;
                    true
                }
                None => false,
            }
        };
        if ok {
            self.persist();
        }
        ok
    }
}

/// Tighten file permissions to owner-only (chat content lives here).
fn restrict(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    let _ = path;
}
