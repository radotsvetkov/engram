//! Workdir checkpoints — snapshot the files in a working directory so a run's changes can be
//! REWOUND, the way Claude Code lets you rewind a session's edits. A checkpoint copies the workdir's
//! files into `<home>/checkpoints/<id>/files/` alongside a `manifest.json`; `restore` copies them
//! back, reverting edits and un-deleting files the run removed. It is deliberately SAFE: restore
//! overwrites/recreates the captured files but never deletes files that appeared after the checkpoint
//! (it reports them instead), so a rewind can't destroy work the snapshot never saw.
//!
//! Bounded (file count + per-file and total bytes) so a giant tree can't blow disk, and it skips the
//! usual heavy/derived dirs (`.git`, `node_modules`, `target`, and the checkpoints store itself).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

const MAX_FILES: usize = 2000;
const MAX_TOTAL_BYTES: u64 = 100 * 1024 * 1024; // 100 MB across the whole snapshot
const MAX_FILE_BYTES: u64 = 25 * 1024 * 1024; // skip any single file larger than this
static SEQ: AtomicU64 = AtomicU64::new(1);

/// A saved snapshot of a working directory at a point in time.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Checkpoint {
    pub id: String,
    pub label: String,
    pub created_ms: u64,
    pub workdir: String,
    pub file_count: usize,
    pub total_bytes: u64,
    /// Relative paths captured (POSIX separators). Restore recreates exactly these.
    pub files: Vec<String>,
    #[serde(default)]
    pub session: Option<String>,
    #[serde(default)]
    pub task: Option<String>,
    /// True if the caps were hit and the snapshot is partial (restore then can't fully rewind).
    #[serde(default)]
    pub truncated: bool,
}

fn store_dir(home: &str) -> PathBuf {
    Path::new(home).join("checkpoints")
}

fn skip_dir(name: &str) -> bool {
    matches!(name, ".git" | "node_modules" | "target" | ".engram_overflow")
}

/// Recursively collect files under `root` (relative paths), skipping heavy/derived dirs, files over
/// the per-file cap, and `exclude` (the checkpoints store itself, so a workdir that PARENTS the store
/// — e.g. workdir == home — can't recurse into and re-snapshot every prior checkpoint). Bounded by
/// MAX_FILES / MAX_TOTAL_BYTES; sets `truncated` when a cap is hit.
#[allow(clippy::too_many_arguments)]
fn collect(root: &Path, dir: &Path, rel: &mut PathBuf, out: &mut Vec<(PathBuf, u64)>, total: &mut u64, truncated: &mut bool, exclude: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        let path = entry.path();
        let Ok(ft) = entry.file_type() else { continue };
        if ft.is_symlink() {
            continue; // never follow symlinks (could escape the workdir)
        }
        if ft.is_dir() {
            if skip_dir(&name_str) || path == exclude {
                continue;
            }
            rel.push(&name);
            collect(root, &path, rel, out, total, truncated, exclude);
            rel.pop();
        } else if ft.is_file() {
            let len = entry.metadata().map(|m| m.len()).unwrap_or(0);
            if len > MAX_FILE_BYTES {
                *truncated = true;
                continue;
            }
            if out.len() >= MAX_FILES || *total + len > MAX_TOTAL_BYTES {
                *truncated = true;
                return;
            }
            *total += len;
            out.push((rel.join(&name), len));
        }
    }
}

/// Snapshot `workdir` into a new checkpoint under `<home>/checkpoints/<id>/`. `now_ms` is passed in
/// (the module never reads the clock itself). Returns the manifest, or an error string on IO failure.
pub fn snapshot(
    home: &str,
    workdir: &Path,
    label: &str,
    session: Option<String>,
    task: Option<String>,
    now_ms: u64,
) -> Result<Checkpoint, String> {
    if !workdir.is_dir() {
        return Err("workdir does not exist".into());
    }
    let id = format!("cp-{now_ms}-{}", SEQ.fetch_add(1, Ordering::Relaxed));
    let dest = store_dir(home).join(&id);
    let files_root = dest.join("files");
    std::fs::create_dir_all(&files_root).map_err(|e| e.to_string())?;

    let mut collected = Vec::new();
    let mut total = 0u64;
    let mut truncated = false;
    let store = store_dir(home);
    collect(workdir, workdir, &mut PathBuf::new(), &mut collected, &mut total, &mut truncated, &store);

    let mut files = Vec::with_capacity(collected.len());
    for (rel, _len) in &collected {
        let src = workdir.join(rel);
        let dst = files_root.join(rel);
        if let Some(parent) = dst.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if std::fs::copy(&src, &dst).is_ok() {
            files.push(rel.to_string_lossy().replace('\\', "/"));
        }
    }
    let cp = Checkpoint {
        id,
        label: label.to_string(),
        created_ms: now_ms,
        workdir: workdir.to_string_lossy().into_owned(),
        file_count: files.len(),
        total_bytes: total,
        files,
        session,
        task,
        truncated,
    };
    let manifest = serde_json::to_string_pretty(&cp).map_err(|e| e.to_string())?;
    std::fs::write(dest.join("manifest.json"), manifest).map_err(|e| e.to_string())?;
    // Bound disk use: the auto-snapshot before every non-git task run would otherwise grow the store
    // without limit. Keep the newest MAX_CHECKPOINTS and delete the rest.
    prune(home);
    Ok(cp)
}

/// Retention cap on the number of stored checkpoints (auto-snapshots accumulate on every task run).
const MAX_CHECKPOINTS: usize = 50;

/// Delete all but the newest [`MAX_CHECKPOINTS`] checkpoints. Best-effort.
fn prune(home: &str) {
    let mut cps = list(home); // newest first
    if cps.len() <= MAX_CHECKPOINTS {
        return;
    }
    for cp in cps.split_off(MAX_CHECKPOINTS) {
        let _ = std::fs::remove_dir_all(store_dir(home).join(&cp.id));
    }
}

/// Delete one checkpoint by id. Returns whether the store directory was removed. The id is confined to
/// a single path segment so it can't escape the store.
pub fn delete(home: &str, id: &str) -> bool {
    if id.is_empty() || id.contains('/') || id.contains('\\') || id.contains("..") {
        return false;
    }
    std::fs::remove_dir_all(store_dir(home).join(id)).is_ok()
}

/// Every checkpoint's manifest, newest first.
pub fn list(home: &str) -> Vec<Checkpoint> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(store_dir(home)) {
        for entry in entries.flatten() {
            let m = entry.path().join("manifest.json");
            if let Ok(text) = std::fs::read_to_string(&m) {
                if let Ok(cp) = serde_json::from_str::<Checkpoint>(&text) {
                    out.push(cp);
                }
            }
        }
    }
    out.sort_by(|a, b| b.created_ms.cmp(&a.created_ms));
    out
}

/// The outcome of a restore: how many files were reverted, and which files now in the workdir were
/// NOT in the checkpoint (created after it). Those are left in place — restore never deletes them.
#[derive(Debug, Serialize)]
pub struct RestoreResult {
    pub restored: usize,
    pub created_since: Vec<String>,
}

/// Restore checkpoint `id` back into its recorded workdir: recreate/overwrite every captured file
/// (reverting edits and un-deleting removed files). Files created after the checkpoint are reported
/// in `created_since` but NOT deleted (safe rewind). Errors if the checkpoint is unknown.
pub fn restore(home: &str, id: &str) -> Result<RestoreResult, String> {
    // Confine `id` to a single path segment so it can't escape the checkpoints store.
    if id.is_empty() || id.contains('/') || id.contains('\\') || id.contains("..") {
        return Err("invalid checkpoint id".into());
    }
    let dest = store_dir(home).join(id);
    let manifest = std::fs::read_to_string(dest.join("manifest.json"))
        .map_err(|_| "no such checkpoint".to_string())?;
    let cp: Checkpoint = serde_json::from_str(&manifest).map_err(|e| e.to_string())?;
    let workdir = PathBuf::from(&cp.workdir);
    let files_root = dest.join("files");

    // Confinement root: the canonicalized workdir. Restore must never write outside it, even if the
    // run replaced a captured file (or a parent dir) with a symlink pointing elsewhere between the
    // snapshot and now (the shell tool can do `ln -sf ~/.ssh/authorized_keys config.txt`). We (a)
    // refuse any rel with a `..` component, (b) verify each recreated parent still resolves inside the
    // root (so a symlinked parent can't redirect the write), and (c) delete a symlink AT the target
    // before copying so `fs::copy` creates a fresh regular file instead of following the link.
    let root = std::fs::canonicalize(&workdir).unwrap_or_else(|_| workdir.clone());
    let mut restored = 0usize;
    for rel in &cp.files {
        let relp = std::path::Path::new(rel);
        if relp
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir | std::path::Component::RootDir))
        {
            continue;
        }
        let dst = workdir.join(relp);
        if let Some(parent) = dst.parent() {
            let _ = std::fs::create_dir_all(parent);
            match std::fs::canonicalize(parent) {
                Ok(p) if p.starts_with(&root) => {}
                _ => continue, // parent escaped the workdir (symlinked) or unresolvable → skip
            }
        }
        if let Ok(md) = std::fs::symlink_metadata(&dst) {
            if md.file_type().is_symlink() {
                let _ = std::fs::remove_file(&dst);
            }
        }
        let src = files_root.join(rel);
        if std::fs::copy(&src, &dst).is_ok() {
            restored += 1;
        }
    }

    // Report (don't delete) files that exist in the workdir now but weren't captured.
    let captured: std::collections::HashSet<&str> = cp.files.iter().map(|s| s.as_str()).collect();
    let mut current = Vec::new();
    let mut total = 0u64;
    let mut trunc = false;
    let store = store_dir(home);
    collect(&workdir, &workdir, &mut PathBuf::new(), &mut current, &mut total, &mut trunc, &store);
    let created_since: Vec<String> = current
        .into_iter()
        .map(|(rel, _)| rel.to_string_lossy().replace('\\', "/"))
        .filter(|p| !captured.contains(p.as_str()))
        .collect();

    Ok(RestoreResult { restored, created_since })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_then_restore_reverts_edits_and_undeletes() {
        let home = std::env::temp_dir().join(format!("engram-cp-home-{}", std::process::id()));
        let work = std::env::temp_dir().join(format!("engram-cp-work-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let _ = std::fs::remove_dir_all(&work);
        std::fs::create_dir_all(work.join("sub")).unwrap();
        std::fs::write(work.join("a.txt"), "original A").unwrap();
        std::fs::write(work.join("sub/b.txt"), "original B").unwrap();

        let cp = snapshot(home.to_str().unwrap(), &work, "before", None, None, 1000).unwrap();
        assert_eq!(cp.file_count, 2);

        // Mutate: edit a.txt, delete sub/b.txt, add c.txt (a "new" file).
        std::fs::write(work.join("a.txt"), "CHANGED").unwrap();
        std::fs::remove_file(work.join("sub/b.txt")).unwrap();
        std::fs::write(work.join("c.txt"), "new file").unwrap();

        let res = restore(home.to_str().unwrap(), &cp.id).unwrap();
        assert_eq!(res.restored, 2);
        // Edits reverted, deletion undone.
        assert_eq!(std::fs::read_to_string(work.join("a.txt")).unwrap(), "original A");
        assert_eq!(std::fs::read_to_string(work.join("sub/b.txt")).unwrap(), "original B");
        // The new file is reported but NOT deleted (safe rewind).
        assert!(res.created_since.contains(&"c.txt".to_string()));
        assert!(work.join("c.txt").exists());

        // list() sees the checkpoint.
        assert_eq!(list(home.to_str().unwrap()).len(), 1);

        // Path-traversal ids are refused.
        assert!(restore(home.to_str().unwrap(), "../etc").is_err());

        let _ = std::fs::remove_dir_all(&home);
        let _ = std::fs::remove_dir_all(&work);
    }

    #[cfg(unix)]
    #[test]
    fn restore_does_not_follow_a_planted_symlink_out_of_the_workdir() {
        use std::os::unix::fs::symlink;
        let home = std::env::temp_dir().join(format!("engram-cp-h2-{}", std::process::id()));
        let work = std::env::temp_dir().join(format!("engram-cp-w2-{}", std::process::id()));
        let outside = std::env::temp_dir().join(format!("engram-cp-out-{}", std::process::id()));
        for d in [&home, &work, &outside] {
            let _ = std::fs::remove_dir_all(d);
        }
        std::fs::create_dir_all(&work).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(work.join("config.txt"), "OLD SECRET").unwrap();
        let victim = outside.join("victim.txt");
        std::fs::write(&victim, "DO NOT OVERWRITE").unwrap();

        let cp = snapshot(home.to_str().unwrap(), &work, "before", None, None, 1000).unwrap();

        // The run replaces the captured regular file with a symlink pointing OUTSIDE the workdir.
        std::fs::remove_file(work.join("config.txt")).unwrap();
        symlink(&victim, work.join("config.txt")).unwrap();

        let res = restore(home.to_str().unwrap(), &cp.id).unwrap();
        assert_eq!(res.restored, 1);
        // The outside target must be UNTOUCHED (the copy must not follow the symlink) ...
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "DO NOT OVERWRITE");
        // ... and config.txt is now a fresh REGULAR file with the restored contents.
        assert!(!std::fs::symlink_metadata(work.join("config.txt")).unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read_to_string(work.join("config.txt")).unwrap(), "OLD SECRET");

        for d in [&home, &work, &outside] {
            let _ = std::fs::remove_dir_all(d);
        }
    }
}
