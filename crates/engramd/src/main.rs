//! `engramd` - the Engram daemon.
//!
//! This is where the parts become an agent. It opens the audit ledger, the hybrid
//! memory, the skill registry, the LLM gateway, and the scheduler, and exposes them
//! over a small local HTTP API plus a dashboard. Every request keeps the brain awake;
//! after an idle window with no requests the process exits to zero, so on a
//! socket-activated VPS there is nothing resident between uses.
//!
//! Env: ENGRAM_HOME (state dir, default ./brain), ENGRAM_ADDR (default 127.0.0.1:8088),
//! ENGRAM_IDLE_SECS (default 900), RUST_LOG.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

mod agents;
mod budget;
mod channels;
mod checkpoints;
mod citation;
mod config;
mod conscious;
mod contradiction;
mod converse;
mod corpus;
mod dissent;
mod distill;
mod embedder;
mod git;
mod hooks;
mod reflection;
mod scope;
mod seed;
mod tasks;
mod telegram;
mod terminal;
mod workspace;

use engram_core::{run_until_idle, Activity, Bus, Ledger, Priority, Spike, VERSION};
use engram_gateway::Gateway;
use engram_memory::{Memory, Region, TrigramHashEmbedder, WriteReq};
use engram_sched::{parse as parse_schedule, Scheduler};
use engram_skills::{Registry, SkillHost, SkillSigner};

#[derive(Clone)]
struct App {
    memory: Arc<Memory>,
    registry: Arc<Registry>,
    host: Arc<SkillHost>,
    gateway: Arc<Gateway>,
    sched: Arc<Scheduler>,
    ledger: Arc<Ledger>,
    bus: Bus,
    activity: Activity,
    workdir: std::path::PathBuf,
    /// Standing instructions (SOUL.md) prepended to every agent run. Behind a lock so the
    /// Settings panel's persona editor can change them live, no restart.
    persona: Arc<std::sync::RwLock<Option<String>>>,
    /// Tools borrowed from connected MCP servers. Behind a lock so editing the MCP list in
    /// Settings reconnects and swaps them in live.
    mcp_tools: Arc<std::sync::RwLock<Vec<Arc<dyn engram_agent::Tool>>>>,
    browser: Arc<dyn engram_agent::BrowserSession>,
    tasks: Arc<tasks::TaskStore>,
    /// Projects and chat sessions backing the desktop sidebar, persisted to disk.
    workspace: Arc<workspace::WorkspaceStore>,
    /// Runtime-mutable shell consent - toggled by the desktop's approval card.
    allow_shell: Arc<std::sync::atomic::AtomicBool>,
    /// Kill switch: set true to stop in-flight agent runs at their next step boundary.
    halt: Arc<std::sync::atomic::AtomicBool>,
    /// Per-session halt flags so one chat can be stopped WITHOUT killing other concurrent chats.
    /// A chat run registers its flag under its session id; `/v1/halt {session}` flips just that one.
    /// The global `halt` above is the emergency "stop everything".
    run_halts: Arc<
        std::sync::Mutex<std::collections::HashMap<String, Arc<std::sync::atomic::AtomicBool>>>,
    >,
    /// Live settings (provider, model, security, cost, MCP), editable from the desktop's
    /// Settings panel and persisted to `config.json`.
    config: Arc<std::sync::RwLock<config::Config>>,
    /// Where the daemon's state lives - needed to persist settings changes.
    home: String,
    /// The running Telegram poller's abort handle + connected bot @username, so the desktop's
    /// Connect/Disconnect can start and stop the bot live (no restart). None when not connected.
    telegram: Arc<std::sync::Mutex<Option<(tokio::task::AbortHandle, String)>>>,
    /// The always-loaded working memory distilled from the brain, prepended to every run. Signed,
    /// editable, revertible - the verifiable-memory layer.
    consciousness: Arc<conscious::Consciousness>,
    /// Durable, named, role-scoped agents assignable to kanban cards - the auditable team.
    agents: Arc<agents::AgentStore>,
    /// How many agent runs are executing right now. The idle-clock (`Activity`) is reset only by the
    /// `keep_awake` HTTP middleware, so a scheduled / Telegram / detached-stream run with no open HTTP
    /// connection did NOT keep the daemon awake: after the idle window the process exited mid-run,
    /// killing unattended work and leaving scheduled jobs to double-fire. A background keepalive task
    /// touches activity while this is non-zero, and shutdown drains in-flight runs before returning.
    in_flight: Arc<std::sync::atomic::AtomicUsize>,
}

/// RAII counter for an in-flight agent run. Incrementing touches the idle clock so a run started
/// without an open HTTP connection (scheduler, Telegram, a stream whose client disconnected) keeps
/// the daemon awake; the count is decremented on drop, so it is correct across every early-return
/// and `?` in the run path. The background keepalive task (spawned in `run()`) re-touches while the
/// count is non-zero, and graceful shutdown waits for it to reach zero.
struct RunGuard {
    counter: Arc<std::sync::atomic::AtomicUsize>,
}
impl RunGuard {
    fn new(activity: &Activity, counter: Arc<std::sync::atomic::AtomicUsize>) -> Self {
        counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        activity.touch();
        RunGuard { counter }
    }
}
impl Drop for RunGuard {
    fn drop(&mut self) {
        self.counter
            .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

impl App {
    /// A read guard over the live settings.
    fn cfg(&self) -> std::sync::RwLockReadGuard<'_, config::Config> {
        self.config.read().expect("config lock")
    }
    /// The model id to send with requests, from the live settings.
    fn model(&self) -> String {
        self.cfg().model()
    }
}

/// Uniform error → JSON 500.
pub(crate) struct ApiError(pub(crate) String);
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": self.0 })),
        )
            .into_response()
    }
}
pub(crate) type ApiResult = Result<Json<Value>, ApiError>;
pub(crate) fn err(e: impl std::fmt::Display) -> ApiError {
    ApiError(e.to_string())
}

#[tokio::main]
async fn main() {
    // `engramd verify [HOME]` - offline, third-party verification of the audit ledger
    // against its published public key, WITHOUT starting (or trusting) the daemon.
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("verify") => std::process::exit(verify_cmd(args.get(2).map(String::as_str))),
        // `engramd verify-autonomy [HOME]` - replay the ledger and reconstruct every autonomous
        // egress against the signed policy that authorized it (the offline "prove it" report).
        Some("verify-autonomy") => {
            std::process::exit(verify_autonomy_cmd(args.get(2).map(String::as_str)))
        }
        // `engramd doctor [HOME]` - a self-diagnostic of the local setup (config, provider,
        // ledger, embedder, channels, port, build features), the way `claude-desktop --doctor`
        // checks an install. Exits 0 when nothing is broken, 1 when a hard problem is found.
        Some("doctor") => std::process::exit(doctor_cmd(args.get(2).map(String::as_str))),
        // `engramd --next-wake [HOME]` - print the soonest scheduled job's fire time (epoch millis)
        // so a deploy wake-timer can arm itself for the NEXT actual job instead of polling on a static
        // calendar. Exits 1 (no output) when nothing is scheduled. Never binds the socket.
        Some("--next-wake") | Some("next-wake") => {
            std::process::exit(next_wake_cmd(args.get(2).map(String::as_str)))
        }
        Some("help") | Some("--help") | Some("-h") => {
            print_help();
            std::process::exit(0);
        }
        Some("--version") | Some("-V") => {
            println!("engramd {VERSION}");
            std::process::exit(0);
        }
        // `engramd --run-due` - fire any scheduled jobs that are due, then exit. This is what
        // the systemd wake-timer runs: it must NEVER start the HTTP server (which would collide
        // with the socket unit). Used for true zero-idle scheduled wakes on a VPS.
        Some("--run-due") | Some("run-due") => {
            init_tracing();
            match run(RunMode::RunDue).await {
                Ok(()) => std::process::exit(0),
                Err(e) => {
                    tracing::error!(error = %e, "run-due failed");
                    std::process::exit(1);
                }
            }
        }
        // Hidden: `engramd --extract-doc <name>` reads a document from STDIN and writes its
        // extracted text to STDOUT (exit 0), 3 = no text, 1 = read error. This is the isolated
        // child `extract_document_text_isolated` spawns so a panic inside a third-party parser
        // (pdf-extract/calamine/zip) — which would abort the whole daemon under `panic="abort"` —
        // only kills this short-lived child. Runs BEFORE init_tracing so stdout stays clean.
        Some("--extract-doc") => {
            use std::io::Read;
            let name = args.get(2).cloned().unwrap_or_default();
            let mut buf = Vec::new();
            if std::io::stdin().read_to_end(&mut buf).is_err() {
                std::process::exit(1);
            }
            match extract_document_text(&name, &buf) {
                Some(text) => {
                    use std::io::Write;
                    let _ = std::io::stdout().write_all(text.as_bytes());
                    std::process::exit(0);
                }
                None => std::process::exit(3),
            }
        }
        // Reject an unrecognized flag with usage instead of silently launching the server.
        Some(other) if other.starts_with('-') => {
            eprintln!("engramd: unknown option '{other}'\n");
            print_help();
            std::process::exit(2);
        }
        _ => {}
    }
    init_tracing();
    if let Err(e) = run(RunMode::Serve).await {
        tracing::error!(error = %e, "fatal");
        std::process::exit(1);
    }
}

/// How `run()` should behave: serve the HTTP API (the default daemon), or fire any due
/// scheduled jobs once and exit (the systemd wake-timer path, never binding the socket).
#[derive(Clone, Copy, PartialEq)]
enum RunMode {
    Serve,
    RunDue,
}

/// Offline verification of `<HOME>/ledger.jsonl` against `<HOME>/ledger.pub`. Exit codes:
/// 0 = signed chain intact, 1 = tampered/broken, 2 = setup error. This is the trust
/// payoff - anyone can confirm conduct without trusting the machine that produced it.
/// `engramd --next-wake [HOME]` — print the epoch-millis of the soonest scheduled job to STDOUT
/// (exit 0), exit 1 (no output) when nothing is scheduled, exit 2 on an open error. A deploy
/// wake-timer consults this to arm itself for the NEXT actual job (true zero-idle scheduling) instead
/// of a static daily poll. Read-only: it opens jobs.json + the ledger and never starts the daemon.
fn next_wake_cmd(home_arg: Option<&str>) -> i32 {
    let home = home_arg
        .map(String::from)
        .or_else(|| std::env::var("ENGRAM_HOME").ok())
        .unwrap_or_else(|| "./brain".into());
    let ledger = match Ledger::open(&home) {
        Ok(l) => Arc::new(l),
        Err(e) => {
            eprintln!("cannot open ledger: {e}");
            return 2;
        }
    };
    match Scheduler::open(&home, ledger).map(|s| s.next_wake()) {
        Ok(Some(ms)) => {
            println!("{ms}");
            0
        }
        Ok(None) => 1,
        Err(e) => {
            eprintln!("cannot open scheduler: {e}");
            2
        }
    }
}

fn verify_cmd(home_arg: Option<&str>) -> i32 {
    let home = home_arg
        .map(String::from)
        .or_else(|| std::env::var("ENGRAM_HOME").ok())
        .unwrap_or_else(|| "./brain".into());
    let dir = std::path::Path::new(&home);
    let ledger_path = dir.join("ledger.jsonl");
    let pub_path = dir.join("ledger.pub");
    // An absent or empty ledger is a setup error, not a verified chain.
    match std::fs::metadata(&ledger_path) {
        Ok(m) if m.len() > 0 => {}
        Ok(_) => {
            eprintln!("ledger is empty: {}", ledger_path.display());
            return 2;
        }
        Err(e) => {
            eprintln!("cannot read ledger {}: {e}", ledger_path.display());
            return 2;
        }
    }
    let pubhex = match std::fs::read_to_string(&pub_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("cannot read public key {}: {e}", pub_path.display());
            return 2;
        }
    };
    let vk = match engram_core::verifying_key_from_hex(&pubhex) {
        Ok(v) => v,
        Err(_) => {
            eprintln!("invalid public key in {}", pub_path.display());
            return 2;
        }
    };
    match engram_core::verify_file(&ledger_path, &vk) {
        Ok(n) => {
            println!(
                "OK - {n} entries, signed hash-chain intact: {}",
                ledger_path.display()
            );
            0
        }
        Err(e) => {
            eprintln!("TAMPER / BROKEN - {e}");
            1
        }
    }
}

/// `engramd verify-autonomy [HOME]` - replay the ledger and reconstruct the autonomy story (policies
/// granted, autonomous sends, staged/refused actions, async approvals), checking the signed chain
/// first. The offline, third-party "prove what the agent did unattended" report. Exit 1 if the chain
/// is broken, 0 otherwise.
fn verify_autonomy_cmd(home_arg: Option<&str>) -> i32 {
    let home = home_arg
        .map(String::from)
        .or_else(|| std::env::var("ENGRAM_HOME").ok())
        .unwrap_or_else(|| "./brain".into());
    let dir = std::path::Path::new(&home);
    let ledger_path = dir.join("ledger.jsonl");
    let pub_path = dir.join("ledger.pub");
    // Integrity first (same as `verify`), if the public key is present.
    let chain = std::fs::read_to_string(&pub_path)
        .ok()
        .and_then(|h| engram_core::verifying_key_from_hex(h.trim()).ok())
        .map(|vk| engram_core::verify_file(&ledger_path, &vk));
    let entries = match engram_core::entries_from_file(&ledger_path) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("cannot read ledger {}: {e}", ledger_path.display());
            return 2;
        }
    };
    match &chain {
        Some(Ok(n)) => println!("Chain: OK - {n} entries, signed hash-chain intact."),
        Some(Err(e)) => println!("Chain: TAMPER / BROKEN - {e}"),
        None => println!(
            "Chain: not checked (no public key at {})",
            pub_path.display()
        ),
    }
    let report = autonomy_report(&entries);
    let t = &report["totals"];
    println!("\nAutonomous egress (reconstructed from the signed ledger):");
    println!("  autonomous sends  : {}", t["autonomous_sends"]);
    println!("  staged for review : {}", t["staged"]);
    println!("  floor refusals    : {}", t["refused"]);
    println!("  later allowlisted : {}", t["allowlisted"]);
    println!("  denied            : {}", t["denied"]);
    println!("  one-time approvals: {}", report["one_time_approvals"]);
    if let Some(scopes) = report["scopes"].as_array() {
        if !scopes.is_empty() {
            println!("\nPer agent:");
            for s in scopes {
                println!(
                    "  {}  policy={}  sends={} staged={} refused={} allowlisted={} denied={}",
                    s["scope"].as_str().unwrap_or(""),
                    s["policy"],
                    s["autonomous_sends"],
                    s["staged"],
                    s["refused"],
                    s["allowlisted"],
                    s["denied"],
                );
            }
        }
    }
    matches!(chain, Some(Err(_))) as i32
}

/// `engramd doctor [HOME]` - a plain-language health check of the local install, so a user
/// (or a support ticket) can see at a glance what is configured and what is broken without
/// digging through files. Mirrors the role of `claude-desktop --doctor`. Exit 0 = healthy
/// (warnings are fine), 1 = at least one hard failure.
fn doctor_cmd(home_arg: Option<&str>) -> i32 {
    let home = home_arg
        .map(String::from)
        .or_else(|| std::env::var("ENGRAM_HOME").ok())
        .unwrap_or_else(|| "./brain".into());
    let dir = std::path::Path::new(&home);

    let mut fails = 0u32;
    let mut warns = 0u32;
    // ✓ ok / ⚠ warn / ✗ fail - one line each, with a short explanation.
    let ok = |label: &str, detail: &str| println!("  \u{2713} {label}: {detail}");
    let mut warn = |label: &str, detail: &str| {
        println!("  \u{26A0} {label}: {detail}");
        warns += 1;
    };
    // `warn` borrows `warns` mutably; failures take their counter by ref to avoid borrow conflicts.
    let fail = |label: &str, detail: &str, fails: &mut u32| {
        println!("  \u{2717} {label}: {detail}");
        *fails += 1;
    };

    println!("Engram doctor - {VERSION}\n");

    // --- State directory -----------------------------------------------------------------
    println!("State directory");
    match std::fs::metadata(dir) {
        Ok(_) => {
            // Probe writability with a temp file (the daemon needs to write here).
            let probe = dir.join(".doctor-write-probe");
            match std::fs::write(&probe, b"ok") {
                Ok(_) => {
                    let _ = std::fs::remove_file(&probe);
                    ok("ENGRAM_HOME", &format!("{} (writable)", dir.display()));
                }
                Err(e) => fail(
                    "ENGRAM_HOME",
                    &format!("{} not writable: {e}", dir.display()),
                    &mut fails,
                ),
            }
        }
        Err(_) => warn(
            "ENGRAM_HOME",
            &format!(
                "{} does not exist yet (created on first run)",
                dir.display()
            ),
        ),
    }

    // --- Model provider ------------------------------------------------------------------
    println!("\nModel provider");
    let cfg = config::Config::load(&home);
    let p = &cfg.provider;
    match p.kind.as_str() {
        "mock" => warn(
            "provider",
            "mock (offline) - answers are canned. Set a provider + API key to think for real.",
        ),
        kind => {
            let key = if p.api_key.is_empty() {
                "no API key"
            } else {
                "API key set"
            };
            if p.kind != "ollama" && p.api_key.is_empty() {
                fail(
                    "provider",
                    &format!("{kind} selected but no API key configured"),
                    &mut fails,
                );
            } else {
                ok(
                    "provider",
                    &format!("{kind} - model {} - {key}", cfg.model()),
                );
            }
        }
    }
    if !cfg!(feature = "http") && p.kind != "mock" {
        warn(
            "build",
            "this build has no network provider (the `http` feature is off) - only the mock runs.",
        );
    }

    // --- Embedder ------------------------------------------------------------------------
    println!("\nMemory / embeddings");
    match cfg.embed.kind.as_str() {
        "static" => {
            if cfg.embed.model_dir.is_empty() {
                fail(
                    "embedder",
                    "mode 'static' but no model directory set",
                    &mut fails,
                );
            } else if std::path::Path::new(&cfg.embed.model_dir)
                .join("model.safetensors")
                .exists()
            {
                ok(
                    "embedder",
                    &format!("static (model2vec) - {}", cfg.embed.model_dir),
                );
            } else {
                fail(
                    "embedder",
                    &format!("static model not found in {}", cfg.embed.model_dir),
                    &mut fails,
                );
            }
        }
        "gateway" => ok("embedder", "gateway (provider embeddings)"),
        _ => ok(
            "embedder",
            "trigram (offline default) - synonyms via the static model are optional",
        ),
    }

    // --- Audit ledger --------------------------------------------------------------------
    println!("\nAudit ledger");
    let ledger_path = dir.join("ledger.jsonl");
    let pub_path = dir.join("ledger.pub");
    match (
        std::fs::metadata(&ledger_path),
        std::fs::read_to_string(&pub_path),
    ) {
        (Ok(m), Ok(pubhex)) if m.len() > 0 => match engram_core::verifying_key_from_hex(&pubhex) {
            Ok(vk) => match engram_core::verify_file(&ledger_path, &vk) {
                Ok(n) => ok("ledger", &format!("{n} entries, signed hash-chain intact")),
                Err(e) => fail("ledger", &format!("TAMPER/BROKEN - {e}"), &mut fails),
            },
            Err(_) => fail("ledger", "public key is invalid", &mut fails),
        },
        _ => warn("ledger", "no ledger yet (written on first run)"),
    }

    // --- Tools / MCP ---------------------------------------------------------------------
    println!("\nTools & connectivity");
    if cfg.mcp.is_empty() {
        ok("mcp", "no MCP servers configured (optional)");
    } else {
        let names: Vec<&str> = cfg.mcp.iter().map(|m| m.name.as_str()).collect();
        ok(
            "mcp",
            &format!("{} server(s): {}", cfg.mcp.len(), names.join(", ")),
        );
    }
    if cfg.channels.telegram_token.is_empty() {
        ok("telegram", "not connected (optional)");
    } else {
        let who = if cfg.channels.telegram_username.is_empty() {
            "connected".to_string()
        } else {
            format!("@{}", cfg.channels.telegram_username)
        };
        ok("telegram", &who);
    }
    ok(
        "shell tool",
        if cfg.security.allow_shell {
            "ALLOWED (side-effecting)"
        } else {
            "off (safe default)"
        },
    );
    ok(
        "browser automation",
        if cfg!(feature = "browser-cdp") {
            "built in"
        } else {
            "not built (optional feature)"
        },
    );

    // --- Security gates ------------------------------------------------------------------
    println!("\nSecurity");
    let addr = std::env::var("ENGRAM_ADDR").unwrap_or_else(|_| "127.0.0.1:8088".into());
    let local = addr.starts_with("127.") || addr.starts_with("localhost");
    if cfg.security.api_token.is_empty() {
        if local {
            ok(
                "api auth",
                "no token, but bound to localhost (fine for desktop use)",
            );
        } else {
            fail(
                "api auth",
                &format!(
                    "NO API TOKEN and bound to {addr} - anyone on the network can drive the agent"
                ),
                &mut fails,
            );
        }
    } else {
        ok("api auth", "bearer token set");
    }
    ok(
        "key custody",
        "API key is memory-only (never written to config.json) - re-seeded from the environment each boot",
    );

    // --- Listener / port -----------------------------------------------------------------
    println!("\nListener");
    match std::net::TcpStream::connect(&addr) {
        Ok(_) => ok(
            "port",
            &format!("{addr} - a daemon is already serving here"),
        ),
        Err(_) => ok(
            "port",
            &format!("{addr} - free (the daemon will bind here)"),
        ),
    }

    // --- Summary -------------------------------------------------------------------------
    println!();
    if fails == 0 && warns == 0 {
        println!("All checks passed. Engram is ready.");
    } else {
        println!("{fails} failure(s), {warns} warning(s). Failures need attention; warnings are usually fine.");
    }
    if fails == 0 {
        0
    } else {
        1
    }
}

/// `engramd help` - a short usage summary for the CLI surface.
fn print_help() {
    println!(
        "engramd {VERSION} - the Engram agent daemon

USAGE:
    engramd                 Start the daemon and serve the dashboard (default)
    engramd doctor [HOME]   Health-check the local install (config, provider, ledger, ports)
    engramd verify [HOME]   Verify the signed audit ledger offline, without trusting the daemon
    engramd verify-autonomy [HOME]  Reconstruct every autonomous egress from the signed ledger
    engramd --run-due       Fire any scheduled jobs that are due, then exit (systemd wake-timer)
    engramd --next-wake     Print the next scheduled job's fire time (epoch ms); exit 1 if none
    engramd help            Show this help
    engramd --version       Print the version

KEY ENV VARS:
    ENGRAM_HOME             State directory (default ./brain)
    ENGRAM_ADDR             Listen address (default 127.0.0.1:8088)
    ENGRAM_IDLE_SECS        Idle seconds before sleeping to zero (default 900)
    ENGRAM_API_TOKEN        Require this bearer token on the HTTP API (set when exposed)
    ANTHROPIC_API_KEY       Bring up the Anthropic provider on a fresh install
    RUST_LOG                Log filter (e.g. info, engramd=debug)

Most configuration is done from the desktop Settings panel and saved to <HOME>/config.json."
    );
}

/// If launched under systemd socket activation (or any activator that follows the LISTEN_FDS
/// protocol), inherit the already-listening socket on fd 3 instead of binding it ourselves.
/// Returns `Ok(None)` when not socket-activated (the normal desktop/dev path).
#[cfg(unix)]
fn systemd_listener() -> std::io::Result<Option<std::net::TcpListener>> {
    use std::os::unix::io::FromRawFd;
    let fds = std::env::var("LISTEN_FDS")
        .ok()
        .and_then(|v| v.parse::<i32>().ok());
    let pid = std::env::var("LISTEN_PID")
        .ok()
        .and_then(|v| v.parse::<u32>().ok());
    // Only honor the handoff if it was meant for THIS process (LISTEN_PID guards against an fd
    // inherited by a child after the activator already consumed it).
    match (fds, pid) {
        (Some(n), Some(p)) if n >= 1 && p == std::process::id() => {
            // SD_LISTEN_FDS_START = 3 (after stdio). We use the first passed fd.
            let listener = unsafe { std::net::TcpListener::from_raw_fd(3) };
            listener.set_nonblocking(true)?;
            Ok(Some(listener))
        }
        _ => Ok(None),
    }
}
#[cfg(not(unix))]
fn systemd_listener() -> std::io::Result<Option<std::net::TcpListener>> {
    Ok(None)
}

/// Tell a `Type=notify` systemd service we are ready to accept connections (best-effort; a no-op
/// when not launched by systemd). Without this, a notify-type unit would wait and time out.
#[cfg(unix)]
fn sd_notify_ready() {
    let Ok(path) = std::env::var("NOTIFY_SOCKET") else {
        return;
    };
    let Ok(sock) = std::os::unix::net::UnixDatagram::unbound() else {
        return;
    };
    // systemd uses a leading '@' for an ABSTRACT-namespace socket (the common case). The std
    // path API can't address that, so build the abstract address explicitly on Linux; fall back
    // to the path form (and the rare leading-NUL form) otherwise. Advisory - ignore errors.
    if let Some(name) = path.strip_prefix('@') {
        #[cfg(target_os = "linux")]
        {
            use std::os::linux::net::SocketAddrExt;
            if let Ok(addr) = std::os::unix::net::SocketAddr::from_abstract_name(name.as_bytes()) {
                let _ = sock.send_to_addr(b"READY=1", &addr);
                return;
            }
        }
        let _ = name;
    } else {
        let _ = sock.send_to(b"READY=1", &path);
    }
}
#[cfg(not(unix))]
fn sd_notify_ready() {}

/// Take an exclusive advisory lock on `<home>/.lock` so only one daemon ever writes a given home's
/// signed ledger + brain. Returns the held File (keep it alive for the process lifetime). Retries
/// briefly so a normal restart - where the previous daemon is still flushing/exiting - waits for the
/// old lock to release instead of overlapping. flock auto-releases on fd-drop or process death (even
/// a crash), so there is no stale-lock problem. On non-unix it's a no-op (returns the open file).
fn acquire_home_lock(home: &str) -> Result<std::fs::File, Box<dyn std::error::Error>> {
    std::fs::create_dir_all(home)?;
    let path = std::path::Path::new(home).join(".lock");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)?;
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        let fd = file.as_raw_fd();
        let mut acquired = false;
        // Short wait (well under the desktop supervisor's 3s "fast exit = another instance owns it"
        // threshold), then fail FAST. Failing fast is correct: the supervisor sees the quick exit and
        // connects to the already-running daemon (or retries the spawn itself). Hanging ~3s here flaps
        // a redundant daemon and briefly makes the UI unable to reach :8088 ("Couldn't reach Engram").
        for attempt in 0..6 {
            // SAFETY: flock on a valid open fd; non-blocking exclusive lock.
            if unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) } == 0 {
                acquired = true;
                break;
            }
            if attempt == 0 {
                tracing::warn!(%home, "home is locked by another engramd - yielding to it");
            }
            std::thread::sleep(Duration::from_millis(150));
        }
        if acquired {
            Ok(file)
        } else {
            Err(format!(
                "another engramd already holds the lock on {home}; refusing to start a second instance \
                 (it would corrupt the signed ledger). Quit the other instance, or set a different ENGRAM_HOME."
            )
            .into())
        }
    }
    #[cfg(not(unix))]
    {
        Ok(file)
    }
}

async fn run(mode: RunMode) -> Result<(), Box<dyn std::error::Error>> {
    let home = std::env::var("ENGRAM_HOME").unwrap_or_else(|_| "./brain".into());
    // Make panics VISIBLE. With `panic = "abort"`, any panic (in any thread/task) takes the whole
    // daemon down with SIGABRT — and the desktop shell swallows the daemon's stderr, so it surfaces
    // only as "Couldn't reach Engram / Load failed" with no cause. Append each panic's location +
    // message + backtrace to `<home>/panic.log`, then chain to the default hook (stderr) so nothing
    // is lost. This is what turns the next abort from a mystery into a one-line fix.
    {
        let log_path = std::path::Path::new(&home).join("panic.log");
        let default_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            use std::io::Write;
            let loc = info
                .location()
                .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
                .unwrap_or_else(|| "<unknown>".into());
            let msg = info
                .payload()
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| info.payload().downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "<non-string panic payload>".into());
            let thread = std::thread::current()
                .name()
                .unwrap_or("unnamed")
                .to_string();
            let bt = std::backtrace::Backtrace::force_capture();
            if let Some(parent) = log_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&log_path)
            {
                let _ = writeln!(
                    f,
                    "[{}] thread '{}' panicked at {}: {}\n{}\n---",
                    engram_core::now_ms(),
                    thread,
                    loc,
                    msg,
                    bt
                );
            }
            default_hook(info);
        }));
    }
    // CRITICAL: hold an exclusive lock on the home for this daemon's whole life. Two daemons on one
    // ENGRAM_HOME interleave appends into the signed ledger and break the hash chain (the source of
    // the "ledger broken / verify fails" corruption). This makes a second instance refuse to start -
    // including a restart where the predecessor is still exiting (it waits briefly for release).
    // `_home_lock` MUST stay in scope; flock releases when the fd drops or the process dies.
    let _home_lock = acquire_home_lock(&home)?;
    let addr: SocketAddr = std::env::var("ENGRAM_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:8088".into())
        .parse()?;
    let idle = Duration::from_secs(env_u64("ENGRAM_IDLE_SECS", 900));

    let ledger = Arc::new(Ledger::open(&home)?);
    // Publish the ledger's public key so anyone can run `engramd verify` offline.
    let _ = std::fs::write(format!("{home}/ledger.pub"), ledger.pubkey_hex());

    // Settings: config.json wins, else seed from the environment (back-compat). Load WITHOUT the OS
    // keyring so a macOS Keychain password prompt can't block the HTTP bind (that stalled startup and
    // showed the desktop a white screen). The persisted key is read in the background below, after
    // the server is up, and hot-swapped into the provider.
    let cfg = config::Config::load_no_keychain(&home);
    apply_web_env(&cfg); // make the configured search-provider keys visible to the web_search tool
    let needs_keychain_key = cfg.provider.api_key.is_empty();
    tracing::info!(provider = %cfg.provider.kind, model = %cfg.model(), embed = %cfg.embed.kind, "settings loaded");
    let gateway = Arc::new(Gateway::new(cfg.build_provider(), ledger.clone()));
    gateway.set_default_effort(Some(cfg.provider.effort.clone()));

    // Pick the embedder: a real model through the gateway, the pure-Rust static model, or
    // the dependency-free trigram default. The gateway path probes its dimension once.
    let embedder: Arc<dyn engram_memory::Embedder> = match cfg.embed.kind.as_str() {
        "gateway" => {
            // Probe the embedding dimension once. If the provider has no embeddings endpoint
            // (Anthropic, the mock, a chat-only base), DON'T crash boot - warn and fall back to
            // the offline trigram embedder so the daemon still starts and recall still works.
            match gateway.embed(&["dimension probe".into()], "init").await {
                Ok(probe) => {
                    let dim = probe.first().map(|v| v.len()).unwrap_or(256);
                    tracing::info!(dim, "using gateway embedder");
                    Arc::new(embedder::GatewayEmbedder::new(
                        gateway.clone(),
                        dim,
                        &cfg.model(),
                    ))
                }
                Err(err) => {
                    tracing::warn!(error = %err, provider = %cfg.provider.kind,
                        "gateway embeddings unavailable - falling back to the trigram embedder");
                    Arc::new(TrigramHashEmbedder::default())
                }
            }
        }
        // Pure-Rust static model2vec embedder - real synonym recall, no model runtime.
        "static" => {
            let model_dir = if cfg.embed.model_dir.is_empty() {
                format!("{home}/embedder")
            } else {
                cfg.embed.model_dir.clone()
            };
            match engram_memory::StaticEmbedder::load(&model_dir) {
                Ok(e) => {
                    tracing::info!(dir = %model_dir, dim = engram_memory::Embedder::dim(&e), "using static model2vec embedder");
                    Arc::new(e)
                }
                Err(err) => {
                    tracing::warn!(dir = %model_dir, error = %err, "static embedder load failed - falling back to trigram");
                    Arc::new(TrigramHashEmbedder::default())
                }
            }
        }
        _ => Arc::new(TrigramHashEmbedder::default()),
    };

    let memory = Arc::new(Memory::open(
        format!("{home}/brain.db"),
        embedder,
        ledger.clone(),
    )?);
    let signer = Arc::new(SkillSigner::load_or_create(format!(
        "{home}/keys/skill.key"
    ))?);
    let registry = Arc::new(Registry::open(&home, signer, ledger.clone())?);
    seed::ensure_seed(&registry)?;
    seed::ensure_seed_skills(&registry)?;
    let sched = Arc::new(Scheduler::open(&home, ledger.clone())?);
    let bus = Bus::new(1024);
    let activity = Activity::new();
    let workdir = std::path::PathBuf::from(
        std::env::var("ENGRAM_WORKDIR").unwrap_or_else(|_| format!("{home}/work")),
    );
    std::fs::create_dir_all(&workdir)?;
    // Personality / standing instructions, shaping every agent run (a SOUL.md persona).
    let persona = std::fs::read_to_string(format!("{home}/SOUL.md")).ok();
    // Connect any MCP servers listed in mcp.json and borrow their tools.
    let mcp_tools = load_mcp(&home).await;
    if !mcp_tools.is_empty() {
        tracing::info!(count = mcp_tools.len(), "mcp tools available to the agent");
    }

    ledger.append(
        "core.boot",
        "core",
        json!({ "version": VERSION, "addr": addr.to_string() }),
    )?;

    let app = App {
        memory,
        registry,
        host: Arc::new(SkillHost::new()),
        gateway,
        sched,
        ledger: ledger.clone(),
        bus,
        activity: activity.clone(),
        workdir,
        persona: Arc::new(std::sync::RwLock::new(persona)),
        mcp_tools: Arc::new(std::sync::RwLock::new(mcp_tools)),
        browser: engram_agent::browser_session(
            Some(cfg.browser.chrome_path.clone()).filter(|p| !p.is_empty()),
            Some(cfg.browser.cdp_port).filter(|p| *p != 0),
        ),
        tasks: Arc::new(tasks::TaskStore::open(std::path::Path::new(&home))),
        workspace: Arc::new(workspace::WorkspaceStore::open(std::path::Path::new(&home))),
        allow_shell: Arc::new(std::sync::atomic::AtomicBool::new(cfg.security.allow_shell)),
        halt: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        run_halts: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        config: Arc::new(std::sync::RwLock::new(cfg)),
        home: home.clone(),
        telegram: Arc::new(std::sync::Mutex::new(None)),
        consciousness: Arc::new(conscious::Consciousness::open(&home)),
        agents: Arc::new(agents::AgentStore::open(std::path::Path::new(&home))),
        in_flight: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
    };
    // Seed working memory once on first run, so the always-loaded block is never empty when the
    // brain already holds memories. Best-effort: a fresh brain just yields an empty block.
    if app.consciousness.snapshot().version == 0 {
        let _ = app.consciousness.distill(&app.memory, &app.ledger);
    }

    let router = Router::new()
        .route("/", get(dashboard))
        .route("/health", get(health))
        .route("/v1/meter", get(meter))
        .route("/v1/memory/stats", get(memory_stats))
        .route("/v1/memory/recent", get(memory_recent))
        .route("/v1/memory/graph", get(memory_graph))
        .route("/v1/memory/reindex", post(memory_reindex))
        .route("/v1/memory/promote", post(memory_promote))
        .route("/v1/embedder/fetch-model", post(embedder_fetch_model))
        .route("/v1/screenshot", get(screenshot_get))
        .route("/v1/artifact", get(artifact_get).delete(artifact_delete))
        .route("/v1/artifacts", get(artifacts_list))
        .route("/v1/remember", post(remember))
        .route("/v1/recall", get(recall))
        .route("/v1/forget", post(forget))
        .route("/v1/supersessions", get(supersessions_list))
        .route(
            "/v1/supersessions/{id}/resolve",
            post(supersessions_resolve),
        )
        .route("/v1/memory/reflections", get(reflections_list))
        .route("/v1/consciousness", get(consciousness_get))
        .route("/v1/consciousness/distill", post(consciousness_distill))
        .route("/v1/consciousness/edit", post(consciousness_edit))
        .route("/v1/consciousness/add", post(consciousness_add))
        .route("/v1/consciousness/remove", post(consciousness_remove))
        .route("/v1/consciousness/revert", post(consciousness_revert))
        .route("/v1/agents", get(agents_list).post(agents_create))
        .route("/v1/agents/{id}", post(agents_update).delete(agents_delete))
        .route("/v1/agents/{id}/policy", post(agent_set_policy))
        .route("/v1/agents/{id}/activity", get(agent_activity))
        .route("/v1/egress/pending", get(egress_pending))
        .route("/v1/egress/approve", post(egress_approve))
        .route("/v1/egress/deny", post(egress_deny))
        .route("/v1/autonomy/report", get(autonomy_report_handler))
        .route("/v1/skills", get(skills).post(skill_create))
        .route("/v1/skills/boilerplate", get(skill_boilerplate))
        .route("/v1/open", post(open_url))
        .route("/v1/tools", get(tools_list))
        .route("/v1/skills/{id}/run", post(run_skill))
        .route("/v1/skills/{id}/enabled", post(skill_toggle))
        .route("/v1/skills/{id}/improve", post(skill_improve))
        .route("/v1/skills/{id}/activate", post(skill_activate))
        .route("/v1/skills/{id}/adopt", post(skill_adopt))
        .route("/v1/skills/{id}/revert", post(skill_revert))
        .route("/v1/skills/{id}/teach", post(skill_teach))
        .route("/v1/swarm", post(run_swarm))
        .route("/v1/mission", post(run_mission))
        .route("/v1/agent", post(agent_handler))
        .route("/v1/voice", post(voice_handler))
        .route("/v1/voice/stream", get(voice_stream))
        .route("/v1/channel/{platform}", post(channels::channel_handler))
        .route("/v1/converse", post(converse_handler))
        .route("/v1/converse/stream", post(converse_stream_handler))
        .route(
            "/v1/upload",
            post(upload_handler).layer(axum::extract::DefaultBodyLimit::max(34 * 1024 * 1024)),
        )
        .route("/v1/ledger/tail", get(ledger_tail))
        .route("/v1/ledger/verify", get(ledger_verify))
        .route(
            "/v1/checkpoints",
            get(checkpoints_list).post(checkpoints_create),
        )
        .route("/v1/checkpoints/{id}/restore", post(checkpoints_restore))
        .route(
            "/v1/checkpoints/{id}",
            axum::routing::delete(checkpoints_delete),
        )
        .route("/v1/schedule", get(schedule_list).post(schedule_add))
        .route("/v1/schedule/preview", get(schedule_preview))
        .route(
            "/v1/schedule/{id}",
            axum::routing::delete(schedule_remove).patch(schedule_update),
        )
        .route("/v1/schedule/{id}/run", post(schedule_run))
        .route("/v1/tasks", get(tasks_list).post(tasks_create))
        .route(
            "/v1/tasks/{id}",
            axum::routing::patch(tasks_update).delete(tasks_delete),
        )
        .route("/v1/tasks/{id}/agent", post(tasks_assign))
        .route("/v1/tasks/{id}/handoff", post(task_handoff))
        .route("/v1/tasks/{id}/review", post(task_review))
        .route("/v1/tasks/{id}/dissent", post(task_dissent))
        .route("/v1/tasks/{id}/run", post(tasks_run))
        .route("/v1/tasks/{id}/run/stream", post(tasks_run_stream))
        .route("/v1/tasks/{id}/audit", get(task_audit))
        .route("/v1/tasks/{id}/receipt", get(task_receipt))
        .route("/v1/projects", get(projects_list).post(projects_create))
        .route(
            "/v1/projects/{id}",
            axum::routing::patch(projects_update).delete(projects_delete),
        )
        .route(
            "/v1/projects/{id}/ensure-workdir",
            post(projects_ensure_workdir),
        )
        .route("/v1/sessions", get(sessions_list).post(sessions_create))
        .route(
            "/v1/sessions/{id}",
            get(session_get)
                .patch(session_update)
                .delete(session_delete),
        )
        .route("/v1/ledger/pubkey", get(ledger_pubkey))
        .route("/v1/policy", get(policy_get).post(policy_set))
        .route("/v1/shell", post(terminal::shell_handler))
        .route("/v1/fs", get(terminal::fs_handler))
        .route("/v1/git/status", get(git::git_status))
        .route("/v1/git/diff", get(git::git_diff))
        .route("/v1/git/branches", get(git::git_branches))
        .route(
            "/v1/git/worktrees",
            get(git::git_worktrees)
                .post(git::git_worktree_create)
                .delete(git::git_worktree_remove),
        )
        .route("/v1/tasks/{id}/changes", get(git::task_changes))
        .route("/v1/config", get(config_get).post(config_set))
        .route("/v1/config/test", post(config_test))
        .route("/v1/config/mcp-test", post(config_mcp_test))
        .route("/v1/channels", get(channels_status))
        .route("/v1/channels/telegram/connect", post(telegram_connect))
        .route(
            "/v1/channels/telegram/disconnect",
            post(telegram_disconnect),
        )
        .route("/v1/persona", get(persona_get).post(persona_set))
        .route("/v1/restart", post(restart_handler))
        .route("/v1/shutdown", post(shutdown_handler))
        .route("/v1/halt", post(halt_set))
        .route("/v1/events", get(events))
        .layer(axum::middleware::from_fn_with_state(
            app.clone(),
            keep_awake,
        ))
        .layer(axum::middleware::from_fn_with_state(
            app.clone(),
            require_auth,
        ))
        .with_state(app.clone());

    // Inbound messaging channel: run as a Telegram bot if a token is configured.
    // Prefer a token saved from the Integrations gallery (config.json); fall back to the env.
    let tg_token = {
        let t = app.cfg().channels.telegram_token.clone();
        if t.is_empty() {
            std::env::var("ENGRAM_TELEGRAM_TOKEN").ok()
        } else {
            Some(t)
        }
    };
    // Only the long-lived server hosts the inbound channels. In --run-due (a one-shot wake) we
    // must NOT start the Telegram long-poll, or it could land a concurrent UNTRUSTED run during a
    // wake that is supposed to fire scheduled jobs and exit.
    if mode == RunMode::Serve {
        if let Some(token) = tg_token.filter(|t| !t.is_empty()) {
            tracing::info!("telegram channel active");
            let handle = telegram::spawn(app.clone(), token);
            let uname = app.cfg().channels.telegram_username.clone();
            *app.telegram.lock().expect("telegram lock") = Some((handle, uname));
        }
    }
    // `--run-due`: fire any scheduled jobs that are due, then exit WITHOUT binding the socket
    // (which systemd owns) or starting the HTTP server. This is the zero-idle wake-timer path.
    if mode == RunMode::RunDue {
        let now = chrono::Utc::now();
        let mut fired = 0usize;
        for job in app.sched.due(now) {
            let task = task_from_schedule(&app, &job.payload, &job.name);
            let _ = app.sched.set_last_task(&job.id, &task.id);
            // Mark fired BEFORE running (matching spawn_scheduler_tick): if this one-shot process dies
            // mid-run, the occurrence is still consumed, so the next wake doesn't re-fire it — the same
            // double-fire (e.g. a digest sent twice) the in-daemon tick was fixed to avoid.
            let _ = app.sched.mark_fired(&job.id, now);
            let _ = run_task_core(&app, &task.id, None, false, false).await;
            fired += 1;
        }
        tracing::info!(fired, "ran due scheduled jobs (--run-due), exiting");
        return Ok(());
    }

    // Fire scheduled jobs while the daemon is awake.
    spawn_scheduler_tick(app.clone());
    spawn_consolidation_tick(app.clone());
    // Keep the daemon awake while ANY agent run is in flight, even one with no open HTTP connection
    // (scheduled / Telegram / a stream whose client disconnected). Without this the idle clock — reset
    // only by inbound HTTP — would fire after the idle window and drop the runtime mid-run.
    spawn_run_keepalive(app.clone());

    // Load the persisted API key OFF the startup path. Reading the OS keyring can pop a blocking
    // macOS Keychain password prompt (adhoc-signed app); doing it before the bind stalled the server
    // and showed the desktop a WHITE SCREEN. The server is up by the time this runs, so the prompt no
    // longer blocks the UI — and once the key arrives we hot-swap the live provider in.
    if needs_keychain_key {
        let app_bg = app.clone();
        let home_bg = home.clone();
        tokio::spawn(async move {
            // An env key would already be set; only consult the keyring if we still have none.
            if !app_bg.cfg().provider.api_key.is_empty() {
                return;
            }
            let key = tokio::task::spawn_blocking(move || config::read_secret_key(&home_bg))
                .await
                .ok()
                .flatten();
            if let Some(k) = key.filter(|k| !k.is_empty()) {
                let new_cfg = {
                    let mut c = app_bg.config.write().expect("config lock");
                    c.provider.api_key = k;
                    c.clone()
                };
                app_bg
                    .gateway
                    .set_provider(std::sync::Arc::from(new_cfg.build_provider()));
                app_bg
                    .gateway
                    .set_default_effort(Some(new_cfg.provider.effort.clone()));
                tracing::info!(
                    "provider key loaded from keyring (background); live provider ready"
                );
            }
        });
    }

    // Prefer a socket-activated listener: when systemd (or any activator) hands us a listening
    // fd via LISTEN_FDS, we inherit it instead of binding - this is what makes "0 MB resident at
    // idle" real, since systemd owns the port and only spawns us on a connection. Binding the
    // port ourselves under socket activation would EADDRINUSE on the very first request. Falling
    // back to a normal bind keeps the desktop/dev path (and non-systemd hosts) working. SO_REUSEADDR
    // keeps the Settings panel's "Restart daemon" reliable across the kernel's TIME_WAIT.
    // Refuse to expose an unauthenticated control plane on ANY listen path (self-bind OR a
    // socket-activated inherited fd): a non-loopback address with no API token would let anyone
    // on the network drive a self-modifying agent that can run a shell and a browser.
    let guard_exposure = |real: SocketAddr| -> Result<(), Box<dyn std::error::Error>> {
        let is_loopback = real.ip().is_loopback();
        if !is_loopback
            && app.cfg().security.api_token.is_empty()
            && std::env::var("ENGRAM_ALLOW_INSECURE").as_deref() != Ok("1")
        {
            return Err(format!(
                "refusing to serve on {real} with no API token - an exposed agent must be \
                 authenticated. Set ENGRAM_API_TOKEN, bind 127.0.0.1 (default), or set \
                 ENGRAM_ALLOW_INSECURE=1 to override."
            )
            .into());
        }
        Ok(())
    };
    let listener = match systemd_listener()? {
        Some(std_listener) => {
            // CRITICAL: apply the exposure guard to the REAL inherited socket address (set by the
            // systemd .socket unit), not ENGRAM_ADDR - otherwise an operator who points
            // ListenStream at 0.0.0.0 but forgets the token gets a world-open control plane.
            let real = std_listener.local_addr()?;
            guard_exposure(real)?;
            tracing::info!(%real, idle_s = idle.as_secs(), "engram awake - socket-activated (inherited fd)");
            tokio::net::TcpListener::from_std(std_listener)?
        }
        None => {
            guard_exposure(addr)?;
            let socket = match addr {
                SocketAddr::V4(_) => tokio::net::TcpSocket::new_v4()?,
                SocketAddr::V6(_) => tokio::net::TcpSocket::new_v6()?,
            };
            socket.set_reuseaddr(true)?;
            socket.bind(addr)?;
            tracing::info!(version = VERSION, %addr, idle_s = idle.as_secs(), "engram awake - http ready");
            socket.listen(1024)?
        }
    };
    // Tell a Type=notify supervisor we're ready to accept (no-op when not under systemd).
    sd_notify_ready();

    let in_flight_shutdown = app.in_flight.clone();
    axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            match run_until_idle(activity, idle).await {
                engram_core::WakeReason::Idle => tracing::info!("idle - sleeping to zero"),
                engram_core::WakeReason::Signal => tracing::info!("signal - sleeping to zero"),
            }
            // Drain in-flight agent runs before letting the runtime drop. The idle path won't fire
            // while a run is live (the keepalive touches activity), but a shutdown SIGNAL can arrive
            // mid-run — without this wait the runtime would drop and abort the detached run, losing its
            // receipt and re-firing scheduled jobs. Bounded so a truly stuck run can't wedge shutdown.
            use std::sync::atomic::Ordering;
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
            while in_flight_shutdown.load(Ordering::SeqCst) > 0
                && std::time::Instant::now() < deadline
            {
                tracing::info!(
                    in_flight = in_flight_shutdown.load(Ordering::SeqCst),
                    "waiting for in-flight agent runs to drain before sleeping"
                );
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }
        })
        .await?;

    let _ = ledger.append("core.sleep", "core", json!({}));
    match ledger.verify() {
        Ok(n) => tracing::info!(entries = n, "ledger verified on exit"),
        Err(e) => tracing::error!(error = %e, "ledger verification failed"),
    }
    Ok(())
}

/// Middleware: every request keeps the brain awake and fires a spike (Live Cortex).
async fn keep_awake(
    State(app): State<App>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    app.activity.touch();
    let path = req.uri().path().to_string();
    app.bus.emit(Spike::new(
        "http.request",
        Priority::Normal,
        json!({ "path": path }),
    ));
    next.run(req).await
}

/// Optional bearer-token auth. When `ENGRAM_API_TOKEN` is unset (the local-desktop
/// default, bound to 127.0.0.1) every request passes. When set - for an exposed
/// deployment - every `/v1` call must present `Authorization: Bearer <token>` (or, for
/// EventSource/WebSocket which cannot set headers, `?token=<token>`). The dashboard,
/// health, and inbound webhooks (which carry their own platform auth) stay open.
async fn require_auth(
    State(app): State<App>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    // DNS-rebinding defense, applied even when there is NO token (the default desktop posture: bound
    // to 127.0.0.1 with an empty token). A malicious website can rebind its own hostname to 127.0.0.1
    // and issue same-origin requests to :8088 from the victim's browser — driving a shell-capable,
    // self-modifying agent. CORS does NOT stop this (the request is same-origin to the attacker's
    // domain). The standard local-daemon fix (Chrome DevTools, Ollama, …) is to require the Host
    // header be a loopback name and reject state-changing requests carrying a foreign Origin — a
    // rebind attack sends the attacker's hostname in Host, which won't match a loopback name.
    let path = req.uri().path();
    // Health/dashboard-root probes stay open (the dashboard HTML carries no secret); everything else
    // must present a loopback Host. A missing Host is allowed (HTTP/1.0 / some proxies) — the token
    // gate below still applies when configured.
    if path != "/" && path != "/health" {
        if let Some(host) = req
            .headers()
            .get(axum::http::header::HOST)
            .and_then(|h| h.to_str().ok())
        {
            if !is_loopback_host(host) {
                return (
                    axum::http::StatusCode::FORBIDDEN,
                    "forbidden: non-loopback Host (DNS-rebinding defense)",
                )
                    .into_response();
            }
        }
        // Reject a foreign Origin on state-changing methods (CSRF / rebind write). Same-origin
        // first-party requests either omit Origin or send a loopback one; a null Origin (opaque
        // sandbox) is also rejected for writes.
        let method = req.method().clone();
        let mutating = !matches!(
            method,
            axum::http::Method::GET | axum::http::Method::HEAD | axum::http::Method::OPTIONS
        );
        if mutating {
            if let Some(origin) = req
                .headers()
                .get(axum::http::header::ORIGIN)
                .and_then(|h| h.to_str().ok())
            {
                if !is_loopback_origin(origin) {
                    return (
                        axum::http::StatusCode::FORBIDDEN,
                        "forbidden: cross-origin write blocked",
                    )
                        .into_response();
                }
            }
        }
    }
    let token = app.cfg().security.api_token.clone();
    if token.is_empty() {
        return next.run(req).await;
    }
    // The dashboard root and the liveness probe are always open. Inbound channel webhooks are
    // exempt from the bearer token ONLY when they carry their own shared secret (the handler
    // enforces it); without a channel secret they fall under the token gate, so an exposed
    // deployment can never be driven by an anonymous caller. (Channel runs also start Untrusted.)
    let channel_has_secret = !app.cfg().security.channel_secret.is_empty();
    if path == "/" || path == "/health" || (path.starts_with("/v1/channel/") && channel_has_secret)
    {
        return next.run(req).await;
    }
    // The `?token=` fallback exists ONLY for EventSource/WebSocket, which cannot set an Authorization
    // header. Restrict it to those routes so a normal fetch that chose the query form doesn't leak the
    // bearer token into browser history, proxies, and access logs. Everything else must use the header.
    let query_token_ok = matches!(path, "/v1/events" | "/v1/voice/stream");
    let presented = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .map(str::to_string)
        .or_else(|| {
            if !query_token_ok {
                return None;
            }
            req.uri()
                .query()
                .and_then(|q| q.split('&').find_map(|kv| kv.strip_prefix("token=")))
                // Percent-decode: a token with reserved chars (+, =, &, %) is sent url-encoded by a
                // correct client, so comparing the raw substring would spuriously reject it.
                .map(percent_decode)
        });
    if presented.map(|t| ct_eq(&t, &token)).unwrap_or(false) {
        next.run(req).await
    } else {
        (axum::http::StatusCode::UNAUTHORIZED, "unauthorized").into_response()
    }
}

/// Whether a Host header names the loopback interface (any port). Used as the DNS-rebinding gate: a
/// rebind attack carries the attacker's own hostname in Host, which is not a loopback name.
fn is_loopback_host(host: &str) -> bool {
    // Strip the optional port. IPv6 hosts are bracketed: [::1]:8088.
    let hostname = if let Some(rest) = host.strip_prefix('[') {
        // [::1]:port or [::1]
        rest.split(']').next().unwrap_or("")
    } else {
        host.split(':').next().unwrap_or("")
    };
    let hostname = hostname.trim();
    hostname.eq_ignore_ascii_case("localhost")
        // RFC 6761 reserves the `.localhost` TLD as loopback; Tauri's WKWebView on Windows/Linux
        // serves the app from `tauri.localhost`, so accept any `*.localhost` name (still loopback).
        || hostname.to_ascii_lowercase().ends_with(".localhost")
        || hostname == "127.0.0.1"
        || hostname == "::1"
        || hostname == "0.0.0.0" // some clients send the bind addr
        // Any 127.0.0.0/8 loopback address.
        || hostname
            .parse::<std::net::Ipv4Addr>()
            .map(|ip| ip.is_loopback())
            .unwrap_or(false)
}

/// Whether an Origin header is a loopback origin (scheme://loopback[:port]). A missing Origin is
/// handled by the caller; here we only classify a present one.
fn is_loopback_origin(origin: &str) -> bool {
    // Origin is `scheme://host[:port]` (or the literal "null" for opaque origins, which we reject).
    let after_scheme = origin.split_once("://").map(|(_, rest)| rest);
    match after_scheme {
        Some(host_port) => is_loopback_host(host_port),
        None => false,
    }
}

/// Minimal application/x-www-form-urlencoded percent-decoder for a single query value. Decodes `%XX`
/// escapes and `+` → space; leaves malformed escapes as-is. Enough for the `?token=` fallback.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hi = (bytes[i + 1] as char).to_digit(16);
                let lo = (bytes[i + 2] as char).to_digit(16);
                match (hi, lo) {
                    (Some(h), Some(l)) => {
                        out.push((h * 16 + l) as u8);
                        i += 3;
                    }
                    _ => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Monotonic counter making each chat run's halt-map key unique within its session, so two runs in
/// the same session don't clobber each other's stop flag.
static RUN_HALT_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// Whether a run-halt map key belongs to a given session. Keys are `<session>#<n>`; an exact match
/// (a legacy bare-session key) is also honored so a stop never silently misses.
fn halt_key_matches(key: &str, session: &str) -> bool {
    key == session || key.split('#').next() == Some(session)
}

/// Constant-time string compare (length may leak; contents do not).
fn ct_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}

async fn dashboard() -> impl IntoResponse {
    // Served without auth so the page can bootstrap - so it must NEVER embed the API token,
    // which would hand it to any unauthenticated caller of "/" and defeat the gate. The
    // first-party dashboard stores the token in the browser (set from Settings, kept in
    // localStorage) and sends it on its own API calls; a fresh client is prompted for it.
    //
    // `no-store` so the embedded webview never renders a stale build after an update: the HTML
    // always comes from the local daemon (no network cost to caching it), but WKWebView's disk
    // cache would otherwise heuristically serve an old page across relaunches.
    (
        [(
            axum::http::header::CACHE_CONTROL,
            "no-store, no-cache, must-revalidate",
        )],
        Html(include_str!("../assets/index.html").to_string()),
    )
}

async fn health(State(app): State<App>) -> ApiResult {
    // "offline" iff the *live* provider is the mock - the single honest signal, derived from
    // what the gateway will actually call, not an env-var guess. (The old heuristic missed a
    // standard ANTHROPIC_API_KEY, a config.json provider, or a custom base, so it could claim
    // "offline" while a real model was connected - exactly the kind of UI bluff we forbid.)
    let offline = app.gateway.provider_id() == "mock";
    Ok(Json(
        json!({ "ok": true, "version": VERSION, "offline": offline }),
    ))
}

async fn meter(State(app): State<App>) -> ApiResult {
    Ok(Json(
        serde_json::to_value(app.gateway.meter()).map_err(err)?,
    ))
}

#[derive(Deserialize, Default)]
struct StatsQuery {
    /// Restrict the breakdown to one ring: "project"/"session"/"user". Omitted = whole brain.
    #[serde(default)]
    scope_kind: Option<String>,
    #[serde(default)]
    scope_id: Option<String>,
}

async fn memory_stats(State(app): State<App>, Query(q): Query<StatsQuery>) -> ApiResult {
    let stats = match &q.scope_kind {
        Some(kind) => app
            .memory
            .stats_for_scope(kind, q.scope_id.as_deref().unwrap_or(""))
            .map_err(err)?,
        None => app.memory.stats().map_err(err)?,
    };
    let mut v = serde_json::to_value(stats).map_err(err)?;
    // Embedder health: what the user configured vs. what's actually embedding vectors right now.
    // The selection at boot (see `run()`) silently falls back to trigram on any failure (a gateway
    // provider with no embeddings endpoint, a missing/unreadable static model dir) and previously
    // only logged a `tracing::warn!` - invisible outside the daemon's own log file. Surfacing it
    // here turns a silent degradation into a fact every surface (desktop/TUI/CLI) can display.
    let configured = app.cfg().embed.kind.clone();
    let active = app.memory.embedder_name().to_string();
    let degraded = match configured.as_str() {
        "gateway" => !active.starts_with("gateway:"),
        "static" => active != "static-model2vec-v1",
        _ => false,
    };
    if let Some(obj) = v.as_object_mut() {
        obj.insert("embedder_configured".into(), json!(configured));
        obj.insert("embedder_active".into(), json!(active));
        obj.insert("embedder_degraded".into(), json!(degraded));
    }
    Ok(Json(v))
}

/// Rebuild the derived binary coarse index from the stored embeddings - a repair hook if the index
/// is ever suspected corrupt. Recall keeps working throughout (the index is derived, not content).
async fn memory_reindex(State(app): State<App>) -> ApiResult {
    let n = app.memory.reindex_binary().map_err(err)?;
    Ok(Json(json!({ "reindexed": n })))
}

/// Download the pinned static (model2vec) embedding model - the one-click recall-quality upgrade
/// from Engram's zero-dependency trigram-hash default to a real semantic embedder (see
/// `embedder::fetch_static_model`; the recall-quality case is in `crates/engram-bench/
/// BENCHMARKS.md` §3 - it closes the exact gap found there). **User-initiated only**, never
/// called automatically - offline-by-default means Engram never reaches for the network unless
/// asked. Only downloads and validates the model; the caller still PATCHes `/v1/config`
/// (`{"embed": {"kind": "static", "model_dir": "<returned path>"}}`) and restarts the daemon to
/// actually switch to it - the same `restart_needed` pattern every other embedder-affecting
/// setting already uses (the CLI's `engram model fetch` does both steps in one command).
async fn embedder_fetch_model(State(app): State<App>) -> ApiResult {
    let dir = embedder::fetch_static_model(std::path::Path::new(&app.home))
        .await
        .map_err(err)?;
    app.ledger
        .append(
            "embedder.fetch_model",
            "user",
            json!({ "model_dir": dir.display().to_string() }),
        )
        .ok();
    Ok(Json(
        json!({ "ok": true, "model_dir": dir.display().to_string() }),
    ))
}

#[derive(Deserialize)]
struct PromoteReq {
    id: i64,
}

/// Promote a project/session memory to the user-global ring, so a fact that turns out to be a
/// durable cross-project preference follows the user everywhere. Ledgered; trusted-only.
async fn memory_promote(State(app): State<App>, Json(r): Json<PromoteReq>) -> Response {
    match app.memory.promote_to_user(r.id, "user") {
        Ok(ok) => Json(json!({ "promoted": ok })).into_response(),
        // A caller-visible constraint, not a server fault: surface it as a 4xx with a clear reason
        // so the UI can gate the "Make global" button instead of showing an opaque 500.
        Err(engram_memory::MemoryError::UntrustedPromotion) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "untrusted memories cannot be promoted to the global ring" })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("could not promote: {e}") })),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
struct RecentQuery {
    #[serde(default)]
    region: Option<String>,
    #[serde(default)]
    n: Option<usize>,
}

async fn memory_recent(State(app): State<App>, Query(q): Query<RecentQuery>) -> ApiResult {
    let region = parse_region(q.region.as_deref());
    let recs = app
        .memory
        .recent(region, q.n.unwrap_or(20).min(100))
        .map_err(err)?;
    Ok(Json(serde_json::to_value(recs).map_err(err)?))
}

#[derive(serde::Deserialize)]
struct ShotQuery {
    path: String,
    /// The run this screenshot belongs to (a chat session id or task/artifact bucket id). A chat in a
    /// project with a bound working directory runs the agent in THAT dir, so the browser screenshot
    /// lands there — not the shared daemon workdir. Without this, `/v1/screenshot?path=…` resolved only
    /// against the shared workdir and 404'd for every project chat (the flagship "shows what it saw"
    /// feature silently never appeared). We try the session's workdir, then the shared workdir, then
    /// the persisted artifacts bucket (where post-run capture copies it), so the image resolves live
    /// AND after the worktree/workdir is gone.
    #[serde(default)]
    session: Option<String>,
}

/// Serve a browser screenshot (or any image the agent saved) from the workspace so the chat/task
/// view can show it inline. Strictly confined to a known run root and to image types - it can never
/// read an arbitrary file off the box.
async fn screenshot_get(State(app): State<App>, Query(q): Query<ShotQuery>) -> Response {
    use axum::http::{header, StatusCode};
    let lower = q.path.to_lowercase();
    let ct = if lower.ends_with(".png") {
        "image/png"
    } else if lower.ends_with(".webp") {
        "image/webp"
    } else if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
        "image/jpeg"
    } else {
        return (StatusCode::BAD_REQUEST, "not an image").into_response();
    };
    // The candidate roots this screenshot could live under, each confined by canonicalize+starts_with:
    //   1. the session's project workdir (project chats run there),
    //   2. the shared daemon workdir (project-less chats / task runs on the shared tree),
    //   3. the persisted per-bucket artifacts dir (post-run capture, survives worktree cleanup).
    let mut roots: Vec<std::path::PathBuf> = Vec::new();
    if let Some(sid) = q.session.as_deref() {
        if let Some(wd) = app.workspace.workdir_for_session(sid) {
            roots.push(wd);
        }
        // Guard the bucket id like /v1/artifact does (single path segment) before joining it.
        if !sid.is_empty() && !sid.contains('/') && !sid.contains('\\') && !sid.contains("..") {
            roots.push(std::path::Path::new(&app.home).join("artifacts").join(sid));
        }
    }
    roots.push(app.workdir.clone());
    // Find the first root under which the requested path canonicalizes and stays confined.
    let resolved = roots.into_iter().find_map(|base| {
        let full = base.join(&q.path);
        match (base.canonicalize(), full.canonicalize()) {
            (Ok(b), Ok(f)) if f.starts_with(&b) => Some(f),
            _ => None,
        }
    });
    let Some(full) = resolved else {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    };
    // Cap the read so a pathologically large file in the workspace can't exhaust memory (a screenshot
    // is normally well under this).
    const MAX_SHOT: u64 = 32 * 1024 * 1024;
    if let Ok(meta) = tokio::fs::metadata(&full).await {
        if meta.len() > MAX_SHOT {
            return (StatusCode::PAYLOAD_TOO_LARGE, "image too large").into_response();
        }
    }
    match tokio::fs::read(&full).await {
        Ok(bytes) => (
            [
                (header::CONTENT_TYPE, ct),
                (header::CACHE_CONTROL, "no-store"),
            ],
            bytes,
        )
            .into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

#[derive(serde::Deserialize)]
struct ArtifactQuery {
    task: String,
    path: String,
    /// When present (any value), serve the file INLINE (so opening it in the default browser RENDERS
    /// it — e.g. an HTML page — instead of downloading the source). `Option<String>` not `bool`
    /// because query bools only deserialize from "true"/"false", so `?view=1` would 400 the request.
    #[serde(default)]
    view: Option<String>,
}

/// Content type for an artifact by extension, plus whether it's safe to render inline. Only raster
/// images render inline (so the Artifacts view can preview them); everything else - including SVG and
/// HTML, which can carry scripts - is sent as a download so it can't execute in the dashboard origin.
fn artifact_type(lower: &str) -> (&'static str, bool) {
    if lower.ends_with(".png") {
        ("image/png", true)
    } else if lower.ends_with(".webp") {
        ("image/webp", true)
    } else if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
        ("image/jpeg", true)
    } else if lower.ends_with(".gif") {
        ("image/gif", true)
    } else if lower.ends_with(".svg") {
        ("image/svg+xml", false)
    } else if lower.ends_with(".pdf") {
        ("application/pdf", false)
    } else if lower.ends_with(".csv") {
        ("text/csv", false)
    } else if lower.ends_with(".json") {
        ("application/json", false)
    } else if lower.ends_with(".html") || lower.ends_with(".htm") {
        ("text/html", false)
    } else if lower.ends_with(".txt") || lower.ends_with(".md") || lower.ends_with(".log") {
        ("text/plain", false)
    } else if lower.ends_with(".mp3") {
        ("audio/mpeg", false)
    } else if lower.ends_with(".wav") {
        ("audio/wav", false)
    } else {
        ("application/octet-stream", false)
    }
}

/// Serve a file the agent produced during a task run, from the persistent per-task artifacts dir
/// (`<home>/artifacts/<task-id>/`). Strictly confined to that dir (no traversal), any type, capped in
/// size. This is how the task's Artifacts view previews/downloads generated charts, reports, and data.
async fn artifact_get(State(app): State<App>, Query(q): Query<ArtifactQuery>) -> Response {
    use axum::http::{header, StatusCode};
    // The task id is a single path segment; reject anything that could climb out of the root.
    if q.task.is_empty() || q.task.contains('/') || q.task.contains('\\') || q.task.contains("..") {
        return (StatusCode::BAD_REQUEST, "bad task id").into_response();
    }
    let base = std::path::Path::new(&app.home)
        .join("artifacts")
        .join(&q.task);
    let full = base.join(&q.path);
    // Canonicalize both and require the target to stay under the per-task root (defeats ../ traversal).
    let ok = match (base.canonicalize(), full.canonicalize()) {
        (Ok(b), Ok(f)) => f.starts_with(&b),
        _ => false,
    };
    if !ok {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    }
    const MAX: u64 = 64 * 1024 * 1024;
    if let Ok(meta) = tokio::fs::metadata(&full).await {
        if meta.len() > MAX {
            return (StatusCode::PAYLOAD_TOO_LARGE, "file too large").into_response();
        }
    }
    let (ct, inline) = artifact_type(&q.path.to_lowercase());
    let fname = std::path::Path::new(&q.path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("artifact")
        .replace(['"', '\n', '\r'], "");
    // `view=1` (used only by the "open in browser" path) serves inline so the browser renders it.
    let disp = if inline || q.view.is_some() {
        format!("inline; filename=\"{fname}\"")
    } else {
        format!("attachment; filename=\"{fname}\"")
    };
    match tokio::fs::read(&full).await {
        Ok(bytes) => (
            [
                (header::CONTENT_TYPE, ct.to_string()),
                (header::CONTENT_DISPOSITION, disp),
                (header::CACHE_CONTROL, "no-store".to_string()),
            ],
            bytes,
        )
            .into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

/// Delete one artifact file from a task/chat bucket. Same confinement as `artifact_get` (single-
/// segment bucket id, canonicalized-within-root) so it can never remove anything outside the
/// artifacts tree. Ledgered.
async fn artifact_delete(State(app): State<App>, Query(q): Query<ArtifactQuery>) -> ApiResult {
    if q.task.is_empty() || q.task.contains('/') || q.task.contains('\\') || q.task.contains("..") {
        return Err(ApiError("bad task id".into()));
    }
    let base = std::path::Path::new(&app.home)
        .join("artifacts")
        .join(&q.task);
    let full = base.join(&q.path);
    let ok = match (base.canonicalize(), full.canonicalize()) {
        (Ok(b), Ok(f)) => f.starts_with(&b),
        _ => false,
    };
    if !ok {
        return Err(ApiError("not found".into()));
    }
    std::fs::remove_file(&full).map_err(err)?;
    let _ = app.ledger.append(
        "artifact.delete",
        "user",
        json!({ "task": q.task, "path": q.path }),
    );
    Ok(Json(json!({ "ok": true })))
}

/// Coarse category for an artifact, by extension - drives the gallery's filter chips.
fn artifact_kind(name_lower: &str) -> &'static str {
    let ext = name_lower.rsplit('.').next().unwrap_or("");
    match ext {
        "png" | "jpg" | "jpeg" | "webp" | "gif" | "svg" | "bmp" => "image",
        "csv" | "tsv" | "json" | "xlsx" | "xls" | "parquet" => "data",
        "pdf" | "md" | "txt" | "log" | "html" | "htm" | "docx" | "rtf" => "doc",
        "mp3" | "wav" | "ogg" | "m4a" | "flac" => "audio",
        _ => "other",
    }
}

/// List every artifact across all task runs - the gallery overview. Walks <home>/artifacts/<task>/,
/// tagging each file with its task title + kind + size + mtime (newest first). Bounded so a huge
/// history can't stall the response. Individual files are fetched/downloaded via GET /v1/artifact.
async fn artifacts_list(State(app): State<App>) -> ApiResult {
    let root = std::path::Path::new(&app.home).join("artifacts");
    let mut items: Vec<Value> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&root) {
        for task_dir in rd.flatten() {
            if !task_dir.file_type().map(|f| f.is_dir()).unwrap_or(false) {
                continue;
            }
            let task_id = task_dir.file_name().to_string_lossy().to_string();
            // The bucket id is either a task id OR a chat session id. Tag which, with the right
            // title, so the gallery's "open the source" button routes to the task panel or the chat
            // session correctly (previously a chat artifact tried to open a non-existent task).
            let (title, origin) = if let Some(t) = app.tasks.get(&task_id) {
                (t.title, "task")
            } else if let Some(s) = app.workspace.session(&task_id) {
                let t = if s.title.trim().is_empty() {
                    "Chat".to_string()
                } else {
                    s.title
                };
                (t, "chat")
            } else {
                (String::new(), "task")
            };
            let base = task_dir.path();
            let mut stack = vec![base.clone()];
            while let Some(dir) = stack.pop() {
                let Ok(entries) = std::fs::read_dir(&dir) else {
                    continue;
                };
                for ent in entries.flatten() {
                    if items.len() >= 2000 {
                        break;
                    }
                    let p = ent.path();
                    if ent.file_type().map(|f| f.is_dir()).unwrap_or(false) {
                        stack.push(p);
                        continue;
                    }
                    let rel = p
                        .strip_prefix(&base)
                        .ok()
                        .map(|r| r.to_string_lossy().replace('\\', "/"))
                        .unwrap_or_default();
                    let name = p
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or("")
                        .to_string();
                    let meta = std::fs::metadata(&p).ok();
                    let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
                    let mtime = meta
                        .as_ref()
                        .and_then(|m| m.modified().ok())
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_millis() as i64)
                        .unwrap_or(0);
                    items.push(json!({
                        "task": task_id, "title": title, "origin": origin, "path": rel, "name": name,
                        "kind": artifact_kind(&name.to_lowercase()), "size": size, "mtime": mtime,
                    }));
                }
            }
        }
    }
    items.sort_by(|a, b| {
        b["mtime"]
            .as_i64()
            .unwrap_or(0)
            .cmp(&a["mtime"].as_i64().unwrap_or(0))
    });
    let total = items.len();
    Ok(Json(json!({ "items": items, "total": total })))
}

/// Nodes for the brain-graph visualization: recent memories across every region, trimmed to the
/// fields the graph needs (region for color/cluster, tier for weight, importance/access for size).
async fn memory_graph(State(app): State<App>, Query(q): Query<RecentQuery>) -> ApiResult {
    let per = q.n.unwrap_or(60).min(150);
    let regions = [
        Region::Identity,
        Region::Semantic,
        Region::Episodic,
        Region::Procedural,
    ];
    let mut nodes = Vec::new();
    for region in regions {
        for r in app.memory.recent(region, per).map_err(err)? {
            nodes.push(json!({
                "id": r.id,
                "region": r.region,
                "text": r.text.chars().take(180).collect::<String>(),
                "importance": r.importance,
                "tier": r.tier,
                "access": r.access_count,
                "created_ms": r.created_ms,
            }));
        }
    }
    let stats = app.memory.stats().map_err(err)?;
    Ok(Json(json!({ "nodes": nodes, "stats": stats })))
}

// ---- Consciousness: the always-loaded working memory ------------------------------------------

async fn consciousness_get(State(app): State<App>) -> ApiResult {
    Ok(Json(
        serde_json::to_value(app.consciousness.snapshot()).map_err(err)?,
    ))
}

async fn consciousness_distill(State(app): State<App>) -> ApiResult {
    let st = app
        .consciousness
        .distill(&app.memory, &app.ledger)
        .map_err(err)?;
    Ok(Json(serde_json::to_value(st).map_err(err)?))
}

async fn consciousness_edit(State(app): State<App>, Json(p): Json<Value>) -> ApiResult {
    let id = p.get("id").and_then(|v| v.as_str()).unwrap_or("");
    let text = p.get("text").and_then(|v| v.as_str()).unwrap_or("");
    let st = app.consciousness.edit(id, text, &app.ledger).map_err(err)?;
    Ok(Json(serde_json::to_value(st).map_err(err)?))
}

async fn consciousness_add(State(app): State<App>, Json(p): Json<Value>) -> ApiResult {
    let text = p.get("text").and_then(|v| v.as_str()).unwrap_or("");
    let st = app.consciousness.add(text, &app.ledger).map_err(err)?;
    Ok(Json(serde_json::to_value(st).map_err(err)?))
}

async fn consciousness_remove(State(app): State<App>, Json(p): Json<Value>) -> ApiResult {
    let id = p.get("id").and_then(|v| v.as_str()).unwrap_or("");
    let st = app.consciousness.remove(id, &app.ledger).map_err(err)?;
    Ok(Json(serde_json::to_value(st).map_err(err)?))
}

async fn consciousness_revert(State(app): State<App>) -> ApiResult {
    let st = app.consciousness.revert(&app.ledger).map_err(err)?;
    Ok(Json(serde_json::to_value(st).map_err(err)?))
}

// ---- Durable named agents (the auditable team) ------------------------------------------------

/// Mask each agent's per-agent api_key in the API view (show only whether one is set), exactly
/// like the provider key elsewhere - secrets never go back to the browser.
fn agent_redacted(a: &agents::AgentDef) -> Value {
    json!({
        "id": a.id, "name": a.name, "role": a.role, "model": a.model,
        "provider": a.provider, "base_url": a.base_url,
        "api_key_set": !a.api_key.is_empty(), "effort": a.effort,
        "allowed_tools": a.allowed_tools,
        "color": a.color, "emoji": a.emoji,
        // The standing autonomy grant (allowlist + budget), without the signature, so the editor can
        // display and re-author it. Absent = no autonomous egress (today's gated behaviour).
        "autonomy": a.autonomy_policy.as_ref().map(|s| &s.policy),
        "created_ms": a.created_ms, "updated_ms": a.updated_ms,
    })
}

/// Parse an `allowed_tools` field from an agent create/update body. Returns `None` when the key is
/// absent (no change); `Some(None)` for an explicit null (clear scope = all tools); `Some(Some(..))`
/// for a list (restrict to those names).
fn parse_allowed_tools(p: &Value) -> Option<Option<Vec<String>>> {
    match p.get("allowed_tools") {
        None => None,
        Some(Value::Null) => Some(None),
        Some(Value::Array(a)) => Some(Some(
            a.iter()
                .filter_map(|v| v.as_str().map(|s| s.trim().to_string()))
                .filter(|s| !s.is_empty())
                .collect(),
        )),
        _ => None,
    }
}

async fn agents_list(State(app): State<App>) -> ApiResult {
    let list: Vec<Value> = app.agents.list().iter().map(agent_redacted).collect();
    Ok(Json(json!(list)))
}

async fn agents_create(State(app): State<App>, Json(p): Json<Value>) -> ApiResult {
    let name = p.get("name").and_then(|v| v.as_str()).unwrap_or("").trim();
    if name.is_empty() {
        return Err(err("an agent needs a name"));
    }
    let role = p.get("role").and_then(|v| v.as_str()).unwrap_or("");
    let model = p.get("model").and_then(|v| v.as_str()).unwrap_or("");
    let provider = p.get("provider").and_then(|v| v.as_str()).unwrap_or("");
    let base_url = p.get("base_url").and_then(|v| v.as_str()).unwrap_or("");
    let api_key = p.get("api_key").and_then(|v| v.as_str()).unwrap_or("");
    let effort = p.get("effort").and_then(|v| v.as_str()).unwrap_or("");
    let mut def = app
        .agents
        .create(name, role, model, provider, base_url, api_key, effort);
    if let Some(tools) = parse_allowed_tools(&p) {
        if let Some(updated) = app.agents.set_allowed_tools(&def.id, tools) {
            def = updated;
        }
    }
    if p.get("color").is_some() || p.get("emoji").is_some() {
        if let Some(updated) = app.agents.set_appearance(
            &def.id,
            p.get("color").and_then(|v| v.as_str()),
            p.get("emoji").and_then(|v| v.as_str()),
        ) {
            def = updated;
        }
    }
    app.ledger
        .append(
            "agent.create",
            "user",
            json!({ "id": def.id, "name": def.name, "model": def.model, "provider": def.provider }),
        )
        .map_err(err)?;
    Ok(Json(agent_redacted(&def)))
}

async fn agents_update(
    State(app): State<App>,
    Path(id): Path<String>,
    Json(p): Json<Value>,
) -> ApiResult {
    let name = p.get("name").and_then(|v| v.as_str());
    let role = p.get("role").and_then(|v| v.as_str());
    let model = p.get("model").and_then(|v| v.as_str());
    let provider = p.get("provider").and_then(|v| v.as_str());
    let base_url = p.get("base_url").and_then(|v| v.as_str());
    let api_key = p.get("api_key").and_then(|v| v.as_str());
    let effort = p.get("effort").and_then(|v| v.as_str());
    let mut def = app
        .agents
        .update(&id, name, role, model, provider, base_url, api_key, effort)
        .ok_or_else(|| err("no such agent"))?;
    if let Some(tools) = parse_allowed_tools(&p) {
        if let Some(updated) = app.agents.set_allowed_tools(&id, tools) {
            def = updated;
        }
    }
    if p.get("color").is_some() || p.get("emoji").is_some() {
        if let Some(updated) = app.agents.set_appearance(
            &id,
            p.get("color").and_then(|v| v.as_str()),
            p.get("emoji").and_then(|v| v.as_str()),
        ) {
            def = updated;
        }
    }
    app.ledger
        .append(
            "agent.update",
            "user",
            json!({ "id": def.id, "name": def.name, "model": def.model, "provider": def.provider }),
        )
        .map_err(err)?;
    Ok(Json(agent_redacted(&def)))
}

async fn agents_delete(State(app): State<App>, Path(id): Path<String>) -> ApiResult {
    if !app.agents.delete(&id) {
        return Err(err("no such agent"));
    }
    app.ledger
        .append("agent.delete", "user", json!({ "id": id }))
        .map_err(err)?;
    Ok(Json(json!({ "ok": true })))
}

/// Author (or revoke) a durable agent's signed standing AUTONOMY policy — the "approve once, ahead of
/// time" surface. The human is present here, so this is the moment authority is captured and SIGNED;
/// thereafter SCHEDULED runs of this agent egress unattended within the allowlist + budget, with no
/// live approval. Body: `{ enabled?, allowed_egress: [str], allowed_actions: [str], max_actions,
/// max_spend_cents?, expires_days?, hardline_floor?: [str] }`. `enabled:false` (or an empty allowlist
/// with no budget) revokes it.
async fn agent_set_policy(
    State(app): State<App>,
    Path(id): Path<String>,
    Json(p): Json<Value>,
) -> ApiResult {
    if app.agents.get(&id).is_none() {
        return Err(err("no such agent"));
    }
    let strs = |key: &str| -> Vec<String> {
        p.get(key)
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str())
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default()
    };
    let enabled = p.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true);
    let allowed_egress = strs("allowed_egress");
    let max_actions = p.get("max_actions").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    // Revoke when explicitly disabled, or when nothing meaningful was provided.
    if !enabled || (allowed_egress.is_empty() && max_actions == 0) {
        let def = app
            .agents
            .set_autonomy_policy(&id, None)
            .ok_or_else(|| err("no such agent"))?;
        let _ = app
            .ledger
            .append("autonomy.policy.revoked", "user", json!({ "id": id }));
        return Ok(Json(agent_redacted(&def)));
    }
    let actions: Vec<engram_core::ActionClass> = strs("allowed_actions")
        .iter()
        .filter_map(|s| match s.to_ascii_lowercase().as_str() {
            "send" => Some(engram_core::ActionClass::Send),
            "post" => Some(engram_core::ActionClass::Post),
            "pay" => Some(engram_core::ActionClass::Pay),
            "other" => Some(engram_core::ActionClass::Other),
            _ => None,
        })
        .collect();
    let expires_days = p.get("expires_days").and_then(|v| v.as_u64()).unwrap_or(0);
    let expires_at_ms = if expires_days > 0 {
        engram_core::now_ms().saturating_add(expires_days.saturating_mul(86_400_000))
    } else {
        0
    };
    let policy = engram_core::AutonomyPolicy {
        scope: format!("agent:{id}"),
        allowed_egress: allowed_egress
            .into_iter()
            .map(engram_core::EgressRule::new)
            .collect(),
        allowed_actions: actions,
        budget: engram_core::EgressBudget {
            max_actions,
            max_spend_cents: p
                .get("max_spend_cents")
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
            expires_at_ms,
        },
        hardline_floor: strs("hardline_floor")
            .into_iter()
            .map(engram_core::EgressRule::new)
            .collect(),
    };
    // Sign it NOW, while the human is present — the captured, frozen authority.
    let signed = app.registry.sign_autonomy(&policy);
    let def = app
        .agents
        .set_autonomy_policy(&id, Some(signed))
        .ok_or_else(|| err("no such agent"))?;
    let _ = app.ledger.append(
        "autonomy.policy.set",
        "user",
        json!({ "id": id, "scope": policy.scope, "rules": policy.allowed_egress.len(),
                "max_actions": max_actions, "expires_at_ms": expires_at_ms }),
    );
    Ok(Json(agent_redacted(&def)))
}

// ---------------------------------------------------------------------------
// Async approve-queue for staged egress — the ledger IS the durable, signed queue
// ---------------------------------------------------------------------------

/// Derive the pending-egress queue from ledger entries: actions an unattended run STAGED
/// (`agent.egress_staged`) minus those a human later resolved (`egress.allowlisted`/`egress.denied`),
/// deduped by (scope, dest), most-recent first. Pure, so it is unit-tested directly.
fn pending_from_entries(entries: &[engram_core::Entry]) -> Vec<Value> {
    use std::collections::HashSet;
    let field = |e: &engram_core::Entry, k: &str| -> String {
        serde_json::from_str::<Value>(e.payload.get())
            .ok()
            .and_then(|p| p.get(k).and_then(|v| v.as_str()).map(String::from))
            .unwrap_or_default()
    };
    let mut resolved: HashSet<(String, String)> = HashSet::new();
    for e in entries {
        if e.kind == "egress.allowlisted" || e.kind == "egress.denied" {
            resolved.insert((field(e, "scope"), field(e, "dest")));
        }
    }
    let mut seen: HashSet<(String, String)> = HashSet::new();
    let mut pending = Vec::new();
    for e in entries.iter().rev() {
        if e.kind != "agent.egress_staged" {
            continue;
        }
        let (scope, dest) = (field(e, "scope"), field(e, "dest"));
        if dest.is_empty() {
            continue;
        }
        let key = (scope.clone(), dest.clone());
        if resolved.contains(&key) || !seen.insert(key) {
            continue;
        }
        pending.push(json!({
            "scope": scope, "dest": dest,
            "tool": field(e, "tool"), "reason": field(e, "reason"),
            "ts_ms": e.ts_ms, "seq": e.seq,
        }));
    }
    pending
}

/// Add `dest` to a policy's allowlist (idempotent), returning the extended policy to re-sign.
fn extend_allowlist(
    mut policy: engram_core::AutonomyPolicy,
    dest: &str,
) -> engram_core::AutonomyPolicy {
    if !policy
        .allowed_egress
        .iter()
        .any(|r| r.pattern.eq_ignore_ascii_case(dest))
    {
        policy
            .allowed_egress
            .push(engram_core::EgressRule::new(dest));
    }
    policy
}

async fn egress_pending(State(app): State<App>) -> ApiResult {
    let entries = app.ledger.tail(2000).map_err(err)?;
    Ok(Json(json!({ "pending": pending_from_entries(&entries) })))
}

#[derive(serde::Deserialize)]
struct EgressResolve {
    scope: String,
    dest: String,
}

/// Approve a staged egress destination: add it to the scoped agent's SIGNED allowlist (re-signing the
/// policy) so future unattended runs send there without asking — the approval moment captured once,
/// out of band. Ledger `egress.allowlisted`.
/// After a staged egress is approved (its destination allowlisted), re-run the task that parked it so
/// the approved action actually happens — the "approve → it proceeds" loop. Best-effort: finds the
/// most recent task linked to this destination via `task.staged_egress` and re-runs it unattended in
/// the background. `try_begin` inside `run_task_core` prevents a double-run, and the destination is now
/// allowlisted so the re-run's egress passes instead of re-staging (no approval loop).
fn requeue_task_for_dest(app: &App, dest: &str) {
    let tail = app.ledger.tail(1000).unwrap_or_default();
    let task_id = tail.iter().rev().find_map(|e| {
        if e.kind != "task.staged_egress" {
            return None;
        }
        let p = serde_json::from_str::<Value>(e.payload.get()).ok()?;
        let d = p.get("dest").and_then(|v| v.as_str())?;
        if d.eq_ignore_ascii_case(dest) {
            p.get("task").and_then(|v| v.as_str()).map(String::from)
        } else {
            None
        }
    });
    if let Some(tid) = task_id {
        let app2 = app.clone();
        tokio::spawn(async move {
            if let Err(e) = run_task_core(&app2, &tid, None, false, false).await {
                tracing::warn!(task = %tid, error = %e, "re-run after egress approval failed");
            }
        });
    }
}

async fn egress_approve(State(app): State<App>, Json(r): Json<EgressResolve>) -> ApiResult {
    let dest = r.dest.trim().to_string();
    if dest.is_empty() {
        return Err(err("dest required"));
    }
    let id = r
        .scope
        .strip_prefix("agent:")
        .unwrap_or(&r.scope)
        .to_string();
    // A staged action from a run with NO per-agent policy carries an empty scope. It has no agent
    // allowlist to extend, so the approval persists to the daemon-global allowlist instead (the egress
    // gate consults it for policy-less runs) — previously this path hard-errored, so the Approve button
    // did nothing for the common default-agent case.
    if id.trim().is_empty() {
        // Store the destination as given; the egress gate normalises both sides (scheme/port/userinfo)
        // when it matches, so no pre-normalisation is needed here.
        {
            let mut cfg = app.config.write().expect("config lock");
            if !cfg
                .security
                .egress_allowlist
                .iter()
                .any(|h| h.eq_ignore_ascii_case(&dest))
            {
                cfg.security.egress_allowlist.push(dest.clone());
                if let Err(e) = cfg.save(&app.home) {
                    return Err(err(format!("could not persist allowlist: {e}")));
                }
            }
        }
        let _ = app.ledger.append(
            "egress.allowlisted",
            "user",
            json!({ "scope": r.scope, "dest": dest, "via": "daemon_allowlist" }),
        );
        // Approve → it proceeds: re-run the task that parked this destination so the action completes.
        requeue_task_for_dest(&app, &dest);
        return Ok(Json(
            json!({ "ok": true, "allowlisted": dest, "scope": "daemon" }),
        ));
    }
    let def = app
        .agents
        .get(&id)
        .ok_or_else(|| err("no such agent for this staged action"))?;
    let policy = def
        .autonomy_policy
        .as_ref()
        .and_then(|s| engram_core::verify_policy(s, app.registry.verifying()).ok())
        // Only extend a policy that belongs to this exact agent (re-bind scope to the holder).
        .filter(|p| p.scope == format!("agent:{id}"))
        .ok_or_else(|| err("agent has no valid autonomy policy to extend"))?;
    let signed = app.registry.sign_autonomy(&extend_allowlist(policy, &dest));
    app.agents
        .set_autonomy_policy(&id, Some(signed))
        .ok_or_else(|| err("no such agent"))?;
    let _ = app.ledger.append(
        "egress.allowlisted",
        "user",
        json!({ "scope": r.scope, "dest": dest }),
    );
    // Approve → it proceeds: re-run the task that parked this destination so the action completes.
    requeue_task_for_dest(&app, &dest);
    Ok(Json(json!({ "ok": true, "allowlisted": dest })))
}

async fn egress_deny(State(app): State<App>, Json(r): Json<EgressResolve>) -> ApiResult {
    let _ = app.ledger.append(
        "egress.denied",
        "user",
        json!({ "scope": r.scope, "dest": r.dest }),
    );
    Ok(Json(json!({ "ok": true, "denied": r.dest })))
}

#[derive(serde::Deserialize)]
struct CheckpointCreateReq {
    #[serde(default)]
    label: String,
    /// Snapshot this chat session's project workdir; omit for the shared workspace.
    #[serde(default)]
    session: Option<String>,
}

/// Snapshot the working directory as a restorable checkpoint (Claude-Code-style rewind). The
/// (blocking) file walk runs off the async runtime.
async fn checkpoints_create(
    State(app): State<App>,
    Json(r): Json<CheckpointCreateReq>,
) -> ApiResult {
    let workdir = r
        .session
        .as_ref()
        .and_then(|sid| app.workspace.workdir_for_session(sid))
        .unwrap_or_else(|| app.workdir.clone());
    let label = if r.label.trim().is_empty() {
        "manual checkpoint".to_string()
    } else {
        r.label.trim().to_string()
    };
    let home = app.home.clone();
    let session = r.session.clone();
    let cp = tokio::task::spawn_blocking(move || {
        checkpoints::snapshot(
            &home,
            &workdir,
            &label,
            session,
            None,
            engram_core::now_ms() as u64,
        )
    })
    .await
    .map_err(err)?
    .map_err(err)?;
    let _ = app.ledger.append(
        "checkpoint.created",
        "user",
        json!({ "id": cp.id, "files": cp.file_count }),
    );
    Ok(Json(serde_json::to_value(cp).map_err(err)?))
}

async fn checkpoints_list(State(app): State<App>) -> ApiResult {
    let home = app.home.clone();
    let list = tokio::task::spawn_blocking(move || checkpoints::list(&home))
        .await
        .map_err(err)?;
    Ok(Json(serde_json::to_value(list).map_err(err)?))
}

async fn checkpoints_restore(State(app): State<App>, Path(id): Path<String>) -> ApiResult {
    let home = app.home.clone();
    let res = tokio::task::spawn_blocking(move || checkpoints::restore(&home, &id))
        .await
        .map_err(err)?
        .map_err(err)?;
    let _ = app.ledger.append(
        "checkpoint.restored",
        "user",
        json!({ "restored": res.restored }),
    );
    Ok(Json(serde_json::to_value(res).map_err(err)?))
}

async fn checkpoints_delete(State(app): State<App>, Path(id): Path<String>) -> ApiResult {
    let home = app.home.clone();
    let ok = tokio::task::spawn_blocking(move || checkpoints::delete(&home, &id))
        .await
        .map_err(err)?;
    Ok(Json(json!({ "ok": ok })))
}

/// Reconstruct the autonomy story from the signed ledger: per agent scope, the policy granted and a
/// tally of every autonomous send, staged action, floor-refusal, and async approve/deny — so a
/// multi-day unattended run is offline-verifiable against the chain. Pure (unit-tested); reused by the
/// HTTP report endpoint and the `verify-autonomy` CLI.
fn autonomy_report(entries: &[engram_core::Entry]) -> Value {
    use std::collections::BTreeMap;
    let pget = |e: &engram_core::Entry| {
        serde_json::from_str::<Value>(e.payload.get()).unwrap_or(json!({}))
    };
    #[derive(Default)]
    struct Agg {
        autonomous: u64,
        staged: u64,
        refused: u64,
        allowlisted: u64,
        denied: u64,
        policy_max: u64,
        policy_rules: u64,
        has_policy: bool,
        revoked: bool,
    }
    let mut scopes: BTreeMap<String, Agg> = BTreeMap::new();
    let mut one_time = 0u64;
    for e in entries {
        let p = pget(e);
        let scope = p["scope"].as_str().unwrap_or("").to_string();
        match e.kind.as_str() {
            "autonomy.policy.set" => {
                let a = scopes.entry(scope).or_default();
                a.has_policy = true;
                a.revoked = false;
                a.policy_max = p["max_actions"].as_u64().unwrap_or(0);
                a.policy_rules = p["rules"].as_u64().unwrap_or(0);
            }
            // The revoke entry carries {id}; normalise to the "agent:<id>" scope key.
            "autonomy.policy.revoked" => {
                let key = format!("agent:{}", p["id"].as_str().unwrap_or(""));
                scopes.entry(key).or_default().revoked = true;
            }
            "agent.egress_autonomous" => scopes.entry(scope).or_default().autonomous += 1,
            "agent.egress_staged" => scopes.entry(scope).or_default().staged += 1,
            "agent.egress_refused" => scopes.entry(scope).or_default().refused += 1,
            "egress.allowlisted" => scopes.entry(scope).or_default().allowlisted += 1,
            "egress.denied" => scopes.entry(scope).or_default().denied += 1,
            "agent.egress_approved" => one_time += 1,
            _ => {}
        }
    }
    let scope_list: Vec<Value> = scopes
        .iter()
        .map(|(scope, a)| {
            let policy = if a.revoked {
                json!("revoked")
            } else if a.has_policy {
                json!({ "max_actions": a.policy_max, "rules": a.policy_rules })
            } else {
                Value::Null
            };
            json!({
                "scope": if scope.is_empty() { "(unscoped)" } else { scope.as_str() },
                "policy": policy,
                "autonomous_sends": a.autonomous, "staged": a.staged, "refused": a.refused,
                "allowlisted": a.allowlisted, "denied": a.denied,
            })
        })
        .collect();
    json!({
        "scopes": scope_list,
        "one_time_approvals": one_time,
        "totals": {
            "autonomous_sends": scopes.values().map(|a| a.autonomous).sum::<u64>(),
            "staged": scopes.values().map(|a| a.staged).sum::<u64>(),
            "refused": scopes.values().map(|a| a.refused).sum::<u64>(),
            "allowlisted": scopes.values().map(|a| a.allowlisted).sum::<u64>(),
            "denied": scopes.values().map(|a| a.denied).sum::<u64>(),
        }
    })
}

async fn autonomy_report_handler(State(app): State<App>) -> ApiResult {
    let entries = app.ledger.read_all().map_err(err)?;
    Ok(Json(autonomy_report(&entries)))
}

/// An agent's accumulated track record: every signed action it has taken (ledger actor == its name),
/// counted by kind, with its most recent actions and the cards assigned to it. The auditable
/// experience of a teammate - it grows as the agent works, and every entry is verifiable.
async fn agent_activity(State(app): State<App>, Path(id): Path<String>) -> ApiResult {
    let name = app
        .agents
        .get(&id)
        .ok_or_else(|| err("no such agent"))?
        .name;
    let entries = app.ledger.read_all().map_err(err)?;
    let mut by_kind: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    let mut total = 0usize;
    let mut last_ms = 0u64;
    for e in entries.iter().filter(|e| e.actor == name) {
        *by_kind.entry(e.kind.clone()).or_default() += 1;
        total += 1;
        last_ms = e.ts_ms;
    }
    let recent: Vec<Value> = entries
        .iter()
        .rev()
        .filter(|e| e.actor == name)
        .take(25)
        .map(|e| json!({ "seq": e.seq, "kind": e.kind, "ts_ms": e.ts_ms, "hash": e.hash }))
        .collect();
    let tasks_assigned = app
        .tasks
        .list()
        .iter()
        .filter(|t| t.agent.as_deref() == Some(id.as_str()))
        .count();
    Ok(Json(json!({
        "name": name, "total": total, "by_kind": by_kind, "recent": recent,
        "last_ms": last_ms, "tasks_assigned": tasks_assigned,
    })))
}

#[derive(Deserialize)]
struct RememberReq {
    region: Option<String>,
    text: String,
    importance: Option<f32>,
    /// The chat session this add belongs to. When set, the memory is routed to that session's scope
    /// (its project ring for durable facts) instead of user-global — so a fact taught while working
    /// inside a project stays in that project rather than bleeding into every other project's recall.
    /// Omitted → user-global, preserving the previous behavior for existing API/CLI clients.
    #[serde(default)]
    session: Option<String>,
}

async fn remember(State(app): State<App>, Json(r): Json<RememberReq>) -> ApiResult {
    let region = parse_region(r.region.as_deref());
    let mut req = WriteReq::new(region, r.text.clone()).actor("user");
    if let Some(i) = r.importance {
        req = req.importance(i);
    }
    // Route the write to the right ring. With a session, classify against that session's scope so a
    // project-context add lands in the project ring; without one, WriteReq keeps its user-global
    // default (back-compat). This closes the "manual adds always bleed into every project" gap.
    if let Some(sid) = r.session.as_deref() {
        let ctx = app.workspace.scope_for_session(sid);
        let write_scope = crate::scope::classify(region, &ctx, &r.text);
        req = req.scope(write_scope);
    }
    let rec = app.memory.remember(req).map_err(err)?;
    // A new identity/semantic fact should appear in the always-loaded consciousness right away, not
    // only after a restart. Idempotent distill: re-ledgers only if the distilled set changed.
    if matches!(region, Region::Identity | Region::Semantic) {
        let _ = app.consciousness.distill(&app.memory, &app.ledger);
    }
    Ok(Json(serde_json::to_value(rec).map_err(err)?))
}

#[derive(Deserialize)]
struct RecallQuery {
    q: String,
    task: Option<String>,
    k: Option<usize>,
    /// Bi-temporal time-travel: "what did I believe on this date" (epoch milliseconds). Omitted =
    /// ordinary current-state recall, unchanged. Routes through the trusted+user-scoped family
    /// (`recall_as_of`) rather than whole-brain `recall()`, since "what did I believe" is a
    /// personal-trust question, not an audit/Atlas one - matching `recall_trusted_scoped`'s family.
    #[serde(default)]
    as_of: Option<i64>,
}

async fn recall(State(app): State<App>, Query(q): Query<RecallQuery>) -> ApiResult {
    let regions = match q.task.as_deref() {
        Some(t) => Region::for_task(t),
        None => vec![],
    };
    let k = q.k.unwrap_or(5);
    let hits = match q.as_of {
        Some(as_of_ms) => app
            .memory
            .recall_as_of(
                &q.q,
                &regions,
                k,
                &engram_core::ScopeCtx::user_only(),
                as_of_ms,
            )
            .map_err(err)?,
        None => app.memory.recall(&q.q, &regions, k).map_err(err)?,
    };
    Ok(Json(serde_json::to_value(hits).map_err(err)?))
}

#[derive(Deserialize)]
struct ForgetReq {
    id: i64,
}

async fn forget(State(app): State<App>, Json(r): Json<ForgetReq>) -> ApiResult {
    let ok = app.memory.forget(r.id, "user", "via api").map_err(err)?;
    Ok(Json(json!({ "forgotten": ok })))
}

/// List not-yet-resolved proposed contradictions - the visible half of turning silent
/// auto-supersede into a confirmable event (crate::contradiction never applies one on its own).
async fn supersessions_list(State(app): State<App>) -> ApiResult {
    let pending = app.memory.pending_supersessions().map_err(err)?;
    Ok(Json(serde_json::to_value(pending).map_err(err)?))
}

#[derive(Deserialize)]
struct ResolveSupersessionReq {
    accept: bool,
}

/// Accept (write the candidate + supersede the old fact) or reject (no-op) a proposed
/// contradiction. This is the ONLY place a proposal can ever take effect.
async fn supersessions_resolve(
    State(app): State<App>,
    Path(id): Path<i64>,
    Json(r): Json<ResolveSupersessionReq>,
) -> ApiResult {
    let ok = app
        .memory
        .resolve_supersession(id, r.accept, "user")
        .map_err(err)?;
    Ok(Json(json!({ "ok": ok, "id": id, "accepted": r.accept })))
}

#[derive(Deserialize, Default)]
struct ReflectionsQuery {
    /// Restrict to one ring: "project"/"session"/"user". Omitted = the user-global ring only
    /// (matches every other model-facing scoped query's default - never silently whole-brain).
    #[serde(default)]
    scope_kind: Option<String>,
    #[serde(default)]
    scope_id: Option<String>,
}

/// List grounded-reflection facts (Phase D: docs/MEMORY-UPGRADE-PLAN.md §6) - synthesized memories
/// the reflection pass wrote, permanently distinguishable from directly-witnessed facts by
/// `source = "reflection"` (`metadata.reflection = true`, `tree_level = 1`, and `metadata.source_ids`
/// citing exactly which facts it drew on). Every surface (desktop/TUI/CLI) reads this route rather
/// than treating a reflection as an ordinary recall hit, per the locked "never visually
/// indistinguishable" rule in §5.
async fn reflections_list(State(app): State<App>, Query(q): Query<ReflectionsQuery>) -> ApiResult {
    let scope = match (q.scope_kind.as_deref(), &q.scope_id) {
        (Some("project"), Some(id)) => engram_core::ScopeCtx::project(id.clone()),
        _ => engram_core::ScopeCtx::user_only(),
    };
    let recs = app
        .memory
        .by_source_scoped("reflection", &scope)
        .map_err(err)?;
    Ok(Json(serde_json::to_value(recs).map_err(err)?))
}

#[derive(Deserialize)]
struct SwarmReq {
    steps: Vec<String>,
    input: String,
}

async fn run_swarm(State(app): State<App>, Json(r): Json<SwarmReq>) -> ApiResult {
    let outcome = engram_skills::run_pipeline(
        &app.host,
        &app.registry,
        &r.steps,
        r.input.as_bytes(),
        Some(app.memory.clone()),
        Some(app.gateway.clone()),
    )
    .await
    .map_err(err)?;
    Ok(Json(json!({
        "output": String::from_utf8_lossy(&outcome.output),
        "steps": outcome.steps,
    })))
}

#[derive(Deserialize)]
struct AgentReq {
    task: String,
    #[serde(default)]
    max_steps: Option<usize>,
    /// Preview mode: plan and report intended actions, but execute no side-effecting tool.
    #[serde(default)]
    dry_run: bool,
}

/// Run the agent on a task with the full configured toolset (built-ins + MCP),
/// persona, and policy. Shared by the HTTP endpoint and the messaging channels.
pub(crate) async fn run_agent_task(
    app: &App,
    task: &str,
    max_steps: usize,
) -> Result<engram_agent::AgentRun, String> {
    run_agent_task_cb(
        app,
        task,
        max_steps,
        engram_core::Taint::Trusted,
        false,
        None,
        None,
        None,
        None,
        false, // approved
        true,  // attended (this wrapper backs interactive conversation)
        app.halt.clone(),
        engram_core::ScopeCtx::user_only(), // no session/project in this path → user-global
    )
    .await
}

/// Run the agent with an explicit initial taint. Untrusted-origin prompts (inbound
/// webhooks, Telegram) start `Untrusted`, so the no-egress guard applies from step one.
/// `dry_run` previews intended actions without executing side-effecting tools.
/// Removes a git worktree when dropped, so a task's isolated tree is cleaned up on every exit path.
struct WorktreeGuard {
    repo: std::path::PathBuf,
    path: std::path::PathBuf,
    task_id: String,
}
/// Before a worktree is torn down, commit whatever the agent changed onto a durable
/// `engram/task-<id>` branch so the work is NEVER lost to `worktree remove --force`. Returns the
/// branch name when a commit was made (there were changes), or None when the tree was clean.
/// Best-effort: a repo with no commits yet, or git failures, just leave nothing behind.
fn preserve_worktree_changes(path: &std::path::Path, task_id: &str) -> Option<String> {
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .arg("-C")
            .arg(path)
            .args(args)
            .output()
    };
    let _ = git(&["add", "-A"]);
    // Nothing staged → clean worktree, nothing to preserve.
    match git(&["diff", "--cached", "--quiet"]) {
        Ok(o) if o.status.success() => return None, // exit 0 = no staged changes
        Err(_) => return None,
        _ => {}
    }
    let branch = format!("engram/task-{task_id}");
    // Commit with an explicit identity so it works even in a repo with no user.name/email set.
    let msg = format!("engram: changes from task {task_id}");
    let committed = std::process::Command::new("git")
        .arg("-C")
        .arg(path)
        .args([
            "-c",
            "user.name=Engram",
            "-c",
            "user.email=engram@localhost",
            "commit",
            "-m",
            &msg,
        ])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !committed {
        return None;
    }
    // Point (or move) the durable branch at the new commit so it survives worktree removal.
    let branched = git(&["branch", "-f", &branch, "HEAD"])
        .map(|o| o.status.success())
        .unwrap_or(false);
    if branched {
        tracing::info!(task = task_id, branch = %branch, "worktree changes committed to branch (merge with `git merge {branch}`)");
        Some(branch)
    } else {
        None
    }
}
impl Drop for WorktreeGuard {
    fn drop(&mut self) {
        let repo = std::mem::take(&mut self.repo);
        let path = std::mem::take(&mut self.path);
        let task_id = std::mem::take(&mut self.task_id);
        let remove = move || {
            // Preserve edits to a branch BEFORE the destructive remove, on every exit path.
            preserve_worktree_changes(&path, &task_id);
            match std::process::Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(["worktree", "remove", "--force"])
                .arg(&path)
                .output()
            {
                Ok(o) if !o.status.success() => tracing::warn!(
                    path = %path.display(),
                    "git worktree remove failed: {}",
                    String::from_utf8_lossy(&o.stderr).trim()
                ),
                Err(e) => {
                    tracing::warn!(path = %path.display(), "git worktree remove could not spawn: {e}")
                }
                _ => {}
            }
        };
        // Drop runs on a tokio worker thread; `git worktree remove` is a BLOCKING syscall that would
        // stall the executor (and every other task on it) if a repo lock or slow disk made git hang.
        // Move it onto the blocking pool. Outside a runtime (e.g. unit tests) run it inline.
        match tokio::runtime::Handle::try_current() {
            Ok(h) => {
                h.spawn_blocking(remove);
            }
            Err(_) => remove(),
        }
    }
}

/// With worktree isolation enabled (Settings > Tools, or `ENGRAM_WORKTREES=1`) and a git workspace,
/// create a detached worktree at `<home>/worktrees/<task-id>` so this task runs isolated from sibling
/// tasks (parallel agents on one project). Returns the workdir override (None = use the shared
/// workspace) and a guard that removes it afterward.
fn prepare_worktree(
    app: &App,
    task_id: &str,
) -> (Option<std::path::PathBuf>, Option<WorktreeGuard>) {
    // Live setting wins; the env var is the headless/server fallback (acts as a floor).
    let enabled = app.cfg().security.enable_worktree_isolation
        || std::env::var("ENGRAM_WORKTREES").as_deref() == Ok("1");
    if !enabled {
        return (None, None);
    }
    if !app.workdir.join(".git").exists() {
        tracing::warn!(
            "worktree isolation is on but the workspace is not a git repo - running shared"
        );
        return (None, None);
    }
    let base = std::path::Path::new(&app.home).join("worktrees");
    let dest = base.join(task_id);
    // Defense in depth: task_id is already slug-sanitized to [a-z0-9-] (no separators, so no
    // traversal), but confirm the joined path stays under the worktrees base before we hand it to
    // git or mkdir - a future id scheme must not be able to escape.
    if !dest.starts_with(&base) {
        tracing::warn!(
            task = task_id,
            "refusing worktree path that escapes the worktrees dir"
        );
        return (None, None);
    }
    let _ = std::fs::create_dir_all(&base);
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(&app.workdir)
        .args(["worktree", "add", "--detach"])
        .arg(&dest)
        .output();
    let ok = match &out {
        Ok(o) if o.status.success() => true,
        Ok(o) => {
            tracing::warn!(
                task = task_id,
                "git worktree add failed, running shared: {}",
                String::from_utf8_lossy(&o.stderr).trim()
            );
            false
        }
        Err(e) => {
            tracing::warn!(
                task = task_id,
                "git worktree add could not spawn, running shared: {e}"
            );
            false
        }
    };
    if ok {
        tracing::info!(task = task_id, path = %dest.display(), "task running in an isolated git worktree");
        (
            Some(dest.clone()),
            Some(WorktreeGuard {
                repo: app.workdir.clone(),
                path: dest,
                task_id: task_id.to_string(),
            }),
        )
    } else {
        (None, None)
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_agent_task_cb(
    app: &App,
    task: &str,
    max_steps: usize,
    taint: engram_core::Taint,
    dry_run: bool,
    on_step: Option<engram_agent::StepCallback>,
    on_narration: Option<engram_agent::NarrationCallback>,
    agent_def: Option<&agents::AgentDef>,
    workdir_override: Option<std::path::PathBuf>,
    // One-time user approval for this run only (the UI's "Approve & continue"): de-escalates the
    // egress trifecta for this run. NEVER persisted — the next run starts gated again.
    approved: bool,
    // Whether a human is watching this run (interactive) vs scheduled/inbound (unattended). An
    // unattended run with no autonomy policy STAGES novel egress for async review instead of refusing.
    attended: bool,
    // The halt flag THIS run checks at each step boundary. Pass a per-session flag so one chat can be
    // stopped without killing others; pass `app.halt` for the global emergency-stop behavior.
    halt: Arc<std::sync::atomic::AtomicBool>,
    // Which rings this run's memory recall and capture are bound to. The chat path passes the
    // session's scope (user ∪ project ∪ session) so a project's work stays isolated; scheduled /
    // inbound runs with no project pass `ScopeCtx::user_only()` and see only user-global memory.
    scope: engram_core::ScopeCtx,
) -> Result<engram_agent::AgentRun, String> {
    // Mark this run in-flight for the whole call, so the idle clock can't fire and drop the runtime
    // mid-step for a run with no open HTTP connection (scheduler / Telegram / detached stream). The
    // guard touches activity now and the count keeps the keepalive task touching until it drops.
    let _run_guard = RunGuard::new(&app.activity, app.in_flight.clone());
    // A named agent may carry a signed standing AutonomyPolicy that lets it egress unattended within
    // an allowlist + budget. Verify the signature with the skill key before honoring it; an
    // unsigned/forged/tampered policy fails closed (treated as no policy = default-deny).
    let autonomy = agent_def.and_then(|a| {
        a.autonomy_policy.as_ref().and_then(|signed| {
            match engram_core::verify_policy(signed, app.registry.verifying()) {
                // Re-bind the signed policy to THIS agent: a valid signature for a different scope must
                // not be honored here (defense-in-depth beyond the signature).
                Ok(p) if p.scope == format!("agent:{}", a.id) => Some(p),
                Ok(p) => {
                    tracing::warn!(agent = %a.id, policy_scope = %p.scope, "autonomy policy scope mismatch, ignoring");
                    None
                }
                Err(e) => {
                    tracing::warn!(agent = %a.id, "autonomy policy failed verification, ignoring: {e}");
                    None
                }
            }
        })
    });
    let policy = engram_agent::Policy {
        allow_shell: app.allow_shell.load(std::sync::atomic::Ordering::Relaxed),
        dry_run,
        // Shell isolation comes from the live settings (configurable in the desktop's Tools
        // panel); fall back to the ENGRAM_SHELL_BACKEND env vars for headless/server installs.
        shell_backend: {
            let resolved = {
                let c = app.cfg();
                config::resolve_shell_backend(&c.security.shell_backend, &c.security.shell_target)
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
        },
        // Per-run media + egress settings, read from the live config (empty = fall through to the
        // ENGRAM_* env var / built-in default at the tool's use-site).
        vision_model: {
            let m = app.cfg().media.vision_model.trim().to_string();
            (!m.is_empty()).then_some(m)
        },
        webhook_url: {
            let u = app.cfg().channels.webhook_url.trim().to_string();
            (!u.is_empty()).then_some(u)
        },
        // Authoring/improving skills is on by default (skills pop up from use); the user can turn it
        // off in the Tools panel (stored inverted so the zero value stays "on").
        allow_skill_author: !app.cfg().security.disable_skill_author,
        // Egress de-escalation, this run only — set when the user clicked "Approve & continue".
        approved,
        // Run surface + standing autonomy grant: an unattended run consults the signed policy
        // instead of a live human; with no policy it stages novel egress rather than refusing.
        attended,
        autonomy,
        // Daemon-global allowlist for policy-less runs (the user's persisted approvals live here).
        daemon_allowlist: app.cfg().security.egress_allowlist.clone(),
        ..Default::default()
    };
    // A named agent brings its own model (the right model per task); else the global default.
    let model = agent_def
        .map(|a| a.model.trim())
        .filter(|m| !m.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| app.model());
    // A named agent may bring its OWN provider (mix backends by task complexity: a cheap model on
    // one provider for triage, a frontier model on another for hard reasoning). When set, run it
    // through a per-agent gateway so a foreign model id doesn't 404 against the global provider.
    let gateway = match agent_def.filter(|a| !a.provider.trim().is_empty()) {
        Some(a) => {
            let gw = std::sync::Arc::new(engram_gateway::Gateway::new(
                config::build_provider_from(&a.provider, &a.base_url, &a.api_key, &app.cfg().media),
                app.ledger.clone(),
            ));
            // The agent's own reasoning effort rides on its own gateway (model-default when empty).
            gw.set_default_effort(Some(a.effort.clone()));
            gw
        }
        None => app.gateway.clone(),
    };
    // FLYWHEEL - auto-recall: surface the few most task-relevant memories into the standing context
    // so the agent benefits from what it learned before, without being asked. Trusted runs only:
    // injecting the user's private knowledge into an untrusted-origin run would hand it to a
    // possibly-adversarial context. When we DO inject, the run is marked sensitive below, arming the
    // trifecta egress gate the moment it also touches untrusted content.
    let memory_block: Option<String> = if taint.is_untrusted() {
        None
    } else {
        let regions = engram_memory::Region::for_task(task);
        app.memory
            .recall_trusted_scoped(task, &regions, 5, &scope)
            .ok()
            .filter(|h| !h.is_empty())
            .map(|hits| {
                let lines = hits
                    .iter()
                    .map(|h| format!("- {}", h.record.text.replace('\n', " ")))
                    .collect::<Vec<_>>()
                    .join("\n");
                format!("Possibly-relevant snippets from past activity (LOWER priority than the confirmed working-memory facts above — these are activity logs that may be stale; if any conflicts with a confirmed fact, ignore it):\n{lines}")
            })
    };
    // AUTO-SELECT: surface the skills the agent already has so it reaches for skill_run / skill_improve
    // instead of re-solving from scratch. Ranked by track record (gold-backed runs first). Trusted
    // runs only — don't advertise the toolbelt to an untrusted-origin run. Mirrors the memory flywheel.
    // Don't advertise the skill toolbelt if the user curated out the tool that runs skills, or to an
    // untrusted-origin run.
    let skill_run_available = !app
        .cfg()
        .security
        .disabled_tools
        .iter()
        .any(|d| d == "skill_run");
    let skills_block: Option<String> = if taint.is_untrusted() || !skill_run_available {
        None
    } else {
        app.registry.skills().ok().and_then(|ids| {
            let mut rows: Vec<(usize, String)> = ids
                .iter()
                .filter_map(|id| {
                    let (signed, _) = app.registry.load_active(id).ok()?;
                    let m = &signed.manifest;
                    // The gold-example count (author-asserted / taught (input,output) pairs). Call it
                    // what it is — NOT "verified": nothing here is execution-checked, and the project
                    // forbids presenting an unbacked trust signal.
                    let gold = app.registry.accepted_runs(id).map(|r| r.len()).unwrap_or(0);
                    let tag = if gold > 0 {
                        format!(" ({gold} gold example{})", if gold == 1 { "" } else { "s" })
                    } else {
                        String::new()
                    };
                    Some((gold, format!("- {}: {}{}", m.id, m.description, tag)))
                })
                .collect();
            if rows.is_empty() {
                return None;
            }
            rows.sort_by_key(|r| std::cmp::Reverse(r.0));
            let lines = rows
                .into_iter()
                .take(8)
                .map(|(_, l)| l)
                .collect::<Vec<_>>()
                .join("\n");
            Some(format!(
                "Skills you can reuse (run one with skill_run, find more with skill_search, or offer a \
                 better version with skill_improve — prefer this over redoing work; when you solve \
                 something reusable, keep it with skill_author):\n{lines}"
            ))
        })
    };
    // Hoisted above the ctx literal so `allowed` can seed ctx.allowed_tools (delegated subagents
    // inherit the parent's tool scope). CURATION: drop tools the user turned off globally
    // (disabled_tools), then, if the assigned agent is scoped (allowed_tools), keep only those.
    let disabled = app.cfg().security.disabled_tools.clone();
    let allowed = agent_def.and_then(|a| a.allowed_tools.clone());
    let ctx = engram_agent::ToolCtx {
        memory: app.memory.clone(),
        skills: app.registry.clone(),
        gateway: gateway.clone(),
        ledger: app.ledger.clone(),
        taint,
        // Only untrusted-origin runs start sensitive. The background auto-recall (flywheel) injects a
        // few GENERAL memories as a convenience and must NOT, by itself, arm the trifecta - doing so
        // made every trusted run sensitive the instant any memory existed, so the egress guard blocked
        // ALL web access after the first page load (it broke multi-site research in chat/tasks). A
        // DELIBERATE recall via the memory_recall tool (reads_sensitive=true) still marks the run
        // sensitive, so genuinely-surfaced private data is still protected by the trifecta.
        sensitive: taint.is_untrusted(),
        policy,
        // An isolated per-task workdir (a git worktree, for parallel agents on one project) when
        // provided, else the shared workspace.
        workdir: workdir_override
            .clone()
            .unwrap_or_else(|| app.workdir.clone()),
        model: model.clone(),
        depth: 0,
        browser: app.browser.clone(),
        // The run's memory rings, so the agent's memory_recall / memory_remember tools (and any
        // subagents it delegates to, via the cloned ctx) stay inside this project's world.
        scope: scope.clone(),
        // halt/token_budget/on_step/on_narration are seeded by Agent::run from the builder calls the
        // daemon already makes below; a delegated subagent inherits them (and the tool scope) via the
        // cloned ctx, so it honors cancel/budget, emits steps, and can't exceed the parent's toolbelt.
        halt: None,
        spend_counter: None,
        token_budget: None,
        on_step: None,
        on_narration: None,
        allowed_tools: allowed.clone(),
    };
    // The global deny-list is also pushed into engram_agent so base_tools()/sub_tools() apply it,
    // which is what makes the curation hold for delegated SUBAGENTS (they never pass here).
    engram_agent::set_global_disabled_tools(disabled.clone());
    let mut tools = engram_agent::default_tools();
    for t in app.mcp_tools.read().expect("mcp lock").iter() {
        tools = tools.with(t.clone());
    }
    // The scheduler tool lives here (needs app.sched). Now the agent can actually CREATE a recurring
    // task ("update me every morning at 8am") instead of telling the user to set up cron themselves.
    tools = tools.with(std::sync::Arc::new(ScheduleTool {
        sched: app.sched.clone(),
    }));
    // base_tools() already dropped globally-disabled built-ins; we still filter here to (a) cover the
    // MCP tools added above, (b) apply the per-agent allowed_tools scope at this chokepoint, and
    // (c) strip the memory tools from untrusted-origin runs. An inbound channel/Telegram/webhook run
    // is Untrusted AND returns run.answer verbatim to an anonymous requester — that reply IS an
    // egress surface the trifecta gate does not cover, and memory_recall serves untrusted runs the
    // ALL-provenance user-global (private) ring. No scope value excludes the private ring, so the
    // only safe defense is to make the memory tools unreachable on these runs.
    if taint.is_untrusted() || !disabled.is_empty() || allowed.is_some() {
        tools = tools.retaining(|name| {
            if taint.is_untrusted() && (name == "memory_recall" || name == "memory_remember") {
                return false;
            }
            if disabled.iter().any(|d| d == name) {
                return false;
            }
            match &allowed {
                Some(list) => list.iter().any(|a| a == name),
                None => true,
            }
        });
    }
    // Production runs verify before finishing (one bounded reflection pass), are bounded by
    // a token budget (runaway-cost guard), and honor the kill switch.
    let budget: u32 = app
        .cfg()
        .cost
        .task_token_budget
        .try_into()
        .unwrap_or(u32::MAX);
    let mut agent = engram_agent::Agent::new(gateway.clone(), tools, model.clone())
        .max_steps(max_steps)
        .reflect(true)
        .token_budget(budget)
        .halt(halt.clone());
    // A named agent signs its steps as itself, so a multi-agent run is auditable per actor.
    if let Some(a) = agent_def {
        agent = agent.actor(a.name.clone());
    }
    // Standing context, in order: the assigned agent's ROLE (its specialization) leads, then the
    // consciousness working memory (facts about the user), then the global persona (style). Together
    // they replace SOUL.md as the source of truth for what the agent always knows.
    // Assemble the standing context as budget-tagged parts (tier 0 = essential/always-kept, higher =
    // droppable-under-pressure), then pack them under a token ceiling so a flood of recalled memory
    // or a large ingested document can never crowd out the essentials or blow the model window.
    let mut parts: Vec<budget::Part> = Vec::new();
    // Ground the agent in the current date — otherwise the model defaults to its training-cutoff year
    // (it was searching "AI news 2024" in mid-2026). Use local wall-clock so "today"/"this morning"
    // line up with the user.
    parts.push(budget::Part::new(
        format!(
            "Today's date is {}. Use the current year for any time-sensitive search or content.",
            chrono::Local::now().format("%A, %-d %B %Y")
        ),
        0,
    ));
    if let Some(a) = agent_def {
        if !a.role.trim().is_empty() {
            parts.push(budget::Part::new(a.role.clone(), 0));
        }
    }
    // Refresh the consciousness from current memory before reading it, so identity/semantic facts
    // the user JUST added (via the Memory view, chat identity-learning, or memory_remember) are
    // reflected in this run. distill() is deterministic and idempotent — it only re-ledgers/persists
    // when the facts actually changed, so this is cheap on an unchanged brain. (Previously the
    // consciousness was only distilled at boot, so newly-added facts never appeared — "what do you
    // know about me?" missed them entirely.)
    let _ = app.consciousness.distill(&app.memory, &app.ledger);
    if let Some(c) = app.consciousness.prompt_block() {
        parts.push(budget::Part::new(c, 0)); // curated working memory: essential
    }
    // Layered working memory: after the always-loaded GLOBAL block, add a per-project block for the
    // active project (its own durable facts), loaded only when a project is in scope - so "what
    // matters in THIS project" is present without leaking into any other project's context.
    if let Some(pid) = &scope.project {
        if let Some(pb) = conscious::project_block(&app.memory, pid) {
            parts.push(budget::Part::new(pb, 1));
        }
        // The active project's own standing instructions (Project.persona, editable in the desktop
        // UI's project settings) - previously only consulted by the legacy /v1/converse path, so
        // editing it had ZERO effect on the live agentic chat every user actually exercises, with
        // no warning that the control was inert. Wiring it in here is the fix.
        if let Some(pp) = app.workspace.project_persona(pid) {
            parts.push(budget::Part::new(pp, 1));
        }
    }
    if let Some(mb) = &memory_block {
        parts.push(budget::Part::new(mb.clone(), 2)); // recalled memories: droppable under pressure
    }
    if let Some(sb) = &skills_block {
        parts.push(budget::Part::new(sb.clone(), 2));
    }
    if let Some(p) = app.persona.read().expect("persona lock").clone() {
        if !p.trim().is_empty() {
            parts.push(budget::Part::new(p, 1));
        }
    }
    // Cap the assembled standing context; history, tools, the user turn, and the reply budget live
    // outside this. Generous but bounded, so it never dominates the window.
    const SYSTEM_CONTEXT_TOKENS: usize = 6000;
    let assembled = budget::pack(parts, SYSTEM_CONTEXT_TOKENS);
    if !assembled.trim().is_empty() {
        agent = agent.persona(assembled);
    }
    if let Some(cb) = on_step {
        agent = agent.on_step(cb);
    }
    if let Some(cb) = on_narration {
        agent = agent.on_narration(cb);
    }
    let result = agent.run(task, ctx).await.map_err(|e| e.to_string());
    // FLYWHEEL - auto-capture: on a completed, real (non-dry) trusted run, write one concise
    // episodic memory so the next task can recall what was done. Best-effort; dedup-on-write
    // collapses near-duplicates and consolidation demotes stale ones, so this can't bloat the brain.
    if let Ok(run) = &result {
        if !dry_run && run.stopped == "final" {
            let answer = run.answer.trim();
            // Capture only SUBSTANTIVE runs — ones that actually used tools or produced a real
            // result. A tool-less, short conversational reply ("try again" → "I'd be happy to
            // help!") is filler: capturing it bloated the brain and polluted the recall ribbon.
            let substantive = !run.steps.is_empty() || answer.chars().count() > 200;
            // EPISODIC CAPTURE stays TRUSTED-ONLY: untrusted-origin content could be adversarial and
            // must not enter durable memory. (The skill reflection below is safe on a tainted run
            // because it is a separate Trusted model call and nothing it proposes becomes active
            // until it passes the verification gate.)
            if !taint.is_untrusted() && !answer.is_empty() && substantive {
                // Record the user's ACTUAL request, not the full constructed prompt (which carries
                // the chat-mode directive + history + a "User's latest message:" prefix). Capturing
                // the whole prompt made huge, everything-matching episodic memories.
                let clean_task = task
                    .rsplit("User's latest message:")
                    .next()
                    .unwrap_or(task)
                    .trim();
                let label: String = clean_task.chars().take(160).collect();
                let snippet: String = answer.chars().take(280).collect();
                let text = format!("Task: {label}\nOutcome: {snippet}");
                // Route the capture to the right ring: a run inside a project keeps its outcome in
                // that project (so it never surfaces in another), while a project-less run stays
                // user-global. This is the single change that stops the flywheel bleed at its source.
                let write_scope =
                    crate::scope::classify(engram_memory::Region::Episodic, &scope, &text);
                let _ = app.memory.remember(
                    engram_memory::WriteReq::new(engram_memory::Region::Episodic, text)
                        .taint(taint)
                        .actor("agent")
                        .scope(write_scope),
                );
            }
        }
        // SELF-IMPROVEMENT REFLECTION (opt-in via auto_distill_skills): after a task that did
        // real multi-step work, reflect once on whether it yielded a reusable program — a NEW
        // skill or an IMPROVEMENT to an existing one — and verify it before it can become active.
        // Runs REGARDLESS of taint (unlike the episodic capture above): the reflection is a
        // separate Trusted model call, and the verification gate (replay against gold, sandboxed,
        // capability-clamped) is what protects activation. One bounded model call, gated by the
        // flag AND a tool-step threshold, so a daemon that opts out pays nothing.
        //
        // Fires on limit/budget stops too, not only "final": a run that spent its whole step or
        // token budget did the most multi-step work of all — exactly the runs whose method is
        // worth distilling. ("loop"/"halted"/"error" stops stay excluded: circling or killed work
        // is not a method worth keeping.)
        //
        // DETACHED: the reflection is spawned, not awaited — the user's answer must never wait on
        // the distiller's model call. Everything it decides lands in the signed ledger
        // (skill.reflect / skill.distill / skill.learn), not in this response.
        const MIN_STEPS_TO_DISTILL: usize = 3;
        if !dry_run
            && matches!(run.stopped, "final" | "limit" | "budget")
            && app.cfg().security.auto_distill_skills
            && run.steps.len() >= MIN_STEPS_TO_DISTILL
            && !app.cfg().security.disable_skill_author
        {
            let app2 = app.clone();
            let gateway2 = gateway.clone();
            let model2 = model.clone();
            let task2 = task.to_string();
            let answer2 = run.answer.clone();
            let steps2 = run.steps.len();
            let tainted2 = taint.is_untrusted();
            tokio::spawn(async move {
                reflect_on_skills(
                    &app2, &gateway2, &model2, &task2, &answer2, steps2, tainted2,
                )
                .await;
            });
        }
    }
    result
}

/// The reflection half of the self-improvement loop. Asks the model (a Trusted call) whether the
/// finished task yields a reusable program; then either improves an existing skill (A/B-gated promote)
/// or installs a NEW proposed skill and tries to EARN its activation by replaying it against its own
/// asserted gold. Nothing here trusts the proposal on faith — a new skill activates only if it (a) is
/// pure (no egress) and (b) reproduces every gold example in the sandbox; otherwise it is left
/// proposed for a human to adopt.
///
/// EVERY exit signs a `skill.reflect` event into the ledger — a decline, a parse failure, a gateway
/// error, an improvement verdict, an adoption verdict. The loop's whole funnel is auditable: "why
/// did no skill come out of that task?" is a ledger query, not a debugging session.
async fn reflect_on_skills(
    app: &App,
    gateway: &engram_gateway::Gateway,
    model: &str,
    task: &str,
    answer: &str,
    steps: usize,
    // Whether the source run read untrusted content. A distilled proposal built from a tainted run's
    // answer can embed injected code, so its verify/replay must be sandboxed (fail-closed downstream).
    source_tainted: bool,
) {
    let reflect_event = |payload: serde_json::Value| {
        let mut p = payload;
        p["steps"] = json!(steps);
        let _ = app.ledger.append("skill.reflect", "distiller", p);
    };
    let existing = app.registry.skills().unwrap_or_default();
    // Real-use experience per skill from the ledger tail: (runs, failures). A skill that keeps
    // failing in production is the best improvement target there is — put that signal in front of
    // the model instead of making it guess which incumbents are weak.
    let mut stats: std::collections::HashMap<String, (u32, u32)> = Default::default();
    if let Ok(entries) = app.ledger.read_all() {
        for e in entries.iter().rev().take(4000) {
            if e.kind != "skill.run" {
                continue;
            }
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(e.payload.get()) {
                if let Some(id) = v.get("id").and_then(serde_json::Value::as_str) {
                    let st = stats.entry(id.to_string()).or_default();
                    st.0 += 1;
                    if v.get("ok").and_then(serde_json::Value::as_bool) == Some(false) {
                        st.1 += 1;
                    }
                }
            }
        }
    }
    // A human-readable catalog (id — description [experience]) so the model can pick an improvement
    // target. Falls back to the LATEST version's manifest for a proposed (inactive) skill — those
    // are legitimate improvement targets too, not invisible.
    let catalog = existing
        .iter()
        .filter_map(|id| {
            let m = app
                .registry
                .load_active(id)
                .ok()
                .map(|(s, _)| s.manifest)
                .or_else(|| {
                    let latest = app.registry.versions(id).ok()?.into_iter().max()?;
                    app.registry.load(id, latest).ok().map(|(s, _)| s.manifest)
                })?;
            let experience = match stats.get(id.as_str()) {
                Some((r, f)) if *f > 0 => format!(" [{r} runs, {f} FAILED recently]"),
                Some((r, _)) => format!(" [{r} runs]"),
                None => String::new(),
            };
            Some(format!("{id} — {}{experience}", m.description))
        })
        .collect::<Vec<_>>()
        .join("\n");
    let p = match distill::propose(gateway, model, task, answer, &catalog).await {
        distill::ProposeOutcome::Proposal(p) => p,
        distill::ProposeOutcome::Unavailable => return, // mock: no model, no noise
        distill::ProposeOutcome::Declined { reason, reply_head } => {
            reflect_event(json!({ "outcome": "declined", "reason": reason, "reply": reply_head }));
            return;
        }
        distill::ProposeOutcome::Error(e) => {
            tracing::warn!("reflection: distiller call failed: {e}");
            reflect_event(json!({ "outcome": "error", "error": e }));
            return;
        }
    };
    let backend = {
        let c = app.cfg();
        config::resolve_shell_backend(&c.security.shell_backend, &c.security.shell_target)
    };
    let params = skill_run_params(app, backend.as_deref(), source_tainted);

    // Resolve the proposal against the registry. A NEW id that collides with a live skill BECOMES
    // an improvement of it (the model re-solving a solved problem is a signal the incumbent is
    // imperfect, not a reason to drop the work); an "improvement" of a skill that doesn't exist
    // BECOMES a new skill under that id (models routinely say "improves" about the script they
    // just wrote in the task — the work is real either way).
    let target_exists = existing.iter().any(|e| e == &p.id);
    let mut p = p;
    if p.improves && !target_exists {
        if app.registry.is_retired(&p.id) {
            reflect_event(json!({ "outcome": "declined", "reason": "id_disabled", "id": p.id }));
            return;
        }
        p.improves = false;
        if p.description.trim().is_empty() {
            p.description = "(distilled skill)".to_string();
        }
    }
    if !p.improves && !target_exists && app.registry.is_retired(&p.id) {
        // The id exists on disk but the user switched it off — re-proposing under it would install
        // invisible versions forever (skills() hides retired dirs, set_active doesn't clear the
        // marker). Refuse loudly; the user can re-enable the skill to make it a target again.
        reflect_event(json!({ "outcome": "declined", "reason": "id_disabled", "id": p.id }));
        return;
    }

    if p.improves || target_exists {
        let active = app.registry.active_version(&p.id).unwrap_or(None);
        // The incumbent's manifest: the active version's, else the latest version's (a proposed
        // skill that never activated can still be improved — its next version re-enters the
        // verify-and-adopt gate below).
        let manifest = match active {
            Some(_) => app
                .registry
                .load_active(&p.id)
                .ok()
                .map(|(s, _)| s.manifest),
            None => app
                .registry
                .versions(&p.id)
                .ok()
                .and_then(|v| v.into_iter().max())
                .and_then(|v| app.registry.load(&p.id, v).ok())
                .map(|(s, _)| s.manifest),
        };
        let Some(m) = manifest else {
            reflect_event(
                json!({ "outcome": "improve_failed", "id": p.id, "error": "no loadable version" }),
            );
            return;
        };
        if m.runtime != engram_skills::Runtime::Process {
            // WASM skills are improved from the dashboard (WAT), not autonomously.
            reflect_event(json!({ "outcome": "declined", "reason": "wasm_target", "id": p.id }));
            return;
        }
        let description = if p.description.trim().is_empty() {
            m.description.clone()
        } else {
            p.description.clone()
        };
        let candidate = engram_skills::NewSkill {
            id: p.id.clone(),
            category: m.category.clone(),
            description,
            capabilities: m.capabilities.clone(),
            metric: m.metric.clone(),
            runtime: engram_skills::Runtime::Process,
            interpreter: Some(p.interpreter.clone()),
            when_to_use: m.when_to_use.clone(),
        };
        if active.is_some() {
            // IMPROVEMENT to an ACTIVE skill: A/B-replay the candidate against the incumbent's gold
            // and promote it only if it measurably wins (shared path with the agent tool + HTTP).
            match engram_agent::improve_skill(
                &app.registry,
                &p.id,
                candidate,
                p.source.as_bytes(),
                &p.examples,
                true,
                "distiller",
                &params,
                Some(&app.halt),
            )
            .await
            {
                Ok(d) => {
                    tracing::info!(id = %p.id, decision = %d["decision"], "reflection: improvement attempt");
                    reflect_event(json!({ "outcome": "improve", "id": p.id, "decision": d }));
                }
                Err(e) => {
                    tracing::warn!(id = %p.id, "reflection: improve failed: {e}");
                    reflect_event(json!({ "outcome": "improve_failed", "id": p.id, "error": e }));
                }
            }
        } else {
            // NEW VERSION of a PROPOSED (inactive) skill: park it inactive alongside the incumbent
            // and let it try to EARN activation against the skill's recorded gold. This is the exit
            // from the old deadlock where an unadopted skill could never change again.
            let Ok(version) = app
                .registry
                .install_inactive(candidate, p.source.as_bytes())
            else {
                reflect_event(json!({ "outcome": "install_failed", "id": p.id }));
                return;
            };
            for (inp, out) in &p.examples {
                let _ =
                    app.registry
                        .record_run(&p.id, version, inp.as_bytes(), out.as_bytes(), 1.0);
            }
            match engram_agent::verify_and_adopt(
                &app.registry,
                &p.id,
                "distiller",
                true,
                &params,
                Some(&app.halt),
            )
            .await
            {
                Ok(d) => {
                    tracing::info!(id = %p.id, version, decision = %d["decision"], "reflection: proposed-skill revision");
                    reflect_event(
                        json!({ "outcome": "revise_proposed", "id": p.id, "version": version, "decision": d }),
                    );
                }
                Err(e) => {
                    reflect_event(
                        json!({ "outcome": "verify_failed", "id": p.id, "version": version, "error": e }),
                    );
                }
            }
        }
        return;
    }

    // NEW skill: install inactive, seed the gold with the asserted examples, then try to earn
    // activation by replaying it against that gold. A pure skill (no declared capabilities) can be
    // auto-adopted; a network/LLM skill is staged for human approval (it can't be replay-verified).
    let capabilities: Vec<engram_skills::Capability> = p
        .capabilities
        .iter()
        .filter_map(|c| match c.as_str() {
            "net" => Some(engram_skills::Capability::Net),
            "llm" => Some(engram_skills::Capability::Llm),
            _ => None,
        })
        .collect();
    let new = engram_skills::NewSkill {
        id: p.id.clone(),
        category: "problem_solving".into(),
        description: p.description.clone(),
        capabilities,
        metric: "exact_match".into(),
        runtime: engram_skills::Runtime::Process,
        interpreter: Some(p.interpreter.clone()),
        when_to_use: p.when_to_use.clone(),
    };
    let Ok(version) = app.registry.install_inactive(new, p.source.as_bytes()) else {
        reflect_event(json!({ "outcome": "install_failed", "id": p.id }));
        return;
    };
    for (inp, out) in &p.examples {
        let _ = app
            .registry
            .record_run(&p.id, version, inp.as_bytes(), out.as_bytes(), 1.0);
    }
    let _ = app.ledger.append(
        "skill.distill",
        "distiller",
        json!({ "id": p.id, "version": version, "active": false,
                "examples": p.examples.len(), "steps": steps }),
    );
    // EARN ACTIVATION: a pure skill that reproduces all its gold in the sandbox is adopted; otherwise
    // it stays proposed for a human to adopt from the dashboard.
    match engram_agent::verify_and_adopt(
        &app.registry,
        &p.id,
        "distiller",
        true,
        &params,
        Some(&app.halt),
    )
    .await
    {
        Ok(d) => {
            tracing::info!(id = %p.id, version, decision = %d["decision"], "reflection: new skill");
            reflect_event(
                json!({ "outcome": "new_skill", "id": p.id, "version": version, "decision": d }),
            );
        }
        Err(e) => {
            tracing::warn!(id = %p.id, "reflection: verify/adopt failed: {e}");
            reflect_event(
                json!({ "outcome": "verify_failed", "id": p.id, "version": version, "error": e }),
            );
        }
    }
}

async fn agent_handler(State(app): State<App>, Json(r): Json<AgentReq>) -> ApiResult {
    let run = run_agent_task_cb(
        &app,
        &r.task,
        r.max_steps.unwrap_or(8),
        engram_core::Taint::Trusted,
        r.dry_run,
        None,
        None,
        None,
        None,
        false, // approved
        true,  // attended (interactive /v1/agent call)
        app.halt.clone(),
        engram_core::ScopeCtx::user_only(), // /v1/agent carries no session/project → user-global
    )
    .await
    .map_err(ApiError)?;
    Ok(Json(serde_json::to_value(run).map_err(err)?))
}

/// A live voice session over a WebSocket. The client streams audio as binary frames
/// and sends a text "end" to close a turn; the server transcribes the accumulated
/// audio, runs the agent, and replies with a JSON text frame (transcript + reply) and
/// a binary frame of synthesized speech. The connection stays open for many turns - a
/// real-time voice loop. (Per-turn STT here; word-by-word streaming STT is a provider
/// extension.) Needs a provider with speech-to-text + text-to-speech.
async fn voice_stream(State(app): State<App>, ws: axum::extract::ws::WebSocketUpgrade) -> Response {
    ws.on_upgrade(move |socket| voice_session(app, socket))
}

async fn voice_session(app: App, mut socket: axum::extract::ws::WebSocket) {
    use axum::extract::ws::Message as Ws;
    // Cap accumulated audio to match /v1/upload (25MB): a buggy or malicious client (any local process
    // on the default no-token desktop) could otherwise stream binary frames forever and OOM the daemon,
    // since the buffer grows across messages. On overflow we drop the buffer, tell the client, and keep
    // the socket open for the next turn rather than accumulating into a crash.
    const MAX_AUDIO: usize = 25 * 1024 * 1024;
    let mut audio: Vec<u8> = Vec::new();
    while let Some(Ok(msg)) = socket.recv().await {
        match msg {
            Ws::Binary(b) => {
                if audio.len().saturating_add(b.len()) > MAX_AUDIO {
                    audio.clear();
                    let _ = socket
                        .send(Ws::Text(
                            json!({ "error": "audio too large (25MB cap) — turn discarded" })
                                .to_string()
                                .into(),
                        ))
                        .await;
                    continue;
                }
                audio.extend_from_slice(&b);
            }
            Ws::Text(t) if t.as_str() == "end" => {
                let turn = process_voice_turn(&app, &audio).await;
                audio.clear();
                let send = match turn {
                    Ok((transcript, reply, out)) => {
                        let _ = socket
                            .send(Ws::Text(
                                json!({ "transcript": transcript, "reply": reply })
                                    .to_string()
                                    .into(),
                            ))
                            .await;
                        socket.send(Ws::Binary(out.into())).await
                    }
                    Err(e) => {
                        socket
                            .send(Ws::Text(json!({ "error": e }).to_string().into()))
                            .await
                    }
                };
                if send.is_err() {
                    break;
                }
            }
            Ws::Close(_) => break,
            _ => {}
        }
    }
}

async fn process_voice_turn(app: &App, audio: &[u8]) -> Result<(String, String, Vec<u8>), String> {
    let transcript = app
        .gateway
        .transcribe(audio, "wav", "voice")
        .await
        .map_err(|e| e.to_string())?;
    let run = run_agent_task(app, &transcript, 8).await?;
    let out = app
        .gateway
        .tts(&run.answer, "alloy", "voice")
        .await
        .map_err(|e| e.to_string())?;
    Ok((transcript, run.answer, out))
}

#[derive(Deserialize)]
struct VoiceReq {
    audio_b64: String,
    #[serde(default)]
    format: Option<String>,
    #[serde(default)]
    voice: Option<String>,
}

/// A voice turn: audio in → transcribe → run the agent → synthesize the reply → audio
/// out. Needs a provider with speech-to-text and text-to-speech (build --features http).
async fn voice_handler(State(app): State<App>, Json(r): Json<VoiceReq>) -> ApiResult {
    use base64::Engine;
    let audio = base64::engine::general_purpose::STANDARD
        .decode(r.audio_b64.as_bytes())
        .map_err(err)?;
    let fmt = r.format.as_deref().unwrap_or("mp3");
    let transcript = app
        .gateway
        .transcribe(&audio, fmt, "voice")
        .await
        .map_err(err)?;
    let run = run_agent_task(&app, &transcript, 8)
        .await
        .map_err(ApiError)?;
    let voice = r.voice.as_deref().unwrap_or("alloy");
    let audio_out = app
        .gateway
        .tts(&run.answer, voice, "voice")
        .await
        .map_err(err)?;
    let audio_b64 = base64::engine::general_purpose::STANDARD.encode(&audio_out);
    Ok(Json(
        json!({ "transcript": transcript, "reply": run.answer, "audio_b64": audio_b64 }),
    ))
}

/// Server-Sent Events: stream the neural bus so the desktop updates the moment anything
/// happens (a task starts, a step completes, a run finishes) instead of polling.
async fn events(
    State(app): State<App>,
) -> axum::response::sse::Sse<
    impl futures_core::Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>,
> {
    use axum::response::sse::{Event, KeepAlive, Sse};
    let mut syn = app.bus.synapse();
    // Cap the connection lifetime so a held-open stream can never block the daemon's
    // graceful idle-exit (zero-idle). The browser's EventSource reconnects seamlessly.
    let stream = async_stream::stream! {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() { break; }
            match tokio::time::timeout(remaining, syn.recv()).await {
                Ok(Some(spike)) => {
                    let data = json!({ "topic": spike.topic, "payload": spike.payload }).to_string();
                    yield Ok(Event::default().event("spike").data(data));
                }
                Ok(None) | Err(_) => break,
            }
        }
    };
    Sse::new(stream).keep_alive(KeepAlive::default())
}

async fn policy_get(State(app): State<App>) -> ApiResult {
    Ok(Json(json!({
        "allow_shell": app.allow_shell.load(std::sync::atomic::Ordering::Relaxed),
        "halted": app.halt.load(std::sync::atomic::Ordering::Relaxed),
    })))
}

#[derive(Deserialize)]
struct HaltReq {
    #[serde(default)]
    on: bool,
    /// Stop JUST this chat session's run (leaving other concurrent chats running). Omit for the
    /// GLOBAL emergency stop that halts every in-flight run.
    #[serde(default)]
    session: Option<String>,
}

/// Stop a run at its next step boundary. `{"on":true,"session":"<id>"}` halts only that chat;
/// `{"on":true}` (no session) is the global emergency stop (every run). `{"on":false}` releases.
async fn halt_set(State(app): State<App>, Json(r): Json<HaltReq>) -> ApiResult {
    use std::sync::atomic::Ordering;
    if let Some(sid) = &r.session {
        // Per-session: flip EVERY run registered under this session (keys are `<sid>#<n>`), so a
        // concurrent second message in the same chat is also stopped. A no-op if nothing is running.
        if let Ok(g) = app.run_halts.lock() {
            for (key, h) in g.iter() {
                if halt_key_matches(key, sid) {
                    h.store(r.on, Ordering::Relaxed);
                }
            }
        }
        let _ = app
            .ledger
            .append("halt.set", "user", json!({ "on": r.on, "session": sid }));
        return Ok(Json(json!({ "halted": r.on, "session": sid })));
    }
    // Global emergency stop: the shared flag AND every registered per-session run.
    app.halt.store(r.on, Ordering::Relaxed);
    if let Ok(g) = app.run_halts.lock() {
        for h in g.values() {
            h.store(r.on, Ordering::Relaxed);
        }
    }
    let _ = app.ledger.append("halt.set", "user", json!({ "on": r.on }));
    Ok(Json(json!({ "halted": r.on })))
}

#[derive(Deserialize)]
struct PolicyReq {
    allow_shell: Option<bool>,
}

/// Grant or revoke a standing capability (the desktop's "always allow"). The decision is
/// recorded in the audit ledger, so even a consent change is on the record.
async fn policy_set(State(app): State<App>, Json(r): Json<PolicyReq>) -> ApiResult {
    if let Some(v) = r.allow_shell {
        app.allow_shell
            .store(v, std::sync::atomic::Ordering::Relaxed);
        // Persist the SAME consent to config so it is one source of truth. Previously this only set
        // the runtime atomic, so GET /v1/config still reported allow_shell:false — Settings › Tools
        // rendered the checkbox unchecked while the agent could actually run shell, and the next Save
        // on that page posted allow_shell:false and silently turned shell back off (the dogfooded
        // "shell-off digest" failure). Also made the grant vanish on restart with no UI hint.
        {
            let mut cfg = app.config.write().expect("config lock");
            cfg.security.allow_shell = v;
            if let Err(e) = cfg.save(&app.home) {
                tracing::warn!(error = %e, "could not persist allow_shell consent");
            }
        }
        let _ = app
            .ledger
            .append("policy.set", "user", json!({ "allow_shell": v }));
    }
    Ok(Json(
        json!({ "allow_shell": app.allow_shell.load(std::sync::atomic::Ordering::Relaxed) }),
    ))
}

async fn tasks_list(State(app): State<App>) -> ApiResult {
    Ok(Json(serde_json::to_value(app.tasks.list()).map_err(err)?))
}

#[derive(Deserialize)]
struct TaskCreateReq {
    title: String,
    #[serde(default)]
    detail: String,
    #[serde(default)]
    origin: Option<String>,
}

async fn tasks_create(State(app): State<App>, Json(r): Json<TaskCreateReq>) -> ApiResult {
    let t = app.tasks.create(
        r.title,
        r.detail,
        r.origin.unwrap_or_else(|| "manual".into()),
    );
    app.bus.emit(Spike::new(
        "task.create",
        Priority::Low,
        json!({ "id": t.id }),
    ));
    Ok(Json(serde_json::to_value(t).map_err(err)?))
}

#[derive(Deserialize)]
struct TaskUpdateReq {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    detail: Option<String>,
}

async fn tasks_update(
    State(app): State<App>,
    Path(id): Path<String>,
    Json(r): Json<TaskUpdateReq>,
) -> ApiResult {
    let t = app
        .tasks
        .update(&id, r.status, r.title, r.detail)
        .ok_or_else(|| ApiError("task not found".into()))?;
    Ok(Json(serde_json::to_value(t).map_err(err)?))
}

/// Assign (or clear) the durable agent that runs a card. Signed as `task.assign` - assigning a
/// teammate to a card is itself an auditable event.
async fn tasks_assign(
    State(app): State<App>,
    Path(id): Path<String>,
    Json(p): Json<Value>,
) -> ApiResult {
    let agent = p
        .get("agent")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(String::from);
    let agent_name = match &agent {
        Some(aid) => Some(
            app.agents
                .get(aid)
                .ok_or_else(|| err("no such agent"))?
                .name,
        ),
        None => None,
    };
    let t = app
        .tasks
        .set_agent(&id, agent.clone())
        .ok_or_else(|| err("task not found"))?;
    app.ledger
        .append(
            "task.assign",
            "user",
            json!({ "task": id, "agent": agent, "agent_name": agent_name }),
        )
        .map_err(err)?;
    Ok(Json(serde_json::to_value(t).map_err(err)?))
}

/// Hand a card from its current agent to another, with a note. Reassigns, appends to the card's
/// hand-off trail, and signs `task.handoff` (from → to + note) - a multi-agent collaboration you
/// can audit end to end.
async fn task_handoff(
    State(app): State<App>,
    Path(id): Path<String>,
    Json(p): Json<Value>,
) -> ApiResult {
    let to_id = p
        .get("to")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(String::from);
    let note = p
        .get("note")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let to_name = match &to_id {
        Some(aid) => {
            app.agents
                .get(aid)
                .ok_or_else(|| err("no such agent"))?
                .name
        }
        None => "Default agent".into(),
    };
    let task = app.tasks.get(&id).ok_or_else(|| err("task not found"))?;
    let from_name = task
        .agent
        .as_ref()
        .and_then(|aid| app.agents.get(aid))
        .map(|a| a.name)
        .unwrap_or_else(|| "Default agent".into());
    let updated = app
        .tasks
        .handoff(&id, to_id, &from_name, &to_name, &note)
        .ok_or_else(|| err("task not found"))?;
    app.ledger
        .append(
            "task.handoff",
            "user",
            json!({ "task": id, "from": from_name, "to": to_name, "note": note }),
        )
        .map_err(err)?;
    Ok(Json(serde_json::to_value(updated).map_err(err)?))
}

/// Pre-run specialist review: surface a grounded objection (citing real recalled memories) if the
/// task conflicts with what Engram knows. Returns `{ dissent: null }` when nothing real conflicts or
/// no model is connected to assess - it never invents an objection.
async fn task_review(State(app): State<App>, Path(id): Path<String>) -> ApiResult {
    let task = app.tasks.get(&id).ok_or_else(|| err("task not found"))?;
    let prompt = if task.detail.trim().is_empty() {
        task.title.clone()
    } else {
        format!("{}\n\n{}", task.title, task.detail)
    };
    // A reviewing agent uses its own model (right model per task); else the global default.
    let model = task
        .agent
        .as_ref()
        .and_then(|aid| app.agents.get(aid))
        .and_then(|a| (!a.model.is_empty()).then_some(a.model))
        .unwrap_or_else(|| app.model());
    let d = dissent::review(
        &app.memory,
        &app.gateway,
        &model,
        &prompt,
        &engram_core::ScopeCtx::user_only(),
    )
    .await;
    Ok(Json(json!({ "dissent": d })))
}

/// Record the user's response to a specialist objection - signing plan + objection + grounds +
/// choice as ONE ledger artifact, attributed to the agent that raised it. The disagreement itself
/// becomes auditable.
async fn task_dissent(
    State(app): State<App>,
    Path(id): Path<String>,
    Json(p): Json<Value>,
) -> ApiResult {
    let objection = p.get("objection").and_then(Value::as_str).unwrap_or("");
    let grounds = p.get("grounds").cloned().unwrap_or_else(|| json!([]));
    let choice = p.get("choice").and_then(Value::as_str).unwrap_or("proceed");
    let actor = app
        .tasks
        .get(&id)
        .and_then(|t| t.agent)
        .and_then(|aid| app.agents.get(&aid))
        .map(|a| a.name)
        .unwrap_or_else(|| "specialist".into());
    app.ledger
        .append(
            "dissent",
            actor,
            json!({ "task": id, "objection": objection, "grounds": grounds, "choice": choice }),
        )
        .map_err(err)?;
    Ok(Json(json!({ "ok": true })))
}

async fn tasks_delete(State(app): State<App>, Path(id): Path<String>) -> ApiResult {
    Ok(Json(json!({ "removed": app.tasks.remove(&id) })))
}

/// Snapshot the set of relative file paths under `root`, skipping VCS/build/dependency dirs and the
/// agent's own state, bounded so a large repo can't stall the run. Diffing this before/after a run
/// isolates the files the run CREATED (its artifacts) from the rest of the workspace.
fn snapshot_files(root: &std::path::Path) -> std::collections::HashSet<std::path::PathBuf> {
    fn skip_dir(name: &str) -> bool {
        name.starts_with('.')
            || matches!(
                name,
                "target" | "node_modules" | "__pycache__" | "venv" | "dist" | "build"
            )
    }
    let mut out = std::collections::HashSet::new();
    let mut stack = vec![root.to_path_buf()];
    let mut budget = 6000usize;
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for ent in rd.flatten() {
            if budget == 0 {
                return out;
            }
            budget -= 1;
            let name = ent.file_name().to_string_lossy().to_string();
            let is_dir = ent.file_type().map(|f| f.is_dir()).unwrap_or(false);
            if is_dir {
                if !skip_dir(&name) {
                    stack.push(ent.path());
                }
            } else if let Ok(rel) = ent.path().strip_prefix(root) {
                out.insert(rel.to_path_buf());
            }
        }
    }
    out
}

/// Relative paths that `write_file`/`append_file` tool calls touched during this run, read back from
/// the signed ledger (kinds `agent.write` / `agent.append`, seq > `start_seq`). Used by
/// `capture_artifacts` to force-capture an output file even when it OVERWRITES a same-named file left
/// by a previous run — e.g. a recurring digest task that writes `evening_digest.html` every time. The
/// plain new-vs-preexisting diff only ever catches that file on the very first run.
fn written_paths_since(
    ledger: &engram_core::Ledger,
    workdir: &std::path::Path,
    start_seq: u64,
) -> std::collections::HashSet<std::path::PathBuf> {
    let mut out = std::collections::HashSet::new();
    let Ok(tail) = ledger.tail(400) else {
        return out;
    };
    for e in tail
        .iter()
        .filter(|e| e.seq > start_seq && (e.kind == "agent.write" || e.kind == "agent.append"))
    {
        let Ok(p) = serde_json::from_str::<Value>(e.payload.get()) else {
            continue;
        };
        let Some(path) = p.get("path").and_then(Value::as_str) else {
            continue;
        };
        if let Ok(rel) = std::path::Path::new(path).strip_prefix(workdir) {
            out.insert(rel.to_path_buf());
        }
    }
    out
}

/// After a run, copy the files that newly appeared in `workdir` (since the `before` snapshot) — union
/// `written` (paths this run's write_file/append_file calls touched, see `written_paths_since`, so a
/// rewritten same-named output still counts) — into a persistent per-task artifacts dir
/// (`<home>/artifacts/<task-id>/`), returning their relative paths. Copying out decouples artifacts
/// from the (possibly ephemeral git-worktree) workdir so they survive cleanup, and capturing only
/// new-or-explicitly-written files keeps incidental edits to existing project files out of the list.
fn capture_artifacts(
    home: &str,
    task_id: &str,
    workdir: &std::path::Path,
    before: &std::collections::HashSet<std::path::PathBuf>,
    written: &std::collections::HashSet<std::path::PathBuf>,
) -> Vec<String> {
    let after = snapshot_files(workdir);
    let mut new_files: std::collections::HashSet<_> = after.difference(before).cloned().collect();
    for p in written {
        if after.contains(p) {
            new_files.insert(p.clone());
        }
    }
    let mut new_files: Vec<_> = new_files.into_iter().collect();
    new_files.sort();
    new_files.truncate(200); // a sane cap so a runaway run can't flood the artifacts dir
    let dest_root = std::path::Path::new(home).join("artifacts").join(task_id);
    let mut rels = Vec::new();
    for rel in new_files {
        let src = workdir.join(&rel);
        // Skip absurdly large outputs (a 64 MB ceiling matches the screenshot serving cap).
        if std::fs::metadata(&src)
            .map(|m| m.len() > 64 * 1024 * 1024)
            .unwrap_or(true)
        {
            continue;
        }
        let dest = dest_root.join(&rel);
        if let Some(parent) = dest.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if std::fs::copy(&src, &dest).is_ok() {
            rels.push(rel.to_string_lossy().replace('\\', "/"));
        }
    }
    rels
}

/// Run a task with the agent and attach a glass-box receipt: mark it doing (and fire a
/// spike so the board shows Running), run, capture the cost delta and the signed ledger
/// head, then mark done - or failed if the agent hit its step limit. Shared by the HTTP
/// endpoint and the in-process scheduler.
/// Best-effort POST of `text` to a channel webhook (Slack/Discord/generic). Lets an UNATTENDED run
/// tell the user, asynchronously, that it staged an action for approval — so they learn without
/// watching the app. No-op when the url is empty. The url is the user's own configured channel
/// (trusted), so a plain post is fine.
async fn post_webhook(url: &str, text: &str) {
    let url = url.trim();
    if url.is_empty() {
        return;
    }
    let Ok(client) = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .build()
    else {
        return;
    };
    // "text" for Slack/Mattermost, "content" for Discord — send both for compatibility.
    let _ = client
        .post(url)
        .json(&json!({ "text": text, "content": text }))
        .send()
        .await;
}

pub(crate) async fn run_task_core(
    app: &App,
    id: &str,
    // When set, each completed step is streamed here as JSON for the live "watch the agent" view.
    step_tx: Option<tokio::sync::mpsc::UnboundedSender<serde_json::Value>>,
    // One-time user approval (the UI's "Approve & continue") — de-escalates the egress trifecta for
    // THIS run only. Never persisted.
    approved: bool,
    // Whether a human is watching: a user-initiated run (true) vs a scheduled/inbound one (false).
    attended: bool,
) -> Result<tasks::Task, String> {
    let task = app.tasks.get(id).ok_or("task not found")?;
    // Atomically claim the task so two concurrent runs (double-click, HTTP + scheduler)
    // can't both execute and corrupt the receipt/cost delta.
    if !app.tasks.try_begin(id) {
        return Err("task is already running".into());
    }
    app.bus.emit(Spike::new(
        "task.run",
        Priority::Normal,
        json!({ "id": id }),
    ));

    let mut prompt = if task.detail.trim().is_empty() {
        task.title.clone()
    } else {
        format!("{}\n\n{}", task.title, task.detail)
    };
    // Make collaboration real: when this card was handed off (or already ran), prepend the
    // upstream agent's work product and the latest hand-off note, so the receiving agent
    // continues the mission instead of restarting from the bare title.
    if let Some(prev) = task.run.as_ref().filter(|r| !r.answer.trim().is_empty()) {
        prompt.push_str(&format!(
            "\n\n--- Previous agent's result (continue/improve on this, don't restart) ---\n{}",
            prev.answer.chars().take(4000).collect::<String>()
        ));
    }
    if let Some(h) = task.handoffs.last().filter(|h| !h.note.trim().is_empty()) {
        prompt.push_str(&format!(
            "\n\n--- Hand-off note from {} to {} ---\n{}",
            h.from, h.to, h.note
        ));
    }
    // Extend the relay beyond the single immediately-prior hop: `prev.answer` above is only ever
    // the LAST run's final answer, truncated to 4000 chars - two hops into a mission, everything
    // from hop N-2 and earlier used to be gone from prompt construction entirely. Plan-milestone
    // breadcrumbs (Region::Episodic, written per completed plan step - see UpdatePlanTool) and
    // paged compaction detail both durably outlive any single hop, so a broader recall keyed on
    // the task's own (stable across hops) title surfaces relevant earlier-hop detail regardless of
    // how many runs back it happened - not just what the immediately-prior answer happened to say.
    if let Ok(breadcrumbs) = app.memory.recall_scoped(
        &task.title,
        &[Region::Episodic],
        6,
        &engram_core::ScopeCtx::user_only(),
    ) {
        // Exclude the last run's own final-answer sentence (already injected verbatim above via
        // prev.answer) so this adds earlier-hop detail instead of restating the same thing twice.
        let already_included = task
            .run
            .as_ref()
            .map(|r| r.answer.trim())
            .unwrap_or_default();
        let extra: Vec<&str> = breadcrumbs
            .iter()
            .map(|h| h.record.text.as_str())
            .filter(|t| !already_included.contains(t))
            .take(5)
            .collect();
        if !extra.is_empty() {
            prompt
                .push_str("\n\n--- Earlier progress on this mission (recalled from memory) ---\n");
            for e in extra {
                prompt.push_str("- ");
                prompt.push_str(e);
                prompt.push('\n');
            }
        }
    }
    let before = app.gateway.meter();
    // The ledger seq before the run, so we can find the egress destinations THIS run parks (the
    // `agent.egress_staged` entries it appends) and link them to this task for re-run-on-approve.
    let start_seq = app.ledger.head().0;
    let started_ms = engram_core::now_ms() as i64;
    // Stream live progress onto the card and over the event bus as each step completes.
    let tasks = app.tasks.clone();
    let bus = app.bus.clone();
    let tid = id.to_string();
    let step_tx2 = step_tx.clone();
    // Captured so each streamed step can carry this run's cumulative tokens/cost so far (the live
    // meter), measured as the delta from the gateway meter at the start of the run.
    let gw = app.gateway.clone();
    let (base_in, base_out, base_cost) = (before.tokens_in, before.tokens_out, before.cost_usd);
    let on_step: engram_agent::StepCallback = Arc::new(move |i, rec: &engram_agent::StepRecord| {
        tasks.set_progress(&tid, format!("step {i} · {}", rec.tool));
        bus.emit(Spike::new(
            "task.step",
            Priority::Low,
            json!({ "id": tid.as_str(), "step": i, "tool": rec.tool }),
        ));
        if let Some(tx) = &step_tx2 {
            // Stream the step as it lands - tool, args, the (truncated) observation, the step's own
            // signed ledger seq+hash, and the live token/cost meter, so the UI shows the glass box
            // filling in (and the bill ticking up) live.
            let obs: String = rec.observation.chars().take(2000).collect();
            let m = gw.meter();
            let _ = tx.send(json!({
                "index": i, "tool": rec.tool, "args": rec.args, "observation": obs,
                "ok": rec.ok, "seq": rec.ledger_seq, "hash": rec.ledger_hash,
                "tokens": m.tokens_in.saturating_sub(base_in) + m.tokens_out.saturating_sub(base_out),
                "cost": (m.cost_usd - base_cost).max(0.0),
            }));
        }
    });
    // If a durable agent is assigned to this card, it drives the run (its role + model) and signs
    // every step as itself - the auditable team.
    let agent_def = task.agent.as_ref().and_then(|aid| app.agents.get(aid));
    // Working-tree isolation: with ENGRAM_WORKTREES=1 and a git workspace, each task runs in its
    // OWN detached git worktree, so several agents can work the same project in parallel without
    // clobbering each other's files. The guard removes the worktree when the run finishes (any path).
    let (workdir_override, _worktree_guard) = prepare_worktree(app, &task.id);
    // Snapshot the workdir so we can capture the files THIS run creates as downloadable artifacts.
    let run_workdir = workdir_override
        .clone()
        .unwrap_or_else(|| app.workdir.clone());
    let artifacts_before = snapshot_files(&run_workdir);
    // Auto-checkpoint the workdir before the run: it powers REWIND (Claude-Code-style) and the
    // task panel's "changes this run made" diff. Git repos are snapshotted too now — history
    // explains commits, but a run's UNCOMMITTED edits need a baseline to diff against. Worktree-
    // isolated runs are still skipped (their edits live on a branch). Bounded + off-runtime, and
    // old auto-checkpoints are pruned so runs can't accumulate unbounded disk.
    if workdir_override.is_none() {
        let home = app.home.clone();
        let wd = run_workdir.clone();
        let label = format!("before task: {}", task.title);
        let tid = task.id.clone();
        let _ = tokio::task::spawn_blocking(move || {
            let r = checkpoints::snapshot(
                &home,
                &wd,
                &label,
                None,
                Some(tid),
                engram_core::now_ms() as u64,
            );
            checkpoints::prune_auto(&home, 20);
            r
        })
        .await;
    }
    // Per-task halt: register a flag keyed `<task-id>#<n>` so `/v1/halt {session:"<task-id>"}` stops
    // JUST this task (not the daemon-wide kill switch, which used to be the only option and silently
    // killed every future run). The global emergency stop still flips it (halt_set iterates all
    // registered flags), and each run removes only its own key.
    let run_halt = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let halt_key = format!(
        "{id}#{}",
        RUN_HALT_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    if let Ok(mut g) = app.run_halts.lock() {
        g.insert(halt_key.clone(), run_halt.clone());
    }
    let run_res = run_agent_task_cb(
        app,
        &prompt,
        16,
        engram_core::Taint::Trusted,
        false,
        Some(on_step),
        None,
        agent_def.as_ref(),
        workdir_override,
        approved,
        attended,
        run_halt,
        engram_core::ScopeCtx::user_only(), // task-board runs are not project-bound yet → user-global
    )
    .await;
    if let Ok(mut g) = app.run_halts.lock() {
        g.remove(&halt_key); // remove only THIS run's flag
    }
    let run = match run_res {
        Ok(r) => r,
        Err(e) => {
            // The agent errored (e.g. provider failure after retries). try_begin already
            // marked the task "doing"; record a failed receipt so it isn't stuck "doing"
            // forever (try_begin would reject every future run).
            let m = app.gateway.meter();
            // Capture any files the run wrote before it errored — otherwise a run that produced files
            // and then failed leaves them unreachable from the UI (the worktree copy is force-removed
            // on the guard's drop). Best-effort; the worktree guard is still alive here.
            let written = written_paths_since(&app.ledger, &run_workdir, start_seq);
            let output_files =
                capture_artifacts(&app.home, id, &run_workdir, &artifacts_before, &written);
            let receipt = tasks::TaskRun {
                answer: format!("(run failed: {e})"),
                steps: Vec::new(),
                stopped: "error".to_string(),
                tokens_in: m.tokens_in.saturating_sub(before.tokens_in),
                tokens_out: m.tokens_out.saturating_sub(before.tokens_out),
                cost_usd: (m.cost_usd - before.cost_usd).max(0.0),
                ledger_head_hash: app.ledger.head().1,
                started_ms,
                finished_ms: engram_core::now_ms() as i64,
                output_files,
            };
            app.tasks.finish(id, receipt, "failed");
            app.bus.emit(Spike::new(
                "task.done",
                Priority::Normal,
                json!({ "id": id, "status": "failed" }),
            ));
            let hooks = app.cfg().hooks.clone();
            if !hooks.is_empty() {
                hooks::run_hooks(
                    &hooks,
                    "task.done",
                    &json!({ "id": id, "status": "failed", "title": task.title }),
                )
                .await;
            }
            return Err(e);
        }
    };
    let finished_ms = engram_core::now_ms() as i64;
    let after = app.gateway.meter();
    let (_, head) = app.ledger.head();

    // Only a clean final answer is a success; halted / budget / loop / limit are all
    // failures (their answer text says so, and the receipt keeps the exact stop reason).
    let status = if run.stopped == "final" {
        "done"
    } else {
        "failed"
    };
    // Capture the files this run created (copied out to <home>/artifacts/<id>/ so they survive
    // worktree cleanup) while the worktree guard is still alive.
    let written = written_paths_since(&app.ledger, &run_workdir, start_seq);
    let output_files = capture_artifacts(&app.home, id, &run_workdir, &artifacts_before, &written);
    // Did THIS unattended run park an egress for approval? (checked before run.steps is moved)
    let staged_here = !attended
        && run
            .steps
            .iter()
            .any(|s| s.observation.contains("egress staged for review"));
    // If this ran in an isolated worktree, commit any edits to a durable `engram/task-<id>` branch
    // (the guard's Drop is a backstop) and tell the user how to merge — otherwise the edits to
    // EXISTING files would vanish with `git worktree remove --force`.
    let mut answer = run.answer;
    if run_workdir != app.workdir {
        if let Some(branch) = preserve_worktree_changes(&run_workdir, id) {
            answer.push_str(&format!(
                "\n\n---\n_Edits from this run were committed to branch `{branch}` — review with `git diff ..{branch}` and apply with `git merge {branch}`._"
            ));
        }
    }
    let receipt = tasks::TaskRun {
        answer,
        steps: run.steps,
        stopped: run.stopped.to_string(),
        tokens_in: after.tokens_in.saturating_sub(before.tokens_in),
        tokens_out: after.tokens_out.saturating_sub(before.tokens_out),
        cost_usd: (after.cost_usd - before.cost_usd).max(0.0),
        ledger_head_hash: head,
        started_ms,
        finished_ms,
        output_files,
    };
    let result = app
        .tasks
        .finish(id, receipt, status)
        .ok_or_else(|| "task vanished".to_string());
    app.bus.emit(Spike::new(
        "task.done",
        Priority::Normal,
        json!({ "id": id, "status": status }),
    ));
    // Fire any user-configured task.done hooks (Claude-Code-style automation), best-effort — the
    // event payload is piped to each hook command as JSON. Empty hooks list = no-op.
    let hooks = app.cfg().hooks.clone();
    if !hooks.is_empty() {
        hooks::run_hooks(
            &hooks,
            "task.done",
            &json!({ "id": id, "status": status, "title": task.title }),
        )
        .await;
    }
    // Link this task to the egress destinations it parked (from the `agent.egress_staged` entries it
    // appended this run), so approving one of those destinations can re-run THIS task and let the
    // parked action complete — the "approve → it proceeds" loop. [16]
    if staged_here {
        let tail = app.ledger.tail(400).unwrap_or_default();
        let mut linked = std::collections::HashSet::new();
        for e in tail
            .iter()
            .filter(|e| e.seq > start_seq && e.kind == "agent.egress_staged")
        {
            if let Ok(p) = serde_json::from_str::<Value>(e.payload.get()) {
                if let Some(dest) = p
                    .get("dest")
                    .and_then(|v| v.as_str())
                    .filter(|d| !d.is_empty() && *d != "(opaque)")
                {
                    if linked.insert(dest.to_string()) {
                        let scope = p.get("scope").and_then(|v| v.as_str()).unwrap_or("");
                        let _ = app.ledger.append(
                            "task.staged_egress",
                            "core",
                            json!({ "task": id, "dest": dest, "scope": scope }),
                        );
                    }
                }
            }
        }
    }
    // Async approve-queue notify: an unattended run that parked an action tells the user out of band
    // (channel webhook), so they learn there's something to approve without watching the app.
    if staged_here {
        let pending = pending_from_entries(&app.ledger.tail(500).unwrap_or_default());
        let dests: Vec<String> = pending
            .iter()
            .filter_map(|p| p.get("dest").and_then(|v| v.as_str()).map(String::from))
            .take(3)
            .collect();
        let msg = format!(
            "Engram staged an action needing your approval (task: {}). {} pending{}. Open the app → Pending approvals.",
            task.title,
            pending.len(),
            if dests.is_empty() { String::new() } else { format!(": {}", dests.join(", ")) }
        );
        let url = app.cfg().channels.webhook_url.clone();
        tokio::spawn(async move {
            post_webhook(&url, &msg).await;
        });
    }
    result
}

/// Query for a task run: `?approved=1` carries the user's one-time "Approve & continue" so the run
/// may egress despite the trifecta. Defaults to false — a plain run stays gated.
#[derive(serde::Deserialize, Default)]
struct RunQuery {
    #[serde(default)]
    approved: bool,
}

async fn tasks_run(
    State(app): State<App>,
    Path(id): Path<String>,
    Query(q): Query<RunQuery>,
) -> ApiResult {
    let updated = run_task_core(&app, &id, None, q.approved, true) // user-initiated run → attended
        .await
        .map_err(ApiError)?;
    Ok(Json(serde_json::to_value(updated).map_err(err)?))
}

/// Run a task and STREAM each step as it happens (Server-Sent Events): a `step` event per tool
/// call with its args/observation/receipt, then a final `done` event with the persisted task -
/// the "watch the agent work" view. The agent runs in a spawned task feeding an mpsc channel.
async fn tasks_run_stream(
    State(app): State<App>,
    Path(id): Path<String>,
    Query(q): Query<RunQuery>,
) -> axum::response::sse::Sse<
    impl futures_core::Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>,
> {
    use axum::response::sse::{Event, KeepAlive, Sse};
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<serde_json::Value>();
    let (done_tx, done_rx) = tokio::sync::oneshot::channel::<Result<tasks::Task, String>>();
    let app2 = app.clone();
    let approved = q.approved;
    tokio::spawn(async move {
        let result = run_task_core(&app2, &id, Some(tx), approved, true).await; // streamed UI run → attended
        let _ = done_tx.send(result);
    });
    let stream = async_stream::stream! {
        while let Some(step) = rx.recv().await {
            yield Ok(Event::default().event("step").data(step.to_string()));
        }
        match done_rx.await {
            Ok(Ok(task)) => yield Ok(Event::default().event("done").data(serde_json::to_string(&task).unwrap_or_default())),
            Ok(Err(e)) => yield Ok(Event::default().event("error").data(json!({ "error": e }).to_string())),
            Err(_) => yield Ok(Event::default().event("error").data(json!({ "error": "run dropped" }).to_string())),
        }
    };
    Sse::new(stream).keep_alive(KeepAlive::default())
}

/// In-process scheduler: while the daemon is awake, fire due jobs by spawning a task and
/// running it. (On a sleeping zero-idle VPS the systemd timer wakes the core instead.)
/// Periodic memory consolidation - the "sleep" pass. Demotes warm memories that are stale AND
/// low-importance to the cold tier so the working set stays small and recall stays fast as the
/// brain grows. Runs while the daemon is awake; cheap and bounded. (consolidate() had no callers.)
/// Skill-sleep prune: soft-retire skills that were PROPOSED (inactive) but never adopted — no active
/// version, never run, and older than `older_than_ms`. Reversible (bytes kept) and ledgered. Only
/// invoked when autonomous distillation is enabled — the mechanism that creates such deadweight — so
/// a daemon that never opts in never has its skills touched.
fn prune_proposed_skills(app: &App, older_than_ms: u64) -> usize {
    let entries = match app.ledger.read_all() {
        Ok(e) => e,
        Err(_) => return 0,
    };
    let mut ran: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut install_ts: std::collections::HashMap<String, u64> = std::collections::HashMap::new();
    for e in &entries {
        let id = serde_json::from_str::<Value>(e.payload.get())
            .ok()
            .and_then(|v| v.get("id").and_then(Value::as_str).map(|s| s.to_string()));
        let Some(id) = id else { continue };
        match e.kind.as_str() {
            "skill.run" => {
                ran.insert(id);
            }
            "skill.install" | "skill.distill" => {
                install_ts.entry(id).or_insert(e.ts_ms);
            }
            _ => {}
        }
    }
    let now = engram_core::now_ms();
    let mut pruned = 0;
    for id in app.registry.skills().unwrap_or_default() {
        // Adopted (has an active version) or ever-run skills are never pruned.
        if app.registry.active_version(&id).ok().flatten().is_some() || ran.contains(&id) {
            continue;
        }
        // Age gate, so a freshly-proposed skill isn't retired before it's had a chance to be adopted.
        let old_enough = install_ts
            .get(&id)
            .map(|ts| now.saturating_sub(*ts) >= older_than_ms)
            .unwrap_or(false);
        if old_enough && app.registry.retire(&id, "skill-sleep").is_ok() {
            pruned += 1;
        }
    }
    pruned
}

/// Retry activation for up to `cap` proposed (inactive, enabled) skills that have gold to verify
/// against. The one-shot verify at distill time fails closed when the environment isn't ready
/// (shell off, sandbox missing); this sweep is how a proposal earns activation once it is. Every
/// verdict is signed into the ledger by verify_and_adopt itself; a "needs_shell"/"needs_approval"/
/// "rejected" verdict just leaves the skill proposed, so re-running is idempotent and cheap.
async fn reverify_proposed_skills(app: &App, cap: usize) {
    let backend = {
        let c = app.cfg();
        config::resolve_shell_backend(&c.security.shell_backend, &c.security.shell_target)
    };
    // Distilled code is unattended here — replay it with the tainted-source clamps on.
    let params = skill_run_params(app, backend.as_deref(), true);
    let mut tried = 0;
    for id in app.registry.skills().unwrap_or_default() {
        if tried >= cap {
            break;
        }
        if app.registry.active_version(&id).ok().flatten().is_some() {
            continue;
        }
        let has_gold = app
            .registry
            .accepted_runs(&id)
            .map(|r| !r.is_empty())
            .unwrap_or(false);
        if !has_gold {
            continue;
        }
        tried += 1;
        match engram_agent::verify_and_adopt(
            &app.registry,
            &id,
            "reverify-sweep",
            true,
            &params,
            Some(&app.halt),
        )
        .await
        {
            Ok(d) => {
                if d["decision"].as_str() == Some("adopted") {
                    tracing::info!(id = %id, "re-verify sweep: proposed skill earned activation");
                }
                let _ = app.ledger.append(
                    "skill.reflect",
                    "reverify-sweep",
                    json!({ "outcome": "reverify", "id": id, "decision": d }),
                );
            }
            Err(e) => tracing::debug!(id = %id, "re-verify sweep: {e}"),
        }
    }
}

fn spawn_consolidation_tick(app: App) {
    tokio::spawn(async move {
        // First pass shortly after boot, then hourly.
        tokio::time::sleep(Duration::from_secs(120)).await;
        loop {
            // 14 days of inactivity is the warm->cold threshold for a low-importance memory.
            match app.memory.consolidate(Duration::from_secs(14 * 24 * 3600)) {
                Ok(n) if n > 0 => tracing::info!(demoted = n, "memory consolidated (warm -> cold)"),
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e, "consolidation failed"),
            }
            // Repair rows that were written while the embedder was degraded (a gateway outage
            // that fell back to trigram): re-embed them now that the daemon's had time to
            // recover, so a transient blip doesn't leave a memory mis-ranked forever.
            match app.memory.reembed_flagged() {
                Ok(n) if n > 0 => tracing::info!(
                    fixed = n,
                    "re-embedded rows flagged during a degraded write"
                ),
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e, "reembed_flagged failed"),
            }
            // The third of the sleep-cycle triad: opt-in, conservative, reversible pruning of
            // memories that are ALREADY superseded (invisible to every recall path regardless) and
            // old enough nobody would plausibly need them. 180 days - deliberately longer than the
            // 14-day warm->cold window, since forgetting is a much bigger step than demoting.
            if app.cfg().security.auto_prune_memories {
                match app
                    .memory
                    .auto_prune(Duration::from_secs(180 * 24 * 3600), "core")
                {
                    Ok(n) if n > 0 => {
                        tracing::info!(pruned = n, "auto-pruned old superseded memories")
                    }
                    Ok(_) => {}
                    Err(e) => tracing::warn!(error = %e, "auto_prune failed"),
                }
            }
            // Skill-sleep: retire proposed-but-never-adopted skills (opt-in, same window as memory).
            if app.cfg().security.auto_distill_skills {
                let pruned = prune_proposed_skills(&app, 14 * 24 * 3600 * 1000);
                if pruned > 0 {
                    tracing::info!(pruned, "skill-sleep: retired proposed skills never adopted");
                }
                // Re-verify sweep: proposals whose activation failed ONCE used to be stuck forever
                // (e.g. distilled while the shell tool was off → "needs_shell" → dead). Conditions
                // change — shell enabled, sandbox installed — so each tick retries a few. Bounded,
                // and verify_and_adopt short-circuits already-active ids, so a clean state is free.
                reverify_proposed_skills(&app, 3).await;
            }
            // Phase D grounded reflection (opt-in, default off - docs/MEMORY-UPGRADE-PLAN.md §6):
            // synthesizes a higher-level fact from a small, bounded, co-scoped group of related
            // Trusted-only memories, citing its sources. Same 14-day staleness window as
            // consolidate() above - it scans the exact same candidate pool, not a separate one.
            // Never fires under the offline mock provider or over Untrusted-tainted memories.
            if app.cfg().security.auto_reflect {
                let n = reflection::run_tick(
                    &app.memory,
                    &app.gateway,
                    &app.model(),
                    Duration::from_secs(14 * 24 * 3600),
                )
                .await;
                if n > 0 {
                    tracing::info!(written = n, "grounded reflection: synthesized new facts");
                }
            }
            tokio::time::sleep(Duration::from_secs(3600)).await;
        }
    });
}

/// Keep the idle clock reset while any agent run is executing, so a run with no open HTTP connection
/// (scheduler / Telegram / a stream whose client disconnected) doesn't get killed when the idle window
/// elapses. Cheap: it only touches activity when a run is actually in flight; on a quiet box it does
/// nothing and the daemon still sleeps to zero as designed.
fn spawn_run_keepalive(app: App) {
    tokio::spawn(async move {
        loop {
            // Poll well inside the smallest sane idle window so activity never goes stale mid-run.
            tokio::time::sleep(Duration::from_secs(30)).await;
            if app.in_flight.load(std::sync::atomic::Ordering::SeqCst) > 0 {
                app.activity.touch();
            }
        }
    });
}

/// Build a task from a scheduled job's payload. Honors `detail`/`prompt` (the instructions the
/// agent actually runs — previously scheduled jobs ran on the bare title) and `agent` (the durable
/// agent whose SIGNED autonomy policy governs this UNATTENDED run). Without an agent the run has no
/// policy, so every egress stages for review — which is why the flagship "runs for days" path needs
/// a job to carry an agent. Returns the created (and agent-assigned) task.
/// Derive a scheduled task run's (title, detail) from the job's payload + its short `name`. Factored
/// out of `task_from_schedule` so this — the actual title/detail split — is unit-testable without a
/// full `App`.
///
/// The task board's card headline IS `title` — show the job's short NAME there (`fallback_name`, the
/// same string as `Job.name`), not the full instructions, or every card from a recurring job (e.g. a
/// daily digest) reads as a wall of prompt text instead of a scannable label. The actual instructions
/// (what the UI puts in `payload.title`, falling back to `payload.prompt`) move into `detail` instead —
/// `run_task_core` already concatenates title+detail into the run prompt, so the agent still receives
/// the full instructions unchanged; only the DISPLAYED title shortens.
fn schedule_task_fields(payload: &Value, fallback_name: &str) -> (String, String) {
    let instructions = payload
        .get("title")
        .or_else(|| payload.get("prompt"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or(fallback_name)
        .to_string();
    let extra_detail = payload.get("detail").and_then(|v| v.as_str()).unwrap_or("");
    let detail = if instructions == fallback_name {
        // No separate instructions were ever set — nothing to move into detail beyond what the UI
        // may have stored there directly (matches the pre-fix behavior for this edge case).
        extra_detail.to_string()
    } else if extra_detail.is_empty() {
        instructions.clone()
    } else {
        format!("{instructions}\n\n{extra_detail}")
    };
    (fallback_name.to_string(), detail)
}

fn task_from_schedule(app: &App, payload: &Value, fallback_name: &str) -> tasks::Task {
    let (title, detail) = schedule_task_fields(payload, fallback_name);
    let task = app.tasks.create(title, detail, "schedule".into());
    if let Some(agent) = payload
        .get("agent")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
    {
        if let Some(updated) = app.tasks.set_agent(&task.id, Some(agent.to_string())) {
            return updated;
        }
    }
    task
}

fn spawn_scheduler_tick(app: App) {
    tokio::spawn(async move {
        // Bound how many scheduled jobs run at once so a burst of due jobs can't flood the box, while
        // still letting them run CONCURRENTLY — one slow job (a long browser task, a hung provider)
        // must not delay every other due job and the next tick behind it.
        let sem = Arc::new(tokio::sync::Semaphore::new(4));
        loop {
            tokio::time::sleep(Duration::from_secs(30)).await;
            let now = chrono::Utc::now();
            let due = app.sched.due(now);
            // Touch activity when work is due: a daemon that OWNS pending schedules must not sleep to
            // zero (it would stop firing until the next inbound HTTP). The scheduler is the unattended
            // surface, so its own liveness keeps the core awake.
            if !due.is_empty() {
                app.activity.touch();
            }
            for job in due {
                let task = task_from_schedule(&app, &job.payload, &job.name);
                tracing::info!(job = %job.name, task = %task.id, "scheduler firing a task");
                // Record the task on the job before running so a crash mid-run still leaves a
                // pointer to the (failed) receipt for the UI's "last run" affordance.
                let _ = app.sched.set_last_task(&job.id, &task.id);
                // Mark fired BEFORE running: the occurrence is consumed once. Previously mark_fired ran
                // only AFTER the run returned, so a daemon death mid-run (idle-exit, restart, crash)
                // left next_fire_ms in the past and the job re-fired on next boot — duplicate task
                // cards and duplicate side effects (e.g. the morning digest sent twice) for one
                // scheduled occurrence.
                let _ = app.sched.mark_fired(&job.id, now);
                // Spawn each run on its own task under the concurrency bound so one long job cannot
                // starve the schedule (the whole point of an unattended cron surface is punctuality).
                let app_run = app.clone();
                let sem = sem.clone();
                let tid = task.id.clone();
                tokio::spawn(async move {
                    let _permit = sem.acquire().await;
                    let _ = run_task_core(&app_run, &tid, None, false, false).await;
                });
            }
        }
    });
}

/// The signed ledger slice for a task's run - the glass-box audit trail behind a card.
async fn task_audit(State(app): State<App>, Path(id): Path<String>) -> ApiResult {
    let task = app
        .tasks
        .get(&id)
        .ok_or_else(|| ApiError("task not found".into()))?;
    let Some(run) = &task.run else {
        return Ok(Json(json!({ "entries": [] })));
    };
    let entries: Vec<_> = app
        .ledger
        .read_all()
        .map_err(err)?
        .into_iter()
        .filter(|e| {
            let ts = e.ts_ms as i64;
            ts >= run.started_ms && ts <= run.finished_ms + 5
        })
        .collect();
    Ok(Json(
        json!({ "entries": entries, "head": run.ledger_head_hash }),
    ))
}

/// The ledger's public key, for offline verification (`engramd verify`) by a third party.
async fn ledger_pubkey(State(app): State<App>) -> ApiResult {
    Ok(Json(
        json!({ "pubkey": app.ledger.pubkey_hex(), "alg": "ed25519" }),
    ))
}

/// A self-contained, independently-verifiable receipt for one task run: the answer, each
/// step with the exact signed ledger seq+hash it produced, those ledger entries, and the
/// public key + verify command - so anyone can confirm the run happened as claimed without
/// trusting this machine.
async fn task_receipt(State(app): State<App>, Path(id): Path<String>) -> ApiResult {
    let task = app
        .tasks
        .get(&id)
        .ok_or_else(|| ApiError("task not found".into()))?;
    let run = task
        .run
        .clone()
        .ok_or_else(|| ApiError("task has no run yet".into()))?;
    let seqs: std::collections::HashSet<u64> = run
        .steps
        .iter()
        .map(|s| s.ledger_seq)
        .filter(|&s| s > 0)
        .collect();
    let by_seq: std::collections::HashMap<u64, String> = app
        .ledger
        .read_all()
        .map_err(err)?
        .into_iter()
        .filter(|e| seqs.contains(&e.seq))
        .map(|e| (e.seq, e.hash))
        .collect();
    // Actually bind the receipt to the ledger: every step's recorded hash must equal the
    // hash of the entry at its seq, or the receipt is flagged inconsistent.
    let consistent = run
        .steps
        .iter()
        .filter(|s| s.ledger_seq > 0)
        .all(|s| by_seq.get(&s.ledger_seq) == Some(&s.ledger_hash));
    let entries: Vec<_> = by_seq
        .into_iter()
        .map(|(seq, hash)| json!({ "seq": seq, "hash": hash }))
        .collect();
    Ok(Json(json!({
        "task": { "id": task.id, "title": task.title },
        "answer": run.answer,
        "stopped": run.stopped,
        "steps": run.steps,
        "ledger_head": run.ledger_head_hash,
        "ledger_entries": entries,
        "steps_match_ledger": consistent,
        "pubkey": app.ledger.pubkey_hex(),
        "alg": "ed25519",
        "verify": "engramd verify <ENGRAM_HOME>",
        "note": "steps_match_ledger confirms each step's hash equals its signed ledger entry; run \
                 the verify command with the published pubkey to confirm the whole chain offline."
    })))
}

#[derive(Deserialize)]
struct ConverseReq {
    text: String,
    /// When set, the turn is appended to this chat session so it survives a reload.
    #[serde(default)]
    session: Option<String>,
    /// Context the user pinned in the composer (attached files, URLs, pinned memories).
    /// Surfaced to the model as a system message before the user's turn.
    #[serde(default)]
    attachments: Vec<converse::Attachment>,
}

async fn converse_handler(State(app): State<App>, Json(r): Json<ConverseReq>) -> ApiResult {
    let persona = r
        .session
        .as_ref()
        .and_then(|sid| app.workspace.persona_for_session(sid));
    let scope = r
        .session
        .as_ref()
        .map(|sid| app.workspace.scope_for_session(sid))
        .unwrap_or_else(engram_core::ScopeCtx::user_only);
    let turn = converse::converse(
        &app.memory,
        &app.gateway,
        &r.text,
        &app.model(),
        persona.as_deref(),
        &r.attachments,
        &scope,
    )
    .await
    .map_err(ApiError)?;
    if let Some(sid) = &r.session {
        app.workspace.append_turn(
            sid,
            &r.text,
            &turn.reply,
            turn.recalled.clone(),
            turn.recalled_refs
                .iter()
                .map(|rf| serde_json::to_value(rf).unwrap_or_default())
                .collect(),
            turn.learned.clone(),
        );
    }
    Ok(Json(json!({
        "reply": turn.reply,
        "recalled": turn.recalled,
        "recalled_refs": turn.recalled_refs,
        "learned": turn.learned,
    })))
}

/// Streaming converse: the reply streams to the chat token-by-token as Server-Sent Events
/// (`token` events), then a final `done` event carries the recalled/learned metadata. A
/// push-to-pull bridge: the model deltas are pushed into a channel and the SSE response
/// pulls from it.
async fn converse_stream_handler(
    State(app): State<App>,
    Json(r): Json<ConverseReq>,
) -> axum::response::sse::Sse<
    impl futures_core::Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>,
> {
    use axum::response::sse::{Event, KeepAlive, Sse};
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Event>();
    tokio::spawn(async move {
        // Agentic chat: the LOCAL user's chat is trusted, so it runs the SAME tool-using agent the
        // task board does - it can browse, run shell, read/write files, see images, recall memory -
        // and just answers when no tool is needed. (Channels stay on the tool-less converse path:
        // an untrusted inbound message must never drive the shell/browser.)
        // Memory features the conversational path gave are preserved: the grounding ribbon and
        // identity learning; the agent's own flywheel recalls + captures the exchange to memory.
        let ribbon_scope = r
            .session
            .as_ref()
            .map(|sid| app.workspace.scope_for_session(sid))
            .unwrap_or_else(engram_core::ScopeCtx::user_only);
        let (recalled, recalled_refs) =
            converse::recall_ribbon(&app.memory, &r.text, &ribbon_scope);
        let learned =
            converse::learn_identity(&app.memory, &app.gateway, &app.model(), &r.text).await;
        // Conversation continuity: hand the agent the recent turns so a follow-up ("let's try again")
        // resolves against what was already said, instead of re-asking for context it already has.
        let history = r
            .session
            .as_ref()
            .map(|sid| app.workspace.recent_turns(sid, 10))
            .unwrap_or_default();
        let mut task = String::new();
        // Chat behaviour directive (this is the live, trusted user chat). Be a proactive partner like
        // a sharp colleague: surface clarifying questions when they'd materially change the result,
        // but never stall on them — proceed with best-effort defaults in the same turn. Research in
        // parallel where possible, and present a clean deliverable (brief intro → tables with real
        // links → a concrete next step), never internal verification notes.
        task.push_str(
            "[Chat mode] You are Engram, talking live with your user. Be proactive and concrete. \
             If the request is ambiguous or missing details that would materially change the result \
             (e.g. dates, budget, one-way vs round-trip, scope), ask 1-3 crisp clarifying questions \
             UP FRONT — but in the SAME turn still kick off the work with sensible default \
             assumptions and state them, rather than stalling. When researching multiple things, run \
             the searches in parallel (delegate sub-tasks). Present the result cleanly: a short \
             intro, then well-formatted tables with real working links, and end with a concrete \
             next step you can take. Never show internal verification checklists or meta-notes.\n\
             GROUNDING (critical): only state facts — prices, links, schedules, availability — that \
             you ACTUALLY retrieved with a tool this turn. Never invent or guess values, and never \
             output a link you didn't get from a real result. If a source is blocked, rate-limited, \
             or you couldn't verify something, say so plainly and give the official site to check, \
             rather than fabricating a number or URL.\n\
             ACT, DON'T NARRATE: never end your turn having only DESCRIBED what you would do. If one \
             site blocks automated access (Amazon, many aggregators do — you'll get a CAPTCHA/robot \
             page), immediately try an accessible alternative IN THE SAME TURN (web_search for a \
             current roundup, then web_fetch/browser the specific result pages) and return the real \
             table. Use the tools now; only stop when you've delivered the result or have genuinely \
             exhausted the accessible options (then say exactly what you tried).\n\
             REUSE & SKILLS: if you already gathered a fact earlier in THIS conversation, use it — do \
             NOT re-run the same search (it burns the run budget and invites rate-limits). For a task \
             a built-in skill covers (flights → flight_search; plus weather, currency, wikipedia, \
             etc.), reach for skill_search / skill_run before raw web scraping. To SAVE a document or \
             webpage for the user, write it to a file with write_file (and append_file to add further \
             parts if it is long) — don't paste a huge page inline.\n\
             SPEED: each model step is slow, so do as much as possible per step. When you need \
             several web searches, pass them ALL to ONE web_search call as a `queries` array (they \
             run concurrently) instead of firing them one at a time across many turns — this is the \
             single biggest thing that makes a run fast.\n\
             TOOL CHOICE: do NOT drive the browser to click through flight/booking sites (Google \
             Flights, Skyscanner, Kayak, Momondo) — they are slow JS apps that block bots, so \
             clicking element-by-element wastes minutes. For flight prices use the flight_search \
             skill; for everything else use web_search to find sources then web_fetch to read them \
             (web_fetch falls back to a reader that handles JS pages). Reach for the interactive \
             browser ONLY as a last resort for one specific page that has no API, skill, or feed.\n\n",
        );
        if !history.is_empty() {
            task.push_str("You are mid-conversation. Here is what was said so far - use it; do NOT re-ask for context you already have:\n");
            for (role, text) in &history {
                let who = if role == "user" { "User" } else { "You" };
                task.push_str(&format!("{who}: {text}\n"));
            }
            task.push('\n');
        }
        // Fold any pinned attachments (files, URLs, memories) into the task so the agent sees them.
        if let Some(ctx) = converse::attachments_context(&r.attachments) {
            task.push_str(&ctx);
            task.push_str("\n\n");
        }
        task.push_str(&format!("User's latest message: {}", r.text));
        // PERSIST THE USER TURN NOW, before the agent runs — so if the app is closed or the task is
        // interrupted, the posted message survives on reopen (it was the user's #1 complaint). The
        // reply is appended on completion. (history was read above, so it excludes this message.)
        if let Some(sid) = &r.session {
            app.workspace.append_user_turn(sid, &r.text);
        }
        // Snapshot the workdir so files this turn creates (e.g. a browser screenshot) are captured as
        // downloadable artifacts in the gallery, bucketed under this chat session. Snapshot the
        // PROJECT's workdir when it has one, so artifacts created there are still captured.
        let art_bucket = r.session.clone().unwrap_or_else(|| "chat".to_string());
        let run_workdir = r
            .session
            .as_ref()
            .and_then(|sid| app.workspace.workdir_for_session(sid))
            .unwrap_or_else(|| app.workdir.clone());
        let artifacts_before = snapshot_files(&run_workdir);
        // So capture_artifacts can force-include a file this turn OVERWRITES (e.g. the same session
        // asks the agent to update the same report twice) — see written_paths_since.
        let chat_start_seq = app.ledger.head().0;
        // Stream each tool step live as it lands - the glass box, in chat.
        let txs = tx.clone();
        let on_step: engram_agent::StepCallback = std::sync::Arc::new(
            move |i, rec: &engram_agent::StepRecord| {
                let obs: String = rec.observation.chars().take(600).collect();
                let _ = txs.send(
                    Event::default().event("step").data(
                        json!({ "index": i, "tool": rec.tool, "ok": rec.ok, "observation": obs, "args": rec.args })
                            .to_string(),
                    ),
                );
            },
        );
        // Stream the model's interim commentary ("I've kicked off two searches…") so the user sees
        // what it's doing live instead of a silent wait that jumps to the final answer. Also collect
        // the notes so they can be PERSISTED with the reply — otherwise the narration (and the whole
        // glass-box trail) evaporates on reload, degrading the transcript to bare Q&A.
        let txn = tx.clone();
        let notes_collected = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let notes_cb = notes_collected.clone();
        let on_narration: engram_agent::NarrationCallback =
            std::sync::Arc::new(move |note: &str| {
                let note: String = note.chars().take(600).collect();
                if let Ok(mut g) = notes_cb.lock() {
                    g.push(note.clone());
                }
                let _ = txn.send(
                    Event::default()
                        .event("narration")
                        .data(json!({ "text": note }).to_string()),
                );
            });
        // Per-session halt: register before the run so `/v1/halt {session}` stops THIS chat only,
        // then deregister after — so concurrent chats run independently and Stop targets just one.
        // Key by a UNIQUE run id (session + counter), not the bare session id: a user can send a
        // second message in the same session while the first run is still going, and a bare-session
        // key made the second insert overwrite the first run's flag (so the first kept checking an
        // orphaned Arc and its Stop button no-op'd). `/v1/halt {session}` flips every run under that
        // session (see `halt_key_matches`), and each run removes only its own exact key.
        let run_halt = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let halt_key = r.session.clone().map(|sid| {
            format!(
                "{sid}#{}",
                RUN_HALT_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            )
        });
        if let Some(key) = halt_key.clone() {
            if let Ok(mut g) = app.run_halts.lock() {
                g.insert(key, run_halt.clone());
            }
        }
        // The chat's scope: user-global ∪ this session's project ∪ this session. This is what keeps
        // one project's memories and captures out of another project's chats.
        let chat_scope = r
            .session
            .as_ref()
            .map(|sid| app.workspace.scope_for_session(sid))
            .unwrap_or_else(engram_core::ScopeCtx::user_only);
        // The active project's working directory, if it has one: file/shell tools this turn run
        // (and are confined to) there, so a project's agent acts on that project's files - not the
        // shared workdir. `None` keeps the shared workdir (back-compat).
        let chat_workdir = r
            .session
            .as_ref()
            .and_then(|sid| app.workspace.workdir_for_session(sid));
        let res = run_agent_task_cb(
            &app,
            &task,
            24,
            engram_core::Taint::Trusted,
            false,
            Some(on_step),
            Some(on_narration),
            None,
            chat_workdir,
            false, // approved
            true,  // attended (interactive streaming conversation)
            run_halt,
            chat_scope,
        )
        .await;
        if let Some(key) = &halt_key {
            if let Ok(mut g) = app.run_halts.lock() {
                g.remove(key); // remove only THIS run's flag, not any sibling run in the same session
            }
        }
        // The narration notes collected across the run, for persistence in either arm.
        let collected_notes = notes_collected
            .lock()
            .map(|g| g.clone())
            .unwrap_or_default();
        match res {
            Ok(run) => {
                if let Some(sid) = &r.session {
                    // The user turn was already persisted up-front; append only the reply now — WITH the
                    // glass-box trail (tool steps + narration), serialized in the same shape the live
                    // `step`/`done` events use, so a reloaded answer keeps its step chips, inline
                    // screenshots, and clickable "wrote a file ↗" affordances instead of degrading to
                    // bare Q&A.
                    let steps_json: Vec<serde_json::Value> = run
                        .steps
                        .iter()
                        .map(|s| serde_json::to_value(s).unwrap_or_default())
                        .collect();
                    app.workspace.append_reply_turn(
                        sid,
                        &run.answer,
                        recalled.clone(),
                        recalled_refs
                            .iter()
                            .map(|rf| serde_json::to_value(rf).unwrap_or_default())
                            .collect(),
                        learned.clone(),
                        steps_json,
                        collected_notes.clone(),
                    );
                    // Tell any other connected view (a second window, a reloaded tab whose fetch
                    // raced the run) that this session got a reply, so it refetches immediately
                    // instead of waiting for the periodic poll.
                    app.bus.emit(Spike::new(
                        "session.reply",
                        Priority::Normal,
                        json!({ "id": sid }),
                    ));
                }
                // Capture any files this turn produced into the gallery (under the session bucket).
                let written = written_paths_since(&app.ledger, &run_workdir, chat_start_seq);
                let _ = capture_artifacts(
                    &app.home,
                    &art_bucket,
                    &run_workdir,
                    &artifacts_before,
                    &written,
                );
                let _ = tx.send(Event::default().event("done").data(
                    json!({ "reply": run.answer, "recalled": recalled, "recalled_refs": recalled_refs, "learned": learned, "steps": run.steps })
                        .to_string(),
                ));
            }
            Err(e) => {
                // Persist the failure as a reply too — the user turn was already saved up-front, so
                // without this a stopped/errored run left the chat showing the question with no answer,
                // which reads as "the chat vanished" after reopening the app. Keep whatever narration
                // was collected so the partial trail survives.
                if let Some(sid) = &r.session {
                    app.workspace.append_reply_turn(
                        sid,
                        &format!("⚠️ This run didn't finish: {e}"),
                        recalled.clone(),
                        recalled_refs
                            .iter()
                            .map(|rf| serde_json::to_value(rf).unwrap_or_default())
                            .collect(),
                        learned.clone(),
                        Vec::new(),
                        collected_notes.clone(),
                    );
                    app.bus.emit(Spike::new(
                        "session.reply",
                        Priority::Normal,
                        json!({ "id": sid }),
                    ));
                }
                // Capture any files the (errored) run wrote before it failed — otherwise a run that
                // produced files but then errored leaves those files unreachable from the UI forever.
                let written = written_paths_since(&app.ledger, &run_workdir, chat_start_seq);
                let _ = capture_artifacts(
                    &app.home,
                    &art_bucket,
                    &run_workdir,
                    &artifacts_before,
                    &written,
                );
                let _ = tx.send(Event::default().event("error").data(e));
            }
        }
    });
    let stream = async_stream::stream! {
        while let Some(ev) = rx.recv().await {
            yield Ok(ev);
        }
    };
    Sse::new(stream).keep_alive(KeepAlive::default())
}

#[derive(Deserialize)]
struct UploadReq {
    name: String,
    /// Bare base64 (no data: prefix) of the file bytes.
    content_b64: String,
    #[serde(default)]
    mime: Option<String>,
    /// The chat session the upload belongs to. When set, the document's text is chunked and
    /// ingested into that session's project as scoped, retrievable memory (so it can be recalled in
    /// later turns), not just attached to this one turn.
    #[serde(default)]
    session: Option<String>,
}

/// Extract readable text from an uploaded document (PDF / DOCX / XLSX / CSV / plain text) so the
/// agent can actually read it. Returns `None` for an unknown/binary type or when the `docs` feature
/// is off (the file is still stored; only text extraction is gated). Output is capped by the caller.
/// Extract document text in an ISOLATED subprocess (a re-exec of this binary with `--extract-doc`),
/// so a panic inside a third-party parser (pdf-extract/calamine/zip) — which aborts the whole daemon
/// under `panic="abort"` — only kills the short-lived child. Bounded by a deadline so a pathological
/// parser can't hang the request forever. Returns None on crash, timeout, spawn failure, or no text.
async fn extract_document_text_isolated(name: &str, bytes: &[u8]) -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    let mut child = tokio::process::Command::new(exe)
        .arg("--extract-doc")
        .arg(name)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .ok()?;
    let mut stdin = child.stdin.take()?;
    let payload = bytes.to_vec();
    // Feed stdin from a separate task so a large extracted output filling the stdout pipe can't
    // deadlock against our write (classic pipe deadlock); dropping stdin signals EOF to the child.
    let writer = tokio::spawn(async move {
        use tokio::io::AsyncWriteExt;
        let _ = stdin.write_all(&payload).await;
        let _ = stdin.shutdown().await;
    });
    let out = tokio::time::timeout(Duration::from_secs(45), child.wait_with_output()).await;
    let _ = writer.await;
    match out {
        Ok(Ok(o)) if o.status.success() => {
            let text = String::from_utf8_lossy(&o.stdout).into_owned();
            (!text.trim().is_empty()).then_some(text)
        }
        // Non-zero exit (3 = no text, or the child crashed on a hostile file), timeout, or wait error.
        Ok(Ok(_)) => None,
        Ok(Err(_)) => None,
        Err(_) => {
            tracing::warn!(doc = %name, "document extraction timed out in the isolated parser");
            None
        }
    }
}

fn extract_document_text(name: &str, bytes: &[u8]) -> Option<String> {
    let lower = name.to_lowercase();
    // Plain-text-ish formats are just UTF-8 (handled even without the docs feature).
    if lower.ends_with(".txt")
        || lower.ends_with(".md")
        || lower.ends_with(".csv")
        || lower.ends_with(".tsv")
        || lower.ends_with(".json")
        || lower.ends_with(".log")
        || lower.ends_with(".yml")
        || lower.ends_with(".yaml")
    {
        return Some(String::from_utf8_lossy(bytes).into_owned());
    }
    #[cfg(feature = "docs")]
    {
        // Cap the text accumulated DURING extraction (not just the final output) so a decompression
        // bomb - a tiny compressed XLSX/DOCX that inflates to gigabytes - cannot exhaust memory
        // before the caller's 600KB output cap is applied. A file crafted to make pdf-extract or
        // calamine itself panic aborts THIS process under `panic = "abort"` - which is why the
        // daemon only ever calls this via `extract_document_text_isolated` (a re-exec'd child), so
        // such a crash kills the child, not the daemon. (Reached directly only in the `--extract-doc`
        // child and in unit tests.)
        const EXTRACT_CAP: usize = 8 * 1024 * 1024;
        if lower.ends_with(".pdf") {
            return pdf_extract::extract_text_from_mem(bytes)
                .ok()
                .filter(|s| !s.trim().is_empty());
        }
        if lower.ends_with(".xlsx") || lower.ends_with(".xls") || lower.ends_with(".ods") {
            use calamine::Reader;
            let cur = std::io::Cursor::new(bytes.to_vec());
            if let Ok(mut wb) = calamine::open_workbook_auto_from_rs(cur) {
                let mut out = String::new();
                'sheets: for s in wb.sheet_names().to_vec() {
                    if let Ok(range) = wb.worksheet_range(&s) {
                        out.push_str(&format!("# Sheet: {s}\n"));
                        for row in range.rows() {
                            let line: Vec<String> = row.iter().map(|c| c.to_string()).collect();
                            out.push_str(&line.join("\t"));
                            out.push('\n');
                            if out.len() > EXTRACT_CAP {
                                break 'sheets; // stop before a bomb inflates without bound
                            }
                        }
                    }
                }
                return Some(out).filter(|s| !s.trim().is_empty());
            }
        }
        if lower.ends_with(".docx") {
            // A .docx is a zip; the body lives in word/document.xml. Read at most EXTRACT_CAP of the
            // DECOMPRESSED entry (Read::take) so a zip bomb cannot inflate without bound, then strip
            // tags, turning paragraph/break tags into newlines.
            use std::io::Read;
            if let Ok(mut zip) = zip::ZipArchive::new(std::io::Cursor::new(bytes.to_vec())) {
                if let Ok(f) = zip.by_name("word/document.xml") {
                    let mut buf = Vec::new();
                    if f.take(EXTRACT_CAP as u64).read_to_end(&mut buf).is_ok() {
                        let xml = String::from_utf8_lossy(&buf)
                            .replace("</w:p>", "\n")
                            .replace("<w:br/>", "\n");
                        let mut text = String::with_capacity(xml.len() / 2);
                        let mut in_tag = false;
                        for ch in xml.chars() {
                            match ch {
                                '<' => in_tag = true,
                                '>' => in_tag = false,
                                c if !in_tag => text.push(c),
                                _ => {}
                            }
                        }
                        return Some(text).filter(|s| !s.trim().is_empty());
                    }
                }
            }
        }
    }
    #[cfg(not(feature = "docs"))]
    let _ = bytes;
    None
}

/// Truncate `t` to at most `cap` bytes on a UTF-8 char boundary, appending a marker when it cuts.
/// `String::truncate` panics on a byte index that lands mid-codepoint, so extracted document text
/// (which can contain multibyte UTF-8 past the cap) must be trimmed back to the nearest boundary.
fn cap_text_on_boundary(mut t: String, cap: usize) -> String {
    if t.len() > cap {
        let mut end = cap;
        while end > 0 && !t.is_char_boundary(end) {
            end -= 1;
        }
        t.truncate(end);
        t.push_str("\n...[document truncated]");
    }
    t
}

/// Store an uploaded (typically binary) file under `<home>/uploads` and return a ref the
/// chat composer can attach to a turn. The filename is sanitized to a basename plus a short
/// nanos prefix, so a hostile `name` can't traverse out of the uploads dir. For documents
/// (PDF/DOCX/XLSX/CSV) the readable text is extracted and returned so the agent can read them.
async fn upload_handler(State(app): State<App>, Json(r): Json<UploadReq>) -> ApiResult {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(r.content_b64.as_bytes())
        .map_err(err)?;
    if bytes.len() > 25 * 1024 * 1024 {
        return Err(ApiError("file too large (25MB max)".into()));
    }
    let base = std::path::Path::new(&r.name)
        .file_name()
        .and_then(|s| s.to_str())
        .map(|s| {
            s.replace(
                |c: char| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')),
                "_",
            )
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "file".to_string());
    let dir = std::path::Path::new(&app.home).join("uploads");
    std::fs::create_dir_all(&dir).map_err(err)?;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let stored = format!("{:x}-{}", nanos, base);
    std::fs::write(dir.join(&stored), &bytes).map_err(err)?;
    // Extract readable text for documents so the agent can actually read them (capped so a huge
    // PDF can't blow the context). The UI attaches this text to the turn. The (synchronous,
    // CPU-heavy) third-party parsers run in an ISOLATED subprocess — a panic on a hostile/malformed
    // file (which aborts the process under panic=abort) only kills that child, not the daemon.
    let extracted = extract_document_text_isolated(&base, &bytes)
        .await
        .map(|t| cap_text_on_boundary(t, 600_000));
    // If the upload belongs to a chat session, ALSO ingest the text into that session's project as
    // scoped, retrievable memory - so the document persists as a first-class part of the project's
    // corpus (recallable in later turns), not just this one turn's attachment. Project-scoped by
    // construction, so one project's documents never surface in another.
    let mut ingested_chunks = 0usize;
    if let (Some(sid), Some(text)) = (r.session.as_ref(), extracted.as_ref()) {
        let write_scope = app.workspace.scope_for_session(sid).durable_write_scope();
        ingested_chunks = corpus::ingest_document(&app.memory, &base, text, &write_scope);
    }
    Ok(Json(json!({
        "ref": stored,
        "name": base,
        "size": bytes.len(),
        "mime": r.mime.unwrap_or_else(|| "application/octet-stream".into()),
        "extracted_text": extracted,
        "ingested_chunks": ingested_chunks,
    })))
}

async fn skills(State(app): State<App>) -> ApiResult {
    // The signed learning history: every replay→A/B→promote/reject decision, with its real
    // before/after scores. This is the honest expertise signal - shown verbatim, never inferred.
    let learn: Vec<(Value, u64, String)> = app
        .ledger
        .read_all()
        .map_err(err)?
        .into_iter()
        .filter(|e| e.kind == "skill.learn")
        .filter_map(|e| {
            serde_json::from_str::<Value>(e.payload.get())
                .ok()
                .map(|v| (v, e.seq, e.hash))
        })
        .collect();
    let mut out = Vec::new();
    // List ALL skills (incl. disabled) so the UI can show a disabled skill greyed with an on/off
    // toggle; the `enabled` flag distinguishes them. Selection/auto-use still uses `skills()`.
    for id in app.registry.skills_all().map_err(err)? {
        let enabled = !app.registry.is_retired(&id);
        let active = app.registry.active_version(&id).map_err(err)?;
        let versions = app.registry.versions(&id).map_err(err)?;
        // A PROPOSED skill: distilled (or authored) but not yet activated. It exists on disk with
        // versions but no active pointer — the UI shows it with an "Adopt" action. Independent of
        // `enabled`: a disabled proposal is still a proposal (hiding it made distilled skills
        // vanish from the UI entirely once toggled off, with no way back).
        let proposed = active.is_none() && !versions.is_empty();
        // The gold-signal size: recorded (input, accepted-output) pairs a candidate is scored
        // against. Zero means there is no scored signal yet - the UI must say "unverified".
        let runs = app
            .registry
            .accepted_runs(&id)
            .map(|r| r.len())
            .unwrap_or(0);
        let events: Vec<Value> = learn
            .iter()
            .filter(|(v, _, _)| v.get("id").and_then(Value::as_str) == Some(id.as_str()))
            .map(|(v, seq, hash)| {
                let mut o = v.clone();
                o["seq"] = json!(seq);
                o["hash"] = json!(hash);
                o
            })
            .collect();
        // Surface the manifest so the UI can label a skill (a process/Python skill the agent authored
        // vs. a WASM transform) and show what it does + which capabilities it holds. Prefer the active
        // version; fall back to the LATEST version so a proposed (inactive) skill still shows its real
        // description/runtime instead of blanks.
        let manifest_version = active.or_else(|| versions.iter().max().copied());
        let (runtime, interpreter, description, when_to_use, capabilities, category) =
            match manifest_version.and_then(|v| app.registry.load(&id, v).ok()) {
                Some((signed, _)) => {
                    let m = signed.manifest;
                    (
                        if m.runtime == engram_skills::Runtime::Process {
                            "process"
                        } else {
                            "wasm"
                        },
                        m.interpreter,
                        m.description,
                        m.when_to_use,
                        m.capabilities
                            .iter()
                            .map(|c| c.as_str())
                            .collect::<Vec<_>>(),
                        m.category,
                    )
                }
                None => ("wasm", None, String::new(), None, vec![], String::new()),
            };
        out.push(
            json!({ "id": id, "active": active, "versions": versions, "runs": runs,
            "runtime": runtime, "interpreter": interpreter, "description": description,
            "when_to_use": when_to_use, "capabilities": capabilities, "category": category,
            "learn": events, "enabled": enabled, "proposed": proposed }),
        );
    }
    Ok(Json(json!({ "skills": out })))
}

fn valid_skill_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}
fn valid_skill_interp(s: &str) -> bool {
    let s = s.trim();
    !s.is_empty()
        && s.len() <= 64
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '/' | '.' | '_' | '-'))
}

#[derive(Deserialize)]
struct SkillCreateReq {
    id: String,
    #[serde(default)]
    interpreter: String,
    source: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    when_to_use: String,
    #[serde(default)]
    category: String,
    #[serde(default)]
    capabilities: Vec<String>,
}

/// Upload (author) a new Process skill from source — the user-facing "add your own skill". Installs a
/// signed Process skill; the interpreter is sanitized (it is interpolated into the sandbox command).
async fn skill_create(State(app): State<App>, Json(r): Json<SkillCreateReq>) -> ApiResult {
    let id = r.id.trim().to_string();
    if !valid_skill_id(&id) {
        return Err(err("invalid id (letters, digits, _ and - only, ≤64 chars)"));
    }
    if app
        .registry
        .skills_all()
        .map_err(err)?
        .iter()
        .any(|s| s == &id)
    {
        return Err(err(
            "a skill with that id already exists — pick another, or Improve the existing one",
        ));
    }
    if r.source.trim().is_empty() {
        return Err(err("the skill source is empty"));
    }
    let interpreter = if r.interpreter.trim().is_empty() {
        "python3".to_string()
    } else {
        r.interpreter.trim().to_string()
    };
    if !valid_skill_interp(&interpreter) {
        return Err(err(
            "invalid interpreter (letters, digits, space, /._- only)",
        ));
    }
    let mut capabilities = Vec::new();
    for c in &r.capabilities {
        let cap = match c.to_ascii_lowercase().as_str() {
            "net" => Some(engram_skills::Capability::Net),
            "llm" => Some(engram_skills::Capability::Llm),
            _ => None,
        };
        if let Some(cap) = cap {
            if !capabilities.contains(&cap) {
                capabilities.push(cap);
            }
        }
    }
    let when_to_use = {
        let w = r.when_to_use.trim();
        (!w.is_empty()).then(|| w.to_string())
    };
    // Validate the category server-side (the UI renders it into a filter chip; a permissive value
    // could carry markup even though the client sink is now data-attribute based). Keep it to the
    // same id-like alphabet used elsewhere.
    let category = {
        let c = r.category.trim();
        if c.is_empty() {
            "problem_solving".to_string()
        } else if c.len() <= 48
            && c.chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-'))
        {
            c.to_string()
        } else {
            return Err(err(
                "invalid category (letters, digits, _ and - only, max 48 chars)",
            ));
        }
    };
    let new = engram_skills::NewSkill {
        id: id.clone(),
        category,
        description: r.description.trim().to_string(),
        capabilities,
        metric: "helpfulness".into(),
        runtime: engram_skills::Runtime::Process,
        interpreter: Some(interpreter),
        when_to_use,
    };
    let version = app
        .registry
        .install(new, r.source.as_bytes())
        .map_err(err)?;
    let _ = app.ledger.append(
        "skill.upload",
        "user",
        json!({ "id": id, "version": version }),
    );
    Ok(Json(json!({ "ok": true, "id": id, "version": version })))
}

#[derive(Deserialize)]
struct SkillToggleReq {
    enabled: bool,
}

/// Turn a skill on or off (the on/off switch). Off = hidden from selection/auto-use but kept and
/// instantly re-enablable.
async fn skill_toggle(
    State(app): State<App>,
    Path(id): Path<String>,
    Json(r): Json<SkillToggleReq>,
) -> ApiResult {
    app.registry.set_enabled(&id, r.enabled).map_err(err)?;
    Ok(Json(json!({ "ok": true, "id": id, "enabled": r.enabled })))
}

/// A ready-to-edit Process-skill template (stdlib-only Python): JSON in on stdin, JSON out on stdout —
/// the shape every Engram skill follows. Downloaded as a starting point for "write your own".
async fn skill_boilerplate() -> ApiResult {
    const TEMPLATE: &str = r#"#!/usr/bin/env python3
"""my_skill — an Engram Process skill.

Reads a JSON request from stdin, writes a JSON result to stdout. Stdlib only
(there is no `pip install` in the sandbox). If it must reach the network,
declare the Net capability when you upload it; secrets come from the daemon's
environment via os.environ (never hard-code them).
"""
import json
import sys


def main():
    try:
        req = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e}))
        return 0

    # --- your skill's work goes here ---
    result = {"echo": req, "note": "replace this with what your skill does"}

    print(json.dumps(result, indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main())
"#;
    Ok(Json(
        json!({ "filename": "my_skill.py", "source": TEMPLATE }),
    ))
}

/// The built-in tools and whether each is currently turned off — drives the Tools curation UI so
/// the toggle list is authoritative (never drifts from the real toolset).
async fn tools_list(State(app): State<App>) -> ApiResult {
    let disabled = app.cfg().security.disabled_tools.clone();
    let tools: Vec<Value> = engram_agent::default_tools()
        .defs()
        .into_iter()
        .map(|d| {
            json!({
                "name": d.name,
                "description": d.description,
                "disabled": disabled.iter().any(|x| x == &d.name),
            })
        })
        .collect();
    Ok(Json(
        json!({ "tools": tools, "disable_skill_author": app.cfg().security.disable_skill_author }),
    ))
}

#[derive(Deserialize)]
struct OpenUrlReq {
    url: String,
}

/// Open a URL in the user's default browser. The desktop webview (WKWebView served from the daemon
/// origin) can't follow `target="_blank"` links itself, so the dashboard routes external link clicks
/// here and the daemon hands the URL to the OS opener. Restricted to http(s) and passed as a single
/// argv (no shell) so it can't launch other handlers or smuggle extra arguments.
async fn open_url(State(app): State<App>, Json(r): Json<OpenUrlReq>) -> ApiResult {
    let url = r.url.trim().to_string();
    if !(url.starts_with("http://") || url.starts_with("https://"))
        || url.chars().any(|c| c.is_control())
    {
        return Err(ApiError("only plain http(s) URLs can be opened".into()));
    }
    let _ = app.ledger.append("open.url", "user", json!({ "url": url }));
    #[cfg(target_os = "macos")]
    let spawned = std::process::Command::new("open").arg(&url).spawn();
    #[cfg(target_os = "linux")]
    let spawned = std::process::Command::new("xdg-open").arg(&url).spawn();
    #[cfg(target_os = "windows")]
    let spawned = std::process::Command::new("cmd")
        .args(["/C", "start", "", &url])
        .spawn();
    match spawned {
        Ok(_) => Ok(Json(json!({ "ok": true }))),
        Err(e) => Err(ApiError(format!("couldn't open the link: {e}"))),
    }
}

#[derive(Deserialize)]
struct RunSkillReq {
    input: String,
}

/// Build the runtime params for executing/scoring a skill from the live config. A process skill
/// inherits the configured shell backend + the shell gate; WASM skills ignore those.
/// `source_tainted` = the code being executed/verified originated from a run that read untrusted
/// content (e.g. a distilled proposal built from a tainted run's answer). When true, the runtime
/// refuses to replay-execute the code outside a network-isolated OS sandbox — fail-closed. Direct,
/// already-adopted, or human-submitted skills pass false.
fn skill_run_params<'a>(
    app: &'a App,
    backend: Option<&'a str>,
    source_tainted: bool,
) -> engram_agent::SkillRunParams<'a> {
    engram_agent::SkillRunParams {
        backend,
        workdir: &app.workdir,
        timeout_secs: 30,
        taint: engram_core::Taint::Trusted,
        allow_exec: app.allow_shell.load(std::sync::atomic::Ordering::Relaxed),
        gateway: app.gateway.clone(),
        memory: app.memory.clone(),
        host: &app.host,
        scope: engram_core::ScopeCtx::user_only(), // direct skill-run endpoint has no project
        scoring: false,
        source_tainted,
    }
}

async fn run_skill(
    State(app): State<App>,
    Path(id): Path<String>,
    Json(r): Json<RunSkillReq>,
) -> ApiResult {
    // Dispatches WASM vs process internally; verifies the signature on both paths.
    let backend = {
        let c = app.cfg();
        config::resolve_shell_backend(&c.security.shell_backend, &c.security.shell_target)
    };
    let p = skill_run_params(&app, backend.as_deref(), false);
    let outcome = engram_agent::run_active(&app.registry, &id, r.input.as_bytes(), &p)
        .await
        .map_err(ApiError)?;
    Ok(Json(json!({
        "output": String::from_utf8_lossy(&outcome.output),
        "fuel_used": outcome.fuel_used,
        "host_calls": outcome.host_calls,
        "duration_us": outcome.duration_us,
        "logs": outcome.logs,
    })))
}

#[derive(Deserialize)]
struct ImproveReq {
    /// WebAssembly Text source for a WASM-skill candidate (compiled here with the `wat` crate).
    wat: Option<String>,
    /// Source for a process-skill candidate (a small program; reads stdin, writes stdout).
    source: Option<String>,
    /// Override the interpreter for a process candidate (defaults to the active version's).
    interpreter: Option<String>,
    description: Option<String>,
    /// For behavior-EXTENDING candidates: asserted (input, output) pairs proving the new behavior.
    /// The candidate must reproduce them (and keep the old gold) while the incumbent fails them.
    #[serde(default)]
    examples: Vec<ImproveExample>,
}

#[derive(Deserialize)]
struct ImproveExample {
    input: String,
    output: String,
}

/// Author + A/B-gate a candidate skill version. The candidate inherits the active version's
/// substrate: a WASM skill takes `wat` (compiled here), a process skill takes `source`. The
/// candidate is installed, then BOTH it and the incumbent are replayed (network-isolated) against
/// the recorded gold runs and the candidate is promoted iff it measurably wins. Every outcome is
/// signed into the ledger. One shared path with the agent's `skill_improve` tool, so they never
/// diverge. This is the route that makes "self-improving skills" exist at runtime.
async fn skill_improve(
    State(app): State<App>,
    Path(id): Path<String>,
    Json(r): Json<ImproveReq>,
) -> ApiResult {
    let (active_signed, _) = app.registry.load_active(&id).map_err(err)?;
    let m = active_signed.manifest.clone();
    // Build the candidate bytes for the active version's substrate.
    let bytes: Vec<u8> = match m.runtime {
        engram_skills::Runtime::Wasm => {
            let wat = r
                .wat
                .as_deref()
                .ok_or_else(|| ApiError("this is a WASM skill — provide `wat`".into()))?;
            wat::parse_str(wat).map_err(|e| err(format!("invalid WAT: {e}")))?
        }
        engram_skills::Runtime::Process => r
            .source
            .clone()
            .ok_or_else(|| ApiError("this is a process skill — provide `source`".into()))?
            .into_bytes(),
    };
    let candidate = engram_skills::NewSkill {
        id: id.clone(),
        category: m.category.clone(),
        description: r.description.unwrap_or_else(|| m.description.clone()),
        capabilities: m.capabilities.clone(),
        metric: m.metric.clone(),
        runtime: m.runtime,
        interpreter: r.interpreter.or_else(|| m.interpreter.clone()),
        when_to_use: m.when_to_use.clone(),
    };
    let backend = {
        let c = app.cfg();
        config::resolve_shell_backend(&c.security.shell_backend, &c.security.shell_target)
    };
    let p = skill_run_params(&app, backend.as_deref(), false);
    let new_examples: Vec<(String, String)> = r
        .examples
        .into_iter()
        .map(|e| (e.input, e.output))
        .collect();
    let decision = engram_agent::improve_skill(
        &app.registry,
        &id,
        candidate,
        &bytes,
        &new_examples,
        true,
        "user",
        &p,
        Some(&app.halt),
    )
    .await
    .map_err(ApiError)?;
    Ok(Json(decision))
}

#[derive(Deserialize)]
struct ActivateReq {
    version: u32,
}

/// Set the active version of a skill (the explicit, one-click promote/rollback control).
async fn skill_activate(
    State(app): State<App>,
    Path(id): Path<String>,
    Json(r): Json<ActivateReq>,
) -> ApiResult {
    app.registry
        .set_active(&id, r.version, "user", "skill.activate")
        .map_err(err)?;
    Ok(Json(json!({ "ok": true, "id": id, "active": r.version })))
}

/// Adopt a PROPOSED (inactive) skill: replay its latest version against its recorded gold and
/// activate it only if it reproduces them. The verified path the dashboard's "Adopt" button calls —
/// unlike `/activate` (a raw set-active escape hatch), this never activates a skill that fails its
/// own examples. A human click consents to a net-capable skill, so purity is not required here.
async fn skill_adopt(State(app): State<App>, Path(id): Path<String>) -> ApiResult {
    let backend = {
        let c = app.cfg();
        config::resolve_shell_backend(&c.security.shell_backend, &c.security.shell_target)
    };
    let p = skill_run_params(&app, backend.as_deref(), false);
    let decision =
        engram_agent::verify_and_adopt(&app.registry, &id, "user", false, &p, Some(&app.halt))
            .await
            .map_err(ApiError)?;
    // An explicit Adopt click on a disabled skill IS the re-enable: activating while the `retired`
    // marker stands would leave it adopted-but-invisible (skills() hides retired dirs, so it would
    // never be selected or listed as a skill again).
    let activated = matches!(decision["decision"].as_str(), Some("adopted" | "approved"));
    if activated && app.registry.is_retired(&id) {
        let _ = app.registry.set_enabled(&id, true);
    }
    Ok(Json(decision))
}

/// Revert a skill to its previous version (or an explicit one) - the auditable undo.
async fn skill_revert(
    State(app): State<App>,
    Path(id): Path<String>,
    Json(r): Json<Value>,
) -> ApiResult {
    let versions = app.registry.versions(&id).map_err(err)?;
    let from = app.registry.active_version(&id).ok().flatten().unwrap_or(0);
    let target = r
        .get("version")
        .and_then(|v| v.as_u64())
        .map(|v| v as u32)
        .or_else(|| {
            // Default: the version just below the current active one.
            versions.iter().copied().filter(|&v| v < from).max()
        });
    let Some(v) = target else {
        return Err(ApiError("no earlier version to revert to".into()));
    };
    app.registry
        .set_active(&id, v, "user", "skill.revert")
        .map_err(err)?;
    // Companion to the promotion memory improve_skill bridges into Region::Procedural: append-only,
    // never edit/delete the old promotion fact - a later reader sees the full history (promoted to
    // vX, then reverted) rather than a silently-vanished record. Best-effort: a memory-write failure
    // here must not fail the revert itself, which already succeeded and is what the user asked for.
    let _ = app.memory.remember(
        engram_memory::WriteReq::new(
            Region::Procedural,
            format!("Skill '{id}' reverted from v{from} to v{v}: the v{from} promotion is no longer in effect"),
        )
        .source(format!("skill:{id}#{v}"))
        .importance(0.6)
        .taint(engram_core::Taint::Trusted)
        .actor("user"),
    );
    Ok(Json(json!({ "ok": true, "id": id, "active": v })))
}

#[derive(Deserialize)]
struct TeachReq {
    input: String,
    gold: String,
    reward: Option<f32>,
}

/// Capture a runtime example as a gold `(input, accepted-output)` pair on the active version, so
/// the replay/scoring set GROWS with real use instead of being frozen at seed time.
async fn skill_teach(
    State(app): State<App>,
    Path(id): Path<String>,
    Json(r): Json<TeachReq>,
) -> ApiResult {
    // Record against the active version, or — for a PROPOSED (not-yet-active) skill — its latest
    // version, so a user can teach gold examples that later let it be adopted.
    let active = match app.registry.active_version(&id).map_err(err)? {
        Some(v) => v,
        None => app
            .registry
            .versions(&id)
            .map_err(err)?
            .into_iter()
            .max()
            .ok_or_else(|| err("no such skill"))?,
    };
    // Validate + clamp the reward: a NaN/inf would write a poison line to runs.jsonl that
    // permanently breaks this skill's replay/improve routes.
    let reward = r.reward.unwrap_or(1.0);
    if !reward.is_finite() {
        return Err(ApiError("reward must be a finite number".into()));
    }
    let reward = reward.clamp(0.0, 1.0);
    app.registry
        .record_run(&id, active, r.input.as_bytes(), r.gold.as_bytes(), reward)
        .map_err(err)?;
    let n = app
        .registry
        .accepted_runs(&id)
        .map(|v| v.len())
        .unwrap_or(0);
    Ok(Json(
        json!({ "ok": true, "id": id, "version": active, "recorded_runs": n }),
    ))
}

#[derive(Deserialize)]
struct MissionReq {
    goal: String,
    max_subtasks: Option<usize>,
}

/// Tolerantly pull a `[{title, detail}, ...]` subtask list out of a planner completion (the model
/// may wrap the JSON in prose or fences). Returns empty if nothing parses.
fn parse_subtasks(text: &str) -> Vec<(String, String)> {
    let (Some(s), Some(e)) = (text.find('['), text.rfind(']')) else {
        return Vec::new();
    };
    if e <= s {
        return Vec::new();
    }
    let arr: Vec<Value> = serde_json::from_str(&text[s..=e]).unwrap_or_default();
    arr.into_iter()
        .filter_map(|v| {
            let title = v.get("title").and_then(|t| t.as_str())?.trim().to_string();
            if title.is_empty() {
                return None;
            }
            let detail = v
                .get("detail")
                .and_then(|d| d.as_str())
                .unwrap_or("")
                .to_string();
            Some((title, detail))
        })
        .collect()
}

/// A mission coordinator: decompose a goal into subtasks (planner pass), run them as real cards
/// CONCURRENTLY (each an auditable, signed run with its own receipt), then synthesize one answer
/// (aggregator pass). This is the "run multiple worker agents on a complex task" capability built
/// on the durable kanban + agent loop, with every step on the ledger.
async fn run_mission(State(app): State<App>, Json(r): Json<MissionReq>) -> ApiResult {
    let goal = r.goal.trim().to_string();
    if goal.is_empty() {
        return Err(ApiError("empty goal".into()));
    }
    let max = r.max_subtasks.unwrap_or(4).clamp(1, 6);
    let model = app.model();

    // 1. PLAN: decompose into independent subtasks.
    let plan_prompt = format!(
        "Decompose this goal into {max} or fewer concrete, independent subtasks that together \
         accomplish it. Reply with ONLY a JSON array like [{{\"title\":\"...\",\"detail\":\"...\"}}].\n\nGoal:\n{goal}"
    );
    let preq = engram_gateway::CompletionRequest::new(
        model.clone(),
        vec![engram_gateway::Message::user(plan_prompt)],
    )
    .max_tokens(800);
    let plan_text = app
        .gateway
        .complete(
            engram_gateway::Call::new(preq)
                .actor("mission")
                .tainted(engram_core::Taint::Trusted),
        )
        .await
        .map(|c| c.text)
        .unwrap_or_default();
    let mut subtasks = parse_subtasks(&plan_text);
    if subtasks.is_empty() {
        subtasks = vec![(goal.clone(), String::new())]; // fallback: treat the goal as one task
    }
    subtasks.truncate(max);
    let _ = app.ledger.append(
        "mission.plan",
        "user",
        json!({ "goal": goal, "subtasks": subtasks.iter().map(|(t, _)| t).collect::<Vec<_>>() }),
    );

    // 2. EXECUTE: one real card per subtask, run concurrently under a small concurrency bound.
    let sem = Arc::new(tokio::sync::Semaphore::new(4));
    let mut handles = Vec::new();
    for (title, detail) in &subtasks {
        let card = app
            .tasks
            .create(title.clone(), detail.clone(), "mission".into());
        let appc = app.clone();
        let cid = card.id.clone();
        let title = title.clone();
        let sem = sem.clone();
        handles.push(tokio::spawn(async move {
            let _permit = sem.acquire().await;
            // Missions are USER-INITIATED from the UI, so the subtask runs are attended: a watching
            // user should get the interactive "Approve once & continue" card on a novel egress, not a
            // silent "staged for review" they never see. (Was `false`, which mislabeled a mission as
            // an unattended/channel run and removed the live approval affordance.)
            let answer = match run_task_core(&appc, &cid, None, false, true).await {
                Ok(t) => t.run.map(|r| r.answer).unwrap_or_default(),
                Err(e) => format!("(failed: {e})"),
            };
            (cid, title, answer)
        }));
    }
    let mut results = Vec::new();
    for h in handles {
        if let Ok(triple) = h.await {
            results.push(triple);
        }
    }

    // 3. AGGREGATE: synthesize the subtask answers into one cohesive result.
    let joined = results
        .iter()
        .map(|(_, t, a)| format!("### {t}\n{a}"))
        .collect::<Vec<_>>()
        .join("\n\n");
    let agg_prompt =
        format!("Synthesize these subtask results into one cohesive answer to the goal.\n\nGoal: {goal}\n\n{joined}");
    let areq = engram_gateway::CompletionRequest::new(
        model,
        vec![engram_gateway::Message::user(agg_prompt)],
    )
    .max_tokens(1500);
    let summary = app
        .gateway
        .complete(
            engram_gateway::Call::new(areq)
                .actor("mission")
                .tainted(engram_core::Taint::Trusted),
        )
        .await
        .map(|c| c.text)
        .unwrap_or_else(|_| joined.clone());
    let _ = app.ledger.append(
        "mission.done",
        "user",
        json!({ "goal": goal, "subtasks": results.len() }),
    );

    Ok(Json(json!({
        "goal": goal,
        "subtasks": results.iter().map(|(cid, t, a)| json!({ "task": cid, "title": t, "answer": a })).collect::<Vec<_>>(),
        "summary": summary,
    })))
}

#[derive(Deserialize)]
struct TailQuery {
    n: Option<usize>,
}

async fn ledger_tail(State(app): State<App>, Query(q): Query<TailQuery>) -> ApiResult {
    let entries = app.ledger.tail(q.n.unwrap_or(50)).map_err(err)?;
    Ok(Json(serde_json::to_value(entries).map_err(err)?))
}

async fn ledger_verify(State(app): State<App>) -> ApiResult {
    let n = app.ledger.verify().map_err(err)?;
    Ok(Json(json!({ "ok": true, "entries": n })))
}

/// The agent-facing tool that creates a scheduled task in Engram's OWN scheduler. Lives here (not in
/// engram-agent) because it needs `app.sched`; it's registered on every run alongside MCP tools. This
/// is what lets the agent answer "remind me / update me every morning" by actually scheduling it,
/// instead of writing a script and telling the user to set up cron themselves.
struct ScheduleTool {
    sched: Arc<Scheduler>,
}

#[async_trait::async_trait]
impl engram_agent::Tool for ScheduleTool {
    fn name(&self) -> &str {
        "schedule_task"
    }
    fn side_effecting(&self) -> bool {
        true
    }
    fn description(&self) -> &str {
        "Schedule a recurring or one-time task that Engram runs AUTOMATICALLY on a cadence — e.g. \
         'every morning at 8am', 'daily at 18:00', 'every Monday 9am', 'in 2 hours'. Engram has its \
         OWN built-in scheduler, so you do NOT need cron or any external service. When it fires, \
         Engram runs `instruction` as a normal agent task (it can web_search, browse, summarize, \
         send a message, etc.) and the result appears as a task card. Use this whenever the user \
         asks to be reminded, updated, or notified on a schedule."
    }
    fn schema(&self) -> Value {
        json!({ "type": "object", "properties": {
            "instruction": { "type": "string", "description": "what to DO each time it fires, e.g. 'fetch and summarize the top 5 AI news headlines with links'" },
            "when": { "type": "string", "description": "natural-language cadence: 'every morning at 8am', 'daily at 8:00', 'every Monday 9am', 'in 30 minutes'" },
            "name": { "type": "string", "description": "optional short label for the schedule" }
        }, "required": ["instruction", "when"] })
    }
    async fn run(
        &self,
        args: &Value,
        ctx: &engram_agent::ToolCtx,
    ) -> std::result::Result<String, String> {
        if ctx.taint.is_untrusted() {
            return Err(
                "scheduling refused: this run read untrusted content (injection guard)".into(),
            );
        }
        let instruction = args["instruction"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or("need an 'instruction' — what to do when it fires")?;
        let when = args["when"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or("need a 'when' — e.g. 'every morning at 8am'")?;
        let now = chrono::Utc::now();
        let rec = engram_sched::parse(when, now)
            .map_err(|e| format!("couldn't understand the schedule '{when}': {e}"))?;
        let name = args["name"]
            .as_str()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| instruction.chars().take(48).collect());
        if ctx.policy.dry_run {
            return Ok(format!("[dry-run] would schedule \"{name}\" — {when}"));
        }
        let job = self
            .sched
            .add(name.clone(), json!({ "title": instruction }), rec, now)
            .map_err(|e| e.to_string())?;
        let _ = ctx.ledger.append(
            "agent.schedule",
            "agent",
            json!({ "id": job.id, "name": name, "when": when }),
        );
        let next = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(job.next_fire_ms)
            .map(|t| {
                t.with_timezone(&chrono::Local)
                    .format("%a %d %b %H:%M")
                    .to_string()
            })
            .unwrap_or_else(|| "soon".into());
        Ok(format!(
            "✓ Scheduled \"{name}\" — {when}. Next run: {next} (local). It runs automatically (no cron needed); the result appears as a task card and you can manage it in the Schedule view."
        ))
    }
}

async fn schedule_list(State(app): State<App>) -> ApiResult {
    Ok(Json(serde_json::to_value(app.sched.list()).map_err(err)?))
}

#[derive(Deserialize)]
struct ScheduleReq {
    name: String,
    when: String,
    #[serde(default)]
    payload: Value,
}

#[derive(Deserialize)]
struct PreviewQuery {
    when: String,
}

/// Parse a natural-language "when" and show the next fire - without creating a job, so
/// the UI can preview live as the user types.
async fn schedule_preview(State(_app): State<App>, Query(q): Query<PreviewQuery>) -> ApiResult {
    let now = chrono::Utc::now();
    match engram_sched::parse(&q.when, now) {
        Ok(rec) => Ok(Json(json!({
            "ok": true,
            "next_fire_ms": rec.next_after(now).map(|t| t.timestamp_millis()),
            // Hand the UI the structured recurrence so it can render a live cadence badge.
            "recurrence": rec,
        }))),
        Err(e) => Ok(Json(json!({ "ok": false, "error": e.to_string() }))),
    }
}

async fn schedule_add(State(app): State<App>, Json(r): Json<ScheduleReq>) -> ApiResult {
    let now = chrono::Utc::now();
    let recurrence = parse_schedule(&r.when, now).map_err(err)?;
    let job = app
        .sched
        .add(r.name, r.payload, recurrence, now)
        .map_err(err)?;
    Ok(Json(serde_json::to_value(job).map_err(err)?))
}

async fn schedule_remove(State(app): State<App>, Path(id): Path<String>) -> ApiResult {
    let removed = app.sched.remove(&id).map_err(err)?;
    Ok(Json(json!({ "removed": removed })))
}

#[derive(Deserialize)]
struct ScheduleUpdateReq {
    name: Option<String>,
    /// Natural-language cadence ("every weekday at 9am"); omitted/blank keeps the current one.
    when: Option<String>,
    payload: Option<Value>,
}

/// Edit a scheduled job in place - rename, retime, or change what it does - without losing its
/// id, history, or last-run link. The update is signed to the ledger like every other decision.
async fn schedule_update(
    State(app): State<App>,
    Path(id): Path<String>,
    Json(r): Json<ScheduleUpdateReq>,
) -> ApiResult {
    let now = chrono::Utc::now();
    let rec = match r.when.as_deref() {
        Some(w) if !w.trim().is_empty() => Some(parse_schedule(w, now).map_err(err)?),
        _ => None,
    };
    let job = app
        .sched
        .update(&id, r.name, r.payload, rec, now)
        .map_err(err)?;
    Ok(Json(serde_json::to_value(job).map_err(err)?))
}

/// Run a scheduled job on demand: build a task from its payload (the same shape the
/// in-process tick uses), run it through the agent, record it as the job's `last_task_id`
/// so the UI can open the per-task receipt, and return the task id + final status.
async fn schedule_run(State(app): State<App>, Path(id): Path<String>) -> ApiResult {
    let job = app
        .sched
        .list()
        .into_iter()
        .find(|j| j.id == id)
        .ok_or_else(|| ApiError("schedule not found".into()))?;
    let task = task_from_schedule(&app, &job.payload, &job.name);
    // Record the task on the job before running so a crash mid-run still leaves a pointer to
    // the (failed) receipt for the UI's "last run" affordance.
    let _ = app.sched.set_last_task(&job.id, &task.id);
    let updated = run_task_core(&app, &task.id, None, false, false) // scheduled run → unattended
        .await
        .map_err(ApiError)?;
    Ok(Json(
        json!({ "task_id": task.id, "status": updated.status }),
    ))
}

fn parse_region(s: Option<&str>) -> Region {
    match s {
        Some("episodic") => Region::Episodic,
        Some("identity") => Region::Identity,
        Some("procedural") => Region::Procedural,
        _ => Region::Semantic,
    }
}

fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
}

#[derive(Deserialize)]
struct McpServerCfg {
    name: String,
    #[serde(default)]
    command: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    cwd: String,
    #[serde(default)]
    trusted: bool,
    /// Remote streamable-HTTP endpoint; when set, `command` may be empty.
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    bearer: Option<String>,
}

/// Load MCP servers from `<home>/mcp.json` (a JSON array of {name, command, args}) and
/// connect them, returning their tools. Missing or invalid config is non-fatal.
async fn load_mcp(home: &str) -> Vec<Arc<dyn engram_agent::Tool>> {
    let Ok(text) = std::fs::read_to_string(format!("{home}/mcp.json")) else {
        return Vec::new();
    };
    let cfg: Vec<McpServerCfg> = match serde_json::from_str(&text) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %e, "invalid mcp.json - ignoring");
            return Vec::new();
        }
    };
    let specs: Vec<engram_agent::McpServerSpec> = cfg
        .into_iter()
        .map(|c| engram_agent::McpServerSpec {
            name: c.name,
            command: c.command,
            args: c.args,
            env: c.env,
            cwd: if c.cwd.is_empty() { None } else { Some(c.cwd) },
            trusted: c.trusted,
            url: c.url.filter(|u| !u.is_empty()),
            bearer: c.bearer.filter(|b| !b.is_empty()),
        })
        .collect();
    engram_agent::connect_servers(&specs).await
}

/// Convert a stored MCP server config into the connector's spec (env/cwd/trusted threaded through).
fn mcp_spec(c: config::McpServer) -> engram_agent::McpServerSpec {
    engram_agent::McpServerSpec {
        name: c.name,
        command: c.command,
        args: c.args,
        env: c.env,
        cwd: if c.cwd.is_empty() { None } else { Some(c.cwd) },
        trusted: c.trusted,
        url: c.url.filter(|u| !u.is_empty()),
        bearer: c.bearer.filter(|b| !b.is_empty()),
    }
}

// --- Settings (read and edited by the desktop's Settings panel) ---------------------

/// Current settings, with secrets masked and the live provider/model reported.
async fn config_get(State(app): State<App>) -> ApiResult {
    let mut v = app.cfg().redacted();
    v["provider_id"] = json!(app.gateway.provider_id());
    v["model_in_use"] = json!(app.model());
    v["http_enabled"] = json!(cfg!(feature = "http"));
    // Honest capability flags the UI badges instead of advertising tools that can only error.
    v["browser_enabled"] = json!(engram_agent::browser_available());
    v["keyring_enabled"] = json!(cfg!(feature = "keyring"));
    // Which provider kinds have a remembered key (names only, never values) - so the UI can say
    // "key saved for openrouter" when you switch backends instead of looking amnesiac.
    let mut kinds: Vec<String> = config::read_secret_map(&app.home).into_keys().collect();
    if !app.cfg().provider.api_key.is_empty() {
        let k = app.cfg().provider.kind.clone();
        if !kinds.contains(&k) {
            kinds.push(k);
        }
    }
    kinds.sort();
    v["keys_saved_for"] = json!(kinds);
    v["version"] = json!(VERSION);
    Ok(Json(v))
}

/// Live status of the messaging channels, for the Integrations gallery.
async fn channels_status(State(app): State<App>) -> ApiResult {
    let (connected, username) = match app.telegram.lock().expect("telegram lock").as_ref() {
        Some((_, u)) => (true, u.clone()),
        None => (false, String::new()),
    };
    Ok(Json(
        json!({ "telegram": { "connected": connected, "username": username } }),
    ))
}

/// Connect Telegram live: validate the token against getMe, (re)start the poller without a
/// restart, persist the token, and sign the connection into the ledger. The token never returns
/// to the browser; only the public bot @username does.
async fn telegram_connect(State(app): State<App>, Json(p): Json<Value>) -> ApiResult {
    let token = p
        .get("token")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if token.is_empty() {
        return Err(err("no token provided"));
    }
    // Live validation - the UI does not claim a connection until Telegram confirms the bot.
    let id = telegram::validate(&token).await.map_err(err)?;
    // Stop any existing poller, then start the new one - live, no restart.
    if let Some((h, _)) = app.telegram.lock().expect("telegram lock").take() {
        h.abort();
    }
    {
        let mut cfg = app.config.write().expect("config lock");
        cfg.channels.telegram_token = token.clone();
        cfg.channels.telegram_username = id.username.clone();
        cfg.save(&app.home)
            .map_err(|e| err(format!("could not save settings: {e}")))?;
    }
    let handle = telegram::spawn(app.clone(), token);
    *app.telegram.lock().expect("telegram lock") = Some((handle, id.username.clone()));
    // Sign the connection - the bot identity only, NEVER the token.
    app.ledger
        .append(
            "channel.connect",
            "user",
            json!({ "channel": "telegram", "bot": id.username }),
        )
        .map_err(err)?;
    Ok(Json(
        json!({ "ok": true, "channel": "telegram", "username": id.username, "name": id.name }),
    ))
}

/// Disconnect Telegram live: stop the poller, wipe the token, and sign the disconnection.
async fn telegram_disconnect(State(app): State<App>) -> ApiResult {
    if let Some((h, _)) = app.telegram.lock().expect("telegram lock").take() {
        h.abort();
    }
    {
        let mut cfg = app.config.write().expect("config lock");
        cfg.channels.telegram_token.clear();
        cfg.channels.telegram_username.clear();
        cfg.save(&app.home)
            .map_err(|e| err(format!("could not save settings: {e}")))?;
    }
    app.ledger
        .append(
            "channel.disconnect",
            "user",
            json!({ "channel": "telegram" }),
        )
        .map_err(err)?;
    Ok(Json(json!({ "ok": true })))
}

/// Save a settings change. Persists `config.json`, then applies what can change live -
/// the model provider is hot-swapped and shell consent is updated immediately; the
/// embedder and MCP servers are wired at boot, so those take effect on the next wake.
async fn config_set(State(app): State<App>, Json(patch): Json<Value>) -> ApiResult {
    let before = app.cfg().clone();
    let mut cfg = before.clone();
    apply_config_patch(&mut cfg, &patch);
    // Provider-kind switch with no key typed: restore the key the user already entered for the
    // NEW kind from the per-provider store (and remember the old kind's key). Without this,
    // openrouter -> anthropic -> openrouter demanded retyping the OpenRouter key every time -
    // the single active slot was the only memory.
    if cfg.provider.kind != before.provider.kind {
        let mut map = config::read_secret_map(&app.home);
        if !before.provider.api_key.is_empty() {
            map.insert(
                before.provider.kind.clone(),
                before.provider.api_key.clone(),
            );
        }
        let typed = patch
            .get("provider")
            .and_then(|p| p.get("api_key"))
            .and_then(|k| k.as_str())
            .map(|k| !k.is_empty())
            .unwrap_or(false);
        if !typed {
            // The blank-keeps rule left the OLD kind's key in the slot - wrong for the new
            // backend. Adopt the stored key for the new kind, or clear so the UI asks honestly.
            cfg.provider.api_key = map.get(&cfg.provider.kind).cloned().unwrap_or_default();
        }
        config::write_secret_map(&app.home, &map);
    }
    // `egress_allowlist` is SERVER-managed — grown by egress_approve, not the settings form. This
    // handler is a read-modify-write on a clone, so an egress approval landing between our read and
    // save would be clobbered (lost update). Unless the patch explicitly set it, re-take the LIVE
    // value just before persisting so a concurrently-approved destination survives a settings save.
    if patch
        .get("security")
        .and_then(|s| s.get("egress_allowlist"))
        .is_none()
    {
        cfg.security.egress_allowlist = app.cfg().security.egress_allowlist.clone();
    }
    apply_web_env(&cfg); // make a just-saved search key/URL live for web_search without a restart

    cfg.save(&app.home)
        .map_err(|e| err(format!("could not save settings: {e}")))?;

    // Hot-swap the provider and shell consent.
    app.gateway.set_provider(Arc::from(cfg.build_provider()));
    app.gateway
        .set_default_effort(Some(cfg.provider.effort.clone()));
    app.allow_shell.store(
        cfg.security.allow_shell,
        std::sync::atomic::Ordering::Relaxed,
    );

    // Reconnect MCP servers live when the list changed (old subprocesses die on drop).
    // Report how many actually connected so the UI can flag a bad command instead of
    // silently dropping it.
    let mut mcp_report: Option<(usize, usize)> = None;
    if cfg.mcp != before.mcp {
        let specs: Vec<engram_agent::McpServerSpec> =
            cfg.mcp.iter().cloned().map(mcp_spec).collect();
        let (tools, connected) = engram_agent::connect_servers_reported(&specs).await;
        tracing::info!(
            connected = connected.len(),
            requested = cfg.mcp.len(),
            tools = tools.len(),
            "mcp servers reconnected after settings change"
        );
        mcp_report = Some((connected.len(), cfg.mcp.len()));
        *app.mcp_tools.write().expect("mcp lock") = tools;
    }

    // The embedder and the browser session are wired once at boot; flag a change to either so the
    // UI can offer a restart. (Provider, shell, worktrees, media models, and webhook apply live.)
    let restart_needed = cfg.embed.kind != before.embed.kind
        || cfg.embed.model_dir != before.embed.model_dir
        || cfg.browser.chrome_path != before.browser.chrome_path
        || cfg.browser.cdp_port != before.browser.cdp_port;

    // Capture before the move so the ledger entry doesn't re-lock the config we just took.
    let (provider_kind, model) = (cfg.provider.kind.clone(), cfg.model());
    *app.config.write().expect("config lock") = cfg;
    app.ledger
        .append(
            "config.update",
            "user",
            json!({ "provider": provider_kind, "model": model, "restart_needed": restart_needed }),
        )
        .ok();

    let mut v = app.cfg().redacted();
    v["provider_id"] = json!(app.gateway.provider_id());
    v["model_in_use"] = json!(app.model());
    v["restart_needed"] = json!(restart_needed);
    if let Some((connected, requested)) = mcp_report {
        v["mcp_connected"] = json!(connected);
        v["mcp_requested"] = json!(requested);
    }
    Ok(Json(v))
}

/// Try a one-line completion against the provider described by the posted settings
/// (merged over the current ones), without saving. Powers the "Test connection" button.
async fn config_test(State(app): State<App>, Json(patch): Json<Value>) -> ApiResult {
    let mut cfg = app.cfg().clone();
    apply_config_patch(&mut cfg, &patch);
    let provider = cfg.build_provider();
    let id = provider.id().to_string();
    let req = engram_gateway::CompletionRequest::new(
        cfg.model(),
        vec![engram_gateway::Message::user(
            "Reply with the single word: ok",
        )],
    )
    .max_tokens(16);
    match provider.complete(&req).await {
        Ok(c) => Ok(Json(json!({
            "ok": true,
            "provider": id,
            "model": c.model,
            "reply": c.text.chars().take(120).collect::<String>(),
            "tokens_out": c.tokens_out,
        }))),
        Err(e) => Ok(Json(
            json!({ "ok": false, "provider": id, "error": e.to_string() }),
        )),
    }
}

/// Spawn a single MCP server from a posted `{name,command,args}`, connect, and report how
/// many tools it exposed - powers the Integrations/Tools per-server "Test" button so a bad
/// command is caught before it's saved. The probe subprocess is dropped immediately after.
/// Reachable only behind `require_auth` (the router-wide layer), like the rest of /v1/*.
async fn config_mcp_test(Json(body): Json<Value>) -> ApiResult {
    let name = body
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("probe")
        .to_string();
    let command = body
        .get("command")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    // Remote streamable-HTTP servers are addressed by `url` instead of a spawned `command`.
    let url = body
        .get("url")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let bearer = body
        .get("bearer")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    if command.is_empty() && url.is_none() {
        return Ok(Json(json!({ "ok": false, "error": "no command or url" })));
    }
    let args: Vec<String> = body
        .get("args")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    // Accept optional per-server env (object of string->string) and cwd so a Test can exercise an
    // authenticated server exactly as it will run after saving.
    let env: std::collections::BTreeMap<String, String> = body
        .get("env")
        .and_then(|v| v.as_object())
        .map(|o| {
            o.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect()
        })
        .unwrap_or_default();
    let cwd = body
        .get("cwd")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    // Honor the trusted flag so the Test exercises the server exactly as it will run after saving
    // (a trusted server's reads do not taint the run; a Test should reflect that same posture).
    let trusted = body
        .get("trusted")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let specs = vec![engram_agent::McpServerSpec {
        name: name.clone(),
        command,
        args,
        env,
        cwd,
        trusted,
        url,
        bearer,
    }];
    let (tools, connected) = engram_agent::connect_servers_reported(&specs).await;
    if connected.is_empty() {
        Ok(Json(
            json!({ "ok": false, "error": "could not connect - check the command and args" }),
        ))
    } else {
        Ok(Json(json!({ "ok": true, "tools": tools.len() })))
    }
}

/// Inject the configured web-search provider keys/URL into the process environment so the
/// `web_search` tool (which reads env vars) picks them up — bridging the GUI (no way to set env
/// vars) to the env-based tool. Only sets non-empty values, so a power user's env-set key survives a
/// blank config field. Called at boot and after every settings save.
fn apply_web_env(cfg: &config::Config) {
    if !cfg.web.tavily_api_key.is_empty() {
        std::env::set_var("TAVILY_API_KEY", &cfg.web.tavily_api_key);
    }
    if !cfg.web.brave_api_key.is_empty() {
        std::env::set_var("BRAVE_API_KEY", &cfg.web.brave_api_key);
    }
    if !cfg.web.searxng_url.is_empty() {
        std::env::set_var("SEARXNG_URL", &cfg.web.searxng_url);
    }
    if !cfg.web.travelpayouts_token.is_empty() {
        std::env::set_var("TRAVELPAYOUTS_TOKEN", &cfg.web.travelpayouts_token);
    }
}

/// Merge a settings patch (the shape the UI posts) into a config. Secret fields are only
/// overwritten when a non-empty value is supplied; a `clear_*` flag wipes them.
fn apply_config_patch(cfg: &mut config::Config, p: &Value) {
    let s = |v: &Value, k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_string);
    let flag = |v: &Value, k: &str| v.get(k).and_then(|x| x.as_bool()) == Some(true);

    if let Some(pr) = p.get("provider") {
        if let Some(x) = s(pr, "kind") {
            cfg.provider.kind = x;
        }
        if let Some(x) = s(pr, "base_url") {
            cfg.provider.base_url = x;
        }
        if let Some(x) = s(pr, "model") {
            cfg.provider.model = x;
        }
        if let Some(x) = s(pr, "api_key") {
            if !x.is_empty() {
                cfg.provider.api_key = x;
            }
        }
        if flag(pr, "clear_api_key") {
            cfg.provider.api_key.clear();
        }
        if let Some(x) = s(pr, "effort") {
            // Only "low"/"medium"/"high" enable it; anything else means the model default.
            cfg.provider.effort = if matches!(x.as_str(), "low" | "medium" | "high") {
                x
            } else {
                String::new()
            };
        }
    }
    if let Some(e) = p.get("embed") {
        if let Some(x) = s(e, "kind") {
            cfg.embed.kind = x;
        }
        if let Some(x) = s(e, "model_dir") {
            cfg.embed.model_dir = x;
        }
    }
    if let Some(sec) = p.get("security") {
        if let Some(x) = s(sec, "api_token") {
            if !x.is_empty() {
                cfg.security.api_token = x;
            }
        }
        if let Some(x) = s(sec, "channel_secret") {
            if !x.is_empty() {
                cfg.security.channel_secret = x;
            }
        }
        if let Some(b) = sec.get("allow_shell").and_then(|v| v.as_bool()) {
            cfg.security.allow_shell = b;
        }
        if let Some(x) = s(sec, "shell_backend") {
            // "sandbox" (built-in OS sandbox), "docker", "ssh" change behaviour; anything else means
            // run on the host (no isolation).
            cfg.security.shell_backend = match x.trim() {
                "sandbox" | "docker" | "ssh" => x.trim().to_string(),
                _ => String::new(),
            };
        }
        if let Some(x) = s(sec, "shell_target") {
            cfg.security.shell_target = x.trim().to_string();
        }
        if let Some(b) = sec
            .get("enable_worktree_isolation")
            .and_then(|v| v.as_bool())
        {
            cfg.security.enable_worktree_isolation = b;
        }
        if let Some(arr) = sec.get("disabled_tools").and_then(|v| v.as_array()) {
            cfg.security.disabled_tools = arr
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.trim().to_string()))
                .filter(|s| !s.is_empty())
                .collect();
        }
        if let Some(b) = sec.get("disable_skill_author").and_then(|v| v.as_bool()) {
            cfg.security.disable_skill_author = b;
        }
        if let Some(b) = sec.get("auto_distill_skills").and_then(|v| v.as_bool()) {
            cfg.security.auto_distill_skills = b;
        }
        if let Some(b) = sec.get("auto_prune_memories").and_then(|v| v.as_bool()) {
            cfg.security.auto_prune_memories = b;
        }
        if let Some(b) = sec.get("auto_reflect").and_then(|v| v.as_bool()) {
            cfg.security.auto_reflect = b;
        }
        if flag(sec, "clear_api_token") {
            cfg.security.api_token.clear();
        }
        if flag(sec, "clear_channel_secret") {
            cfg.security.channel_secret.clear();
        }
    }
    if let Some(c) = p.get("cost") {
        if let Some(n) = c.get("task_token_budget").and_then(|v| v.as_u64()) {
            cfg.cost.task_token_budget = n.max(1_000);
        }
    }
    if let Some(w) = p.get("web") {
        // Secret keys follow the "blank keeps it" rule (they're masked in the UI), with an explicit
        // clear flag. The SearXNG URL is not a secret, so it's set/replaced on any value.
        if let Some(x) = s(w, "tavily_api_key") {
            if !x.trim().is_empty() {
                cfg.web.tavily_api_key = x.trim().to_string();
            }
        }
        if flag(w, "clear_tavily_api_key") {
            cfg.web.tavily_api_key.clear();
        }
        if let Some(x) = s(w, "brave_api_key") {
            if !x.trim().is_empty() {
                cfg.web.brave_api_key = x.trim().to_string();
            }
        }
        if flag(w, "clear_brave_api_key") {
            cfg.web.brave_api_key.clear();
        }
        if let Some(x) = s(w, "searxng_url") {
            cfg.web.searxng_url = x.trim().to_string();
        }
        if let Some(x) = s(w, "travelpayouts_token") {
            if !x.trim().is_empty() {
                cfg.web.travelpayouts_token = x.trim().to_string();
            }
        }
        if flag(w, "clear_travelpayouts_token") {
            cfg.web.travelpayouts_token.clear();
        }
    }
    if let Some(m) = p.get("media") {
        // Each empty string means "use the built-in default" - so blanking a field resets it.
        if let Some(x) = s(m, "vision_model") {
            cfg.media.vision_model = x.trim().to_string();
        }
        if let Some(x) = s(m, "image_model") {
            cfg.media.image_model = x.trim().to_string();
        }
        if let Some(x) = s(m, "tts_model") {
            cfg.media.tts_model = x.trim().to_string();
        }
        if let Some(x) = s(m, "stt_model") {
            cfg.media.stt_model = x.trim().to_string();
        }
    }
    if let Some(b) = p.get("browser") {
        if let Some(x) = s(b, "chrome_path") {
            cfg.browser.chrome_path = x.trim().to_string();
        }
        if let Some(n) = b.get("cdp_port").and_then(|v| v.as_u64()) {
            // 0 = unset (fall back to env/9222); otherwise clamp into the valid TCP range.
            cfg.browser.cdp_port = if n == 0 { 0 } else { n.clamp(1, 65_535) as u16 };
        }
    }
    if let Some(ch) = p.get("channels") {
        if let Some(x) = s(ch, "telegram_token") {
            if !x.is_empty() {
                cfg.channels.telegram_token = x;
            }
        }
        if flag(ch, "clear_telegram_token") {
            cfg.channels.telegram_token.clear();
        }
        // The webhook URL follows the "blank keeps it" rule (it's masked in the redacted view),
        // with an explicit clear flag to remove it.
        if let Some(x) = s(ch, "webhook_url") {
            if !x.trim().is_empty() {
                cfg.channels.webhook_url = x.trim().to_string();
            }
        }
        if flag(ch, "clear_webhook_url") {
            cfg.channels.webhook_url.clear();
        }
    }
    if let Some(arr) = p.get("mcp").and_then(|v| v.as_array()) {
        let existing = cfg.mcp.clone();
        // The redacted view masks env VALUES, so a settings round-trip must not wipe secrets. We
        // use the RAW JSON to tell "env omitted" (a UI with no env editor - inherit the previous
        // env) apart from "env present" (even an explicit {} clears it), and un-mask any value the
        // UI sent back as the mask placeholder (same "blank keeps it" rule as the api_key).
        const MASK: &str = "\u{2022}\u{2022}\u{2022}";
        let mut next: Vec<config::McpServer> = Vec::new();
        for m in arr {
            let Ok(mut srv) = serde_json::from_value::<config::McpServer>(m.clone()) else {
                continue;
            };
            // A server needs a name and SOME way to reach it — a spawn command or a remote
            // URL. Requiring a command here silently deleted every url-only remote server on
            // any settings round-trip that rewrote the array.
            let has_url = srv.url.as_deref().map(|u| !u.is_empty()).unwrap_or(false);
            if srv.name.is_empty() || (srv.command.is_empty() && !has_url) {
                continue;
            }
            let raw_has_env = m.get("env").map(|e| e.is_object()).unwrap_or(false);
            if let Some(prev) = existing.iter().find(|e| e.name == srv.name) {
                if !raw_has_env {
                    srv.env = prev.env.clone();
                } else {
                    for (k, v) in srv.env.iter_mut() {
                        if v == MASK {
                            if let Some(pv) = prev.env.get(k) {
                                *v = pv.clone();
                            }
                        }
                    }
                }
                // The redacted view reports only `bearer_set`, never the token itself, so a
                // round-trip arrives with no bearer — inherit the stored one unless the client
                // sent a real replacement or asked for an explicit clear (same "blank keeps
                // it" rule as every other secret in this file).
                let clear_bearer = flag(m, "clear_bearer");
                let sent_bearer = srv.bearer.as_deref().map(str::trim).unwrap_or("");
                if clear_bearer {
                    srv.bearer = None;
                } else if sent_bearer.is_empty() || sent_bearer == MASK {
                    srv.bearer = prev.bearer.clone();
                }
            }
            // Never persist a literal mask as if it were a secret: a value still equal to the mask
            // here had no previous value to restore (a new server, a renamed key, or a server that
            // never had that key), so storing it would write "•••" as the real secret. Drop it.
            srv.env.retain(|_, v| v != MASK);
            if srv.bearer.as_deref() == Some(MASK) {
                srv.bearer = None;
            }
            next.push(srv);
        }
        cfg.mcp = next;
    }
}

/// The persona (SOUL.md) - the standing instructions prepended to every agent run.
async fn persona_get(State(app): State<App>) -> ApiResult {
    let text = app
        .persona
        .read()
        .expect("persona lock")
        .clone()
        .unwrap_or_default();
    Ok(Json(json!({ "persona": text })))
}

/// Save the persona. Writes `<home>/SOUL.md` (or removes it when cleared) and updates the
/// live value, so it shapes the very next run without a restart.
async fn persona_set(State(app): State<App>, Json(body): Json<Value>) -> ApiResult {
    let text = body
        .get("persona")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let path = std::path::Path::new(&app.home).join("SOUL.md");
    if text.is_empty() {
        let _ = std::fs::remove_file(&path);
        *app.persona.write().expect("persona lock") = None;
    } else {
        std::fs::write(&path, &text).map_err(|e| err(format!("could not save persona: {e}")))?;
        *app.persona.write().expect("persona lock") = Some(text.clone());
    }
    app.ledger
        .append("persona.set", "user", json!({ "length": text.len() }))
        .ok();
    Ok(Json(json!({ "ok": true, "length": text.len() })))
}

/// Restart the daemon to apply settings that are only wired at boot (the embedder). The
/// process exits cleanly; a supervisor (the desktop shell, or systemd socket activation)
/// brings it back, and it re-reads `config.json` on the way up. In-flight memory and ledger
/// writes are durable, so this is safe; any running task is interrupted by design.
async fn restart_handler(State(app): State<App>) -> ApiResult {
    use std::sync::atomic::{AtomicBool, Ordering};
    // One exit is enough. Latch on the first request so a flood of /v1/restart can't spawn a
    // storm of exit tasks - a cheap brake on restart-as-DoS (the endpoint is also behind the
    // API-token gate when one is set).
    static RESTARTING: AtomicBool = AtomicBool::new(false);
    if RESTARTING.swap(true, Ordering::SeqCst) {
        return Ok(Json(
            json!({ "ok": true, "restarting": true, "already": true }),
        ));
    }
    app.ledger.append("core.restart", "user", json!({})).ok();
    // Carry the live, memory-only API key across the restart so reloading boot-time settings
    // (e.g. a new embedder) doesn't silently drop a connected provider back to the offline mock.
    // The key moves process-memory -> successor process-memory via env + exec; it never touches
    // disk, preserving the key-custody policy. (Unix only; elsewhere we fall back to a plain exit
    // and the key is re-seeded from the environment as before.)
    let carry_key = app.cfg().provider.api_key.clone();
    tokio::spawn(async move {
        // Let the HTTP response flush before we restart.
        tokio::time::sleep(Duration::from_millis(300)).await;
        // NOTE: we deliberately do NOT push the key into the process environment before re-exec.
        // That would leak it (via inheritance) to /v1/shell commands and every MCP child process,
        // including untrusted ones. Persistence across the restart is handled by the secret store
        // (config.rs read_secret_key: OS keyring or the 0600 secret.key), so the successor reloads
        // the key with no env exposure. (carry_key kept only for the absent-secret-store edge.)
        let _ = &carry_key;
        #[cfg(unix)]
        {
            if let Ok(exe) = std::env::current_exe() {
                use std::os::unix::process::CommandExt;
                tracing::info!(
                    "restart requested - re-exec to reload settings (key carried in memory)"
                );
                // exec replaces this image in place (same PID), so the supervisor keeps waiting on
                // us and never sees a gap; the bound socket fd is CLOEXEC so the successor rebinds.
                let err = std::process::Command::new(exe).exec();
                tracing::error!(error = %err, "re-exec failed - exiting for the supervisor to respawn");
            }
        }
        tracing::info!("restart requested - exiting to reload boot-time settings");
        std::process::exit(0);
    });
    Ok(Json(json!({ "ok": true, "restarting": true })))
}

/// Cleanly EXIT the process (no re-exec), so a supervisor can spawn a *different* binary in our
/// place. The desktop shell calls this on a cold launch to retire a stale daemon left running from a
/// previous app version (re-exec wouldn't help — it would relaunch the old binary), then starts its
/// freshly bundled daemon. Latched so a flood can't spawn an exit storm; behind the API-token gate.
async fn shutdown_handler(State(app): State<App>) -> ApiResult {
    use std::sync::atomic::{AtomicBool, Ordering};
    static STOPPING: AtomicBool = AtomicBool::new(false);
    if STOPPING.swap(true, Ordering::SeqCst) {
        return Ok(Json(
            json!({ "ok": true, "stopping": true, "already": true }),
        ));
    }
    app.ledger.append("core.shutdown", "user", json!({})).ok();
    tokio::spawn(async move {
        // Let the HTTP response flush before the process exits and frees the port.
        tokio::time::sleep(Duration::from_millis(200)).await;
        tracing::info!("shutdown requested - exiting so a newer daemon can take the port");
        std::process::exit(0);
    });
    Ok(Json(json!({ "ok": true, "stopping": true })))
}

// --- workspace: projects + sessions (the desktop sidebar) ---
#[derive(Deserialize)]
struct SessionsQuery {
    project: Option<String>,
}
async fn projects_list(State(app): State<App>) -> ApiResult {
    Ok(Json(
        serde_json::to_value(app.workspace.projects()).map_err(err)?,
    ))
}
async fn projects_create(State(app): State<App>, Json(b): Json<Value>) -> ApiResult {
    let name = b
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("Project")
        .trim()
        .to_string();
    let name = if name.is_empty() {
        "Project".into()
    } else {
        name
    };
    // Optional working directory for this project: attach-or-create. A relative or ~-path is
    // resolved; the directory is created if missing, then stored canonicalised.
    let workdir = match b.get("workdir").and_then(|v| v.as_str()).map(str::trim) {
        Some(w) if !w.is_empty() => Some(resolve_project_dir(w).map_err(ApiError)?),
        _ => None,
    };
    Ok(Json(
        serde_json::to_value(app.workspace.create_project(name, workdir)).map_err(err)?,
    ))
}

/// Resolve a user-supplied project directory: expand a leading `~`, create it if it doesn't exist
/// (attach-or-create), and return the canonical absolute path. Errors if the path exists but is a
/// file, or can't be created.
fn resolve_project_dir(raw: &str) -> Result<String, String> {
    let expanded = if let Some(rest) = raw.strip_prefix("~/") {
        match std::env::var("HOME") {
            Ok(h) => format!("{h}/{rest}"),
            Err(_) => raw.to_string(),
        }
    } else {
        raw.to_string()
    };
    let path = std::path::Path::new(&expanded);
    if path.exists() {
        if !path.is_dir() {
            return Err(format!("{expanded} exists but is not a directory"));
        }
    } else {
        std::fs::create_dir_all(path).map_err(|e| format!("could not create {expanded}: {e}"))?;
    }
    Ok(path
        .canonicalize()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or(expanded))
}
async fn projects_update(
    State(app): State<App>,
    Path(id): Path<String>,
    Json(b): Json<Value>,
) -> ApiResult {
    let name = b
        .get("name")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string());
    let persona = b
        .get("persona")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    // A provided workdir is attach-or-created; an empty string clears it back to the shared workdir.
    let workdir = match b.get("workdir").and_then(|v| v.as_str()).map(str::trim) {
        Some("") => Some(String::new()), // explicit clear
        Some(w) => Some(resolve_project_dir(w).map_err(ApiError)?),
        None => None, // unchanged
    };
    let p = app
        .workspace
        .update_project(&id, name, persona, workdir)
        .ok_or_else(|| ApiError("project not found".into()))?;
    Ok(Json(serde_json::to_value(p).map_err(err)?))
}
/// Auto-provision a per-project folder the first time something needs one and the project
/// doesn't already have one - `<home>/projects/<slug>`, created if missing. Keeps every project
/// isolated by default without making the user pick a folder before they can open its terminal;
/// a project created before this feature existed gets backfilled the same way, lazily, on first
/// use. A name collision (two projects called the same thing) disambiguates with a suffix from
/// the project id.
fn default_project_dir(home: &str, project_id: &str, project_name: &str) -> Result<String, String> {
    let mut slug: String = project_name
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    while slug.contains("--") {
        slug = slug.replace("--", "-");
    }
    let slug = slug.trim_matches('-').to_string();
    let base = std::path::Path::new(home).join("projects");
    let stem = if slug.is_empty() {
        project_id.to_string()
    } else {
        slug
    };
    let mut dir = base.join(&stem);
    if dir.exists() {
        let suffix = &project_id[project_id.len().saturating_sub(6)..];
        dir = base.join(format!("{stem}-{suffix}"));
    }
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    Ok(dir
        .canonicalize()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| dir.to_string_lossy().into_owned()))
}
/// POST /v1/projects/{id}/ensure-workdir — give this project a folder if it doesn't have one yet,
/// so opening its terminal never has to ask first. Idempotent: a project that already has a
/// workdir is returned unchanged.
async fn projects_ensure_workdir(State(app): State<App>, Path(id): Path<String>) -> ApiResult {
    let p = app
        .workspace
        .projects()
        .into_iter()
        .find(|p| p.id == id)
        .ok_or_else(|| err("project not found"))?;
    if p.workdir.as_deref().is_some_and(|w| !w.trim().is_empty()) {
        return Ok(Json(serde_json::to_value(p).map_err(err)?));
    }
    let dir = default_project_dir(&app.home, &p.id, &p.name).map_err(ApiError)?;
    let updated = app
        .workspace
        .update_project(&id, None, None, Some(dir))
        .ok_or_else(|| ApiError("project not found".into()))?;
    Ok(Json(serde_json::to_value(updated).map_err(err)?))
}
async fn projects_delete(State(app): State<App>, Path(id): Path<String>) -> ApiResult {
    // Gather the project's session ids BEFORE deleting so their session-scoped memories can be
    // cascade-forgotten too (delete_project also drops those sessions from the workspace).
    let session_ids: Vec<String> = app
        .workspace
        .sessions_meta(&id)
        .into_iter()
        .map(|s| s.id)
        .collect();
    let ok = app.workspace.delete_project(&id);
    if ok {
        // Cascade-forget the project's memories and each of its sessions' memories, so a deleted
        // project can't leave scoped facts behind to bleed into other projects' recall.
        if let Ok(n) = app
            .memory
            .forget_scope("project", &id, "user", "project deleted")
        {
            if n > 0 {
                tracing::info!(project = %id, count = n, "cascade-forgot project-scoped memories");
            }
        }
        for sid in &session_ids {
            let _ = app
                .memory
                .forget_scope("session", sid, "user", "project deleted");
        }
    }
    Ok(Json(json!({ "ok": ok })))
}
async fn sessions_list(State(app): State<App>, Query(q): Query<SessionsQuery>) -> ApiResult {
    let proj = q.project.unwrap_or_else(|| "personal".into());
    Ok(Json(
        serde_json::to_value(app.workspace.sessions_meta(&proj)).map_err(err)?,
    ))
}
async fn sessions_create(State(app): State<App>, Json(b): Json<Value>) -> ApiResult {
    let project_id = b
        .get("project_id")
        .and_then(|v| v.as_str())
        .unwrap_or("personal")
        .to_string();
    let title = b.get("title").and_then(|v| v.as_str()).map(str::to_string);
    Ok(Json(
        serde_json::to_value(app.workspace.create_session(project_id, title)).map_err(err)?,
    ))
}
async fn session_get(State(app): State<App>, Path(id): Path<String>) -> ApiResult {
    let s = app
        .workspace
        .session(&id)
        .ok_or_else(|| ApiError("session not found".into()))?;
    Ok(Json(serde_json::to_value(s).map_err(err)?))
}
async fn session_update(
    State(app): State<App>,
    Path(id): Path<String>,
    Json(b): Json<Value>,
) -> ApiResult {
    let title = b.get("title").and_then(|v| v.as_str()).map(str::to_string);
    let fav = b.get("fav").and_then(|v| v.as_bool());
    let project_id = b
        .get("project_id")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let s = app
        .workspace
        .update_session(&id, title, fav, project_id)
        .ok_or_else(|| ApiError("session not found".into()))?;
    Ok(Json(serde_json::to_value(s).map_err(err)?))
}
async fn session_delete(State(app): State<App>, Path(id): Path<String>) -> ApiResult {
    let ok = app.workspace.delete_session(&id);
    if ok {
        // Cascade-forget this session's scoped memories so a deleted chat leaves nothing behind.
        let _ = app
            .memory
            .forget_scope("session", &id, "user", "session deleted");
    }
    Ok(Json(json!({ "ok": ok })))
}

fn init_tracing() {
    use tracing_subscriber::{fmt, EnvFilter};
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    fmt()
        .with_env_filter(filter)
        .with_target(false)
        .compact()
        .init();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_egress_excludes_resolved_and_dedupes() {
        let dir = tempfile::tempdir().unwrap();
        let l = engram_core::Ledger::open(dir.path()).unwrap();
        let stage = |dest: &str| {
            l.append(
                "agent.egress_staged",
                "agent",
                json!({"scope":"agent:1","dest":dest,"tool":"send_message","reason":"destination_not_allowlisted"}),
            )
            .unwrap();
        };
        stage("a.com");
        stage("b.com");
        stage("b.com"); // duplicate staging of the same destination
        l.append(
            "egress.allowlisted",
            "user",
            json!({"scope":"agent:1","dest":"a.com"}),
        )
        .unwrap();
        let pending = pending_from_entries(&l.read_all().unwrap());
        let dests: Vec<&str> = pending
            .iter()
            .map(|p| p["dest"].as_str().unwrap())
            .collect();
        // a.com was resolved (allowlisted) and b.com is deduped -> only one pending item.
        assert_eq!(dests, vec!["b.com"], "got: {dests:?}");
    }

    #[tokio::test]
    async fn post_webhook_delivers_the_message_and_noops_on_empty() {
        // Empty url: a clean no-op (no panic, no connection attempt).
        post_webhook("", "ignored").await;
        // Configured url: the message body reaches the endpoint.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let got = std::sync::Arc::new(tokio::sync::Mutex::new(String::new()));
        let g2 = got.clone();
        let server = tokio::spawn(async move {
            if let Ok((mut sock, _)) = listener.accept().await {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let mut buf = vec![0u8; 8192];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                *g2.lock().await = String::from_utf8_lossy(&buf[..n]).to_string();
                let _ = sock
                    .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n")
                    .await;
            }
        });
        post_webhook(
            &format!("http://{addr}/hook"),
            "2 actions awaiting approval",
        )
        .await;
        let _ = server.await;
        assert!(
            got.lock().await.contains("2 actions awaiting approval"),
            "the webhook endpoint must receive the staged-action message"
        );
    }

    #[test]
    fn autonomy_report_tallies_per_scope_and_resolutions() {
        let dir = tempfile::tempdir().unwrap();
        let l = engram_core::Ledger::open(dir.path()).unwrap();
        l.append(
            "autonomy.policy.set",
            "user",
            json!({"id":"1","scope":"agent:1","rules":2,"max_actions":50}),
        )
        .unwrap();
        l.append(
            "agent.egress_autonomous",
            "agent",
            json!({"scope":"agent:1","dest":"a.com"}),
        )
        .unwrap();
        l.append(
            "agent.egress_autonomous",
            "agent",
            json!({"scope":"agent:1","dest":"a.com"}),
        )
        .unwrap();
        l.append(
            "agent.egress_staged",
            "agent",
            json!({"scope":"agent:1","dest":"b.com"}),
        )
        .unwrap();
        l.append(
            "egress.denied",
            "user",
            json!({"scope":"agent:1","dest":"b.com"}),
        )
        .unwrap();
        l.append(
            "agent.egress_approved",
            "agent",
            json!({"tool":"send_message"}),
        )
        .unwrap();
        let r = autonomy_report(&l.read_all().unwrap());
        assert_eq!(r["totals"]["autonomous_sends"], 2);
        assert_eq!(r["totals"]["staged"], 1);
        assert_eq!(r["totals"]["denied"], 1);
        assert_eq!(r["one_time_approvals"], 1);
        let s = &r["scopes"][0];
        assert_eq!(s["scope"], "agent:1");
        assert_eq!(s["policy"]["max_actions"], 50);
        assert_eq!(s["autonomous_sends"], 2);
    }

    #[test]
    fn extend_allowlist_is_idempotent_and_case_insensitive() {
        let p = engram_core::AutonomyPolicy {
            scope: "agent:1".into(),
            allowed_egress: vec![engram_core::EgressRule::new("x.com")],
            allowed_actions: vec![],
            budget: engram_core::EgressBudget {
                max_actions: 5,
                max_spend_cents: 0,
                expires_at_ms: 0,
            },
            hardline_floor: vec![],
        };
        let p = extend_allowlist(p, "y.com");
        assert_eq!(p.allowed_egress.len(), 2);
        let p = extend_allowlist(p, "Y.COM"); // already present (case-insensitive) -> no duplicate
        assert_eq!(p.allowed_egress.len(), 2);
    }

    // The newly-surfaced env-only settings (worktrees, media models, browser, webhook) must
    // round-trip through apply_config_patch, and the webhook URL must be masked in redacted().
    #[test]
    fn config_patch_round_trips_the_surfaced_settings() {
        let mut cfg = config::Config::default();
        apply_config_patch(
            &mut cfg,
            &json!({
                "security": { "enable_worktree_isolation": true, "auto_reflect": true },
                "media": { "vision_model": "gpt-4o", "image_model": "dall-e-3",
                           "tts_model": "tts-1-hd", "stt_model": "whisper-large" },
                "browser": { "chrome_path": "/opt/chromium", "cdp_port": 9333 },
                "channels": { "webhook_url": "https://hooks.slack.com/services/SECRET" },
            }),
        );
        assert!(cfg.security.enable_worktree_isolation);
        assert!(cfg.security.auto_reflect);
        assert_eq!(cfg.media.vision_model, "gpt-4o");
        assert_eq!(cfg.media.image_model, "dall-e-3");
        assert_eq!(cfg.media.tts_model, "tts-1-hd");
        assert_eq!(cfg.media.stt_model, "whisper-large");
        assert_eq!(cfg.browser.chrome_path, "/opt/chromium");
        assert_eq!(cfg.browser.cdp_port, 9333);
        assert_eq!(
            cfg.channels.webhook_url,
            "https://hooks.slack.com/services/SECRET"
        );

        // The redacted view exposes presence + the non-secret fields, but NEVER the webhook URL.
        let red = cfg.redacted();
        assert_eq!(red["security"]["enable_worktree_isolation"], json!(true));
        assert_eq!(red["media"]["vision_model"], json!("gpt-4o"));
        assert_eq!(red["browser"]["cdp_port"], json!(9333));
        assert_eq!(red["channels"]["webhook_url_set"], json!(true));
        assert!(
            !red.to_string().contains("SECRET"),
            "the webhook URL must be masked in the redacted view"
        );

        // The "blank keeps it" rule: an empty webhook_url must NOT wipe the saved one...
        apply_config_patch(&mut cfg, &json!({ "channels": { "webhook_url": "  " } }));
        assert_eq!(
            cfg.channels.webhook_url,
            "https://hooks.slack.com/services/SECRET"
        );
        // ...but the explicit clear flag removes it.
        apply_config_patch(
            &mut cfg,
            &json!({ "channels": { "clear_webhook_url": true } }),
        );
        assert!(cfg.channels.webhook_url.is_empty());

        // A 0 CDP port means "unset" (fall through to env/9222), not a literal port 0.
        apply_config_patch(&mut cfg, &json!({ "browser": { "cdp_port": 0 } }));
        assert_eq!(cfg.browser.cdp_port, 0);
    }

    #[test]
    fn capture_artifacts_records_only_new_files_and_copies_them() {
        use std::time::{SystemTime, UNIX_EPOCH};
        let n = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let base = std::env::temp_dir().join(format!("engram-artifacts-test-{n}"));
        let workdir = base.join("work");
        let home = base.join("home");
        std::fs::create_dir_all(&workdir).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        // A pre-existing file + a skipped dir present BEFORE the run.
        std::fs::write(workdir.join("existing.txt"), "old").unwrap();
        std::fs::create_dir_all(workdir.join(".git")).unwrap();
        std::fs::write(workdir.join(".git").join("HEAD"), "ref").unwrap();
        let before = snapshot_files(&workdir);

        // The run creates new files (incl. one in a subdir) and EDITS the existing one.
        std::fs::write(workdir.join("chart.png"), b"PNG").unwrap();
        std::fs::create_dir_all(workdir.join("out")).unwrap();
        std::fs::write(workdir.join("out").join("data.csv"), "a,b").unwrap();
        std::fs::write(workdir.join("existing.txt"), "changed").unwrap();

        let mut arts = capture_artifacts(
            home.to_str().unwrap(),
            "task1",
            &workdir,
            &before,
            &std::collections::HashSet::new(),
        );
        arts.sort();
        // Only the NEW files are captured (the edit and the .git/ dir are not).
        assert_eq!(
            arts,
            vec!["chart.png".to_string(), "out/data.csv".to_string()]
        );
        // And they were copied into the persistent per-task artifacts dir.
        assert!(home.join("artifacts/task1/chart.png").exists());
        assert!(home.join("artifacts/task1/out/data.csv").exists());
        assert!(!home.join("artifacts/task1/existing.txt").exists());
        std::fs::remove_dir_all(&base).ok();
    }

    // Regression for a real UX bug: a recurring job's task-board card showed the ENTIRE prompt as its
    // title (e.g. a multi-paragraph "evening digest" instruction), because task_from_schedule put the
    // full instructions in `title` and only fell back to the job's short name when no instructions were
    // set. The short name should always be the title; instructions belong in `detail`.
    #[test]
    fn schedule_task_fields_uses_short_name_as_title_not_the_full_prompt() {
        let (title, detail) = schedule_task_fields(
            &json!({ "title": "Create today's evening digest for Hamburg...\n\n(long instructions)" }),
            "Evening digest",
        );
        assert_eq!(title, "Evening digest");
        assert_eq!(
            detail,
            "Create today's evening digest for Hamburg...\n\n(long instructions)"
        );

        // payload.prompt is the same kind of field under a different key — same treatment.
        let (title2, detail2) =
            schedule_task_fields(&json!({ "prompt": "do the thing" }), "My job");
        assert_eq!(title2, "My job");
        assert_eq!(detail2, "do the thing");

        // A separate payload.detail is appended after the instructions, not lost.
        let (_, detail3) = schedule_task_fields(
            &json!({ "title": "do the thing", "detail": "extra context" }),
            "My job",
        );
        assert_eq!(detail3, "do the thing\n\nextra context");

        // No instructions at all (edge case): title falls back to the name, detail is whatever
        // payload.detail carries (unchanged from the pre-fix behavior) — nothing duplicated.
        let (title4, detail4) = schedule_task_fields(&json!({}), "Bare job");
        assert_eq!(title4, "Bare job");
        assert_eq!(detail4, "");
    }

    #[test]
    fn written_paths_since_extracts_write_and_append_but_not_edit() {
        let dir = tempfile::tempdir().unwrap();
        let l = engram_core::Ledger::open(dir.path()).unwrap();
        let workdir = std::path::Path::new("/work");
        let start_seq = l.head().0;
        l.append(
            "agent.write",
            "agent",
            json!({"path": "/work/evening_digest.html", "bytes": 42}),
        )
        .unwrap();
        l.append(
            "agent.append",
            "agent",
            json!({"path": "/work/out/report.csv", "bytes": 10}),
        )
        .unwrap();
        // An edit to an existing file must NOT be treated as a force-captured artifact.
        l.append("agent.edit", "agent", json!({"path": "/work/existing.txt"}))
            .unwrap();

        let written = written_paths_since(&l, workdir, start_seq);
        assert_eq!(written.len(), 2);
        assert!(written.contains(&std::path::PathBuf::from("evening_digest.html")));
        assert!(written.contains(&std::path::PathBuf::from("out/report.csv")));
        assert!(!written.contains(&std::path::PathBuf::from("existing.txt")));
    }

    // Regression for a real bug: a recurring task (e.g. a daily "evening digest") writes to the same
    // filename every run via write_file. Run 2+ overwrite a file that already existed BEFORE that run
    // started, so the plain new-vs-preexisting diff drops it — the digest is generated and even
    // rendered (e.g. a screenshot of it) but never shows up in the task's persisted artifacts. The fix:
    // capture_artifacts must also capture files write_file/append_file explicitly touched this run.
    #[test]
    fn capture_artifacts_recaptures_a_write_file_target_that_already_existed() {
        use std::time::{SystemTime, UNIX_EPOCH};
        let n = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let base = std::env::temp_dir().join(format!("engram-artifacts-rewrite-test-{n}"));
        let workdir = base.join("work");
        let home = base.join("home");
        std::fs::create_dir_all(&workdir).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        // A previous run's digest is already sitting in the workdir.
        std::fs::write(
            workdir.join("evening_digest.html"),
            "<html>yesterday</html>",
        )
        .unwrap();
        let before = snapshot_files(&workdir);

        // This run's write_file call overwrites the same filename with today's digest.
        std::fs::write(workdir.join("evening_digest.html"), "<html>today</html>").unwrap();
        let written: std::collections::HashSet<_> =
            [std::path::PathBuf::from("evening_digest.html")]
                .into_iter()
                .collect();

        let arts = capture_artifacts(home.to_str().unwrap(), "task1", &workdir, &before, &written);
        assert_eq!(arts, vec!["evening_digest.html".to_string()]);
        assert_eq!(
            std::fs::read_to_string(home.join("artifacts/task1/evening_digest.html")).unwrap(),
            "<html>today</html>"
        );
        std::fs::remove_dir_all(&base).ok();
    }

    // Regression for the audit's HIGH panic finding: String::truncate panics on a non-char-boundary
    // byte index, which a >cap document containing multibyte UTF-8 would hit. cap_text_on_boundary
    // must trim back to a boundary and never panic.
    #[test]
    fn cap_text_truncates_on_char_boundary_without_panic() {
        // 'é' is 2 bytes; a cap that lands mid-codepoint must be walked back, not panic.
        let s = "é".repeat(1000); // 2000 bytes
        let out = cap_text_on_boundary(s, 1001); // 1001 is mid-'é'
        assert!(out.starts_with('é'));
        assert!(out.ends_with("[document truncated]"));
        // The kept prefix is valid UTF-8 ending on a boundary (<= the cap).
        let kept = out.trim_end_matches("\n...[document truncated]");
        assert!(kept.len() <= 1001);
        assert!(kept.chars().all(|c| c == 'é'));

        // Emoji (4 bytes) at every offset around the cap must also be safe.
        let e = "😀".repeat(500); // 2000 bytes
        for cap in 1998..=2002 {
            let _ = cap_text_on_boundary(e.clone(), cap); // must not panic
        }
        // Under cap: returned unchanged.
        assert_eq!(cap_text_on_boundary("hi".into(), 100), "hi");
    }

    // Plain-text extraction works even without the `docs` feature, and binary/unknown types yield None.
    #[test]
    fn extract_plain_text_and_unknown() {
        let csv = b"name,role\nAda,founder\n";
        assert_eq!(
            extract_document_text("team.csv", csv).as_deref(),
            Some("name,role\nAda,founder\n")
        );
        assert_eq!(
            extract_document_text("notes.md", b"# Hi").as_deref(),
            Some("# Hi")
        );
        // An unknown/binary type extracts nothing (the file is still stored by the caller).
        assert_eq!(extract_document_text("photo.png", &[0u8, 1, 2, 3]), None);
    }

    // Regression for the audit's HIGH decompression-bomb finding: a DOCX whose document.xml inflates
    // past the extraction cap must be bounded, not read without limit. (docs feature only.)
    #[cfg(feature = "docs")]
    #[test]
    fn docx_extraction_is_bounded() {
        use std::io::Write;
        // Build a DOCX (zip) whose document.xml is ~12MB of text, above the 8MB EXTRACT_CAP.
        let body = format!(
            "<w:document><w:body><w:p><w:r><w:t>{}</w:t></w:r></w:p></w:body></w:document>",
            "A".repeat(12 * 1024 * 1024)
        );
        let mut buf = Vec::new();
        {
            let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            zip.start_file::<_, ()>(
                "word/document.xml",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
            zip.write_all(body.as_bytes()).unwrap();
            zip.finish().unwrap();
        }
        let out = extract_document_text("big.docx", &buf).expect("some text");
        // Bounded by EXTRACT_CAP (8MB) + a little tag overhead, never the full 12MB.
        assert!(
            out.len() <= 9 * 1024 * 1024,
            "extraction not bounded: {} bytes",
            out.len()
        );
    }

    #[test]
    fn loopback_host_accepts_localhost_rejects_rebind() {
        // Legitimate first-party Host values (any port, ipv4/ipv6/name) pass.
        assert!(is_loopback_host("127.0.0.1:8088"));
        assert!(is_loopback_host("localhost:8088"));
        assert!(is_loopback_host("LocalHost")); // case-insensitive
        assert!(is_loopback_host("[::1]:8088"));
        assert!(is_loopback_host("127.0.0.1"));
        assert!(is_loopback_host("127.5.5.5")); // 127.0.0.0/8 is all loopback
        assert!(is_loopback_host("tauri.localhost")); // Tauri WKWebView (Win/Linux) app origin
                                                      // A DNS-rebind attack carries the attacker's own hostname — rejected.
        assert!(!is_loopback_host("evil.example.com"));
        assert!(!is_loopback_host("evil.example.com:8088"));
        assert!(!is_loopback_host("10.0.0.5:8088"));
        assert!(!is_loopback_host("engram.attacker.test"));
    }

    #[test]
    fn loopback_origin_accepts_loopback_rejects_foreign_and_null() {
        assert!(is_loopback_origin("http://127.0.0.1:8088"));
        assert!(is_loopback_origin("http://localhost:8088"));
        assert!(is_loopback_origin("https://localhost"));
        assert!(!is_loopback_origin("https://evil.example.com"));
        assert!(!is_loopback_origin("null")); // opaque origin → rejected for writes
        assert!(!is_loopback_origin("file://"));
    }

    #[test]
    fn percent_decode_handles_reserved_chars() {
        assert_eq!(percent_decode("plain"), "plain");
        assert_eq!(percent_decode("a%2Bb%3Dc"), "a+b=c"); // + and =
        assert_eq!(percent_decode("x%26y"), "x&y"); // &
        assert_eq!(percent_decode("a+b"), "a b"); // + → space
        assert_eq!(percent_decode("100%25"), "100%"); // %
                                                      // A malformed trailing escape is left as-is rather than dropped.
        assert_eq!(percent_decode("bad%2"), "bad%2");
    }

    #[test]
    fn halt_key_matches_session_prefix_and_legacy() {
        // Unique per-run keys `<session>#<n>` all belong to their session.
        assert!(halt_key_matches("s-123#1", "s-123"));
        assert!(halt_key_matches("s-123#42", "s-123"));
        // A legacy bare-session key still matches (never silently miss a stop).
        assert!(halt_key_matches("s-123", "s-123"));
        // A different session is not matched (Stop targets only its own chat).
        assert!(!halt_key_matches("s-999#1", "s-123"));
        assert!(!halt_key_matches("s-1234#1", "s-123"));
    }
}
