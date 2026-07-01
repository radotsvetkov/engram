//! Daemon discovery and (optional) auto-spawn.
//!
//! `engram` should "just work": if the daemon isn't up, find the `engramd`
//! binary and start it, then wait for `/health`. A spawned daemon is detached
//! (`std::process::Child` is not killed when the parent exits on Unix), so it
//! keeps serving after the CLI/TUI closes — matching the zero-idle model where
//! the daemon sleeps itself out after the idle window.

use crate::api::Client;
use anyhow::{anyhow, Result};
use std::path::PathBuf;
use std::time::Duration;

/// Is a daemon answering at this client's address?
pub async fn is_up(client: &Client) -> bool {
    client.health().await.map(|h| h.ok).unwrap_or(false)
}

/// Locate the `engramd` binary across the usual spots.
pub fn find_engramd() -> Option<PathBuf> {
    // 1. Explicit override.
    if let Ok(p) = std::env::var("ENGRAMD_BIN") {
        let pb = PathBuf::from(p);
        if pb.exists() {
            return Some(pb);
        }
    }
    // 2. Next to our own binary (installed side-by-side).
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            for name in ["engramd", "engramd.exe"] {
                let cand = dir.join(name);
                if cand.exists() {
                    return Some(cand);
                }
            }
        }
    }
    // 3. Cargo target dirs relative to CWD (dev workflow).
    for rel in [
        "target/release/engramd",
        "target/debug/engramd",
        "../target/release/engramd",
        "../target/debug/engramd",
    ] {
        let cand = PathBuf::from(rel);
        if cand.exists() {
            return Some(cand);
        }
    }
    // 4. On PATH.
    if let Ok(out) = std::process::Command::new("sh")
        .arg("-c")
        .arg("command -v engramd")
        .output()
    {
        if out.status.success() {
            let p = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !p.is_empty() {
                return Some(PathBuf::from(p));
            }
        }
    }
    None
}

/// The address engramd should bind, derived from the client base URL.
fn addr_from_base(base: &str) -> String {
    base.trim_start_matches("http://")
        .trim_start_matches("https://")
        .trim_end_matches('/')
        .to_string()
}

/// Resolve a single, canonical `ENGRAM_HOME` to hand the spawned daemon, so it
/// never lands on a surprising CWD-relative `./brain` and starts a *second*
/// signed ledger. Order: explicit `$ENGRAM_HOME` → a `./brain` in the current
/// directory (the dev/repo brain), made absolute → the stable per-user
/// `~/.engram` the installed app uses.
pub fn resolve_home() -> PathBuf {
    if let Ok(h) = std::env::var("ENGRAM_HOME") {
        if !h.is_empty() {
            return PathBuf::from(h);
        }
    }
    let cwd_brain = PathBuf::from("brain");
    if cwd_brain.is_dir() {
        return cwd_brain.canonicalize().unwrap_or(cwd_brain);
    }
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home).join(".engram");
    }
    cwd_brain
}

/// Spawn `engramd` (detached) and poll `/health` until it answers or we give up.
pub async fn spawn_and_wait(client: &Client, quiet: bool) -> Result<()> {
    let bin = find_engramd().ok_or_else(|| {
        anyhow!(
            "the engram daemon isn't running and the `engramd` binary wasn't found.\n\
             Build it with `cargo build --release` or set $ENGRAMD_BIN, then retry."
        )
    })?;
    let addr = addr_from_base(client.base_url());
    let home = resolve_home();
    if !quiet {
        eprintln!(
            "· starting engramd ({}) · home {}…",
            bin.display(),
            home.display()
        );
    }

    // Keep the spawn log next to the brain it serves (per-home, per-user) rather
    // than a shared temp file.
    let _ = std::fs::create_dir_all(&home);
    let log_path = home.join("engramd-spawn.log");
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .ok();

    let mut cmd = std::process::Command::new(&bin);
    cmd.env("ENGRAM_ADDR", &addr);
    // Always pass the resolved home explicitly — never rely on the daemon's
    // CWD-relative default.
    cmd.env("ENGRAM_HOME", &home);
    cmd.stdin(std::process::Stdio::null());
    if let Some(f) = log {
        let f2 = f.try_clone().ok();
        cmd.stdout(std::process::Stdio::from(f));
        if let Some(f2) = f2 {
            cmd.stderr(std::process::Stdio::from(f2));
        }
    } else {
        cmd.stdout(std::process::Stdio::null());
        cmd.stderr(std::process::Stdio::null());
    }
    cmd.spawn()
        .map_err(|e| anyhow!("failed to spawn engramd: {e}"))?;

    // Poll for readiness (~20s budget).
    for _ in 0..80 {
        tokio::time::sleep(Duration::from_millis(250)).await;
        if is_up(client).await {
            if !quiet {
                eprintln!("{}", crate::cli::output::good("· engramd is up"));
            }
            return Ok(());
        }
    }
    // Surface the daemon's real failure (bind clash, ledger lock, …) from its log.
    let tail = std::fs::read_to_string(&log_path)
        .ok()
        .map(|s| {
            s.lines()
                .rev()
                .take(6)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join("\n")
        })
        .filter(|s| !s.trim().is_empty())
        .map(|s| format!("\n--- last log lines ({}) ---\n{s}", log_path.display()))
        .unwrap_or_else(|| format!(" — see {}", log_path.display()));
    Err(anyhow!("engramd did not become healthy within 20s{tail}"))
}

/// Ensure the daemon is reachable, spawning it if allowed.
pub async fn ensure(client: &Client, auto_spawn: bool, quiet: bool) -> Result<()> {
    if is_up(client).await {
        return Ok(());
    }
    if auto_spawn {
        spawn_and_wait(client, quiet).await
    } else {
        Err(anyhow!(
            "no engram daemon reachable at {} (start it with `engram serve`)",
            client.base_url()
        ))
    }
}
