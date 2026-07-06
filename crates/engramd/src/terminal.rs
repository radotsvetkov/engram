//! Glass-box terminal - the human's half of the shell.
//!
//! The agent's shell tool already runs behind a gate (it must be explicitly enabled), a taint
//! guard, and a signed ledger entry. A terminal that bypassed all that would punch a
//! human-shaped hole in the glass box: actions nobody could later audit. So commands typed here
//! run through the same `allow_shell` gate and the same backend selection as the agent, and each
//! one is signed into the ledger as `terminal.exec` by actor `user` - deliberately distinct from
//! the agent's `agent.shell`, so a receipt can always say who ran what. `/v1/fs` (list) and
//! `/v1/fs/read` (preview one file) are the file browser that sits beside it; `/v1/fs/write`
//! lets the human save an edit from that same preview, signed as `file.write` by actor `user` -
//! distinct from the agent's `agent.write`, for the same who-did-what reason.

use std::path::PathBuf;
use std::time::Duration;

use axum::extract::{Query, State};
use axum::http::{header, HeaderValue, StatusCode};
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
    /// "1"/"true" to include dotfiles (hidden by default).
    #[serde(default)]
    pub hidden: Option<String>,
}

#[derive(Deserialize)]
pub struct FsReadQuery {
    #[serde(default)]
    pub path: Option<String>,
    /// "1"/"true" to stream the raw bytes (images, downloads) instead of a JSON text preview.
    #[serde(default)]
    pub raw: Option<String>,
    /// "1"/"true" to serve inline even for non-raster types (the explicit "open in browser"
    /// path) - same escape hatch `/v1/artifact` has.
    #[serde(default)]
    pub view: Option<String>,
}

#[derive(Deserialize)]
pub struct FsWriteReq {
    pub path: String,
    pub content: String,
}

fn shell_enabled(app: &App) -> bool {
    app.allow_shell.load(std::sync::atomic::Ordering::Relaxed)
}

/// The shell backend for the human terminal - resolved exactly like the agent task runner's
/// (`run_agent_task_cb` in main.rs), so a human command and an agent command run in the SAME place
/// (host, the UI-selected OS sandbox / Docker container, ssh, ...). The live Settings-panel backend
/// wins; the `ENGRAM_SHELL_BACKEND` env vars are the headless/server fallback. This is what keeps the
/// glass-box promise honest: without it a user who selects sandbox/docker/ssh in Settings would get
/// raw host execution here while the daemon signed it as if it ran where agent commands run.
fn backend(app: &App) -> Option<String> {
    let resolved = {
        let c = app.cfg();
        crate::config::resolve_shell_backend(&c.security.shell_backend, &c.security.shell_target)
    };
    resolved.or_else(|| match std::env::var("ENGRAM_SHELL_BACKEND").as_deref() {
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
    })
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

    let (program, args) =
        engram_agent::tools::shell_command(backend(&app).as_deref(), &cwd, &command);
    // Set PWD to match the real working directory: the `pwd`/`$PWD` shell builtins trust an
    // inherited PWD over getcwd(), so without this they'd echo the daemon's stale PWD, not `cwd`.
    // kill_on_drop(true): when the 60s timeout fires the output() future is dropped — without this
    // the spawned child would keep running detached, still executing side effects while we report a
    // timeout, and repeated hangs would leak processes. Dropping the child now sends it SIGKILL.
    let fut = tokio::process::Command::new(&program)
        .args(&args)
        .current_dir(&cwd)
        .env("PWD", &cwd)
        .kill_on_drop(true)
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

fn flag(v: Option<&str>) -> bool {
    matches!(v, Some("1") | Some("true"))
}

/// List a directory for the file browser (dirs first, dotfiles hidden unless `hidden=1`, capped).
/// Read-only, but gated behind the same shell switch so the whole files-and-shell surface is one
/// consent.
pub async fn fs_handler(State(app): State<App>, Query(q): Query<FsQuery>) -> Response {
    if !shell_enabled(&app) {
        return Json(json!({ "denied": true, "error": "Shell access is off." })).into_response();
    }
    let show_hidden = flag(q.hidden.as_deref());
    let dir = resolve_dir(&app, q.path.as_deref());
    let mut entries: Vec<(bool, String, u64)> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten().take(4000) {
            let name = e.file_name().to_string_lossy().into_owned();
            if !show_hidden && name.starts_with('.') {
                continue; // hide dotfiles by default
            }
            // std::fs::metadata follows symlinks (DirEntry::file_type doesn't), so a linked
            // folder browses as a folder instead of posing as an unopenable zero-byte file.
            let md = std::fs::metadata(e.path()).ok();
            let is_dir = md.as_ref().map(|m| m.is_dir()).unwrap_or(false);
            let size = if is_dir {
                0
            } else {
                md.as_ref().map(|m| m.len()).unwrap_or(0)
            };
            entries.push((is_dir, name, size));
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
        .map(|(dir, name, size)| json!({ "dir": dir, "name": name, "size": size }))
        .collect();
    Json(json!({
        "path": dir.display().to_string(),
        "parent": dir.parent().map(|p| p.display().to_string()),
        "entries": list,
    }))
    .into_response()
}

/// Best-effort content type for raw file serving - enough for the preview pane to show images
/// and for the browser to open PDFs; everything unrecognized downloads as octet-stream.
fn mime_for(name: &str) -> &'static str {
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "bmp" => "image/bmp",
        "ico" => "image/x-icon",
        "pdf" => "application/pdf",
        "html" | "htm" => "text/html; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        "txt" | "md" | "log" | "csv" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

/// Whether a raw file may render inline in the dashboard origin. Only raster images are safe;
/// SVG/HTML (scriptable) and everything else are forced to `attachment` so a file dropped into
/// the workdir can't execute same-origin when someone navigates to its raw URL. `view=1` (the
/// explicit open-in-browser path) opts back in. Mirrors `/v1/artifact`'s policy.
fn serves_inline(mime: &str, view: bool) -> bool {
    view || matches!(
        mime,
        "image/png" | "image/jpeg" | "image/gif" | "image/webp" | "image/bmp" | "image/x-icon"
    )
}

/// How much of a file the JSON preview returns; bigger files are truncated with a flag so the UI
/// can say so and offer the raw download instead.
const PREVIEW_CAP: u64 = 1_500_000;
/// Hard cap for raw serving - the drawer is a preview, not a file server.
const RAW_CAP: u64 = 64 * 1024 * 1024;

/// Read one file for the files-drawer preview. Default: JSON `{name,size,mtime_ms,binary,
/// truncated,content}` with a capped UTF-8 preview; `raw=1`: the bytes themselves with a guessed
/// content type (for `<img>`, PDFs, and the Download link). Same read-only consent gate as
/// `/v1/fs`; like directory listings, reads aren't ledger events - the ledger records intent
/// (commands), not browsing.
pub async fn fs_read_handler(State(app): State<App>, Query(q): Query<FsReadQuery>) -> Response {
    if !shell_enabled(&app) {
        return Json(json!({ "denied": true, "error": "Shell access is off." })).into_response();
    }
    let Some(req) = q.path.as_deref().filter(|p| !p.trim().is_empty()) else {
        return (StatusCode::BAD_REQUEST, "missing path").into_response();
    };
    let path = match std::fs::canonicalize(req) {
        Ok(p) if p.is_file() => p,
        _ => return (StatusCode::NOT_FOUND, "not a file").into_response(),
    };
    let meta = match std::fs::metadata(&path) {
        Ok(m) => m,
        Err(e) => return (StatusCode::NOT_FOUND, e.to_string()).into_response(),
    };
    let size = meta.len();
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".into());
    let mtime_ms = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64);

    if flag(q.raw.as_deref()) {
        // Enforce the cap on the actual read, not just the stat - a file that grows between the
        // two would otherwise balloon the response vec past the "hard cap" the comment promises.
        use std::io::Read;
        let mut bytes = Vec::new();
        match std::fs::File::open(&path) {
            Ok(f) => {
                if let Err(e) = f.take(RAW_CAP + 1).read_to_end(&mut bytes) {
                    return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
                }
            }
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
        if bytes.len() as u64 > RAW_CAP {
            return (StatusCode::PAYLOAD_TOO_LARGE, "file larger than 64MB").into_response();
        }
        let mime = mime_for(&name);
        let inline = serves_inline(mime, flag(q.view.as_deref()));
        let mut resp = bytes.into_response();
        resp.headers_mut()
            .insert(header::CONTENT_TYPE, HeaderValue::from_static(mime));
        resp.headers_mut().insert(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        );
        let dispo = format!(
            "{}; filename=\"{}\"",
            if inline { "inline" } else { "attachment" },
            name.replace(['"', '\\'], "_")
        );
        if let Ok(v) = HeaderValue::from_str(&dispo) {
            resp.headers_mut().insert(header::CONTENT_DISPOSITION, v);
        }
        return resp;
    }

    use std::io::Read;
    let mut buf = Vec::new();
    match std::fs::File::open(&path) {
        Ok(f) => {
            if let Err(e) = f.take(PREVIEW_CAP + 1).read_to_end(&mut buf) {
                return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
            }
        }
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
    let truncated = buf.len() as u64 > PREVIEW_CAP;
    if truncated {
        buf.truncate(PREVIEW_CAP as usize);
    }
    // NUL in the head = not text. Lossy UTF-8 is fine for a preview pane.
    let binary = buf.iter().take(8192).any(|&b| b == 0);
    let content = if binary {
        String::new()
    } else {
        String::from_utf8_lossy(&buf).into_owned()
    };
    Json(json!({
        "path": path.display().to_string(),
        "name": name,
        "size": size,
        "mtime_ms": mtime_ms,
        "binary": binary,
        "truncated": truncated,
        "content": content,
    }))
    .into_response()
}

/// Cap on a single save from the preview pane's editor - generous for any file a human would
/// reasonably hand-edit there, but not an open-ended write sink.
const WRITE_CAP: usize = 20 * 1024 * 1024;

/// Resolve a write target: the parent must already exist (so this can't create arbitrary new
/// directories), but the file itself doesn't have to - saving recreates it if it was deleted out
/// from under the preview between load and save. Pure/testable, split out of the handler like
/// `resolve_dir` is.
fn resolve_write_target(requested: &str) -> Result<PathBuf, &'static str> {
    let raw = PathBuf::from(requested);
    let file_name = raw.file_name().ok_or("missing file name")?;
    let parent = match raw.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => return Err("missing parent directory"),
    };
    match std::fs::canonicalize(parent) {
        Ok(p) if p.is_dir() => Ok(p.join(file_name)),
        _ => Err("parent directory not found"),
    }
}

/// Save an edit made in the files-drawer preview pane. Same one-switch consent as everything
/// else here (`allow_shell`); like `/v1/shell`, the human isn't confined to the project workdir -
/// they already granted file-system access on their own machine.
pub async fn fs_write_handler(State(app): State<App>, Json(r): Json<FsWriteReq>) -> Response {
    if !shell_enabled(&app) {
        return Json(json!({ "denied": true, "error": "Shell access is off." })).into_response();
    }
    if r.content.len() > WRITE_CAP {
        return (StatusCode::PAYLOAD_TOO_LARGE, "content larger than 20MB").into_response();
    }
    let path = match resolve_write_target(&r.path) {
        Ok(p) => p,
        Err(e) => return (StatusCode::NOT_FOUND, e).into_response(),
    };
    if let Err(e) = std::fs::write(&path, r.content.as_bytes()) {
        return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
    }
    let meta = std::fs::metadata(&path).ok();
    let size = meta
        .as_ref()
        .map(|m| m.len())
        .unwrap_or(r.content.len() as u64);
    let mtime_ms = meta
        .as_ref()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64);
    let entry = app
        .ledger
        .append(
            "file.write",
            "user",
            json!({ "path": path.display().to_string(), "bytes": r.content.len() }),
        )
        .ok();
    Json(json!({
        "ok": true,
        "path": path.display().to_string(),
        "size": size,
        "mtime_ms": mtime_ms,
        "seq": entry.as_ref().map(|e| e.seq),
        "hash": entry.as_ref().map(|e| e.hash.clone()),
    }))
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scriptable_types_never_render_inline_without_view() {
        // The whole point of the raw-serving policy: a repo-cloned or agent-written .html/.svg in
        // the workdir must download, not execute same-origin.
        assert!(!serves_inline(mime_for("evil.html"), false));
        assert!(!serves_inline(mime_for("evil.htm"), false));
        assert!(!serves_inline(mime_for("evil.svg"), false));
        assert!(!serves_inline(mime_for("data.json"), false));
        assert!(!serves_inline(mime_for("notes.txt"), false));
        // Raster images are safe to inline (so <img> previews work).
        assert!(serves_inline(mime_for("shot.png"), false));
        assert!(serves_inline(mime_for("photo.JPEG"), false));
        assert!(serves_inline(mime_for("anim.gif"), false));
        // view=1 (explicit open-in-browser) opts anything back into inline.
        assert!(serves_inline(mime_for("evil.html"), true));
        assert!(serves_inline(mime_for("doc.pdf"), true));
    }

    #[test]
    fn mime_guess_covers_the_preview_whitelist() {
        assert_eq!(mime_for("a.png"), "image/png");
        assert_eq!(mime_for("a.webp"), "image/webp");
        assert_eq!(mime_for("a.svg"), "image/svg+xml");
        assert_eq!(mime_for("a.pdf"), "application/pdf");
        assert_eq!(mime_for("noext"), "application/octet-stream");
        assert_eq!(mime_for("UPPER.PNG"), "image/png");
    }

    #[test]
    fn flag_only_accepts_the_truthy_forms() {
        assert!(flag(Some("1")));
        assert!(flag(Some("true")));
        assert!(!flag(Some("0")));
        assert!(!flag(Some("")));
        assert!(!flag(None));
    }

    #[test]
    fn resolve_write_target_recreates_a_deleted_file_but_never_a_missing_directory() {
        let dir = tempfile::tempdir().unwrap();
        let existing = dir.path().join("notes.md");
        std::fs::write(&existing, "original").unwrap();

        // Overwriting an existing file resolves to itself.
        let resolved = resolve_write_target(existing.to_str().unwrap()).unwrap();
        assert_eq!(std::fs::canonicalize(&resolved).unwrap(), resolved);
        assert_eq!(resolved.file_name().unwrap(), "notes.md");

        // A file that no longer exists but whose PARENT does still resolves - saving recreates it.
        std::fs::remove_file(&existing).unwrap();
        let resolved = resolve_write_target(existing.to_str().unwrap()).unwrap();
        assert_eq!(resolved.file_name().unwrap(), "notes.md");
        assert_eq!(
            resolved.parent().unwrap(),
            std::fs::canonicalize(dir.path()).unwrap()
        );

        // A parent directory that doesn't exist is rejected outright - this must never silently
        // create directories on the user's disk.
        let missing_parent = dir.path().join("nonexistent-subdir").join("file.txt");
        assert!(resolve_write_target(missing_parent.to_str().unwrap()).is_err());
    }
}
