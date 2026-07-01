//! The write-time memory router: decides *which ring* a captured memory belongs to.
//!
//! Recall isolates by ring (see [`engram_core::ScopeCtx`]); this is the other half - deciding,
//! at write time, whether a new fact is durable-user, project-local, or ephemeral-session. The
//! rule is **default to the narrowest applicable ring, promote only on an explicit signal**:
//! a wrong "user-global" contaminates every project, while a wrong "project" is harmless and
//! easy to promote later.

use engram_core::{Scope, ScopeCtx};
use engram_memory::Region;

/// Phrases that mark a fact as a *durable, cross-project* preference about how the user wants to
/// work - so it is promoted to the user-global ring even when a project is active.
const GLOBAL_PREF_MARKERS: &[&str] = &[
    "i always ",
    "i never ",
    "in general ",
    "generally ",
    "as a rule",
    "by default",
    "my preference is",
    "i prefer to always",
    "across all my projects",
    "for all my projects",
    "in every project",
    "whenever i",
];

/// Phrases that mark a turn as ephemeral - relevant only to the current chat, not worth keeping
/// past the session. Routed to the session ring (swept over time), never to a durable ring.
const EPHEMERAL_MARKERS: &[&str] = &[
    "for now",
    "just this once",
    "never mind",
    "nvm",
    "forget that",
    "scratch that",
    "temporarily",
    "for the moment",
];

fn contains_any(haystack_lower: &str, needles: &[&str]) -> bool {
    needles.iter().any(|n| haystack_lower.contains(n))
}

/// Classify where a captured memory should live, given its region, the active turn context, and
/// its text. Narrowest applicable ring by default:
/// - identity facts (about the *person*) are always user-global - they must follow the user;
/// - an explicit global-preference signal promotes to user-global even inside a project;
/// - an ephemeral marker routes to the session ring (if a session is active);
/// - otherwise a project ring when a project is active;
/// - otherwise a session ring for episodic chatter with only a session;
/// - otherwise user-global.
pub fn classify(region: Region, ctx: &ScopeCtx, text: &str) -> Scope {
    // Facts about the user follow them everywhere - never trap them in a project.
    if region == Region::Identity {
        return Scope::user();
    }
    let lower = text.to_lowercase();
    // A durable cross-project preference is user-global even mid-project.
    if contains_any(&lower, GLOBAL_PREF_MARKERS) {
        return Scope::user();
    }
    // Ephemeral turn state stays in the session ring (swept later), never durable.
    if contains_any(&lower, EPHEMERAL_MARKERS) {
        if let Some(s) = &ctx.session {
            return Scope::session(s.clone());
        }
    }
    // A project in context claims project-shaped memory (the thing that was bleeding).
    if let Some(p) = &ctx.project {
        return Scope::project(p.clone());
    }
    // Only a session in context: episodic run/chat state is session-scoped; consolidated
    // knowledge (semantic/procedural) with no project defaults to user-global.
    if let Some(s) = &ctx.session {
        if region == Region::Episodic {
            return Scope::session(s.clone());
        }
    }
    Scope::user()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_is_always_user_global() {
        let ctx = ScopeCtx::project("P");
        assert_eq!(
            classify(Region::Identity, &ctx, "the user lives in Munich"),
            Scope::user()
        );
    }

    #[test]
    fn project_context_claims_episodic_and_semantic() {
        let ctx = ScopeCtx::project("P");
        assert_eq!(
            classify(Region::Episodic, &ctx, "Task: deploy\nOutcome: ok"),
            Scope::project("P")
        );
        assert_eq!(
            classify(Region::Semantic, &ctx, "the api base url is x"),
            Scope::project("P")
        );
    }

    #[test]
    fn global_preference_promotes_over_project() {
        let ctx = ScopeCtx::project("P");
        assert_eq!(
            classify(Region::Semantic, &ctx, "I always use pnpm across all my projects"),
            Scope::user()
        );
    }

    #[test]
    fn ephemeral_marker_routes_to_session() {
        let ctx = ScopeCtx::project("P").with_session("S");
        assert_eq!(
            classify(Region::Episodic, &ctx, "just this once, skip the tests"),
            Scope::session("S")
        );
    }

    #[test]
    fn no_context_defaults_to_user() {
        let ctx = ScopeCtx::user_only();
        assert_eq!(
            classify(Region::Episodic, &ctx, "Task: x\nOutcome: y"),
            Scope::user()
        );
    }
}
