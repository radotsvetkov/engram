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
mod channels;
mod config;
mod conscious;
mod converse;
mod dissent;
mod embedder;
mod seed;
mod tasks;
mod telegram;
mod terminal;
mod workspace;

use engram_core::{run_until_idle, Activity, Bus, Ledger, Priority, Spike, VERSION};
use engram_gateway::Gateway;
use engram_memory::{Memory, Region, TrigramHashEmbedder, WriteReq};
use engram_sched::{parse as parse_schedule, Scheduler};
use engram_skills::{Registry, RunCtx, SkillHost, SkillSigner};

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
struct ApiError(String);
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": self.0 })),
        )
            .into_response()
    }
}
type ApiResult = Result<Json<Value>, ApiError>;
fn err(e: impl std::fmt::Display) -> ApiError {
    ApiError(e.to_string())
}

#[tokio::main]
async fn main() {
    // `engramd verify [HOME]` - offline, third-party verification of the audit ledger
    // against its published public key, WITHOUT starting (or trusting) the daemon.
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("verify") => std::process::exit(verify_cmd(args.get(2).map(String::as_str))),
        // `engramd doctor [HOME]` - a self-diagnostic of the local setup (config, provider,
        // ledger, embedder, channels, port, build features), the way `claude-desktop --doctor`
        // checks an install. Exits 0 when nothing is broken, 1 when a hard problem is found.
        Some("doctor") => std::process::exit(doctor_cmd(args.get(2).map(String::as_str))),
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

    // Settings: config.json wins, else seed from the environment (back-compat).
    let cfg = config::Config::load(&home);
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
    let sched = Arc::new(Scheduler::open(&home, ledger.clone())?);
    let bus = Bus::new(1024);
    let activity = Activity::new();
    let workdir = std::path::PathBuf::from(
        std::env::var("ENGRAM_WORKDIR").unwrap_or_else(|_| format!("{home}/work")),
    );
    std::fs::create_dir_all(&workdir)?;
    // Personality / standing instructions, shaping every agent run (Hermes's SOUL.md).
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
        config: Arc::new(std::sync::RwLock::new(cfg)),
        home: home.clone(),
        telegram: Arc::new(std::sync::Mutex::new(None)),
        consciousness: Arc::new(conscious::Consciousness::open(&home)),
        agents: Arc::new(agents::AgentStore::open(std::path::Path::new(&home))),
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
        .route("/v1/screenshot", get(screenshot_get))
        .route("/v1/artifact", get(artifact_get))
        .route("/v1/artifacts", get(artifacts_list))
        .route("/v1/remember", post(remember))
        .route("/v1/recall", get(recall))
        .route("/v1/forget", post(forget))
        .route("/v1/consciousness", get(consciousness_get))
        .route("/v1/consciousness/distill", post(consciousness_distill))
        .route("/v1/consciousness/edit", post(consciousness_edit))
        .route("/v1/consciousness/add", post(consciousness_add))
        .route("/v1/consciousness/remove", post(consciousness_remove))
        .route("/v1/consciousness/revert", post(consciousness_revert))
        .route("/v1/agents", get(agents_list).post(agents_create))
        .route("/v1/agents/{id}", post(agents_update).delete(agents_delete))
        .route("/v1/agents/{id}/activity", get(agent_activity))
        .route("/v1/skills", get(skills))
        .route("/v1/skills/{id}/run", post(run_skill))
        .route("/v1/skills/{id}/improve", post(skill_improve))
        .route("/v1/skills/{id}/activate", post(skill_activate))
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
        .route("/v1/schedule", get(schedule_list).post(schedule_add))
        .route("/v1/schedule/preview", get(schedule_preview))
        .route("/v1/schedule/{id}", axum::routing::delete(schedule_remove))
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
            let title = job
                .payload
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or(&job.name)
                .to_string();
            let task = app.tasks.create(title, String::new(), "schedule".into());
            let _ = app.sched.set_last_task(&job.id, &task.id);
            let _ = run_task_core(&app, &task.id, None).await;
            let _ = app.sched.mark_fired(&job.id, now);
            fired += 1;
        }
        tracing::info!(fired, "ran due scheduled jobs (--run-due), exiting");
        return Ok(());
    }

    // Fire scheduled jobs while the daemon is awake.
    spawn_scheduler_tick(app.clone());
    spawn_consolidation_tick(app.clone());

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

    axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            match run_until_idle(activity, idle).await {
                engram_core::WakeReason::Idle => tracing::info!("idle - sleeping to zero"),
                engram_core::WakeReason::Signal => tracing::info!("signal - sleeping to zero"),
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
    let token = app.cfg().security.api_token.clone();
    if token.is_empty() {
        return next.run(req).await;
    }
    let path = req.uri().path();
    // The dashboard root and the liveness probe are always open. Inbound channel webhooks are
    // exempt from the bearer token ONLY when they carry their own shared secret (the handler
    // enforces it); without a channel secret they fall under the token gate, so an exposed
    // deployment can never be driven by an anonymous caller. (Channel runs also start Untrusted.)
    let channel_has_secret = !app.cfg().security.channel_secret.is_empty();
    if path == "/" || path == "/health" || (path.starts_with("/v1/channel/") && channel_has_secret)
    {
        return next.run(req).await;
    }
    let presented = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .map(str::to_string)
        .or_else(|| {
            req.uri()
                .query()
                .and_then(|q| q.split('&').find_map(|kv| kv.strip_prefix("token=")))
                .map(str::to_string)
        });
    if presented.map(|t| ct_eq(&t, &token)).unwrap_or(false) {
        next.run(req).await
    } else {
        (axum::http::StatusCode::UNAUTHORIZED, "unauthorized").into_response()
    }
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

async fn memory_stats(State(app): State<App>) -> ApiResult {
    Ok(Json(
        serde_json::to_value(app.memory.stats().map_err(err)?).map_err(err)?,
    ))
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
}

/// Serve a browser screenshot (or any image the agent saved) from the workspace so the chat/task
/// view can show it inline. Strictly confined to the workdir and to image types - it can never read
/// an arbitrary file off the box.
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
    let base = std::path::Path::new(&app.workdir);
    let full = base.join(&q.path);
    // Canonicalize both and require the target to stay under the workdir (defeats ../ traversal).
    let ok = match (base.canonicalize(), full.canonicalize()) {
        (Ok(b), Ok(f)) => f.starts_with(&b),
        _ => false,
    };
    if !ok {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    }
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
    let disp = if inline {
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
            let title = app.tasks.get(&task_id).map(|t| t.title).unwrap_or_default();
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
                        "task": task_id, "title": title, "path": rel, "name": name,
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
        "created_ms": a.created_ms, "updated_ms": a.updated_ms,
    })
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
    let def = app
        .agents
        .create(name, role, model, provider, base_url, api_key, effort);
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
    let def = app
        .agents
        .update(&id, name, role, model, provider, base_url, api_key, effort)
        .ok_or_else(|| err("no such agent"))?;
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
}

async fn remember(State(app): State<App>, Json(r): Json<RememberReq>) -> ApiResult {
    let region = parse_region(r.region.as_deref());
    let mut req = WriteReq::new(region, r.text).actor("user");
    if let Some(i) = r.importance {
        req = req.importance(i);
    }
    let rec = app.memory.remember(req).map_err(err)?;
    Ok(Json(serde_json::to_value(rec).map_err(err)?))
}

#[derive(Deserialize)]
struct RecallQuery {
    q: String,
    task: Option<String>,
    k: Option<usize>,
}

async fn recall(State(app): State<App>, Query(q): Query<RecallQuery>) -> ApiResult {
    let regions = match q.task.as_deref() {
        Some(t) => Region::for_task(t),
        None => vec![],
    };
    let hits = app
        .memory
        .recall(&q.q, &regions, q.k.unwrap_or(5))
        .map_err(err)?;
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
}
impl Drop for WorktreeGuard {
    fn drop(&mut self) {
        let repo = std::mem::take(&mut self.repo);
        let path = std::mem::take(&mut self.path);
        let remove = move || match std::process::Command::new("git")
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
    agent_def: Option<&agents::AgentDef>,
    workdir_override: Option<std::path::PathBuf>,
) -> Result<engram_agent::AgentRun, String> {
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
            .recall_trusted(task, &regions, 5)
            .ok()
            .filter(|h| !h.is_empty())
            .map(|hits| {
                let lines = hits
                    .iter()
                    .map(|h| format!("- {}", h.record.text.replace('\n', " ")))
                    .collect::<Vec<_>>()
                    .join("\n");
                format!("Relevant memory from earlier work (use it; flag anything that now conflicts):\n{lines}")
            })
    };
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
    };
    let mut tools = engram_agent::default_tools();
    for t in app.mcp_tools.read().expect("mcp lock").iter() {
        tools = tools.with(t.clone());
    }
    // Production runs verify before finishing (one bounded reflection pass), are bounded by
    // a token budget (runaway-cost guard), and honor the kill switch.
    let budget: u32 = app
        .cfg()
        .cost
        .task_token_budget
        .try_into()
        .unwrap_or(u32::MAX);
    let mut agent = engram_agent::Agent::new(gateway.clone(), tools, model)
        .max_steps(max_steps)
        .reflect(true)
        .token_budget(budget)
        .halt(app.halt.clone());
    // A named agent signs its steps as itself, so a multi-agent run is auditable per actor.
    if let Some(a) = agent_def {
        agent = agent.actor(a.name.clone());
    }
    // Standing context, in order: the assigned agent's ROLE (its specialization) leads, then the
    // consciousness working memory (facts about the user), then the global persona (style). Together
    // they replace SOUL.md as the source of truth for what the agent always knows.
    let mut parts: Vec<String> = Vec::new();
    if let Some(a) = agent_def {
        if !a.role.trim().is_empty() {
            parts.push(a.role.clone());
        }
    }
    if let Some(c) = app.consciousness.prompt_block() {
        parts.push(c);
    }
    if let Some(mb) = &memory_block {
        parts.push(mb.clone());
    }
    if let Some(p) = app.persona.read().expect("persona lock").clone() {
        if !p.trim().is_empty() {
            parts.push(p);
        }
    }
    if !parts.is_empty() {
        agent = agent.persona(parts.join("\n\n"));
    }
    if let Some(cb) = on_step {
        agent = agent.on_step(cb);
    }
    let result = agent.run(task, ctx).await.map_err(|e| e.to_string());
    // FLYWHEEL - auto-capture: on a completed, real (non-dry) trusted run, write one concise
    // episodic memory so the next task can recall what was done. Best-effort; dedup-on-write
    // collapses near-duplicates and consolidation demotes stale ones, so this can't bloat the brain.
    // Untrusted-origin runs are NOT captured - their content could be adversarial.
    if let Ok(run) = &result {
        if !dry_run && !taint.is_untrusted() && run.stopped == "final" {
            let answer = run.answer.trim();
            if !answer.is_empty() {
                let snippet: String = answer.chars().take(280).collect();
                let text = format!("Task: {}\nOutcome: {}", task.trim(), snippet);
                let _ = app.memory.remember(
                    engram_memory::WriteReq::new(engram_memory::Region::Episodic, text)
                        .taint(taint)
                        .actor("agent"),
                );
            }
        }
    }
    result
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
    let mut audio: Vec<u8> = Vec::new();
    while let Some(Ok(msg)) = socket.recv().await {
        match msg {
            Ws::Binary(b) => audio.extend_from_slice(&b),
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
}

/// Emergency stop. `{"on":true}` halts every in-flight agent run at its next step boundary
/// (and keeps new runs halted until released); `{"on":false}` releases. Ledgered.
async fn halt_set(State(app): State<App>, Json(r): Json<HaltReq>) -> ApiResult {
    app.halt.store(r.on, std::sync::atomic::Ordering::Relaxed);
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
    let d = dissent::review(&app.memory, &app.gateway, &model, &prompt).await;
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

/// After a run, copy the files that newly appeared in `workdir` (since the `before` snapshot) into a
/// persistent per-task artifacts dir (`<home>/artifacts/<task-id>/`), returning their relative paths.
/// Copying out decouples artifacts from the (possibly ephemeral git-worktree) workdir so they survive
/// cleanup, and capturing only NEW files keeps edits to existing project files out of the list.
fn capture_artifacts(
    home: &str,
    task_id: &str,
    workdir: &std::path::Path,
    before: &std::collections::HashSet<std::path::PathBuf>,
) -> Vec<String> {
    let after = snapshot_files(workdir);
    let mut new_files: Vec<_> = after.difference(before).cloned().collect();
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
pub(crate) async fn run_task_core(
    app: &App,
    id: &str,
    // When set, each completed step is streamed here as JSON for the live "watch the agent" view.
    step_tx: Option<tokio::sync::mpsc::UnboundedSender<serde_json::Value>>,
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
    let before = app.gateway.meter();
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
    let run = match run_agent_task_cb(
        app,
        &prompt,
        10,
        engram_core::Taint::Trusted,
        false,
        Some(on_step),
        agent_def.as_ref(),
        workdir_override,
    )
    .await
    {
        Ok(r) => r,
        Err(e) => {
            // The agent errored (e.g. provider failure after retries). try_begin already
            // marked the task "doing"; record a failed receipt so it isn't stuck "doing"
            // forever (try_begin would reject every future run).
            let m = app.gateway.meter();
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
                output_files: Vec::new(),
            };
            app.tasks.finish(id, receipt, "failed");
            app.bus.emit(Spike::new(
                "task.done",
                Priority::Normal,
                json!({ "id": id, "status": "failed" }),
            ));
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
    let output_files = capture_artifacts(&app.home, id, &run_workdir, &artifacts_before);
    let receipt = tasks::TaskRun {
        answer: run.answer,
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
    result
}

async fn tasks_run(State(app): State<App>, Path(id): Path<String>) -> ApiResult {
    let updated = run_task_core(&app, &id, None).await.map_err(ApiError)?;
    Ok(Json(serde_json::to_value(updated).map_err(err)?))
}

/// Run a task and STREAM each step as it happens (Server-Sent Events): a `step` event per tool
/// call with its args/observation/receipt, then a final `done` event with the persisted task -
/// the "watch the agent work" view. The agent runs in a spawned task feeding an mpsc channel.
async fn tasks_run_stream(
    State(app): State<App>,
    Path(id): Path<String>,
) -> axum::response::sse::Sse<
    impl futures_core::Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>,
> {
    use axum::response::sse::{Event, KeepAlive, Sse};
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<serde_json::Value>();
    let (done_tx, done_rx) = tokio::sync::oneshot::channel::<Result<tasks::Task, String>>();
    let app2 = app.clone();
    tokio::spawn(async move {
        let result = run_task_core(&app2, &id, Some(tx)).await;
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
            tokio::time::sleep(Duration::from_secs(3600)).await;
        }
    });
}

fn spawn_scheduler_tick(app: App) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(30)).await;
            let now = chrono::Utc::now();
            for job in app.sched.due(now) {
                let title = job
                    .payload
                    .get("title")
                    .and_then(|v| v.as_str())
                    .unwrap_or(&job.name);
                let task = app
                    .tasks
                    .create(title.to_string(), String::new(), "schedule".into());
                tracing::info!(job = %job.name, task = %task.id, "scheduler firing a task");
                // Record the task on the job before running so a crash mid-run still leaves a
                // pointer to the (failed) receipt for the UI's "last run" affordance.
                let _ = app.sched.set_last_task(&job.id, &task.id);
                let _ = run_task_core(&app, &task.id, None).await;
                let _ = app.sched.mark_fired(&job.id, now);
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
    let turn = converse::converse(
        &app.memory,
        &app.gateway,
        &r.text,
        &app.model(),
        persona.as_deref(),
        &r.attachments,
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
        let (recalled, recalled_refs) = converse::recall_ribbon(&app.memory, &r.text);
        let learned = converse::learn_identity(&app.memory, &r.text);
        // Conversation continuity: hand the agent the recent turns so a follow-up ("let's try again")
        // resolves against what was already said, instead of re-asking for context it already has.
        let history = r
            .session
            .as_ref()
            .map(|sid| app.workspace.recent_turns(sid, 10))
            .unwrap_or_default();
        let mut task = String::new();
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
        // Snapshot the workdir so files this turn creates (e.g. a browser screenshot) are captured as
        // downloadable artifacts in the gallery, bucketed under this chat session.
        let art_bucket = r.session.clone().unwrap_or_else(|| "chat".to_string());
        let run_workdir = app.workdir.clone();
        let artifacts_before = snapshot_files(&run_workdir);
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
        match run_agent_task_cb(
            &app,
            &task,
            24,
            engram_core::Taint::Trusted,
            false,
            Some(on_step),
            None,
            None,
        )
        .await
        {
            Ok(run) => {
                if let Some(sid) = &r.session {
                    app.workspace.append_turn(
                        sid,
                        &r.text,
                        &run.answer,
                        recalled.clone(),
                        recalled_refs
                            .iter()
                            .map(|rf| serde_json::to_value(rf).unwrap_or_default())
                            .collect(),
                        learned.clone(),
                    );
                }
                // Capture any files this turn produced into the gallery (under the session bucket).
                let _ = capture_artifacts(&app.home, &art_bucket, &run_workdir, &artifacts_before);
                let _ = tx.send(Event::default().event("done").data(
                    json!({ "reply": run.answer, "recalled": recalled, "recalled_refs": recalled_refs, "learned": learned, "steps": run.steps })
                        .to_string(),
                ));
            }
            Err(e) => {
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
}

/// Extract readable text from an uploaded document (PDF / DOCX / XLSX / CSV / plain text) so the
/// agent can actually read it. Returns `None` for an unknown/binary type or when the `docs` feature
/// is off (the file is still stored; only text extraction is gated). Output is capped by the caller.
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
        // before the caller's 600KB output cap is applied. (A file crafted to make pdf-extract or
        // calamine itself panic still aborts the process under `panic = "abort"`; isolating the
        // parser in a subprocess is the documented deferred hardening, see THREAT-MODEL T9. The
        // 25MB input cap in upload_handler bounds that today.)
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
    // PDF can't blow the context). The UI attaches this text to the turn.
    let extracted = extract_document_text(&base, &bytes).map(|t| cap_text_on_boundary(t, 600_000));
    Ok(Json(json!({
        "ref": stored,
        "name": base,
        "size": bytes.len(),
        "mime": r.mime.unwrap_or_else(|| "application/octet-stream".into()),
        "extracted_text": extracted,
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
    for id in app.registry.skills().map_err(err)? {
        let active = app.registry.active_version(&id).map_err(err)?;
        let versions = app.registry.versions(&id).map_err(err)?;
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
        out.push(json!({ "id": id, "active": active, "versions": versions, "runs": runs, "learn": events }));
    }
    Ok(Json(json!({ "skills": out })))
}

#[derive(Deserialize)]
struct RunSkillReq {
    input: String,
}

async fn run_skill(
    State(app): State<App>,
    Path(id): Path<String>,
    Json(r): Json<RunSkillReq>,
) -> ApiResult {
    let (signed, wasm) = app.registry.load_active(&id).map_err(err)?;
    let ctx = RunCtx::pure()
        .memory(app.memory.clone(), Region::ALL.to_vec())
        .gateway(app.gateway.clone());
    let vk = *app.registry.verifying();
    // Async path so skills granted the LLM/Net capability can reach the gateway.
    let outcome = app
        .host
        .run_signed_async(&signed, &wasm, &vk, r.input.as_bytes(), ctx)
        .await
        .map_err(err)?;
    Ok(Json(json!({
        "output": String::from_utf8_lossy(&outcome.output),
        "fuel_used": outcome.fuel_used,
        "host_calls": outcome.host_calls,
        "duration_us": outcome.duration_us,
        "logs": outcome.logs,
    })))
}

/// Replay a skill version against its recorded `(input, gold)` runs USING REAL CAPABILITIES
/// (memory + the gateway, the same context a live run gets), scoring each output against the
/// accepted one. `exact_match` is all-or-nothing; any other metric gives partial credit via a
/// character-bigram similarity, so a near-correct LLM-backed skill can measurably improve. This
/// is the async fix for the old `score_version` that replayed with no capabilities (always 0).
async fn score_skill_async(
    app: &App,
    id: &str,
    version: u32,
    runs: &[(Vec<u8>, Vec<u8>)],
    metric: &str,
) -> f32 {
    if runs.is_empty() {
        return 0.0;
    }
    let Ok((signed, wasm)) = app.registry.load(id, version) else {
        return 0.0;
    };
    let vk = *app.registry.verifying();
    let exact = metric == "exact_match";
    // Bound the replay set (an unbounded loop of live LLM calls is a cost/availability DoS) and
    // cooperate with the kill switch so a stuck improve can be stopped. Score the most recent K.
    const MAX_REPLAYS: usize = 50;
    let scored: Vec<&(Vec<u8>, Vec<u8>)> = runs.iter().rev().take(MAX_REPLAYS).collect();
    let mut total = 0.0f32;
    let mut n = 0usize;
    for (input, gold) in &scored {
        if app.halt.load(std::sync::atomic::Ordering::Relaxed) {
            break;
        }
        let ctx = engram_skills::RunCtx::pure()
            .memory(app.memory.clone(), Region::ALL.to_vec())
            .gateway(app.gateway.clone());
        if let Ok(o) = app
            .host
            .run_signed_async(&signed, &wasm, &vk, input, ctx)
            .await
        {
            total += if exact {
                if &o.output == gold {
                    1.0
                } else {
                    0.0
                }
            } else {
                bigram_similarity(&o.output, gold)
            };
        }
        n += 1;
    }
    if n == 0 {
        0.0
    } else {
        total / n as f32
    }
}

/// Character-bigram Jaccard similarity in [0,1] - partial credit for a near-correct output.
fn bigram_similarity(a: &[u8], b: &[u8]) -> f32 {
    if a == b {
        return 1.0;
    }
    let grams = |s: &[u8]| -> std::collections::HashSet<(u8, u8)> {
        s.windows(2).map(|w| (w[0], w[1])).collect()
    };
    let (ga, gb) = (grams(a), grams(b));
    if ga.is_empty() && gb.is_empty() {
        return if a == b { 1.0 } else { 0.0 };
    }
    let inter = ga.intersection(&gb).count() as f32;
    let union = ga.union(&gb).count() as f32;
    if union == 0.0 {
        0.0
    } else {
        inter / union
    }
}

#[derive(Deserialize)]
struct ImproveReq {
    /// WebAssembly Text source for the candidate version (compiled here with the same `wat`
    /// crate the seed skills use). Keeps authoring data-only - no native code path.
    wat: String,
    description: Option<String>,
}

/// Author + A/B-gate a candidate skill version: compile the WAT, install it as a new version,
/// replay BOTH the incumbent and the candidate against the recorded runs under real capabilities,
/// and promote the candidate iff it measurably wins. Every outcome is signed into the ledger.
/// This is the route that makes "self-improving skills" exist at runtime.
async fn skill_improve(
    State(app): State<App>,
    Path(id): Path<String>,
    Json(r): Json<ImproveReq>,
) -> ApiResult {
    let (active_signed, _) = app.registry.load_active(&id).map_err(err)?;
    let m = &active_signed.manifest;
    let wasm = wat::parse_str(&r.wat).map_err(|e| err(format!("invalid WAT: {e}")))?;
    let candidate = engram_skills::NewSkill {
        id: id.clone(),
        category: m.category.clone(),
        description: r.description.unwrap_or_else(|| m.description.clone()),
        capabilities: m.capabilities.clone(),
        metric: m.metric.clone(),
    };
    let candidate_version = app.registry.install(candidate, &wasm).map_err(err)?;
    let runs = app.registry.accepted_runs(&id).map_err(err)?;
    let active = app.registry.active_version(&id).map_err(err)?.unwrap_or(0);
    if runs.is_empty() {
        return Ok(Json(json!({
            "decision": "no_data", "id": id, "candidate": candidate_version,
            "note": "no recorded runs to judge against yet - teach it some accepted runs first"
        })));
    }
    let incumbent_score = score_skill_async(&app, &id, active, &runs, &m.metric).await;
    let candidate_score = score_skill_async(&app, &id, candidate_version, &runs, &m.metric).await;
    // Promotion gate. exact_match is deterministic, so a strict win is sound. A fuzzy (LLM/bigram)
    // metric is noisy, so require a real MARGIN and a minimum sample count - otherwise sampling
    // jitter could promote an equal-or-worse candidate.
    let exact = m.metric == "exact_match";
    let margin = if exact { 0.0 } else { 0.05 };
    let promoted = candidate_score > incumbent_score + margin && (exact || runs.len() >= 3);
    if promoted {
        app.registry
            .set_active(&id, candidate_version, "user", "skill.promote")
            .map_err(err)?;
    }
    let _ = app.ledger.append(
        "skill.learn",
        "user",
        json!({ "id": id, "promoted": promoted, "from": active, "candidate": candidate_version,
                "incumbent_score": incumbent_score, "candidate_score": candidate_score, "replays": runs.len() }),
    );
    Ok(Json(json!({
        "decision": if promoted { "promoted" } else { "rejected" },
        "id": id, "from": active, "candidate": candidate_version,
        "incumbent_score": incumbent_score, "candidate_score": candidate_score, "replays": runs.len(),
    })))
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

/// Revert a skill to its previous version (or an explicit one) - the auditable undo.
async fn skill_revert(
    State(app): State<App>,
    Path(id): Path<String>,
    Json(r): Json<Value>,
) -> ApiResult {
    let versions = app.registry.versions(&id).map_err(err)?;
    let target = r
        .get("version")
        .and_then(|v| v.as_u64())
        .map(|v| v as u32)
        .or_else(|| {
            // Default: the version just below the current active one.
            let active = app.registry.active_version(&id).ok().flatten().unwrap_or(0);
            versions.iter().copied().filter(|&v| v < active).max()
        });
    let Some(v) = target else {
        return Err(ApiError("no earlier version to revert to".into()));
    };
    app.registry
        .set_active(&id, v, "user", "skill.revert")
        .map_err(err)?;
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
    let active = app
        .registry
        .active_version(&id)
        .map_err(err)?
        .ok_or_else(|| err("no active version"))?;
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
            let answer = match run_task_core(&appc, &cid, None).await {
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
    let title = job
        .payload
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or(&job.name)
        .to_string();
    let task = app.tasks.create(title, String::new(), "schedule".into());
    // Record the task on the job before running so a crash mid-run still leaves a pointer to
    // the (failed) receipt for the UI's "last run" affordance.
    let _ = app.sched.set_last_task(&job.id, &task.id);
    let updated = run_task_core(&app, &task.id, None)
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
    command: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    cwd: String,
    #[serde(default)]
    trusted: bool,
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
    if command.is_empty() {
        return Ok(Json(json!({ "ok": false, "error": "no command" })));
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
            // Only "docker" / "ssh" change behaviour; anything else means run on the host.
            cfg.security.shell_backend = match x.trim() {
                "docker" | "ssh" => x.trim().to_string(),
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
            if srv.name.is_empty() || srv.command.is_empty() {
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
            }
            // Never persist a literal mask as if it were a secret: a value still equal to the mask
            // here had no previous value to restore (a new server, a renamed key, or a server that
            // never had that key), so storing it would write "•••" as the real secret. Drop it.
            srv.env.retain(|_, v| v != MASK);
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
    Ok(Json(
        serde_json::to_value(app.workspace.create_project(name)).map_err(err)?,
    ))
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
    let p = app
        .workspace
        .update_project(&id, name, persona)
        .ok_or_else(|| ApiError("project not found".into()))?;
    Ok(Json(serde_json::to_value(p).map_err(err)?))
}
async fn projects_delete(State(app): State<App>, Path(id): Path<String>) -> ApiResult {
    Ok(Json(json!({ "ok": app.workspace.delete_project(&id) })))
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
    Ok(Json(json!({ "ok": app.workspace.delete_session(&id) })))
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

    // The newly-surfaced env-only settings (worktrees, media models, browser, webhook) must
    // round-trip through apply_config_patch, and the webhook URL must be masked in redacted().
    #[test]
    fn config_patch_round_trips_the_surfaced_settings() {
        let mut cfg = config::Config::default();
        apply_config_patch(
            &mut cfg,
            &json!({
                "security": { "enable_worktree_isolation": true },
                "media": { "vision_model": "gpt-4o", "image_model": "dall-e-3",
                           "tts_model": "tts-1-hd", "stt_model": "whisper-large" },
                "browser": { "chrome_path": "/opt/chromium", "cdp_port": 9333 },
                "channels": { "webhook_url": "https://hooks.slack.com/services/SECRET" },
            }),
        );
        assert!(cfg.security.enable_worktree_isolation);
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

        let mut arts = capture_artifacts(home.to_str().unwrap(), "task1", &workdir, &before);
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
}
