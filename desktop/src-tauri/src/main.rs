//! Engram desktop shell.
//!
//! A native window onto the local agent - but a *real* desktop app, not a webview over a
//! URL. On launch it best-effort starts the `engramd` daemon and opens a webview pointed at
//! its dashboard (the daemon's own origin, so the dashboard's API calls just work - no CORS,
//! no duplicated frontend). On top of that it wires the native integration surface a real
//! agent app needs and that the web page cannot reach on its own:
//!
//!  - **System tray** with Show / Hide / Restart agent / Open at login / Quit, so Engram is
//!    reachable from the menu bar even with its window closed.
//!  - **Close-to-tray**: closing the window hides it (the agent stays warm); Cmd/Ctrl+Q or the
//!    tray's Quit fully exits and lets the daemon sleep itself to zero on idle.
//!  - **Native menu** (Edit: undo/cut/copy/paste/select-all, Window, App). Without this,
//!    clipboard shortcuts don't work inside a webview on macOS - a correctness fix, not chrome.
//!  - **Global hotkey** (Cmd/Ctrl+Shift+Space) to summon Engram from anywhere.
//!  - **Single-instance lock**: a second launch focuses the existing window instead of spawning
//!    a duplicate daemon + window.
//!  - **Run at login** (a real, persisted autostart entry) toggled from the tray.
//!  - **Window-state persistence** (size/position remembered across launches).
//!  - **Deep links**: the `engram://` URL scheme routes into the running window.
//!
//! The daemon is supervised: if it exits while the app is open (the Settings panel can ask it
//! to restart so a new embedder takes effect), it is brought straight back. When the app fully
//! quits, the daemon is left running to sleep itself to zero on idle, true to the zero-idle
//! design.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, RunEvent, WebviewUrl, WebviewWindowBuilder, WindowEvent};

use tauri_plugin_autostart::{MacosLauncher, ManagerExt};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

const ADDR: &str = "127.0.0.1:8088";

/// True once the user has chosen to fully quit (tray Quit / Cmd+Q), so the window-close
/// interceptor stops hiding-to-tray and lets the process exit.
static QUITTING: AtomicBool = AtomicBool::new(false);

/// Where the `engramd` binary might live, most-specific first:
///  1. next to the app executable - the bundled sidecar (an installed `.app`);
///  2. the workspace `target/` dirs, relative to the shell's CWD (a dev run);
///  3. bare `engramd`, found on `PATH`.
fn daemon_candidates() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            // Tauri sidecars are installed next to the app exe, optionally target-triple-suffixed.
            v.push(dir.join("engramd"));
            v.push(dir.join(format!("engramd-{}", std::env::consts::ARCH)));
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

/// The system-wide summon hotkey: Cmd+Shift+Space on macOS, Ctrl+Shift+Space elsewhere.
fn summon_shortcut() -> Shortcut {
    #[cfg(target_os = "macos")]
    let mods = Modifiers::SUPER | Modifiers::SHIFT;
    #[cfg(not(target_os = "macos"))]
    let mods = Modifiers::CONTROL | Modifiers::SHIFT;
    Shortcut::new(Some(mods), Code::Space)
}

fn main() {
    let mut builder = tauri::Builder::default();

    // The single-instance plugin must be the FIRST plugin registered. On a second launch it
    // runs this callback in the already-running instance (instead of starting a new one) and
    // we surface the existing window - so we never spawn a duplicate daemon or window.
    builder = builder.plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
        show_main(app);
        // A second launch may carry a deep link as argv (Linux/Windows) - route it.
        for arg in argv.iter().skip(1) {
            if arg.starts_with("engram://") {
                route_deep_link(app, arg);
            }
        }
    }));

    builder
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            Some(vec![]),
        ))
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _shortcut, event| {
                    // Toggle on key-down only, so press+release doesn't cancel itself out.
                    if event.state == ShortcutState::Pressed {
                        toggle_main(app);
                    }
                })
                .build(),
        )
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
                let _ = w.set_focus();
            }

            // Native menu bar: the OS default gives a working Edit menu (undo/cut/copy/paste/
            // select-all) plus App and Window menus. On macOS this is what makes clipboard
            // shortcuts work inside the webview at all.
            let menu = Menu::default(app.handle())?;
            app.set_menu(menu)?;

            build_tray(app.handle())?;

            // Register the global summon hotkey. Non-fatal if the OS denies it (e.g. the combo
            // is already taken) - the app is still fully usable from the tray and dock.
            if let Err(e) = app.global_shortcut().register(summon_shortcut()) {
                eprintln!("engram: could not register global hotkey: {e}");
            }

            // Catch engram:// deep links delivered while the app is already running.
            {
                use tauri_plugin_deep_link::DeepLinkExt;
                let handle = app.handle().clone();
                app.deep_link().on_open_url(move |event| {
                    for url in event.urls() {
                        route_deep_link(&handle, url.as_str());
                    }
                });
                // Best-effort: ensure the scheme is registered at runtime (Linux/dev).
                let _ = app.deep_link().register_all();
            }

            Ok(())
        })
        // Closing the window hides it to the tray (the agent stays warm and reachable). Cmd/Ctrl+Q,
        // the App menu's Quit, and the tray's Quit set QUITTING and let the process exit, after
        // which the daemon sleeps itself to zero on idle.
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                if !QUITTING.load(Ordering::SeqCst) {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("error while running Engram desktop")
        .run(|app, event| match event {
            // macOS: clicking the dock icon (with no visible windows) re-shows Engram.
            #[cfg(target_os = "macos")]
            RunEvent::Reopen { .. } => show_main(app),
            RunEvent::ExitRequested { .. } => {
                QUITTING.store(true, Ordering::SeqCst);
            }
            _ => {
                let _ = app;
            }
        });
}

/// Build the system tray: an always-available menu bar presence with Show / Hide / Restart /
/// Open-at-login / Quit. Left-clicking the tray toggles the window; the menu covers the rest.
fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "Show Engram", true, None::<&str>)?;
    let hide = MenuItem::with_id(app, "hide", "Hide Window", true, None::<&str>)?;
    let restart = MenuItem::with_id(app, "restart", "Restart Agent", true, None::<&str>)?;
    let login_enabled = app.autolaunch().is_enabled().unwrap_or(false);
    let login = CheckMenuItem::with_id(
        app,
        "login",
        "Open at Login",
        true,
        login_enabled,
        None::<&str>,
    )?;
    let sep1 = PredefinedMenuItem::separator(app)?;
    let sep2 = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Engram", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &hide, &restart, &sep1, &login, &sep2, &quit])?;

    TrayIconBuilder::with_id("engram-tray")
        .icon(app.default_window_icon().unwrap().clone())
        .tooltip("Engram - your local agent (warm while running)")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => show_main(app),
            "hide" => {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.hide();
                }
            }
            "restart" => restart_agent(),
            "login" => {
                let mgr = app.autolaunch();
                let now = mgr.is_enabled().unwrap_or(false);
                let _ = if now { mgr.disable() } else { mgr.enable() };
            }
            "quit" => {
                QUITTING.store(true, Ordering::SeqCst);
                app.exit(0);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                toggle_main(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

/// Show + focus the main window, recreating it if it was destroyed.
fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    } else {
        let _ = WebviewWindowBuilder::new(
            app,
            "main",
            WebviewUrl::External(format!("http://{ADDR}").parse().unwrap()),
        )
        .title("Engram")
        .inner_size(1180.0, 800.0)
        .min_inner_size(720.0, 560.0)
        .build();
    }
}

/// Toggle the main window's visibility (the tray-click and global-hotkey behaviour).
fn toggle_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        if w.is_visible().unwrap_or(false) && !w.is_minimized().unwrap_or(false) {
            let _ = w.hide();
        } else {
            let _ = w.unminimize();
            let _ = w.show();
            let _ = w.set_focus();
        }
    } else {
        show_main(app);
    }
}

/// Ask the running daemon to restart itself (the supervisor brings it straight back). Used by
/// the tray's "Restart Agent". Best-effort and non-blocking.
fn restart_agent() {
    std::thread::spawn(|| {
        let _ = std::net::TcpStream::connect(ADDR).map(|mut s| {
            use std::io::Write;
            let _ = s.write_all(
                b"POST /v1/restart HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
        });
    });
}

/// Route an `engram://` deep link into the running window. The window loads the daemon's
/// origin, so we map the link's host/path onto the dashboard's `#hash` router and surface
/// the window. e.g. `engram://settings` -> `#settings`, `engram://task/<id>` -> `#task/<id>`.
fn route_deep_link(app: &AppHandle, url: &str) {
    show_main(app);
    let rest = url.trim_start_matches("engram://").trim_matches('/');
    if rest.is_empty() {
        return;
    }
    // Only allow a conservative hash route - never inject arbitrary script.
    let safe: String = rest
        .chars()
        .filter(|c| {
            c.is_ascii_alphanumeric() || matches!(c, '/' | '-' | '_' | '.' | '?' | '=' | '&')
        })
        .collect();
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.eval(format!(
            "window.location.hash = {};",
            serde_json::to_string(&safe).unwrap_or_else(|_| "\"\"".into())
        ));
    }
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
            // Stop respawning once the user has chosen to quit, so the daemon is free to sleep.
            if QUITTING.load(Ordering::SeqCst) {
                break;
            }
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
