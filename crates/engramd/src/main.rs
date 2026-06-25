//! `engramd` — the Engram daemon.
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

mod channels;
mod converse;
mod embedder;
mod seed;
mod telegram;

use engram_core::{run_until_idle, Activity, Bus, Ledger, Priority, Spike, VERSION};
use engram_gateway::{Gateway, MockProvider, Provider};
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
    persona: Option<String>,
    mcp_tools: Vec<Arc<dyn engram_agent::Tool>>,
    browser: Arc<dyn engram_agent::BrowserSession>,
}

/// Uniform error → JSON 500.
struct ApiError(String);
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": self.0 }))).into_response()
    }
}
type ApiResult = Result<Json<Value>, ApiError>;
fn err(e: impl std::fmt::Display) -> ApiError {
    ApiError(e.to_string())
}

#[tokio::main]
async fn main() {
    init_tracing();
    if let Err(e) = run().await {
        tracing::error!(error = %e, "fatal");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let home = std::env::var("ENGRAM_HOME").unwrap_or_else(|_| "./brain".into());
    let addr: SocketAddr = std::env::var("ENGRAM_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:8088".into())
        .parse()?;
    let idle = Duration::from_secs(env_u64("ENGRAM_IDLE_SECS", 900));

    let ledger = Arc::new(Ledger::open(&home)?);
    let gateway = Arc::new(Gateway::new(make_provider(), ledger.clone()));

    // Pick the embedder: a real model through the gateway (ENGRAM_EMBED=gateway), or
    // the dependency-free offline default. The gateway path probes its dimension once.
    let embedder: Arc<dyn engram_memory::Embedder> =
        if std::env::var("ENGRAM_EMBED").as_deref() == Ok("gateway") {
            let probe = gateway.embed(&["dimension probe".into()], "init").await?;
            let dim = probe.first().map(|v| v.len()).unwrap_or(256);
            tracing::info!(dim, "using gateway embedder");
            Arc::new(embedder::GatewayEmbedder::new(gateway.clone(), dim))
        } else {
            Arc::new(TrigramHashEmbedder::default())
        };

    let memory = Arc::new(Memory::open(format!("{home}/brain.db"), embedder, ledger.clone())?);
    let signer = Arc::new(SkillSigner::load_or_create(format!("{home}/keys/skill.key"))?);
    let registry = Arc::new(Registry::open(&home, signer, ledger.clone())?);
    seed::ensure_seed(&registry)?;
    let sched = Arc::new(Scheduler::open(&home, ledger.clone())?);
    let bus = Bus::new(1024);
    let activity = Activity::new();
    let workdir =
        std::path::PathBuf::from(std::env::var("ENGRAM_WORKDIR").unwrap_or_else(|_| format!("{home}/work")));
    std::fs::create_dir_all(&workdir)?;
    // Personality / standing instructions, shaping every agent run (Hermes's SOUL.md).
    let persona = std::fs::read_to_string(format!("{home}/SOUL.md")).ok();
    // Connect any MCP servers listed in mcp.json and borrow their tools.
    let mcp_tools = load_mcp(&home).await;
    if !mcp_tools.is_empty() {
        tracing::info!(count = mcp_tools.len(), "mcp tools available to the agent");
    }

    ledger.append("core.boot", "core", json!({ "version": VERSION, "addr": addr.to_string() }))?;

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
        persona,
        mcp_tools,
        browser: engram_agent::browser_session(),
    };

    let router = Router::new()
        .route("/", get(dashboard))
        .route("/health", get(health))
        .route("/v1/meter", get(meter))
        .route("/v1/memory/stats", get(memory_stats))
        .route("/v1/remember", post(remember))
        .route("/v1/recall", get(recall))
        .route("/v1/forget", post(forget))
        .route("/v1/skills", get(skills))
        .route("/v1/skills/{id}/run", post(run_skill))
        .route("/v1/swarm", post(run_swarm))
        .route("/v1/agent", post(agent_handler))
        .route("/v1/voice", post(voice_handler))
        .route("/v1/channel/{platform}", post(channels::channel_handler))
        .route("/v1/converse", post(converse_handler))
        .route("/v1/ledger/tail", get(ledger_tail))
        .route("/v1/ledger/verify", get(ledger_verify))
        .route("/v1/schedule", get(schedule_list).post(schedule_add))
        .route("/v1/schedule/{id}", axum::routing::delete(schedule_remove))
        .layer(axum::middleware::from_fn_with_state(app.clone(), keep_awake))
        .with_state(app.clone());

    // Inbound messaging channel: run as a Telegram bot if a token is configured.
    if let Ok(token) = std::env::var("ENGRAM_TELEGRAM_TOKEN") {
        tracing::info!("telegram channel active");
        telegram::spawn(app.clone(), token);
    }

    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(version = VERSION, %addr, idle_s = idle.as_secs(), "engram awake — http ready");

    axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            match run_until_idle(activity, idle).await {
                engram_core::WakeReason::Idle => tracing::info!("idle — sleeping to zero"),
                engram_core::WakeReason::Signal => tracing::info!("signal — sleeping to zero"),
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
    app.bus
        .emit(Spike::new("http.request", Priority::Normal, json!({ "path": path })));
    next.run(req).await
}

async fn dashboard() -> Html<&'static str> {
    Html(include_str!("../assets/index.html"))
}

async fn health() -> ApiResult {
    Ok(Json(json!({ "ok": true, "version": VERSION })))
}

async fn meter(State(app): State<App>) -> ApiResult {
    Ok(Json(serde_json::to_value(app.gateway.meter()).map_err(err)?))
}

async fn memory_stats(State(app): State<App>) -> ApiResult {
    Ok(Json(serde_json::to_value(app.memory.stats().map_err(err)?).map_err(err)?))
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
    let hits = app.memory.recall(&q.q, &regions, q.k.unwrap_or(5)).map_err(err)?;
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
}

/// Run the agent on a task with the full configured toolset (built-ins + MCP),
/// persona, and policy. Shared by the HTTP endpoint and the messaging channels.
pub(crate) async fn run_agent_task(
    app: &App,
    task: &str,
    max_steps: usize,
) -> Result<engram_agent::AgentRun, String> {
    let policy = engram_agent::Policy {
        allow_shell: std::env::var("ENGRAM_TOOLS_SHELL").as_deref() == Ok("1"),
        shell_backend: match std::env::var("ENGRAM_SHELL_BACKEND").as_deref() {
            Ok("docker") => Some(std::env::var("ENGRAM_DOCKER_IMAGE").unwrap_or_else(|_| "alpine".into())),
            Ok("ssh") => std::env::var("ENGRAM_SSH_HOST").ok().map(|h| format!("ssh:{h}")),
            Ok("singularity") => std::env::var("ENGRAM_SINGULARITY_IMAGE").ok().map(|i| format!("singularity:{i}")),
            _ => None,
        },
        ..Default::default()
    };
    let model = std::env::var("ENGRAM_MODEL").unwrap_or_else(|_| "claude-haiku".into());
    let ctx = engram_agent::ToolCtx {
        memory: app.memory.clone(),
        skills: app.registry.clone(),
        gateway: app.gateway.clone(),
        ledger: app.ledger.clone(),
        taint: engram_core::Taint::Trusted,
        policy,
        workdir: app.workdir.clone(),
        model: model.clone(),
        depth: 0,
        browser: app.browser.clone(),
    };
    let mut tools = engram_agent::default_tools();
    for t in &app.mcp_tools {
        tools = tools.with(t.clone());
    }
    let mut agent = engram_agent::Agent::new(app.gateway.clone(), tools, model).max_steps(max_steps);
    if let Some(p) = &app.persona {
        agent = agent.persona(p.clone());
    }
    agent.run(task, ctx).await.map_err(|e| e.to_string())
}

async fn agent_handler(State(app): State<App>, Json(r): Json<AgentReq>) -> ApiResult {
    let run = run_agent_task(&app, &r.task, r.max_steps.unwrap_or(8)).await.map_err(ApiError)?;
    Ok(Json(serde_json::to_value(run).map_err(err)?))
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
    let transcript = app.gateway.transcribe(&audio, fmt, "voice").await.map_err(err)?;
    let run = run_agent_task(&app, &transcript, 8).await.map_err(ApiError)?;
    let voice = r.voice.as_deref().unwrap_or("alloy");
    let audio_out = app.gateway.tts(&run.answer, voice, "voice").await.map_err(err)?;
    let audio_b64 = base64::engine::general_purpose::STANDARD.encode(&audio_out);
    Ok(Json(json!({ "transcript": transcript, "reply": run.answer, "audio_b64": audio_b64 })))
}

#[derive(Deserialize)]
struct ConverseReq {
    text: String,
}

async fn converse_handler(State(app): State<App>, Json(r): Json<ConverseReq>) -> ApiResult {
    let turn = converse::converse(&app.memory, &app.gateway, &r.text)
        .await
        .map_err(ApiError)?;
    Ok(Json(json!({
        "reply": turn.reply,
        "recalled": turn.recalled,
        "learned": turn.learned,
    })))
}

async fn skills(State(app): State<App>) -> ApiResult {
    let mut out = Vec::new();
    for id in app.registry.skills().map_err(err)? {
        let active = app.registry.active_version(&id).map_err(err)?;
        let versions = app.registry.versions(&id).map_err(err)?;
        out.push(json!({ "id": id, "active": active, "versions": versions }));
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

async fn schedule_add(State(app): State<App>, Json(r): Json<ScheduleReq>) -> ApiResult {
    let now = chrono::Utc::now();
    let recurrence = parse_schedule(&r.when, now).map_err(err)?;
    let job = app.sched.add(r.name, r.payload, recurrence, now).map_err(err)?;
    Ok(Json(serde_json::to_value(job).map_err(err)?))
}

async fn schedule_remove(State(app): State<App>, Path(id): Path<String>) -> ApiResult {
    let removed = app.sched.remove(&id).map_err(err)?;
    Ok(Json(json!({ "removed": removed })))
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
    std::env::var(key).ok().and_then(|s| s.parse().ok()).unwrap_or(default)
}

#[derive(Deserialize)]
struct McpServerCfg {
    name: String,
    command: String,
    #[serde(default)]
    args: Vec<String>,
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
            tracing::warn!(error = %e, "invalid mcp.json — ignoring");
            return Vec::new();
        }
    };
    let configs: Vec<(String, String, Vec<String>)> =
        cfg.into_iter().map(|c| (c.name, c.command, c.args)).collect();
    engram_agent::connect_servers(&configs).await
}

/// Choose the model backend. With `--features http` and ENGRAM_LLM_BASE_URL +
/// ENGRAM_LLM_API_KEY set, use a real OpenAI-compatible provider; otherwise the
/// offline mock. This is the single switch that turns the agent from offline-demo
/// into a real, model-backed assistant — for both completions and embeddings.
fn make_provider() -> Box<dyn Provider> {
    #[cfg(feature = "http")]
    {
        if let (Ok(base), Ok(key)) =
            (std::env::var("ENGRAM_LLM_BASE_URL"), std::env::var("ENGRAM_LLM_API_KEY"))
        {
            let id = std::env::var("ENGRAM_LLM_ID").unwrap_or_else(|_| "openai".into());
            tracing::info!(%base, "using http LLM provider");
            return Box::new(engram_gateway::HttpProvider::new(id, base, key));
        }
        tracing::warn!("http feature on but ENGRAM_LLM_BASE_URL/API_KEY unset — using mock");
    }
    Box::new(MockProvider)
}

fn init_tracing() {
    use tracing_subscriber::{fmt, EnvFilter};
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    fmt().with_env_filter(filter).with_target(false).compact().init();
}
