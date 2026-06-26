//! Engram desktop shell.
//!
//! A thin native window onto the local agent. On launch it best-effort starts the
//! `engramd` daemon (if it isn't already running) and opens a webview pointed at its
//! dashboard. Because the window loads the daemon's own origin, the dashboard's API
//! calls just work - no CORS, no duplicated frontend. When the window closes the
//! daemon is left to sleep itself to zero on idle, true to the zero-idle design.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::process::Command;

use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

const ADDR: &str = "127.0.0.1:8088";

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            spawn_daemon();
            WebviewWindowBuilder::new(
                app,
                "main",
                WebviewUrl::External(format!("http://{ADDR}").parse().unwrap()),
            )
            .title("Engram")
            .inner_size(1180.0, 800.0)
            .min_inner_size(720.0, 560.0)
            .build()?;
            // Surface the window once it's ready.
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Engram desktop");
}

/// Try to start the daemon from a few likely locations. Best-effort: if it is already
/// running, the bind fails harmlessly and the window still connects.
fn spawn_daemon() {
    let candidates = [
        "engramd",
        "../target/release/engramd",
        "../target/debug/engramd",
        "../../target/release/engramd",
        "../../target/debug/engramd",
    ];
    for path in candidates {
        if Command::new(path).env("ENGRAM_ADDR", ADDR).spawn().is_ok() {
            return;
        }
    }
}
