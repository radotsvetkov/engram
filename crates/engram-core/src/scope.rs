//! Memory scope - *which world* a memory belongs to, orthogonal to its region.
//!
//! `region` (episodic/semantic/identity/procedural) says *what kind* of memory a fact
//! is. Scope says *which world* it lives in: the user-global ring (facts about the
//! person, durable preferences - these follow them everywhere), a project ring (work
//! that must stay walled inside its project), or a session ring (ephemeral turn state).
//!
//! Recall is a **union of rings**: always user-global, plus the active project ring,
//! plus the active session ring - ranked so the more specific ring wins ties. A brand
//! new project has an empty project ring, so it starts clean by construction while
//! still seeing what's known about the user.

use serde::{Deserialize, Serialize};

/// The three rings a memory can live in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScopeKind {
    /// User-global: facts about the person and durable preferences. Recalled everywhere.
    User,
    /// Bound to one project. Recalled only when that project is active.
    Project,
    /// Ephemeral turn state, bound to one chat session. Swept over time.
    Session,
}

impl ScopeKind {
    /// The lowercase wire/SQL string (`user` | `project` | `session`).
    pub fn as_str(self) -> &'static str {
        match self {
            ScopeKind::User => "user",
            ScopeKind::Project => "project",
            ScopeKind::Session => "session",
        }
    }

    /// Parse from the SQL/wire string; anything unrecognised is the safe default (User).
    pub fn parse(s: &str) -> ScopeKind {
        match s {
            "project" => ScopeKind::Project,
            "session" => ScopeKind::Session,
            _ => ScopeKind::User,
        }
    }

    /// Specificity rank - higher is more specific, so a session hit outranks a project
    /// hit outranks a user-global hit on an otherwise-equal recall score.
    pub fn specificity(self) -> u8 {
        match self {
            ScopeKind::User => 0,
            ScopeKind::Project => 1,
            ScopeKind::Session => 2,
        }
    }
}

/// Where a single memory lives: a ring kind plus the id of that ring (`""` for User).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Scope {
    pub kind: ScopeKind,
    /// The project id or session id. Always empty for `User`.
    #[serde(default)]
    pub id: String,
}

impl Scope {
    /// The user-global ring.
    pub fn user() -> Self {
        Scope {
            kind: ScopeKind::User,
            id: String::new(),
        }
    }
    /// A project ring.
    pub fn project(id: impl Into<String>) -> Self {
        Scope {
            kind: ScopeKind::Project,
            id: id.into(),
        }
    }
    /// A session ring.
    pub fn session(id: impl Into<String>) -> Self {
        Scope {
            kind: ScopeKind::Session,
            id: id.into(),
        }
    }
    /// Rebuild from the two SQL columns, normalising a User scope's id to empty.
    pub fn from_parts(kind: &str, id: &str) -> Self {
        let kind = ScopeKind::parse(kind);
        let id = if kind == ScopeKind::User {
            String::new()
        } else {
            id.to_string()
        };
        Scope { kind, id }
    }
}

impl Default for Scope {
    fn default() -> Self {
        Scope::user()
    }
}

/// The active turn's scope context: which project and session (if any) are in play.
/// Recall spans the union of the rings this implies.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScopeCtx {
    /// The active project id, if a project is in context.
    #[serde(default)]
    pub project: Option<String>,
    /// The active session id, if a session is in context.
    #[serde(default)]
    pub session: Option<String>,
    /// When set, recall spans EVERY scope - the Atlas "show everything" view. Never used
    /// for model-facing recall (which must stay ringed), only for whole-brain inspection.
    #[serde(default)]
    pub any: bool,
}

impl ScopeCtx {
    /// No project or session in context: recall sees only the user-global ring.
    pub fn user_only() -> Self {
        ScopeCtx::default()
    }
    /// A context scoped to one project (no session).
    pub fn project(id: impl Into<String>) -> Self {
        ScopeCtx {
            project: Some(id.into()),
            ..Default::default()
        }
    }
    /// The whole-brain view (Atlas / graph): recall ignores rings.
    pub fn any() -> Self {
        ScopeCtx {
            any: true,
            ..Default::default()
        }
    }
    /// Add a session id to this context (builder).
    pub fn with_session(mut self, id: impl Into<String>) -> Self {
        self.session = Some(id.into());
        self
    }
    /// True when this context is the plain user-global view (no project, no session, not any).
    pub fn is_user_only(&self) -> bool {
        !self.any && self.project.is_none() && self.session.is_none()
    }

    /// The ring a *deliberate, durable* write should land in: the active project if any, else
    /// user-global. Session is intentionally skipped - a fact the agent chose to remember should
    /// persist, not be swept with the session.
    pub fn durable_write_scope(&self) -> Scope {
        match &self.project {
            Some(p) => Scope::project(p.clone()),
            None => Scope::user(),
        }
    }
    /// The (kind, id) rings recall should include, in specificity order (user first). Empty
    /// when `any` is set - callers treat that as "no ring filter".
    pub fn rings(&self) -> Vec<(ScopeKind, String)> {
        if self.any {
            return Vec::new();
        }
        let mut rings = vec![(ScopeKind::User, String::new())];
        if let Some(p) = &self.project {
            rings.push((ScopeKind::Project, p.clone()));
        }
        if let Some(s) = &self.session {
            rings.push((ScopeKind::Session, s.clone()));
        }
        rings
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_scope_normalises_id_to_empty() {
        assert_eq!(Scope::from_parts("user", "ignored").id, "");
        assert_eq!(Scope::from_parts("project", "p1").id, "p1");
        assert_eq!(Scope::from_parts("bogus", "x").kind, ScopeKind::User);
    }

    #[test]
    fn user_only_ctx_yields_just_the_user_ring() {
        let c = ScopeCtx::user_only();
        assert_eq!(c.rings(), vec![(ScopeKind::User, String::new())]);
        assert!(c.is_user_only());
    }

    #[test]
    fn project_ctx_unions_user_and_project_rings() {
        let c = ScopeCtx::project("p1").with_session("s1");
        assert_eq!(
            c.rings(),
            vec![
                (ScopeKind::User, String::new()),
                (ScopeKind::Project, "p1".into()),
                (ScopeKind::Session, "s1".into()),
            ]
        );
    }

    #[test]
    fn any_ctx_has_no_ring_filter() {
        assert!(ScopeCtx::any().rings().is_empty());
    }

    #[test]
    fn specificity_orders_session_over_project_over_user() {
        assert!(ScopeKind::Session.specificity() > ScopeKind::Project.specificity());
        assert!(ScopeKind::Project.specificity() > ScopeKind::User.specificity());
    }
}
