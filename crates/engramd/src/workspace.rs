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
    /// The working directory this project's agent operates in: file/shell tools run here (and are
    /// confined to it), artifacts land here. `None` falls back to the shared daemon workdir, so a
    /// project without a directory behaves exactly as before. This is the third leg of a project,
    /// alongside its memory scope and its persona.
    #[serde(default)]
    pub workdir: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Msg {
    pub role: String, // "user" | "engram"
    pub text: String,
    #[serde(default)]
    pub recalled: Vec<String>,
    /// The recalled memories with id/region/text, so a reloaded answer keeps its clickable recall
    /// ribbon (each chip links to its brain node). Stored as JSON to stay decoupled from converse.
    #[serde(default)]
    pub recalled_refs: Vec<serde_json::Value>,
    #[serde(default)]
    pub learned: Vec<String>,
    /// The run's tool steps (tool, args, observation, ok, …) as JSON, so a reloaded answer keeps its
    /// glass-box trail — the step chips, inline screenshots, and clickable "wrote a file ↗" affordances
    /// the chat renderer builds from these. Without it, every prior turn degrades to bare Q&A on reload,
    /// erasing the verifiable-audit-trail that is the product's core pitch. Stored as raw JSON to stay
    /// decoupled from the agent's StepRecord shape.
    #[serde(default)]
    pub steps: Vec<serde_json::Value>,
    /// The model's interim narration notes streamed during the run ("I've kicked off two searches…"),
    /// re-rendered under the reloaded answer exactly as they appeared live.
    #[serde(default)]
    pub notes: Vec<String>,
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
    format!(
        "{prefix}-{}-{}",
        now_ms(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    )
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
            data.projects.push(Project {
                id: "personal".into(),
                name: "Personal".into(),
                created_ms: now_ms(),
                persona: String::new(),
                workdir: None,
            });
        }
        let store = WorkspaceStore {
            path,
            data: Mutex::new(data),
        };
        store.persist();
        store
    }

    /// Write atomically (temp file + rename) and owner-only, so a crash mid-write can't leave a
    /// half-written file that boot would discard, and chat content isn't world-readable.
    ///
    /// Each call writes to a UNIQUE tmp path, then renames it into place. This matters because the
    /// daemon runs on a multithreaded runtime: two concurrent persists (e.g. `append_user_turn` in one
    /// chat racing `append_reply_turn` in another) previously shared one `workspace.json.tmp`, so writer
    /// B could truncate/overwrite the tmp while writer A was mid-write and A would then rename a torn
    /// file into place — corrupting the store (detected on next boot, backed up, "starts fresh": every
    /// project/session gone). With a per-write tmp path the writers can't corrupt each other and the
    /// last rename to land is a consistent, complete snapshot. Stale tmp files are cleaned up by the
    /// writer that created them.
    fn persist(&self) {
        let bytes = {
            let d = self.data.lock().expect("workspace mutex");
            serde_json::to_vec_pretty(&*d).unwrap_or_default()
        };
        // Unique per write: pid + a process-monotonic counter, so concurrent writers never share a tmp.
        let uniq = format!(
            "{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        );
        let tmp = self.path.with_extension(format!("json.tmp.{uniq}"));
        if std::fs::write(&tmp, &bytes).is_ok() {
            restrict(&tmp);
            if std::fs::rename(&tmp, &self.path).is_err() {
                // Rename failed: don't leak the tmp file.
                let _ = std::fs::remove_file(&tmp);
            }
        }
    }
    /// Whether a project id exists - used to keep sessions from being orphaned.
    fn has_project(&self, id: &str) -> bool {
        self.data
            .lock()
            .expect("ws")
            .projects
            .iter()
            .any(|p| p.id == id)
    }

    // --- projects ---
    pub fn projects(&self) -> Vec<Project> {
        self.data.lock().expect("ws").projects.clone()
    }
    pub fn create_project(&self, name: String, workdir: Option<String>) -> Project {
        let p = Project {
            id: new_id("p"),
            name,
            created_ms: now_ms(),
            persona: String::new(),
            workdir: workdir.filter(|w| !w.trim().is_empty()),
        };
        self.data.lock().expect("ws").projects.push(p.clone());
        self.persist();
        p
    }
    pub fn update_project(
        &self,
        id: &str,
        name: Option<String>,
        persona: Option<String>,
        // `Some(path)` sets the working directory (an empty/blank path clears it back to the shared
        // workdir); `None` leaves it unchanged.
        workdir: Option<String>,
    ) -> Option<Project> {
        let out = {
            let mut d = self.data.lock().expect("ws");
            d.projects.iter_mut().find(|p| p.id == id).map(|p| {
                if let Some(n) = name {
                    p.name = n;
                }
                if let Some(per) = persona {
                    p.persona = per;
                }
                if let Some(w) = workdir {
                    let w = w.trim();
                    p.workdir = if w.is_empty() {
                        None
                    } else {
                        Some(w.to_string())
                    };
                }
                p.clone()
            })
        };
        if out.is_some() {
            self.persist();
        }
        out
    }

    /// The working directory for a chat session: its project's `workdir`, if that project has one.
    /// `None` means the session should use the shared daemon workdir (back-compat).
    pub fn workdir_for_session(&self, session_id: &str) -> Option<PathBuf> {
        let d = self.data.lock().expect("ws");
        let pid = d
            .sessions
            .iter()
            .find(|s| s.id == session_id)
            .map(|s| s.project_id.clone())?;
        d.projects
            .iter()
            .find(|p| p.id == pid)
            .and_then(|p| p.workdir.clone())
            .filter(|w| !w.trim().is_empty())
            .map(PathBuf::from)
    }
    /// The last `n` turns of a session as (role, text), oldest-first - the conversation history the
    /// agentic chat needs so a follow-up ("let's try again") resolves against what was already said
    /// instead of re-asking for context. Long messages are truncated to keep the prompt bounded.
    pub fn recent_turns(&self, session_id: &str, n: usize) -> Vec<(String, String)> {
        let d = self.data.lock().expect("ws");
        let Some(s) = d.sessions.iter().find(|s| s.id == session_id) else {
            return Vec::new();
        };
        let mut turns: Vec<(String, String)> = s
            .messages
            .iter()
            .rev()
            .take(n)
            .map(|m| {
                (
                    m.role.clone(),
                    m.text.chars().take(2000).collect::<String>(),
                )
            })
            .collect();
        turns.reverse();
        turns
    }

    /// The standing-instructions persona for the project that owns a session (if any).
    pub fn persona_for_session(&self, session_id: &str) -> Option<String> {
        let d = self.data.lock().expect("ws");
        let pid = d
            .sessions
            .iter()
            .find(|s| s.id == session_id)
            .map(|s| s.project_id.clone())?;
        d.projects
            .iter()
            .find(|p| p.id == pid)
            .map(|p| p.persona.clone())
            .filter(|p| !p.trim().is_empty())
    }

    /// The scope context for a chat session: its project ring plus the session ring, so recall
    /// spans user-global ∪ this project ∪ this session and captures land in the right ring. A
    /// session under no known project (or an unknown session) resolves to the session ring alone,
    /// which still keeps its chatter out of every other chat.
    pub fn scope_for_session(&self, session_id: &str) -> engram_core::ScopeCtx {
        let d = self.data.lock().expect("ws");
        let project = d
            .sessions
            .iter()
            .find(|s| s.id == session_id)
            .map(|s| s.project_id.clone())
            .filter(|pid| d.projects.iter().any(|p| p.id == *pid));
        engram_core::ScopeCtx {
            project,
            session: Some(session_id.to_string()),
            any: false,
        }
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
        v.sort_by_key(|s| {
            (
                std::cmp::Reverse(s.updated_ms),
                std::cmp::Reverse(s.created_ms),
            )
        }); // most-recent first, stable on ties
        v
    }
    pub fn session(&self, id: &str) -> Option<Session> {
        self.data
            .lock()
            .expect("ws")
            .sessions
            .iter()
            .find(|s| s.id == id)
            .cloned()
    }
    pub fn create_session(&self, project_id: String, title: Option<String>) -> Session {
        // Keep sessions from being orphaned under a project that doesn't exist.
        let project_id = if self.has_project(&project_id) {
            project_id
        } else {
            self.data
                .lock()
                .expect("ws")
                .projects
                .first()
                .map(|p| p.id.clone())
                .unwrap_or_else(|| "personal".into())
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
    /// Persist the USER's message the instant it's received — BEFORE the agent runs — so a chat that
    /// is closed or interrupted mid-task still has the posted message on reopen. Sets the session
    /// title from the first message. The matching reply is appended later via `append_reply_turn`.
    pub fn append_user_turn(&self, id: &str, user_text: &str) -> bool {
        let now = now_ms();
        let ok = {
            let mut d = self.data.lock().expect("ws");
            match d.sessions.iter_mut().find(|s| s.id == id) {
                Some(s) => {
                    if s.title.is_empty() || s.title == "New chat" {
                        let t: String = user_text.trim().chars().take(42).collect();
                        s.title = if t.is_empty() { "New chat".into() } else { t };
                    }
                    s.messages.push(Msg {
                        role: "user".into(),
                        text: user_text.into(),
                        recalled: vec![],
                        recalled_refs: vec![],
                        learned: vec![],
                        steps: vec![],
                        notes: vec![],
                        ts_ms: now,
                    });
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

    /// Append the agent's REPLY to a session (paired with a prior `append_user_turn`). `steps` is the `steps` is the
    /// run's tool trail and `notes` its interim narration, persisted so the glass-box view survives a
    /// reload rather than degrading to bare Q&A.
    #[allow(clippy::too_many_arguments)]
    pub fn append_reply_turn(
        &self,
        id: &str,
        reply: &str,
        recalled: Vec<String>,
        recalled_refs: Vec<serde_json::Value>,
        learned: Vec<String>,
        steps: Vec<serde_json::Value>,
        notes: Vec<String>,
    ) -> bool {
        let now = now_ms();
        let ok = {
            let mut d = self.data.lock().expect("ws");
            match d.sessions.iter_mut().find(|s| s.id == id) {
                Some(s) => {
                    s.messages.push(Msg {
                        role: "engram".into(),
                        text: reply.into(),
                        recalled,
                        recalled_refs,
                        learned,
                        steps,
                        notes,
                        ts_ms: now,
                    });
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

    pub fn append_turn(
        &self,
        id: &str,
        user_text: &str,
        reply: &str,
        recalled: Vec<String>,
        recalled_refs: Vec<serde_json::Value>,
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
                    s.messages.push(Msg {
                        role: "user".into(),
                        text: user_text.into(),
                        recalled: vec![],
                        recalled_refs: vec![],
                        learned: vec![],
                        steps: vec![],
                        notes: vec![],
                        ts_ms: now,
                    });
                    s.messages.push(Msg {
                        role: "engram".into(),
                        text: reply.into(),
                        recalled,
                        recalled_refs,
                        learned,
                        steps: vec![],
                        notes: vec![],
                        ts_ms: now,
                    });
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_for_session_resolves_the_project_ring() {
        let dir = tempfile::tempdir().unwrap();
        let ws = WorkspaceStore::open(dir.path());
        let p = ws.create_project("Apollo".into(), None);
        let s = ws.create_session(p.id.clone(), None);

        // The session resolves to user ∪ its project ∪ itself — the linkage that used to be dropped
        // before memory recall/capture, which is what caused the cross-project bleed.
        let ctx = ws.scope_for_session(&s.id);
        assert_eq!(ctx.project.as_deref(), Some(p.id.as_str()));
        assert_eq!(ctx.session.as_deref(), Some(s.id.as_str()));
        assert_eq!(
            ctx.durable_write_scope(),
            engram_core::Scope::project(p.id.clone()),
            "a durable capture in this session lands in its project's ring"
        );

        // A session in a different project resolves to a DIFFERENT ring (isolation at the source).
        let p2 = ws.create_project("Zephyr".into(), None);
        let s2 = ws.create_session(p2.id.clone(), None);
        let ctx2 = ws.scope_for_session(&s2.id);
        assert_ne!(ctx.project, ctx2.project);

        // An unknown session still gets its own session ring (never another project's).
        let unknown = ws.scope_for_session("does-not-exist");
        assert!(unknown.project.is_none());
        assert_eq!(unknown.session.as_deref(), Some("does-not-exist"));
    }

    #[test]
    fn workdir_for_session_resolves_the_projects_directory() {
        let dir = tempfile::tempdir().unwrap();
        let ws = WorkspaceStore::open(dir.path());
        // A project WITH a working directory: its sessions resolve to that dir.
        let p = ws.create_project("Coderepo".into(), Some("/tmp/some-repo".into()));
        let s = ws.create_session(p.id.clone(), None);
        assert_eq!(
            ws.workdir_for_session(&s.id),
            Some(PathBuf::from("/tmp/some-repo"))
        );
        // A project WITHOUT one falls back (None → shared daemon workdir).
        let p2 = ws.create_project("NoDir".into(), None);
        let s2 = ws.create_session(p2.id.clone(), None);
        assert_eq!(ws.workdir_for_session(&s2.id), None);
        // update_project can set it…
        ws.update_project(&p2.id, None, None, Some("/tmp/added".into()));
        assert_eq!(
            ws.workdir_for_session(&s2.id),
            Some(PathBuf::from("/tmp/added"))
        );
        // …and an empty string clears it back to the shared workdir.
        ws.update_project(&p2.id, None, None, Some(String::new()));
        assert_eq!(ws.workdir_for_session(&s2.id), None);
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
