//! Glass-box terminal - the human's half of the shell.
//!
//! The agent's shell tool already runs behind a gate (it must be explicitly enabled), a taint
//! guard, and a signed ledger entry. A terminal that bypassed all that would punch a
//! human-shaped hole in the glass box: actions nobody could later audit. So commands typed here
//! run through the same `allow_shell` gate and the same backend selection as the agent, and each
//! one is signed into the ledger as `terminal.exec` by actor `user` - deliberately distinct from
//! the agent's `agent.shell`, so a receipt can always say who ran what. `/v1/fs` is the
//! read-only file tree that sits beside it.

use std::path::PathBuf;
use std::time::Duration;

use axum::extract::{Query, State};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use crate::App;

#[derive(Deserialize)]
pub struct ShellReq {
    pub command: String,
    #[serde(default)]
    pub cwd: Option<String>,
}

#[derive(Deserialize)]
pub struct FsQuery {
    #[serde(default)]
    pub path: Option<String>,
}

fn shell_enabled(app: &App) -> bool {
    app.allow_shell.load(std::sync::atomic::Ordering::Relaxed)
}

/// The shell backend selected by env - identical to the agent task runner's, so a human command
/// and an agent command run in the same place (local, a network-isolated container, ssh, ...).
fn backend() -> Option<String> {
    match std::env::var("ENGRAM_SHELL_BACKEND").as_deref() {
        Ok("docker") => {
            Some(std::env::var("ENGRAM_DOCKER_IMAGE").unwrap_or_else(|_| "alpine".into()))
        }
        Ok("ssh") => std::env::var("ENGRAM_SSH_HOST")
            .ok()
            .map(|h| format!("ssh:{h}")),
        Ok("singularity") => std::env::var("ENGRAM_SINGULARITY_IMAGE")
            .ok()
            .map(|i| format!("singularity:{i}")),
        _ => None,
    }
}

fn home() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/"))
}

/// Resolve a requested directory to an existing one, defaulting to the workdir. The human
/// terminal isn't confined to the workdir (the user enabled shell on their own machine), but a
/// path that doesn't resolve to a real directory falls back rather than erroring.
fn resolve_dir(app: &App, requested: Option<&str>) -> PathBuf {
    let cand = match requested {
        Some(p) if !p.trim().is_empty() => PathBuf::from(p),
        _ => app.workdir.clone(),
    };
    match std::fs::canonicalize(&cand) {
        Ok(c) if c.is_dir() => c,
        _ => app.workdir.clone(),
    }
}

pub async fn shell_handler(State(app): State<App>, Json(r): Json<ShellReq>) -> Response {
    if !shell_enabled(&app) {
        return Json(json!({
            "denied": true,
            "error": "Shell access is off. Enable it in Settings \u{203a} Security to use the terminal.",
        }))
        .into_response();
    }
    let command = r.command.trim().to_string();
    let cwd = resolve_dir(&app, r.cwd.as_deref());
    if command.is_empty() {
        return Json(json!({ "ok": true, "stdout": "", "stderr": "", "exit": 0, "cwd": cwd.display().to_string() }))
            .into_response();
    }

    // `cd` is handled here: each command is its own process, so a child's chdir would vanish.
    // Resolve it against the current cwd and hand the new directory back to the client, which
    // threads it into the next command. A bare `cd` goes home, like a real shell.
    if command == "cd" || command.starts_with("cd ") {
        let target = command.strip_prefix("cd").unwrap_or("").trim();
        let dest = if target.is_empty() || target == "~" {
            home()
        } else if let Some(rest) = target.strip_prefix("~/") {
            home().join(rest)
        } else {
            let p = PathBuf::from(target);
            if p.is_absolute() {
                p
            } else {
                cwd.join(p)
            }
        };
        return match std::fs::canonicalize(&dest) {
            Ok(c) if c.is_dir() => {
                let entry = app
                    .ledger
                    .append(
                        "terminal.exec",
                        "user",
                        json!({ "command": command, "cwd": c.display().to_string() }),
                    )
                    .ok();
                Json(json!({
                    "ok": true, "stdout": "", "stderr": "", "exit": 0,
                    "cwd": c.display().to_string(),
                    "seq": entry.as_ref().map(|e| e.seq),
                    "hash": entry.as_ref().map(|e| e.hash.clone()),
                }))
                .into_response()
            }
            _ => Json(json!({
                "ok": false, "stdout": "", "stderr": format!("cd: no such directory: {target}"),
                "exit": 1, "cwd": cwd.display().to_string(),
            }))
            .into_response(),
        };
    }

    let (program, args) = engram_agent::tools::shell_command(backend().as_deref(), &cwd, &command);
    // Set PWD to match the real working directory: the `pwd`/`$PWD` shell builtins trust an
    // inherited PWD over getcwd(), so without this they'd echo the daemon's stale PWD, not `cwd`.
    let fut = tokio::process::Command::new(&program)
        .args(&args)
        .current_dir(&cwd)
        .env("PWD", &cwd)
        .output();
    let (exit, stdout, stderr) = match tokio::time::timeout(Duration::from_secs(60), fut).await {
        Ok(Ok(out)) => (
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        ),
        Ok(Err(e)) => (-1, String::new(), e.to_string()),
        Err(_) => (-1, String::new(), "command timed out after 60s".into()),
    };
    // Sign the human's command into the same ledger as the agent's - no human-shaped hole.
    let entry = app
        .ledger
        .append(
            "terminal.exec",
            "user",
            json!({ "command": command, "cwd": cwd.display().to_string(), "exit": exit }),
        )
        .ok();
    Json(json!({
        "ok": exit == 0, "exit": exit, "stdout": stdout, "stderr": stderr,
        "cwd": cwd.display().to_string(),
        "seq": entry.as_ref().map(|e| e.seq),
        "hash": entry.as_ref().map(|e| e.hash.clone()),
    }))
    .into_response()
}

/// List a directory for the file tree (dirs first, dotfiles hidden, capped). Read-only, but
/// gated behind the same shell switch so the whole terminal surface is one consent.
pub async fn fs_handler(State(app): State<App>, Query(q): Query<FsQuery>) -> Response {
    if !shell_enabled(&app) {
        return Json(json!({ "denied": true, "error": "Shell access is off." })).into_response();
    }
    let dir = resolve_dir(&app, q.path.as_deref());
    let mut entries: Vec<(bool, String)> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten().take(4000) {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue; // hide dotfiles by default
            }
            let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
            entries.push((is_dir, name));
        }
    }
    // Directories first, then case-insensitive by name.
    entries.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| a.1.to_lowercase().cmp(&b.1.to_lowercase()))
    });
    entries.truncate(500);
    let list: Vec<_> = entries
        .into_iter()
        .map(|(dir, name)| json!({ "dir": dir, "name": name }))
        .collect();
    Json(json!({
        "path": dir.display().to_string(),
        "parent": dir.parent().map(|p| p.display().to_string()),
        "entries": list,
    }))
    .into_response()
}
