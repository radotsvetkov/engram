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
mod tasks;
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
    tasks: Arc<tasks::TaskStore>,
    /// Runtime-mutable shell consent — toggled by the desktop's approval card.
    allow_shell: Arc<std::sync::atomic::AtomicBool>,
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
        tasks: Arc::new(tasks::TaskStore::open(std::path::Path::new(&home))),
        allow_shell: Arc::new(std::sync::atomic::AtomicBool::new(
            std::env::var("ENGRAM_TOOLS_SHELL").as_deref() == Ok("1"),
        )),
    };

    let router = Router::new()
        .route("/", get(dashboard))
        .route("/health", get(health))
        .route("/v1/meter", get(meter))
        .route("/v1/memory/stats", get(memory_stats))
        .route("/v1/memory/recent", get(memory_recent))
        .route("/v1/remember", post(remember))
        .route("/v1/recall", get(recall))
        .route("/v1/forget", post(forget))
        .route("/v1/skills", get(skills))
        .route("/v1/skills/{id}/run", post(run_skill))
        .route("/v1/swarm", post(run_swarm))
        .route("/v1/agent", post(agent_handler))
        .route("/v1/voice", post(voice_handler))
        .route("/v1/voice/stream", get(voice_stream))
        .route("/v1/channel/{platform}", post(channels::channel_handler))
        .route("/v1/converse", post(converse_handler))
        .route("/v1/ledger/tail", get(ledger_tail))
        .route("/v1/ledger/verify", get(ledger_verify))
        .route("/v1/schedule", get(schedule_list).post(schedule_add))
        .route("/v1/schedule/preview", get(schedule_preview))
        .route("/v1/schedule/{id}", axum::routing::delete(schedule_remove))
        .route("/v1/tasks", get(tasks_list).post(tasks_create))
        .route("/v1/tasks/{id}", axum::routing::patch(tasks_update).delete(tasks_delete))
        .route("/v1/tasks/{id}/run", post(tasks_run))
        .route("/v1/tasks/{id}/audit", get(task_audit))
        .route("/v1/policy", get(policy_get).post(policy_set))
        .route("/v1/events", get(events))
        .layer(axum::middleware::from_fn_with_state(app.clone(), keep_awake))
        .layer(axum::middleware::from_fn(require_auth))
        .with_state(app.clone());

    // Inbound messaging channel: run as a Telegram bot if a token is configured.
    if let Ok(token) = std::env::var("ENGRAM_TELEGRAM_TOKEN") {
        tracing::info!("telegram channel active");
        telegram::spawn(app.clone(), token);
    }
    // Fire scheduled jobs while the daemon is awake.
    spawn_scheduler_tick(app.clone());

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

/// Optional bearer-token auth. When `ENGRAM_API_TOKEN` is unset (the local-desktop
/// default, bound to 127.0.0.1) every request passes. When set — for an exposed
/// deployment — every `/v1` call must present `Authorization: Bearer <token>` (or, for
/// EventSource/WebSocket which cannot set headers, `?token=<token>`). The dashboard,
/// health, and inbound webhooks (which carry their own platform auth) stay open.
async fn require_auth(req: axum::extract::Request, next: axum::middleware::Next) -> Response {
    let token = std::env::var("ENGRAM_API_TOKEN").unwrap_or_default();
    if token.is_empty() {
        return next.run(req).await;
    }
    let path = req.uri().path();
    if path == "/" || path == "/health" || path.starts_with("/v1/channel/") {
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

async fn dashboard() -> Html<String> {
    let html = include_str!("../assets/index.html");
    // When a token is configured, hand it to the first-party dashboard so its own fetches
    // authenticate; otherwise serve the page unchanged.
    let token = std::env::var("ENGRAM_API_TOKEN").unwrap_or_default();
    if token.is_empty() {
        return Html(html.to_string());
    }
    let inject = format!("<script>window.__ENGRAM_TOKEN={};</script>", serde_json::Value::String(token));
    Html(html.replacen("</head>", &format!("{inject}</head>"), 1))
}

async fn health() -> ApiResult {
    // "offline" when no real model provider is configured — the UI surfaces this honestly
    // rather than returning fake answers.
    let offline = std::env::var("ENGRAM_LLM_BASE_URL").is_err();
    Ok(Json(json!({ "ok": true, "version": VERSION, "offline": offline })))
}

async fn meter(State(app): State<App>) -> ApiResult {
    Ok(Json(serde_json::to_value(app.gateway.meter()).map_err(err)?))
}

async fn memory_stats(State(app): State<App>) -> ApiResult {
    Ok(Json(serde_json::to_value(app.memory.stats().map_err(err)?).map_err(err)?))
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
    let recs = app.memory.recent(region, q.n.unwrap_or(20).min(100)).map_err(err)?;
    Ok(Json(serde_json::to_value(recs).map_err(err)?))
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
    run_agent_task_cb(app, task, max_steps, engram_core::Taint::Trusted, None).await
}

/// Run the agent with an explicit initial taint. Untrusted-origin prompts (inbound
/// webhooks, Telegram) start `Untrusted`, so the no-egress guard applies from step one.
pub(crate) async fn run_agent_task_cb(
    app: &App,
    task: &str,
    max_steps: usize,
    taint: engram_core::Taint,
    on_step: Option<engram_agent::StepCallback>,
) -> Result<engram_agent::AgentRun, String> {
    let policy = engram_agent::Policy {
        allow_shell: app.allow_shell.load(std::sync::atomic::Ordering::Relaxed),
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
        taint,
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
    if let Some(cb) = on_step {
        agent = agent.on_step(cb);
    }
    agent.run(task, ctx).await.map_err(|e| e.to_string())
}

async fn agent_handler(State(app): State<App>, Json(r): Json<AgentReq>) -> ApiResult {
    let run = run_agent_task(&app, &r.task, r.max_steps.unwrap_or(8)).await.map_err(ApiError)?;
    Ok(Json(serde_json::to_value(run).map_err(err)?))
}

/// A live voice session over a WebSocket. The client streams audio as binary frames
/// and sends a text "end" to close a turn; the server transcribes the accumulated
/// audio, runs the agent, and replies with a JSON text frame (transcript + reply) and
/// a binary frame of synthesized speech. The connection stays open for many turns — a
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
                            .send(Ws::Text(json!({ "transcript": transcript, "reply": reply }).to_string().into()))
                            .await;
                        socket.send(Ws::Binary(out.into())).await
                    }
                    Err(e) => socket.send(Ws::Text(json!({ "error": e }).to_string().into())).await,
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
    let transcript = app.gateway.transcribe(audio, "wav", "voice").await.map_err(|e| e.to_string())?;
    let run = run_agent_task(app, &transcript, 8).await?;
    let out = app.gateway.tts(&run.answer, "alloy", "voice").await.map_err(|e| e.to_string())?;
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
    let transcript = app.gateway.transcribe(&audio, fmt, "voice").await.map_err(err)?;
    let run = run_agent_task(&app, &transcript, 8).await.map_err(ApiError)?;
    let voice = r.voice.as_deref().unwrap_or("alloy");
    let audio_out = app.gateway.tts(&run.answer, voice, "voice").await.map_err(err)?;
    let audio_b64 = base64::engine::general_purpose::STANDARD.encode(&audio_out);
    Ok(Json(json!({ "transcript": transcript, "reply": run.answer, "audio_b64": audio_b64 })))
}

/// Server-Sent Events: stream the neural bus so the desktop updates the moment anything
/// happens (a task starts, a step completes, a run finishes) instead of polling.
async fn events(
    State(app): State<App>,
) -> axum::response::sse::Sse<impl futures_core::Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>>
{
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
    Ok(Json(json!({ "allow_shell": app.allow_shell.load(std::sync::atomic::Ordering::Relaxed) })))
}

#[derive(Deserialize)]
struct PolicyReq {
    allow_shell: Option<bool>,
}

/// Grant or revoke a standing capability (the desktop's "always allow"). The decision is
/// recorded in the audit ledger, so even a consent change is on the record.
async fn policy_set(State(app): State<App>, Json(r): Json<PolicyReq>) -> ApiResult {
    if let Some(v) = r.allow_shell {
        app.allow_shell.store(v, std::sync::atomic::Ordering::Relaxed);
        let _ = app.ledger.append("policy.set", "user", json!({ "allow_shell": v }));
    }
    Ok(Json(json!({ "allow_shell": app.allow_shell.load(std::sync::atomic::Ordering::Relaxed) })))
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
    let t = app.tasks.create(r.title, r.detail, r.origin.unwrap_or_else(|| "manual".into()));
    app.bus.emit(Spike::new("task.create", Priority::Low, json!({ "id": t.id })));
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

async fn tasks_delete(State(app): State<App>, Path(id): Path<String>) -> ApiResult {
    Ok(Json(json!({ "removed": app.tasks.remove(&id) })))
}

/// Run a task with the agent and attach a glass-box receipt: mark it doing (and fire a
/// spike so the board shows Running), run, capture the cost delta and the signed ledger
/// head, then mark done — or failed if the agent hit its step limit. Shared by the HTTP
/// endpoint and the in-process scheduler.
pub(crate) async fn run_task_core(app: &App, id: &str) -> Result<tasks::Task, String> {
    let task = app.tasks.get(id).ok_or("task not found")?;
    // Atomically claim the task so two concurrent runs (double-click, HTTP + scheduler)
    // can't both execute and corrupt the receipt/cost delta.
    if !app.tasks.try_begin(id) {
        return Err("task is already running".into());
    }
    app.bus.emit(Spike::new("task.run", Priority::Normal, json!({ "id": id })));

    let prompt = if task.detail.trim().is_empty() {
        task.title.clone()
    } else {
        format!("{}\n\n{}", task.title, task.detail)
    };
    let before = app.gateway.meter();
    let started_ms = engram_core::now_ms() as i64;
    // Stream live progress onto the card and over the event bus as each step completes.
    let tasks = app.tasks.clone();
    let bus = app.bus.clone();
    let tid = id.to_string();
    let on_step: engram_agent::StepCallback = Arc::new(move |i, tool, _ok| {
        tasks.set_progress(&tid, format!("step {i} · {tool}"));
        bus.emit(Spike::new("task.step", Priority::Low, json!({ "id": tid.as_str(), "step": i, "tool": tool })));
    });
    let run = run_agent_task_cb(app, &prompt, 10, engram_core::Taint::Trusted, Some(on_step)).await?;
    let finished_ms = engram_core::now_ms() as i64;
    let after = app.gateway.meter();
    let (_, head) = app.ledger.head();

    let status = if run.stopped == "limit" { "failed" } else { "done" };
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
    };
    let result = app.tasks.finish(id, receipt, status).ok_or_else(|| "task vanished".to_string());
    app.bus.emit(Spike::new("task.done", Priority::Normal, json!({ "id": id, "status": status })));
    result
}

async fn tasks_run(State(app): State<App>, Path(id): Path<String>) -> ApiResult {
    let updated = run_task_core(&app, &id).await.map_err(ApiError)?;
    Ok(Json(serde_json::to_value(updated).map_err(err)?))
}

/// In-process scheduler: while the daemon is awake, fire due jobs by spawning a task and
/// running it. (On a sleeping zero-idle VPS the systemd timer wakes the core instead.)
fn spawn_scheduler_tick(app: App) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(30)).await;
            let now = chrono::Utc::now();
            for job in app.sched.due(now) {
                let title = job.payload.get("title").and_then(|v| v.as_str()).unwrap_or(&job.name);
                let task = app.tasks.create(title.to_string(), String::new(), "schedule".into());
                tracing::info!(job = %job.name, task = %task.id, "scheduler firing a task");
                let _ = run_task_core(&app, &task.id).await;
                let _ = app.sched.mark_fired(&job.id, now);
            }
        }
    });
}

/// The signed ledger slice for a task's run — the glass-box audit trail behind a card.
async fn task_audit(State(app): State<App>, Path(id): Path<String>) -> ApiResult {
    let task = app.tasks.get(&id).ok_or_else(|| ApiError("task not found".into()))?;
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
    Ok(Json(json!({ "entries": entries, "head": run.ledger_head_hash })))
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

#[derive(Deserialize)]
struct PreviewQuery {
    when: String,
}

/// Parse a natural-language "when" and show the next fire — without creating a job, so
/// the UI can preview live as the user types.
async fn schedule_preview(State(_app): State<App>, Query(q): Query<PreviewQuery>) -> ApiResult {
    let now = chrono::Utc::now();
    match engram_sched::parse(&q.when, now) {
        Ok(rec) => Ok(Json(json!({
            "ok": true,
            "next_fire_ms": rec.next_after(now).map(|t| t.timestamp_millis()),
        }))),
        Err(e) => Ok(Json(json!({ "ok": false, "error": e.to_string() }))),
    }
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
