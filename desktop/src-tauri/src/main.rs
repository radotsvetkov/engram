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
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{
    AppHandle, Manager, RunEvent, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent,
};

use tauri_plugin_autostart::{MacosLauncher, ManagerExt};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

const ADDR: &str = "127.0.0.1:8088";

/// True once the user has chosen to fully quit (tray Quit / Cmd+Q), so the window-close
/// interceptor stops hiding-to-tray and lets the process exit.
static QUITTING: AtomicBool = AtomicBool::new(false);

/// A deep-link route (`#hash` fragment) awaiting the dashboard. A link can arrive before the daemon
/// page has loaded — at cold start the window shows the splash and navigates to the daemon origin
/// only once it is healthy. Setting `location.hash` on the splash would be wiped by that navigation,
/// so we stash the route here and fold it into the navigation URL (and re-apply on any later link).
static PENDING_ROUTE: Mutex<Option<String>> = Mutex::new(None);

/// True once the main window has navigated from the splash to the live daemon page, so a deep link
/// arriving now can set `location.hash` directly instead of only being stashed for the navigation.
/// Reset whenever a fresh window is put on the splash (initial build / recreate).
static DAEMON_PAGE_LIVE: AtomicBool = AtomicBool::new(false);

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

/// The API bearer token the daemon requires on `/v1/*`, read from `config.json` under the daemon's
/// home. When the user sets a token from Settings (the documented step for any network exposure), the
/// `require_auth` middleware gates `/v1/shutdown` and `/v1/restart` behind it — so the shell must send
/// `Authorization: Bearer <token>` or its POSTs 401 silently. Empty string means the gate is off.
fn daemon_token() -> Option<String> {
    let home = daemon_home()?;
    let raw = std::fs::read_to_string(PathBuf::from(home).join("config.json")).ok()?;
    let cfg: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let tok = cfg
        .get("security")
        .and_then(|s| s.get("api_token"))
        .and_then(|t| t.as_str())
        .unwrap_or("");
    (!tok.is_empty()).then(|| tok.to_string())
}

/// POST an empty body to `path` on the running daemon, attaching `Authorization: Bearer <token>` when
/// one is configured. Reads the status line and returns `Ok(())` only on a 2xx, so callers can tell a
/// real acknowledgement from a 401 (token gate) or a dropped connection instead of firing blind.
fn post_daemon(path: &str) -> Result<(), String> {
    use std::io::{Read, Write};
    let mut s = std::net::TcpStream::connect(ADDR).map_err(|e| format!("connect {ADDR}: {e}"))?;
    let _ = s.set_read_timeout(Some(Duration::from_secs(3)));
    let auth = daemon_token()
        .map(|t| format!("Authorization: Bearer {t}\r\n"))
        .unwrap_or_default();
    let req = format!(
        "POST {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n{auth}Content-Length: 0\r\nConnection: close\r\n\r\n"
    );
    s.write_all(req.as_bytes())
        .map_err(|e| format!("write {path}: {e}"))?;
    let mut buf = [0u8; 64];
    let n = s.read(&mut buf).map_err(|e| format!("read {path}: {e}"))?;
    let head = String::from_utf8_lossy(&buf[..n]);
    if head.starts_with("HTTP/1.1 2") || head.starts_with("HTTP/1.0 2") {
        Ok(())
    } else {
        Err(head
            .lines()
            .next()
            .unwrap_or("no response")
            .trim()
            .to_string())
    }
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

    builder = builder
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
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
        );

    //  updater: register the auto-update plugin ONLY when built with `--features updater`. It is a
    //  no-op unless the maintainer has configured a signing pubkey + endpoint (see UPDATER.md), and
    //  building with a placeholder pubkey fails at bundle time — so the whole path is feature-gated
    //  and off by default. A dev/unconfigured build simply never registers it and never crashes.
    #[cfg(feature = "updater")]
    {
        builder = builder
            .plugin(tauri_plugin_updater::Builder::new().build())
            // Expose the on-demand update commands to the frontend (a "Check for updates" button /
            // menu item can invoke them). Only registered with the feature on, so a default build
            // carries no updater IPC surface at all.
            .invoke_handler(tauri::generate_handler![check_for_updates, install_update]);
    }

    builder
        .setup(|app| {
            retire_stale_daemon(); // replace a daemon left over from a previous app version
            supervise_daemon(app.handle());

            // Open on the bundled "Waking the local agent…" splash immediately (WebviewUrl::App =
            // ../dist/index.html), NOT on the daemon origin. If the daemon is slow to boot (first-run
            // seeding, an MCP handshake, the embedder probe), the user sees the branded splash instead
            // of WebKit's connection-refused page — and there is a real recovery path, because we
            // navigate to the live dashboard from a background poll rather than a one-shot 5s wait.
            WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
                .title("Engram")
                .inner_size(1180.0, 800.0)
                .min_inner_size(720.0, 560.0)
                .build()?;
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
                let _ = w.set_focus();
                navigate_to_daemon_when_ready(w);
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

                // COLD START: a first launch triggered by an engram:// link carries the URL in the
                // initial process's argv (Windows/Linux) rather than through on_open_url. The plugin
                // exposes it via get_current(); fall back to scanning argv. Route it so a first-run
                // deep link isn't silently dropped and left on the default view.
                let cold = app
                    .deep_link()
                    .get_current()
                    .ok()
                    .flatten()
                    .map(|urls| urls.iter().map(|u| u.to_string()).collect::<Vec<_>>())
                    .filter(|v| !v.is_empty())
                    .unwrap_or_else(|| {
                        std::env::args()
                            .skip(1)
                            .filter(|a| a.starts_with("engram://"))
                            .collect()
                    });
                for url in cold {
                    route_deep_link(app.handle(), &url);
                }
            }

            // Best-effort background update check. Only compiled in with `--features updater`; even
            // then it is fully non-fatal — a placeholder/unreachable endpoint or a missing pubkey
            // just logs and skips, never blocking launch or crashing. If an update is available it
            // fires a native notification (existing notification plumbing) rather than force-updating.
            #[cfg(feature = "updater")]
            check_for_updates_on_launch(app.handle());

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
            "restart" => restart_agent(app),
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

/// Show + focus the main window, recreating it if it was destroyed. A recreated window opens on the
/// splash and navigates to the daemon origin once it answers (the daemon may be mid-restart), so it
/// never lands on a connection-refused page.
fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    } else if let Ok(w) =
        WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
            .title("Engram")
            .inner_size(1180.0, 800.0)
            .min_inner_size(720.0, 560.0)
            .build()
    {
        let _ = w.set_focus();
        navigate_to_daemon_when_ready(w);
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

/// On a COLD launch, retire a daemon left running from a *previous app version* BEFORE we spawn our
/// own — but ONLY if it is genuinely stale. Without a version check we would kill a perfectly healthy
/// current daemon on every relaunch, hard-aborting whatever it was doing (an in-flight agent run, a
/// scheduled job, a Telegram-triggered task) — the opposite of the zero-idle, warm-daemon design.
///
/// So: probe the running daemon's version on the unauthenticated `/health` endpoint and compare it to
/// the version our bundled `engramd` reports. If they match (or we can't read either one — fail safe,
/// don't kill what we can't identify), we leave it alone and let the readiness poll / `supervise_daemon`
/// reuse it. Only on a confirmed mismatch do we ask it to cleanly EXIT (`/v1/shutdown` — not
/// `/v1/restart`, which would just re-exec the same old binary) and wait, bounded, for the port to
/// free so our freshly bundled daemon can bind.
fn retire_stale_daemon() {
    if std::net::TcpStream::connect(ADDR).is_err() {
        return; // clean first launch — nothing is running, nothing to retire
    }
    // Only retire on a real version mismatch. If either version is unknown, do NOT shut down: an
    // unidentifiable listener might be a healthy daemon (or another app entirely) — reuse/attach
    // rather than aborting someone's in-flight work.
    let running = running_daemon_version();
    let bundled = bundled_daemon_version();
    match (running, bundled) {
        (Some(r), Some(b)) if r == b => return, // same version — healthy current daemon, leave it
        (Some(_), Some(_)) => {}                // confirmed mismatch — fall through and retire
        _ => return,                            // unknown on either side — fail safe, don't kill
    }

    // Authenticated shutdown: send the bearer token if one is configured, so the polite path works
    // even on an exposed install instead of always falling through to the kill fallback.
    let _ = post_daemon("/v1/shutdown");
    for _ in 0..60 {
        // ~3s for a clean /v1/shutdown
        if std::net::TcpStream::connect(ADDR).is_err() {
            return; // port freed — our bundled daemon can now take it
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    // Fallback: a stale daemon from BEFORE /v1/shutdown existed (or a hung one) won't exit politely.
    // Kill it — but only if we can confirm the LISTENER is actually an engramd process, never a
    // client or an unrelated app. Best-effort, never fatal.
    kill_port_owner();
    for _ in 0..40 {
        if std::net::TcpStream::connect(ADDR).is_err() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The version reported by the daemon currently listening on `ADDR`, read from its unauthenticated
/// `/health` JSON (`{"ok":true,"version":"…","offline":…}`). `None` if nothing answers, the response
/// is malformed, or no version field is present.
fn running_daemon_version() -> Option<String> {
    use std::io::{Read, Write};
    let mut s = std::net::TcpStream::connect(ADDR).ok()?;
    let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
    s.write_all(
        b"GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\nAccept: application/json\r\nConnection: close\r\n\r\n",
    )
    .ok()?;
    let mut buf = String::new();
    let _ = s.read_to_string(&mut buf); // best-effort; a short read still yields the JSON body
    let body = buf.split_once("\r\n\r\n").map(|(_, b)| b).unwrap_or(&buf);
    // Extract "version":"…" without pulling in a JSON dep for one field.
    let key = "\"version\"";
    let start = body.find(key)? + key.len();
    let rest = &body[start..];
    let q1 = rest.find('"')? + 1;
    let q2 = rest[q1..].find('"')? + q1;
    Some(rest[q1..q2].to_string())
}

/// The version of the `engramd` binary we would spawn, from `engramd --version` (prints
/// `engramd <version>`). `None` if no bundled binary is found or it doesn't answer.
fn bundled_daemon_version() -> Option<String> {
    for path in daemon_candidates() {
        if let Ok(out) = Command::new(&path).arg("--version").output() {
            if out.status.success() {
                let s = String::from_utf8_lossy(&out.stdout);
                if let Some(v) = s.split_whitespace().last() {
                    if !v.is_empty() {
                        return Some(v.to_string());
                    }
                }
            }
        }
    }
    None
}

/// Kill the process holding the daemon port (best-effort, Unix). Used only as a fallback when a stale
/// daemon won't shut down politely. Critically, this must kill ONLY the listener owned by engramd —
/// never a client connected to the port (a browser tab with the dashboard, the `engram` CLI/TUI, a
/// curl script) and never an unrelated app that legitimately owns the port.
fn kill_port_owner() {
    let port = ADDR.rsplit(':').next().unwrap_or("8088");
    // `-sTCP:LISTEN` restricts to the LISTENING socket, so we never match clients whose *remote*
    // port happens to be 8088. Bare `lsof -i tcp:8088` would list every connected client too.
    let Ok(out) = Command::new("lsof")
        .args(["-ti", &format!("tcp:{port}"), "-sTCP:LISTEN"])
        .output()
    else {
        return;
    };
    for pid in String::from_utf8_lossy(&out.stdout).split_whitespace() {
        // Verify the listener is actually engramd before signalling it. A non-engramd owner is a
        // genuine port conflict — killing the user's unrelated process would be strictly worse than
        // surfacing the conflict, so we skip it and let the spawn path report the bind failure.
        let is_engramd = Command::new("ps")
            .args(["-o", "comm=", "-p", pid])
            .output()
            .ok()
            .map(|o| {
                let comm = String::from_utf8_lossy(&o.stdout);
                comm.trim()
                    .rsplit('/')
                    .next()
                    .unwrap_or("")
                    .starts_with("engramd")
            })
            .unwrap_or(false);
        if is_engramd {
            let _ = Command::new("kill").arg(pid).status();
        } else {
            eprintln!(
                "engram: port {port} is held by a non-engramd process (pid {pid}); \
                 leaving it alone. Free the port or point Engram elsewhere via ENGRAM_ADDR."
            );
        }
    }
}

/// Ask the running daemon to restart itself (the supervisor brings it straight back). Used by the
/// tray's "Restart Agent". Sends the bearer token so it works on a token-gated install, and — if
/// nothing is listening (a dead/crashed daemon, where a bare HTTP POST would just no-op) — kicks the
/// supervisor to spawn a fresh one instead of silently dropping the click. Failures surface as a
/// native dialog rather than vanishing. Non-blocking: the work runs off the UI thread.
fn restart_agent(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        if std::net::TcpStream::connect(ADDR).is_err() {
            // Nothing is listening: the daemon is down. `/v1/restart` can't resurrect it, so make
            // sure a supervisor is running to spawn one (QUITTING is false here — the app is open).
            supervise_daemon(&app);
            return;
        }
        if let Err(e) = post_daemon("/v1/restart") {
            use tauri_plugin_dialog::DialogExt;
            app.dialog()
                .message(format!(
                    "Couldn't restart the agent: {e}.\n\nIf you've set an API token, the shell reads \
                     it from config.json under your Engram home; make sure it's saved there."
                ))
                .title("Engram")
                .blocking_show();
        }
    });
}

/// Route an `engram://` deep link into the running window. The window loads the daemon's
/// origin, so we map the link's host/path onto the dashboard's `#hash` router and surface
/// the window. e.g. `engram://settings` -> `#settings`, `engram://task/<id>` -> `#task/<id>`.
///
/// The route is always stashed in `PENDING_ROUTE` so it survives the splash->daemon navigation at
/// cold start (where an immediate `location.hash` set would be wiped). If the daemon page is already
/// live we also apply it right away for an instant response.
fn route_deep_link(app: &AppHandle, url: &str) {
    let Some(safe) = sanitize_route(url) else {
        show_main(app);
        return;
    };
    *PENDING_ROUTE.lock().unwrap() = Some(safe.clone());
    show_main(app);
    // If the window is already on the live daemon page, apply the hash immediately; otherwise leave
    // it stashed for the splash->daemon navigation to fold in (setting the hash on the splash would
    // be wiped by that navigation).
    if DAEMON_PAGE_LIVE.load(Ordering::SeqCst) {
        if let Some(w) = app.get_webview_window("main") {
            let _ = w.eval(format!(
                "window.location.hash = {};",
                serde_json::to_string(&safe).unwrap_or_else(|_| "\"\"".into())
            ));
        }
        // Consume it so it isn't re-applied by a later navigation.
        *PENDING_ROUTE.lock().unwrap() = None;
    }
}

/// Sanitize an `engram://…` link into a conservative hash route (alphanumerics + a few URL chars),
/// never arbitrary script. `None` for an empty route.
fn sanitize_route(url: &str) -> Option<String> {
    let rest = url.trim_start_matches("engram://").trim_matches('/');
    if rest.is_empty() {
        return None;
    }
    let safe: String = rest
        .chars()
        .filter(|c| {
            c.is_ascii_alphanumeric() || matches!(c, '/' | '-' | '_' | '.' | '?' | '=' | '&')
        })
        .collect();
    (!safe.is_empty()).then_some(safe)
}

/// Navigate the given window from the bundled splash to the live dashboard once the daemon answers.
///
/// This replaces the old fixed ~5s wait, whose deadline could expire before a slow first boot
/// (seeding, an MCP handshake, the embedder probe) finished — stranding the window on WebKit's
/// connection-refused page with no way back. Here the splash is already showing, and a background
/// thread polls `/health` INDEFINITELY, so a slow daemon is covered rather than a race we lose. The
/// moment the daemon is reachable we point the webview at its origin from the main thread.
fn navigate_to_daemon_when_ready(window: WebviewWindow) {
    // This window starts on the splash, not the daemon page — a deep link arriving before we
    // navigate must be stashed, not eval'd onto the splash.
    DAEMON_PAGE_LIVE.store(false, Ordering::SeqCst);
    std::thread::spawn(move || {
        loop {
            // If the user quit while we were waiting, stop — the window is gone.
            if QUITTING.load(Ordering::SeqCst) {
                return;
            }
            if daemon_is_healthy() {
                // Fold any pending deep-link route into the URL so it survives this navigation
                // (setting location.hash on the splash first would be wiped here).
                let hash = PENDING_ROUTE
                    .lock()
                    .unwrap()
                    .take()
                    .map(|r| format!("#{r}"))
                    .unwrap_or_default();
                let url = format!("http://{ADDR}/{hash}");
                if let Ok(u) = url.parse() {
                    // Prefer a real navigation (survives reloads and sets the origin correctly for
                    // the remote-capability match); fall back to a scripted reload if unavailable.
                    if window.navigate(u).is_err() {
                        let _ = window.eval(&format!("window.location.replace({url:?})"));
                    }
                }
                DAEMON_PAGE_LIVE.store(true, Ordering::SeqCst);
                return;
            }
            std::thread::sleep(Duration::from_millis(150));
        }
    });
}

/// True once the daemon accepts a connection AND its `/health` probe answers 2xx — a real readiness
/// signal, not just an open socket (which can precede the router being wired up).
fn daemon_is_healthy() -> bool {
    use std::io::{Read, Write};
    let Ok(mut s) = std::net::TcpStream::connect(ADDR) else {
        return false;
    };
    let _ = s.set_read_timeout(Some(Duration::from_secs(1)));
    if s.write_all(b"GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
        .is_err()
    {
        return false;
    }
    let mut buf = [0u8; 64];
    match s.read(&mut buf) {
        Ok(n) if n > 0 => {
            let head = String::from_utf8_lossy(&buf[..n]);
            head.starts_with("HTTP/1.1 2") || head.starts_with("HTTP/1.0 2")
        }
        _ => false,
    }
}

/// Start the daemon and keep it alive for as long as the app is open. If it exits after a
/// real session (for example, the Settings panel asked it to restart to load a new
/// embedder), respawn it. If it exits almost immediately AND the port is already served,
/// another instance owns it - attach to that one instead of spinning. If it exits fast and
/// nothing is listening, that is a genuine crash loop: after a few tries we give up and tell
/// the user (with a pointer to the logs) instead of failing silently.
fn supervise_daemon(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
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
                // Capture the daemon's stdout/stderr to a per-home log. In a Finder-launched .app
                // the inherited streams go nowhere, so bind errors, "home is locked", config and
                // provider failures — the exact class behind a quick-exit crash loop — would be
                // invisible. Mirrors the CLI's engramd-spawn.log. Best-effort: fall back to
                // inherited streams if the file can't be opened.
                if let Some(h) = &home {
                    if let Ok(f) = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(PathBuf::from(h).join("daemon.log"))
                    {
                        if let Ok(f2) = f.try_clone() {
                            cmd.stdout(std::process::Stdio::from(f));
                            cmd.stderr(std::process::Stdio::from(f2));
                        }
                    }
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
                // A fast exit has two very different causes. Probe the port to tell them apart:
                //  - something is listening  -> another instance owns the port; nothing to do, stop.
                //  - nothing is listening     -> the daemon crashed at boot. Retry a few times to
                //    ride out a slow port release (SO_REUSEADDR), then give up and alert the user.
                if std::net::TcpStream::connect(ADDR).is_ok() {
                    break; // port already served by another healthy instance — attach, don't spin
                }
                quick_exits += 1;
                if quick_exits >= 5 {
                    let log = home
                        .as_deref()
                        .map(|h| format!("{h}/daemon.log"))
                        .unwrap_or_else(|| "~/.engram/daemon.log".into());
                    use tauri_plugin_dialog::DialogExt;
                    app.dialog()
                        .message(format!(
                            "Engram's local agent keeps exiting at startup and could not be kept \
                             running.\n\nCheck the log for the cause:\n{log}\n(panics are also in \
                             the same folder's panic.log). Fix the issue, then use the tray's \
                             \"Restart Agent\"."
                        ))
                        .title("Engram - agent won't start")
                        .blocking_show();
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

// ---------------------------------------------------------------------------------------------------
// Auto-update (feature = "updater")
//
// Everything below is compiled ONLY when the `updater` feature is on (see Cargo.toml). It stays out
// of a default/dev build entirely, so an unconfigured checkout never depends on a signing pubkey and
// never risks the app crashing over a placeholder endpoint. Once the maintainer has generated a Tauri
// signing keypair and filled in `plugins.updater.{endpoints,pubkey}` in tauri.conf.json (see
// desktop/UPDATER.md), building with `--features updater` turns this on. The runtime path is
// deliberately conservative: it *notifies* about an available update rather than force-installing.
// ---------------------------------------------------------------------------------------------------

/// Fire off a one-shot, best-effort update check on launch. Runs on a background task so it never
/// blocks the window coming up, and every failure mode (unreachable endpoint, placeholder pubkey,
/// bad manifest, no update available) is logged and swallowed — the check must never crash the app.
#[cfg(feature = "updater")]
fn check_for_updates_on_launch(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        match find_update(&app).await {
            Ok(Some(version)) => notify_update_available(&app, &version),
            Ok(None) => { /* up to date — nothing to do */ }
            // Placeholder endpoint / offline / unconfigured pubkey all land here. Log, don't crash.
            Err(e) => eprintln!("engram: update check skipped: {e}"),
        }
    });
}

/// Query the configured update manifest and, if a newer version is on offer, return its version
/// string. Any error (network, manifest parse, unconfigured/placeholder endpoint or pubkey) is
/// returned as a string for the caller to log — this function never panics and never installs.
#[cfg(feature = "updater")]
async fn find_update(app: &AppHandle) -> Result<Option<String>, String> {
    use tauri_plugin_updater::UpdaterExt;
    let updater = app.updater().map_err(|e| e.to_string())?;
    match updater.check().await {
        Ok(Some(update)) => Ok(Some(update.version.clone())),
        Ok(None) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

/// Surface an available update through the existing native-notification plumbing. Purely
/// informational — the actual download+install is user-initiated (the `install_update` command),
/// so a background check can never silently swap the running binary out from under the user.
#[cfg(feature = "updater")]
fn notify_update_available(app: &AppHandle, version: &str) {
    use tauri_plugin_notification::NotificationExt;
    let _ = app
        .notification()
        .builder()
        .title("Engram update available")
        .body(format!(
            "Version {version} is ready to install. Open Engram to update."
        ))
        .show();
}

/// Frontend-invocable command: check for an update and report the new version (or `None`). The
/// dashboard (or a menu item) can call this on demand. Returns `Ok(None)` when up to date, and an
/// `Err(String)` — never a panic — when the endpoint/pubkey are placeholders or unreachable, so the
/// UI can show "you're up to date" / "couldn't check" instead of the app dying.
#[cfg(feature = "updater")]
#[tauri::command]
async fn check_for_updates(app: AppHandle) -> Result<Option<String>, String> {
    find_update(&app).await
}

/// Frontend-invocable command: download and install the pending update, then relaunch. Only does
/// anything if `check_for_updates` reported one. Non-fatal on error (returns `Err(String)`); the
/// signature verification against the configured pubkey is enforced by the updater plugin itself, so
/// an unsigned or tampered artifact is rejected before it is applied.
#[cfg(feature = "updater")]
#[tauri::command]
async fn install_update(app: AppHandle) -> Result<(), String> {
    use tauri_plugin_updater::UpdaterExt;
    let updater = app.updater().map_err(|e| e.to_string())?;
    let Some(update) = updater.check().await.map_err(|e| e.to_string())? else {
        return Ok(()); // nothing to install
    };
    update
        .download_and_install(|_chunk, _total| {}, || {})
        .await
        .map_err(|e| e.to_string())?;
    app.restart() // diverges (`-> !`): relaunches into the freshly installed binary
}
