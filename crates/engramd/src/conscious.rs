//! Consciousness - a tiny, always-loaded working memory distilled from the brain.
//!
//! This is the "conscious" layer over the deep memory store: a handful of lines (<= [`MAX_LINES`])
//! loaded into EVERY agent run, so the model always holds the most important, durable facts about
//! the user without a recall round-trip. It is deliberately NOT a fifth memory region or an LLM
//! summary - each distilled line is the verbatim text of a REAL, TRUSTED stored memory, tagged with
//! its region and carrying its source id. That keeps it *verifiable* (every line traces to evidence
//! you can open in the brain) and *injection-safe* (untrusted memories never reach the always-loaded
//! block). The user can edit, add, remove, and revert lines; every change is signed into the ledger,
//! so the working memory that shapes the agent is itself auditable - the verifiable-memory wedge.

use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use engram_core::Ledger;
use engram_memory::{Memory, Region};
use serde::{Deserialize, Serialize};
use serde_json::json;

/// Hard ceiling on always-loaded lines - working memory must stay small to stay "conscious".
const MAX_LINES: usize = 9;
/// How many prior versions to keep for revert.
const HISTORY: usize = 12;
/// Trim each line so one verbose memory can't dominate the block.
const LINE_CHARS: usize = 160;

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Where a conscious line came from - the provenance that makes it verifiable.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Source {
    /// Distilled from a stored memory; the line text is that memory's text. Click-through opens it.
    Memory { id: i64 },
    /// Authored or edited by the user directly.
    User,
}

/// One line of always-loaded working memory.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Line {
    /// Stable id: `m<memory_id>` for distilled lines, `u<n>` for user lines. Edits target this.
    pub id: String,
    pub text: String,
    pub region: String,
    pub source: Source,
    /// User-pinned: never auto-dropped on re-distill. Editing or adding a line pins it.
    pub pinned: bool,
}

/// The current conscious state - a versioned, tiny set of lines.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct State {
    pub version: u64,
    pub distilled_at_ms: i64,
    pub lines: Vec<Line>,
}

#[derive(Default, Serialize, Deserialize)]
struct Persisted {
    current: State,
    history: VecDeque<State>,
    /// Monotonic counter for user-line ids, so a removed-then-added line never reuses an id.
    next_uid: u64,
}

/// The consciousness store. Holds only its own state + path; distillation borrows the memory store
/// and ledger at call time (no Arc cycle).
pub struct Consciousness {
    path: PathBuf,
    inner: Mutex<Persisted>,
}

impl Consciousness {
    /// Load `<home>/consciousness.json`, or start empty.
    pub fn open(home: &str) -> Self {
        let path = std::path::Path::new(home).join("consciousness.json");
        let inner = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<Persisted>(&s).ok())
            .unwrap_or_default();
        Self {
            path,
            inner: Mutex::new(inner),
        }
    }

    /// The current state, for the UI.
    pub fn snapshot(&self) -> State {
        self.inner
            .lock()
            .expect("consciousness lock")
            .current
            .clone()
    }

    /// The always-loaded block prepended to every run's system prompt. `None` when empty, so the
    /// run falls back to the persona alone (back-compat). Only TRUSTED memory ever lands here, so
    /// this block is safe to treat as standing instruction.
    pub fn prompt_block(&self) -> Option<String> {
        let g = self.inner.lock().expect("consciousness lock");
        if g.current.lines.is_empty() {
            return None;
        }
        let mut s = String::from(
            "Working memory - the user's CONFIRMED, current facts about themselves (signed, \
             user-editable, distilled from durable identity/semantic memory). Treat these as \
             AUTHORITATIVE: when answering questions about the user, use these facts, and if a \
             recalled snippet or a past activity log conflicts with one, THIS wins (the user \
             curated these; old captures may be stale tests). Do not 'correct' a fact here from an \
             older recalled note.\n",
        );
        for l in &g.current.lines {
            s.push_str("- ");
            s.push_str(&l.text);
            s.push('\n');
        }
        Some(s)
    }

    /// Re-distill: keep pinned lines, then fill the rest from the most important TRUSTED identity
    /// and semantic memories. Deterministic (no model call) so every line traces to real evidence.
    pub fn distill(&self, mem: &Memory, ledger: &Ledger) -> Result<State, String> {
        // Gather candidates first (no lock held during DB work).
        let mut cands = Vec::new();
        for region in [Region::Identity, Region::Semantic] {
            let recs = mem.recent(region, 60).map_err(|e| e.to_string())?;
            for r in recs {
                if r.taint.eq_ignore_ascii_case("trusted") {
                    cands.push(r);
                }
            }
        }
        // Identity before semantic; within a region, most important first, then most recent.
        cands.sort_by(|a, b| {
            region_rank(&a.region)
                .cmp(&region_rank(&b.region))
                .then(
                    b.importance
                        .partial_cmp(&a.importance)
                        .unwrap_or(std::cmp::Ordering::Equal),
                )
                .then(b.created_ms.cmp(&a.created_ms))
        });

        let mut g = self.inner.lock().expect("consciousness lock");
        let mut lines: Vec<Line> = g
            .current
            .lines
            .iter()
            .filter(|l| l.pinned)
            .cloned()
            .collect();
        let mut used: HashSet<i64> = lines
            .iter()
            .filter_map(|l| match l.source {
                Source::Memory { id } => Some(id),
                Source::User => None,
            })
            .collect();
        for r in cands {
            if lines.len() >= MAX_LINES {
                break;
            }
            if used.contains(&r.id) {
                continue;
            }
            used.insert(r.id);
            lines.push(Line {
                id: format!("m{}", r.id),
                text: trim(&r.text),
                region: r.region.clone(),
                source: Source::Memory { id: r.id },
                pinned: false,
            });
        }

        // Idempotent: if the distilled lines are identical to what's already current, do NOT bump the
        // version, ledger, or rewrite the file. This makes it safe to call distill() on every memory
        // write and at the start of every run (to keep the consciousness fresh) without bloating the
        // ledger or churning consciousness.json when nothing changed.
        let unchanged = lines.len() == g.current.lines.len()
            && lines
                .iter()
                .zip(g.current.lines.iter())
                .all(|(a, b)| a.text == b.text && a.region == b.region && a.source == b.source);
        if unchanged {
            return Ok(g.current.clone());
        }

        let next_version = g.current.version + 1;
        let sources: Vec<_> = lines
            .iter()
            .map(|l| json!({ "id": l.id, "region": l.region, "pinned": l.pinned }))
            .collect();
        ledger
            .append(
                "consciousness.distill",
                "user",
                json!({ "version": next_version, "n": lines.len(), "lines": sources }),
            )
            .map_err(|e| e.to_string())?;
        push_history(&mut g);
        g.current = State {
            version: next_version,
            distilled_at_ms: now_ms(),
            lines,
        };
        self.persist(&g);
        Ok(g.current.clone())
    }

    /// Edit a line's text (and pin it, since the user has taken ownership of it). Signed.
    pub fn edit(&self, id: &str, text: &str, ledger: &Ledger) -> Result<State, String> {
        let text = trim(text.trim());
        if text.is_empty() {
            return Err("empty line".into());
        }
        let mut g = self.inner.lock().expect("consciousness lock");
        let Some(line) = g.current.lines.iter().position(|l| l.id == id) else {
            return Err("no such line".into());
        };
        let next_version = g.current.version + 1;
        ledger
            .append(
                "consciousness.edit",
                "user",
                json!({ "version": next_version, "id": id, "text": text }),
            )
            .map_err(|e| e.to_string())?;
        push_history(&mut g);
        g.current.version = next_version;
        g.current.lines[line].text = text;
        g.current.lines[line].pinned = true;
        self.persist(&g);
        Ok(g.current.clone())
    }

    /// Add a new user-authored, pinned line. Signed.
    pub fn add(&self, text: &str, ledger: &Ledger) -> Result<State, String> {
        let text = trim(text.trim());
        if text.is_empty() {
            return Err("empty line".into());
        }
        let mut g = self.inner.lock().expect("consciousness lock");
        if g.current.lines.len() >= MAX_LINES {
            return Err(format!(
                "working memory is full ({MAX_LINES} lines) - remove one first"
            ));
        }
        let uid = g.next_uid;
        g.next_uid += 1;
        let next_version = g.current.version + 1;
        ledger
            .append(
                "consciousness.add",
                "user",
                json!({ "version": next_version, "id": format!("u{uid}"), "text": text }),
            )
            .map_err(|e| e.to_string())?;
        push_history(&mut g);
        g.current.version = next_version;
        g.current.lines.push(Line {
            id: format!("u{uid}"),
            text,
            region: "identity".into(),
            source: Source::User,
            pinned: true,
        });
        self.persist(&g);
        Ok(g.current.clone())
    }

    /// Remove a line. Signed.
    pub fn remove(&self, id: &str, ledger: &Ledger) -> Result<State, String> {
        let mut g = self.inner.lock().expect("consciousness lock");
        if !g.current.lines.iter().any(|l| l.id == id) {
            return Err("no such line".into());
        }
        let next_version = g.current.version + 1;
        ledger
            .append(
                "consciousness.remove",
                "user",
                json!({ "version": next_version, "id": id }),
            )
            .map_err(|e| e.to_string())?;
        push_history(&mut g);
        g.current.version = next_version;
        g.current.lines.retain(|l| l.id != id);
        self.persist(&g);
        Ok(g.current.clone())
    }

    /// Revert to the previous version. Signed.
    pub fn revert(&self, ledger: &Ledger) -> Result<State, String> {
        let mut g = self.inner.lock().expect("consciousness lock");
        let Some(prev) = g.history.pop_back() else {
            return Err("nothing to revert to".into());
        };
        let from = g.current.version;
        ledger
            .append(
                "consciousness.revert",
                "user",
                json!({ "from_version": from, "to_version": prev.version }),
            )
            .map_err(|e| e.to_string())?;
        g.current = prev;
        self.persist(&g);
        Ok(g.current.clone())
    }

    /// Atomic, owner-only write (temp + rename), so a crash mid-write can't corrupt the file.
    fn persist(&self, g: &Persisted) {
        let Ok(bytes) = serde_json::to_vec_pretty(g) else {
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

fn push_history(g: &mut Persisted) {
    if g.history.len() >= HISTORY {
        g.history.pop_front();
    }
    g.history.push_back(g.current.clone());
}

fn region_rank(region: &str) -> u8 {
    match region {
        "identity" => 0,
        "semantic" => 1,
        _ => 2,
    }
}

fn trim(s: &str) -> String {
    let s = s.trim();
    if s.chars().count() <= LINE_CHARS {
        return s.to_string();
    }
    let mut out: String = s.chars().take(LINE_CHARS - 1).collect();
    out.push('…');
    out
}
