//! Read-only git + change inspection — the glass box applied to the working tree.
//!
//! Two surfaces, both STRICTLY read-only (no staging, committing, or pushing from the UI - those
//! stay deliberate, human, terminal acts):
//!   * `/v1/git/status` + `/v1/git/diff` — branch, dirty/staged/untracked files, recent log, and
//!     per-file unified diffs for the session's working directory, via the `git` CLI.
//!   * `/v1/tasks/{id}/changes` — what a RUN changed: the delta between the auto-checkpoint taken
//!     before the run (plain file copies under `<home>/checkpoints/<id>/files/`) and the workdir
//!     now. Needs no git at all, so it works in any folder.
//!
//! Every git invocation is a fixed argv (never a shell), pinned to the resolved workdir, with a
//! hard timeout; requested paths are joined + canonicalized and must stay inside the workdir.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use axum::extract::{Path as AxPath, Query, State};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{err, ApiResult, App};

const GIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(6);
const MAX_DIFF_BYTES: usize = 200 * 1024;
const MAX_CHANGE_FILES: usize = 60;
const MAX_TEXT_BYTES: u64 = 256 * 1024;

#[derive(Deserialize)]
pub struct GitQuery {
    #[serde(default)]
    pub session: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
    /// diff the STAGED (index) side instead of the worktree side
    #[serde(default)]
    pub staged: Option<bool>,
}

/// The session's project workdir, else the shared workspace — same resolution the agent uses.
fn resolve_workdir(app: &App, session: Option<&str>) -> PathBuf {
    session
        .and_then(|sid| app.workspace.workdir_for_session(sid))
        .unwrap_or_else(|| app.workdir.clone())
}

/// Run `git <args>` in `cwd` with a timeout. Fixed argv — nothing here passes through a shell.
async fn run_git(cwd: &Path, args: &[&str]) -> Result<String, String> {
    let mut cmd = tokio::process::Command::new("git");
    cmd.args(args)
        .current_dir(cwd)
        .env("GIT_TERMINAL_PROMPT", "0") // never hang on a credential prompt
        .stdin(std::process::Stdio::null());
    let fut = cmd.output();
    let out = tokio::time::timeout(GIT_TIMEOUT, fut)
        .await
        .map_err(|_| "git timed out".to_string())?
        .map_err(|e| format!("git unavailable: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// A client-supplied relative path, contained to the workdir (no traversal, no absolute escapes).
fn contained(workdir: &Path, rel: &str) -> Option<PathBuf> {
    if rel.is_empty() || rel.starts_with('/') || rel.contains("..") {
        return None;
    }
    let joined = workdir.join(rel);
    // The file may be deleted (diffing a deletion), so canonicalize the PARENT.
    let parent = joined.parent()?.canonicalize().ok()?;
    let root = workdir.canonicalize().ok()?;
    parent.starts_with(&root).then_some(joined)
}

/// GET /v1/git/status?session= — branch, ahead/behind, staged/unstaged/untracked, recent commits.
pub async fn git_status(State(app): State<App>, Query(q): Query<GitQuery>) -> ApiResult {
    let wd = resolve_workdir(&app, q.session.as_deref());
    if !wd.join(".git").exists() {
        // Walking up to a parent repo is a choice, not an accident — report honestly instead.
        return Ok(Json(json!({ "repo": false, "workdir": wd.to_string_lossy() })));
    }
    let porcelain = run_git(&wd, &["status", "--porcelain=v2", "--branch"])
        .await
        .map_err(err)?;
    let mut branch = String::new();
    let (mut ahead, mut behind) = (0i64, 0i64);
    let mut staged: Vec<Value> = Vec::new();
    let mut unstaged: Vec<Value> = Vec::new();
    let mut untracked: Vec<Value> = Vec::new();
    for line in porcelain.lines() {
        if let Some(rest) = line.strip_prefix("# branch.head ") {
            branch = rest.trim().to_string();
        } else if let Some(rest) = line.strip_prefix("# branch.ab ") {
            for tok in rest.split_whitespace() {
                if let Some(n) = tok.strip_prefix('+') {
                    ahead = n.parse().unwrap_or(0);
                } else if let Some(n) = tok.strip_prefix('-') {
                    behind = n.parse().unwrap_or(0);
                }
            }
        } else if let Some(rest) = line.strip_prefix("? ") {
            untracked.push(json!({ "path": rest, "status": "untracked" }));
        } else if line.starts_with("1 ") || line.starts_with("2 ") {
            // "1 XY sub mH mI mW hH hI path" — XY: index then worktree status letters.
            let mut toks = line.split(' ');
            let _kind = toks.next();
            let xy = toks.next().unwrap_or("..");
            let path = line.split(' ').nth(8).unwrap_or("").to_string();
            if path.is_empty() {
                continue;
            }
            let (x, y) = (xy.chars().next().unwrap_or('.'), xy.chars().nth(1).unwrap_or('.'));
            let name = |c: char| match c {
                'M' => "modified",
                'A' => "added",
                'D' => "deleted",
                'R' => "renamed",
                'C' => "copied",
                _ => "changed",
            };
            if x != '.' {
                staged.push(json!({ "path": path, "status": name(x) }));
            }
            if y != '.' {
                unstaged.push(json!({ "path": path, "status": name(y) }));
            }
        }
    }
    // Recent history: hash / subject / author / epoch-seconds, unit-separated so subjects with
    // spaces survive. An empty repo (no commits yet) is fine — log just errors and we show none.
    let log_raw = run_git(
        &wd,
        &["log", "-n", "15", "--pretty=format:%h\u{1f}%s\u{1f}%an\u{1f}%ct"],
    )
    .await
    .unwrap_or_default();
    let log: Vec<Value> = log_raw
        .lines()
        .filter_map(|l| {
            let p: Vec<&str> = l.split('\u{1f}').collect();
            (p.len() == 4).then(|| {
                json!({ "hash": p[0], "subject": p[1], "author": p[2],
                        "ts_ms": p[3].parse::<i64>().unwrap_or(0) * 1000 })
            })
        })
        .collect();
    Ok(Json(json!({
        "repo": true,
        "workdir": wd.to_string_lossy(),
        "branch": branch,
        "ahead": ahead,
        "behind": behind,
        "staged": staged,
        "unstaged": unstaged,
        "untracked": untracked,
        "log": log,
    })))
}

/// GET /v1/git/diff?session=&path=&staged= — one file's unified diff (worktree or staged side).
/// Untracked files diff against /dev/null so "what would this add" is still visible.
pub async fn git_diff(State(app): State<App>, Query(q): Query<GitQuery>) -> ApiResult {
    let wd = resolve_workdir(&app, q.session.as_deref());
    let rel = q.path.as_deref().unwrap_or("");
    let abs = contained(&wd, rel).ok_or_else(|| err("path escapes the working directory"))?;
    let mut text = if q.staged == Some(true) {
        run_git(&wd, &["diff", "--cached", "--", rel]).await.map_err(err)?
    } else {
        let d = run_git(&wd, &["diff", "--", rel]).await.map_err(err)?;
        if d.is_empty() && abs.is_file() {
            // Untracked: no-index against the null device produces a plain "new file" diff.
            // git exits 1 when the trees differ here, so run it raw and accept that exit code.
            let out = tokio::time::timeout(
                GIT_TIMEOUT,
                tokio::process::Command::new("git")
                    .args(["diff", "--no-index", "--", "/dev/null"])
                    .arg(&abs)
                    .current_dir(&wd)
                    .output(),
            )
            .await
            .map_err(|_| err("git timed out"))?
            .map_err(|e| err(format!("git unavailable: {e}")))?;
            String::from_utf8_lossy(&out.stdout).into_owned()
        } else {
            d
        }
    };
    let truncated = text.len() > MAX_DIFF_BYTES;
    if truncated {
        text.truncate(MAX_DIFF_BYTES);
        text.push_str("\n… (truncated)");
    }
    Ok(Json(json!({ "path": rel, "diff": text, "truncated": truncated })))
}

// ---- per-task changes: checkpoint delta, no git required --------------------------------------

/// GET /v1/tasks/{id}/changes — files the run touched, with unified diffs for text files, computed
/// against the auto-checkpoint taken just before the run.
pub async fn task_changes(State(app): State<App>, AxPath(id): AxPath<String>) -> ApiResult {
    // Latest checkpoint recorded for this task (snapshot() tags it with the task id).
    let cps = crate::checkpoints::list(&app.home);
    let cp = cps
        .into_iter()
        .filter(|c| c.task.as_deref() == Some(id.as_str()))
        .max_by_key(|c| c.created_ms)
        .ok_or_else(|| err("no checkpoint recorded for this task"))?;
    let cp_root = Path::new(&app.home).join("checkpoints").join(&cp.id).join("files");
    let workdir = PathBuf::from(&cp.workdir);
    if !workdir.is_dir() {
        return Err(err("the task's working directory no longer exists"));
    }

    // Current tree, bounded the same way the snapshot was.
    let now_files = crate::checkpoints::collect_tree(&workdir, &app.home);
    let before: BTreeMap<&str, ()> = cp.files.iter().map(|f| (f.as_str(), ())).collect();

    let mut changes: Vec<Value> = Vec::new();
    let mut more = 0usize;
    let mut push = |v: Value, changes: &mut Vec<Value>| {
        if changes.len() < MAX_CHANGE_FILES {
            changes.push(v);
        } else {
            more += 1;
        }
    };

    // Modified + added (present now).
    for rel in &now_files {
        let old_p = cp_root.join(rel);
        let new_p = workdir.join(rel);
        let existed = before.contains_key(rel.as_str());
        if existed {
            if files_equal(&old_p, &new_p) {
                continue;
            }
            let (diff, adds, dels) = unified(&old_p, &new_p, rel);
            push(
                json!({ "path": rel, "status": "modified", "adds": adds, "dels": dels, "diff": diff }),
                &mut changes,
            );
        } else {
            let (diff, adds, _) = unified(Path::new("/dev/null"), &new_p, rel);
            push(
                json!({ "path": rel, "status": "added", "adds": adds, "dels": 0, "diff": diff }),
                &mut changes,
            );
        }
    }
    // Deleted (captured then, gone now).
    let now_set: BTreeMap<&str, ()> = now_files.iter().map(|f| (f.as_str(), ())).collect();
    for rel in &cp.files {
        if !now_set.contains_key(rel.as_str()) {
            let (diff, _, dels) = unified(&cp_root.join(rel), Path::new("/dev/null"), rel);
            push(
                json!({ "path": rel, "status": "deleted", "adds": 0, "dels": dels, "diff": diff }),
                &mut changes,
            );
        }
    }

    Ok(Json(json!({
        "task": id,
        "checkpoint": cp.id,
        "label": cp.label,
        "workdir": cp.workdir,
        "truncated_snapshot": cp.truncated,
        "files": changes,
        "more": more,
    })))
}

fn files_equal(a: &Path, b: &Path) -> bool {
    match (std::fs::metadata(a), std::fs::metadata(b)) {
        (Ok(ma), Ok(mb)) => {
            ma.len() == mb.len()
                && std::fs::read(a).ok() == std::fs::read(b).ok()
        }
        _ => false,
    }
}

/// A unified diff between two files ("/dev/null" for absent sides). Binary or oversized content
/// degrades to a one-line note instead of a garbage diff. Returns (diff_text, adds, dels).
fn unified(old: &Path, new: &Path, rel: &str) -> (String, usize, usize) {
    let read = |p: &Path| -> Option<String> {
        if p == Path::new("/dev/null") {
            return Some(String::new());
        }
        let meta = std::fs::metadata(p).ok()?;
        if meta.len() > MAX_TEXT_BYTES {
            return None;
        }
        let bytes = std::fs::read(p).ok()?;
        if bytes.contains(&0) {
            return None; // binary
        }
        Some(String::from_utf8_lossy(&bytes).into_owned())
    };
    let (Some(a), Some(b)) = (read(old), read(new)) else {
        return (format!("(binary or oversized file: {rel})"), 0, 0);
    };
    let diff = similar::TextDiff::from_lines(&a, &b);
    let (mut adds, mut dels) = (0usize, 0usize);
    for ch in diff.iter_all_changes() {
        match ch.tag() {
            similar::ChangeTag::Insert => adds += 1,
            similar::ChangeTag::Delete => dels += 1,
            similar::ChangeTag::Equal => {}
        }
    }
    let text = diff
        .unified_diff()
        .context_radius(3)
        .header(&format!("a/{rel}"), &format!("b/{rel}"))
        .to_string();
    let text = if text.len() > MAX_DIFF_BYTES {
        format!("{}\n… (truncated)", &text[..MAX_DIFF_BYTES])
    } else {
        text
    };
    (text, adds, dels)
}
