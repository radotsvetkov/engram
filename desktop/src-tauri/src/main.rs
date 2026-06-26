//! Engram desktop shell.
//!
//! A thin native window onto the local agent. On launch it best-effort starts the
//! `engramd` daemon and opens a webview pointed at its dashboard. Because the window
//! loads the daemon's own origin, the dashboard's API calls just work - no CORS, no
//! duplicated frontend.
//!
//! The daemon is supervised: if it exits while the app is open (the Settings panel can
//! ask it to restart so a new embedder takes effect), it is brought straight back. When
//! the window closes the whole app quits, so the daemon is left to sleep itself to zero
//! on idle, true to the zero-idle design.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

use tauri::{Manager, WebviewUrl, WebviewWindowBuilder, WindowEvent};

const ADDR: &str = "127.0.0.1:8088";

/// Where the `engramd` binary might live, most-specific first:
///  1. next to the app executable - the bundled sidecar (an installed `.app`);
///  2. the workspace `target/` dirs, relative to the shell's CWD (a dev run);
///  3. bare `engramd`, found on `PATH`.
fn daemon_candidates() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            v.push(dir.join("engramd"));
        }
    }
    for p in [
        "engramd",
        "../target/release/engramd",
        "../target/debug/engramd",
        "../../target/release/engramd",
        "../../target/debug/engramd",
    ] {
        v.push(PathBuf::from(p));
    }
    v
}

/// The daemon's state directory: an explicit `ENGRAM_HOME`, else a stable per-user folder.
fn daemon_home() -> Option<String> {
    if let Ok(h) = std::env::var("ENGRAM_HOME") {
        return Some(h);
    }
    std::env::var("HOME").ok().map(|h| format!("{h}/.engram"))
}

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            supervise_daemon();
            wait_for_daemon();
            WebviewWindowBuilder::new(
                app,
                "main",
                WebviewUrl::External(format!("http://{ADDR}").parse().unwrap()),
            )
            .title("Engram")
            .inner_size(1180.0, 800.0)
            .min_inner_size(720.0, 560.0)
            .build()?;
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
            }
            Ok(())
        })
        // Closing the window quits the app (and with it the supervisor), so a separately
        // running daemon is free to sleep to zero on idle.
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { .. } = event {
                window.app_handle().exit(0);
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running Engram desktop");
}

/// Wait (up to ~5s) for the daemon to start accepting connections, so the window opens onto
/// a live dashboard instead of a connection-refused page. Returns early the moment it answers.
fn wait_for_daemon() {
    for _ in 0..100 {
        if std::net::TcpStream::connect(ADDR).is_ok() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Start the daemon and keep it alive for as long as the app is open. If it exits after a
/// real session (for example, the Settings panel asked it to restart to load a new
/// embedder), respawn it. If it exits almost immediately, another instance already owns
/// the port - connect to that one instead of spinning.
fn supervise_daemon() {
    std::thread::spawn(|| {
        let home = daemon_home();
        if let Some(h) = &home {
            let _ = std::fs::create_dir_all(h);
        }
        let mut quick_exits = 0u8;
        loop {
            let started = Instant::now();
            let mut launched = false;
            for path in daemon_candidates() {
                let mut cmd = Command::new(&path);
                cmd.env("ENGRAM_ADDR", ADDR);
                if let Some(h) = &home {
                    cmd.env("ENGRAM_HOME", h);
                }
                if let Ok(mut child) = cmd.spawn() {
                    launched = true;
                    let _ = child.wait(); // blocks for the whole session, then returns on exit
                    break;
                }
            }
            if !launched {
                break; // no binary found anywhere
            }
            if started.elapsed() < Duration::from_secs(3) {
                // A fast exit usually means the port is already served by another instance, or
                // the kernel has not released it yet after a restart. Retry a few times to ride
                // out that race (the daemon binds with SO_REUSEADDR), then give up rather than
                // spin forever against a port someone else owns.
                quick_exits += 1;
                if quick_exits >= 5 {
                    break;
                }
                std::thread::sleep(Duration::from_millis(500));
            } else {
                quick_exits = 0; // a real session ended (a restart): bring it straight back
                std::thread::sleep(Duration::from_millis(300));
            }
        }
    });
}
