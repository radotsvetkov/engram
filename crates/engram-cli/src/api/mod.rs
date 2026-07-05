//! The typed HTTP client for engramd.
//!
//! One [`Client`] holds the base URL + optional bearer token and exposes a
//! method per endpoint the CLI/TUI uses. Streaming endpoints hand back an
//! `mpsc` receiver fed by a background task, so the TUI event loop can simply
//! select over it.

// This is a complete typed client for the daemon's API surface; not every endpoint
// is wired into a command yet, so a few methods and stream fields are unused for now.
#![allow(dead_code)]

pub mod sse;
pub mod types;

use anyhow::{anyhow, Context, Result};
use futures_util::StreamExt;
use serde_json::{json, Value};
use tokio::sync::mpsc;

pub use sse::{ChatEvent, Spike, SseDecoder, TaskEvent};
pub use types::*;

/// Default daemon address, matching engramd's `ENGRAM_ADDR` default.
pub const DEFAULT_ADDR: &str = "127.0.0.1:8088";

#[derive(Clone)]
pub struct Client {
    base: String,
    token: Option<String>,
    http: reqwest::Client,
}

impl Client {
    /// Build a client for `base` (e.g. `http://127.0.0.1:8088`).
    pub fn new(base: impl Into<String>, token: Option<String>) -> Self {
        let http = reqwest::Client::builder()
            .user_agent(concat!("engram-cli/", env!("CARGO_PKG_VERSION")))
            // Per-request timeouts are applied selectively; streaming requests must not time out.
            .build()
            .expect("reqwest client");
        Self {
            base: base.into().trim_end_matches('/').to_string(),
            token,
            http,
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }

    fn auth(&self, rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.token {
            Some(t) => rb.bearer_auth(t),
            None => rb,
        }
    }

    // ---- core verbs -------------------------------------------------------

    async fn get_value(&self, path: &str) -> Result<Value> {
        let rb = self
            .auth(self.http.get(self.url(path)))
            .timeout(std::time::Duration::from_secs(20));
        let resp = rb.send().await.with_context(|| format!("GET {path}"))?;
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(anyhow!("GET {path} → {status}: {}", truncate(&body, 300)));
        }
        serde_json::from_str(&body).with_context(|| format!("decode {path}"))
    }

    async fn get<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T> {
        let v = self.get_value(path).await?;
        serde_json::from_value(v).with_context(|| format!("typed-decode {path}"))
    }

    async fn post_value(&self, path: &str, body: Value) -> Result<Value> {
        self.post_value_timeout(path, body, Some(std::time::Duration::from_secs(180)))
            .await
    }

    /// POST with an explicit timeout (or `None` for no client-side timeout). Long-running endpoints
    /// like `/v1/agent` — multi-step tool loops that can run for minutes — must pass `None`, else the
    /// CLI errors out while the daemon keeps running and the answer is lost.
    async fn post_value_timeout(
        &self,
        path: &str,
        body: Value,
        timeout: Option<std::time::Duration>,
    ) -> Result<Value> {
        let mut rb = self.auth(self.http.post(self.url(path))).json(&body);
        if let Some(t) = timeout {
            rb = rb.timeout(t);
        }
        let resp = rb.send().await.with_context(|| format!("POST {path}"))?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(anyhow!("POST {path} → {status}: {}", truncate(&text, 300)));
        }
        if text.trim().is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_str(&text).with_context(|| format!("decode {path}"))
    }

    async fn post<T: serde::de::DeserializeOwned>(&self, path: &str, body: Value) -> Result<T> {
        let v = self.post_value(path, body).await?;
        serde_json::from_value(v).with_context(|| format!("typed-decode {path}"))
    }

    // ---- status / trust spine --------------------------------------------

    pub async fn health(&self) -> Result<Health> {
        // Health must answer fast — used to probe whether the daemon is up.
        let rb = self
            .auth(self.http.get(self.url("/health")))
            .timeout(std::time::Duration::from_secs(3));
        let resp = rb.send().await.context("GET /health")?;
        resp.json().await.context("decode /health")
    }

    pub async fn meter(&self) -> Result<Meter> {
        self.get("/v1/meter").await
    }

    pub async fn ledger_verify(&self) -> Result<LedgerVerify> {
        self.get("/v1/ledger/verify").await
    }

    pub async fn ledger_tail(&self, n: usize) -> Result<Vec<LedgerEntry>> {
        self.get(&format!("/v1/ledger/tail?n={n}")).await
    }

    pub async fn ledger_pubkey(&self) -> Result<Value> {
        self.get_value("/v1/ledger/pubkey").await
    }

    // ---- memory -----------------------------------------------------------

    pub async fn memory_stats(&self) -> Result<MemoryStats> {
        self.get("/v1/memory/stats").await
    }

    /// Create a project, optionally bound to a working directory (attach-or-create on the daemon).
    pub async fn project_create(&self, name: &str, workdir: Option<&str>) -> Result<Project> {
        let mut body = serde_json::json!({ "name": name });
        if let Some(w) = workdir {
            body["workdir"] = serde_json::Value::String(w.to_string());
        }
        self.post("/v1/projects", body).await
    }

    pub async fn memory_recent(&self, region: Option<&str>, n: usize) -> Result<Vec<MemRecord>> {
        let mut path = format!("/v1/memory/recent?n={n}");
        if let Some(r) = region {
            path.push_str(&format!("&region={r}"));
        }
        self.get(&path).await
    }

    pub async fn recall(&self, q: &str, k: usize, task: Option<&str>) -> Result<Vec<RecallHit>> {
        self.recall_as_of(q, k, task, None).await
    }

    /// `as_of_ms: Some(t)` answers "what did I believe on this date" (bi-temporal time-travel)
    /// instead of ordinary current-state recall.
    pub async fn recall_as_of(
        &self,
        q: &str,
        k: usize,
        task: Option<&str>,
        as_of_ms: Option<i64>,
    ) -> Result<Vec<RecallHit>> {
        let mut path = format!("/v1/recall?q={}&k={}", urlencode(q), k);
        if let Some(t) = task {
            path.push_str(&format!("&task={}", urlencode(t)));
        }
        if let Some(t) = as_of_ms {
            path.push_str(&format!("&as_of={t}"));
        }
        self.get(&path).await
    }

    pub async fn remember(
        &self,
        region: &str,
        text: &str,
        importance: Option<f32>,
    ) -> Result<Value> {
        self.post_value(
            "/v1/remember",
            json!({ "region": region, "text": text, "importance": importance }),
        )
        .await
    }

    pub async fn forget(&self, id: i64) -> Result<Value> {
        self.post_value("/v1/forget", json!({ "id": id })).await
    }

    pub async fn consciousness(&self) -> Result<Consciousness> {
        self.get("/v1/consciousness").await
    }

    pub async fn consciousness_distill(&self) -> Result<Value> {
        // Distillation can take a while (it summarizes the whole self-model) — no client timeout.
        self.post_value_timeout("/v1/consciousness/distill", json!({}), None)
            .await
    }

    /// Edit an existing consciousness line's text in place (pins it — it survives re-distillation).
    pub async fn consciousness_edit(&self, id: &str, text: &str) -> Result<Value> {
        self.post_value(
            "/v1/consciousness/edit",
            json!({ "id": id, "text": text }),
        )
        .await
    }

    /// Add a new, permanently-pinned consciousness line (capped at the working-memory ceiling).
    pub async fn consciousness_add(&self, text: &str) -> Result<Value> {
        self.post_value("/v1/consciousness/add", json!({ "text": text }))
            .await
    }

    /// Remove a consciousness line by id.
    pub async fn consciousness_remove(&self, id: &str) -> Result<Value> {
        self.post_value("/v1/consciousness/remove", json!({ "id": id }))
            .await
    }

    /// Revert consciousness to its previous version.
    pub async fn consciousness_revert(&self) -> Result<Value> {
        self.post_value("/v1/consciousness/revert", json!({})).await
    }

    /// Not-yet-resolved proposed contradictions (crate::contradiction never applies one on its
    /// own - this is the inbox of things a human still needs to accept or reject).
    pub async fn supersessions(&self) -> Result<Value> {
        self.get_value("/v1/supersessions").await
    }

    /// Typed variant of [`Client::supersessions`], for surfaces (TUI) that render structured
    /// fields rather than walking a raw `Value`.
    pub async fn supersessions_typed(&self) -> Result<Vec<PendingSupersession>> {
        self.get("/v1/supersessions").await
    }

    pub async fn supersession_resolve(&self, id: i64, accept: bool) -> Result<Value> {
        self.post_value(
            &format!("/v1/supersessions/{id}/resolve"),
            json!({ "accept": accept }),
        )
        .await
    }

    /// Grounded-reflection facts (Phase D) - synthesized memories the reflection pass wrote, each
    /// citing exactly which facts it drew on via `metadata.source_ids`. `project`: restrict to that
    /// project's ring; omitted = the user-global ring only.
    pub async fn reflections(&self, project: Option<&str>) -> Result<Vec<MemRecord>> {
        let mut path = "/v1/memory/reflections".to_string();
        if let Some(p) = project {
            path.push_str(&format!("?scope_kind=project&scope_id={}", urlencode(p)));
        }
        self.get(&path).await
    }

    // ---- tasks ------------------------------------------------------------

    pub async fn tasks(&self) -> Result<Vec<Task>> {
        self.get("/v1/tasks").await
    }

    pub async fn task_create(
        &self,
        title: &str,
        detail: Option<&str>,
        origin: Option<&str>,
    ) -> Result<Task> {
        // The daemon's `detail` is a plain `String` with `#[serde(default)]`, so a
        // JSON `null` (what `Option` serializes to) fails to deserialize — send a
        // string, never null.
        self.post(
            "/v1/tasks",
            json!({
                "title": title,
                "detail": detail.unwrap_or(""),
                "origin": origin.unwrap_or("manual"),
            }),
        )
        .await
    }

    pub async fn task_run(&self, id: &str) -> Result<Task> {
        // A synchronous task run drives a full agent loop — no client timeout (see `agent`).
        let v = self
            .post_value_timeout(&format!("/v1/tasks/{id}/run"), json!({}), None)
            .await?;
        serde_json::from_value(v).context("typed-decode /v1/tasks/run")
    }

    pub async fn task_receipt(&self, id: &str) -> Result<Value> {
        self.get_value(&format!("/v1/tasks/{id}/receipt")).await
    }

    pub async fn task_audit(&self, id: &str) -> Result<Value> {
        self.get_value(&format!("/v1/tasks/{id}/audit")).await
    }

    // ---- skills -----------------------------------------------------------

    pub async fn skills(&self) -> Result<SkillsResp> {
        self.get("/v1/skills").await
    }

    pub async fn skill_run(&self, id: &str, input: &str) -> Result<SkillRun> {
        self.post(&format!("/v1/skills/{id}/run"), json!({ "input": input }))
            .await
    }

    pub async fn skill_set_enabled(&self, id: &str, enabled: bool) -> Result<Value> {
        self.post_value(
            &format!("/v1/skills/{id}/enabled"),
            json!({ "enabled": enabled }),
        )
        .await
    }

    /// Adopt a proposed skill: the daemon replays it against its recorded gold
    /// examples and activates it only if they reproduce — no client timeout,
    /// since the replay runs the skill for real.
    pub async fn skill_adopt(&self, id: &str) -> Result<Value> {
        self.post_value_timeout(&format!("/v1/skills/{id}/adopt"), json!({}), None)
            .await
    }

    /// Author + A/B-gate a candidate skill version. `wat` for a WASM skill, `source` for a
    /// process skill — the daemon dispatches on the active version's runtime, matching the
    /// desktop UI's improve modal. No client timeout: the replay runs the skill for real.
    pub async fn skill_improve(
        &self,
        id: &str,
        wat: Option<&str>,
        source: Option<&str>,
        interpreter: Option<&str>,
        description: Option<&str>,
    ) -> Result<Value> {
        let mut body = json!({});
        if let Some(w) = wat {
            body["wat"] = json!(w);
        }
        if let Some(s) = source {
            body["source"] = json!(s);
        }
        if let Some(i) = interpreter {
            body["interpreter"] = json!(i);
        }
        if let Some(d) = description {
            body["description"] = json!(d);
        }
        self.post_value_timeout(&format!("/v1/skills/{id}/improve"), body, None)
            .await
    }

    /// Set the active version of a skill — the explicit one-click promote/rollback control.
    pub async fn skill_activate(&self, id: &str, version: u32) -> Result<Value> {
        self.post_value(
            &format!("/v1/skills/{id}/activate"),
            json!({ "version": version }),
        )
        .await
    }

    /// Revert a skill to its previous version, or an explicit one.
    pub async fn skill_revert(&self, id: &str, version: Option<u32>) -> Result<Value> {
        let body = match version {
            Some(v) => json!({ "version": v }),
            None => json!({}),
        };
        self.post_value(&format!("/v1/skills/{id}/revert"), body).await
    }

    /// Record a runtime example as a gold `(input, accepted-output)` pair on the active version.
    pub async fn skill_teach(
        &self,
        id: &str,
        input: &str,
        gold: &str,
        reward: Option<f32>,
    ) -> Result<Value> {
        let mut body = json!({ "input": input, "gold": gold });
        if let Some(r) = reward {
            body["reward"] = json!(r);
        }
        self.post_value(&format!("/v1/skills/{id}/teach"), body).await
    }

    // ---- schedule ---------------------------------------------------------

    pub async fn schedule(&self) -> Result<Vec<ScheduleJob>> {
        self.get("/v1/schedule").await
    }

    pub async fn schedule_add(&self, name: &str, when: &str, payload: Value) -> Result<Value> {
        self.post_value(
            "/v1/schedule",
            json!({ "name": name, "when": when, "payload": payload }),
        )
        .await
    }

    /// Rename, retime, or repoint an existing job. `when` blank keeps the current cadence — the
    /// stored `Recurrence` is structured JSON that doesn't always round-trip losslessly back
    /// through the natural-language parser, so the daemon treats an omitted `when` as "unchanged".
    pub async fn schedule_update(
        &self,
        id: &str,
        name: &str,
        when: &str,
        payload: Value,
    ) -> Result<Value> {
        let mut body = serde_json::Map::new();
        body.insert("name".into(), json!(name));
        if !when.is_empty() {
            body.insert("when".into(), json!(when));
        }
        body.insert("payload".into(), payload);
        self.patch_value(&format!("/v1/schedule/{id}"), Value::Object(body))
            .await
    }

    pub async fn schedule_preview(&self, when: &str) -> Result<SchedulePreview> {
        // The daemon serves this as GET with a `when` query param (not a POST body).
        self.get(&format!("/v1/schedule/preview?when={}", urlencode(when)))
            .await
    }

    pub async fn schedule_run(&self, id: &str) -> Result<Value> {
        self.post_value(&format!("/v1/schedule/{id}/run"), json!({}))
            .await
    }

    // ---- autonomy / egress ------------------------------------------------

    pub async fn autonomy_report(&self) -> Result<AutonomyReport> {
        self.get("/v1/autonomy/report").await
    }

    pub async fn egress_pending(&self) -> Result<EgressPending> {
        self.get("/v1/egress/pending").await
    }

    pub async fn egress_approve(&self, scope: &str, dest: &str) -> Result<Value> {
        self.post_value(
            "/v1/egress/approve",
            json!({ "scope": scope, "dest": dest }),
        )
        .await
    }

    pub async fn egress_deny(&self, scope: &str, dest: &str) -> Result<Value> {
        self.post_value("/v1/egress/deny", json!({ "scope": scope, "dest": dest }))
            .await
    }

    // ---- introspection ----------------------------------------------------

    pub async fn tools(&self) -> Result<ToolsResp> {
        self.get("/v1/tools").await
    }

    pub async fn config(&self) -> Result<Config> {
        self.get("/v1/config").await
    }

    pub async fn config_raw(&self) -> Result<Value> {
        self.get_value("/v1/config").await
    }

    pub async fn config_set(&self, patch: Value) -> Result<Value> {
        self.post_value("/v1/config", patch).await
    }

    pub async fn projects(&self) -> Result<Vec<Project>> {
        self.get("/v1/projects").await
    }

    /// Create a real chat session under a project, so its turns persist server-side and its memory
    /// + working directory are scoped to that project. Returns the created session.
    pub async fn session_create(
        &self,
        project_id: &str,
        title: Option<&str>,
    ) -> Result<SessionMeta> {
        let mut body = serde_json::json!({ "project_id": project_id });
        if let Some(t) = title {
            body["title"] = serde_json::Value::String(t.to_string());
        }
        self.post("/v1/sessions", body).await
    }

    pub async fn sessions(&self, project: Option<&str>) -> Result<Vec<SessionMeta>> {
        let path = match project {
            Some(p) => format!("/v1/sessions?project={p}"),
            None => "/v1/sessions".to_string(),
        };
        self.get(&path).await
    }

    pub async fn session_detail(&self, id: &str) -> Result<SessionDetail> {
        self.get(&format!("/v1/sessions/{id}")).await
    }

    pub async fn channels(&self) -> Result<Value> {
        self.get_value("/v1/channels").await
    }

    /// Named agents registered with the daemon (`GET /v1/agents`).
    pub async fn agents_list(&self) -> Result<Vec<Value>> {
        let v = self.get_value("/v1/agents").await?;
        Ok(v.as_array().cloned().unwrap_or_default())
    }

    pub async fn agent_activity(&self, id: &str) -> Result<Value> {
        self.get_value(&format!("/v1/agents/{id}/activity")).await
    }

    async fn delete_value(&self, path: &str) -> Result<Value> {
        let rb = self
            .auth(self.http.delete(self.url(path)))
            .timeout(std::time::Duration::from_secs(20));
        let resp = rb.send().await.with_context(|| format!("DELETE {path}"))?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(anyhow!(
                "DELETE {path} → {status}: {}",
                truncate(&text, 200)
            ));
        }
        Ok(serde_json::from_str(&text).unwrap_or(Value::Null))
    }

    async fn patch_value(&self, path: &str, body: Value) -> Result<Value> {
        let rb = self
            .auth(self.http.patch(self.url(path)))
            .json(&body)
            .timeout(std::time::Duration::from_secs(20));
        let resp = rb.send().await.with_context(|| format!("PATCH {path}"))?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(anyhow!("PATCH {path} → {status}: {}", truncate(&text, 300)));
        }
        if text.trim().is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_str(&text).with_context(|| format!("decode {path}"))
    }

    pub async fn agents_create(&self, body: Value) -> Result<Value> {
        self.post_value("/v1/agents", body).await
    }

    pub async fn agents_update(&self, id: &str, body: Value) -> Result<Value> {
        self.post_value(&format!("/v1/agents/{id}"), body).await
    }

    pub async fn agents_delete(&self, id: &str) -> Result<Value> {
        self.delete_value(&format!("/v1/agents/{id}")).await
    }

    pub async fn agent_set_policy(&self, id: &str, body: Value) -> Result<Value> {
        self.post_value(&format!("/v1/agents/{id}/policy"), body)
            .await
    }

    pub async fn schedule_remove(&self, id: &str) -> Result<Value> {
        self.delete_value(&format!("/v1/schedule/{id}")).await
    }

    /// Test the current provider config (or a patch) with a tiny completion.
    pub async fn config_test(&self, patch: Value) -> Result<Value> {
        self.post_value("/v1/config/test", patch).await
    }

    // ---- agent / chat (non-stream) ---------------------------------------

    pub async fn agent(&self, task: &str, max_steps: Option<usize>) -> Result<AgentResp> {
        // No client timeout: agent runs are 24-step tool loops (web/browser/shell) that routinely
        // exceed any fixed cap. A timeout here would drop the answer the daemon is still producing.
        let v = self
            .post_value_timeout(
                "/v1/agent",
                json!({ "task": task, "max_steps": max_steps }),
                None,
            )
            .await?;
        serde_json::from_value(v).context("typed-decode /v1/agent")
    }

    pub async fn converse(&self, text: &str, session: Option<&str>) -> Result<ConverseDone> {
        self.post("/v1/converse", json!({ "text": text, "session": session }))
            .await
    }

    // ---- lifecycle --------------------------------------------------------

    pub async fn restart(&self) -> Result<Value> {
        self.post_value("/v1/restart", json!({})).await
    }

    pub async fn shutdown(&self) -> Result<Value> {
        self.post_value("/v1/shutdown", json!({})).await
    }

    /// Engage a halt. `on=false` releases it. The daemon's `on` flag defaults to
    /// false, so it must be sent explicitly or the call would *clear* a halt.
    pub async fn halt(&self, session: Option<&str>, on: bool) -> Result<Value> {
        self.post_value("/v1/halt", json!({ "on": on, "session": session }))
            .await
    }

    // ---- streaming --------------------------------------------------------

    /// Stream a live chat turn. The returned receiver yields [`ChatEvent`]s and
    /// closes when the turn ends or the connection drops.
    pub fn converse_stream(
        &self,
        text: String,
        session: Option<String>,
        attachments: Vec<Value>,
    ) -> mpsc::UnboundedReceiver<ChatEvent> {
        let (tx, rx) = mpsc::unbounded_channel();
        let req = self
            .auth(self.http.post(self.url("/v1/converse/stream")))
            .json(&json!({ "text": text, "session": session, "attachments": attachments }));
        spawn_sse(req, ChatEvent::from_frame, ChatEvent::Disconnected, tx);
        rx
    }

    /// Stream a task run.
    pub fn task_run_stream(&self, id: &str) -> mpsc::UnboundedReceiver<TaskEvent> {
        let (tx, rx) = mpsc::unbounded_channel();
        let req = self
            .auth(
                self.http
                    .post(self.url(&format!("/v1/tasks/{id}/run/stream"))),
            )
            .json(&json!({}));
        spawn_sse(req, TaskEvent::from_frame, TaskEvent::Disconnected, tx);
        rx
    }

    /// Stream the global spike bus.
    pub fn events_stream(&self) -> mpsc::UnboundedReceiver<Spike> {
        let (tx, rx) = mpsc::unbounded_channel();
        let req = self.auth(self.http.get(self.url("/v1/events")));
        spawn_sse(
            req,
            Spike::from_frame,
            |_| Spike {
                topic: "__disconnected".into(),
                payload: Value::Null,
            },
            tx,
        );
        rx
    }
}

/// Drive an SSE response in the background, mapping each frame into `T` and
/// pushing it down `tx`. On stream end/error, sends one final disconnect event.
fn spawn_sse<T, M, D>(
    req: reqwest::RequestBuilder,
    map: M,
    on_disconnect: D,
    tx: mpsc::UnboundedSender<T>,
) where
    T: Send + 'static,
    M: Fn(&str, &str) -> Option<T> + Send + 'static,
    D: Fn(String) -> T + Send + 'static,
{
    tokio::spawn(async move {
        let resp = match req.send().await {
            Ok(r) => r,
            Err(e) => {
                let _ = tx.send(on_disconnect(format!("connect failed: {e}")));
                return;
            }
        };
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            let _ = tx.send(on_disconnect(format!("{status}: {}", truncate(&body, 200))));
            return;
        }
        let mut decoder = SseDecoder::new();
        let mut frames = Vec::new();
        let mut stream = resp.bytes_stream();
        // Did the run send a terminal frame before the stream closed? The daemon emits a `done` (or
        // `error`) event and then closes the connection, so a clean EOF is EXPECTED after one — the
        // synthetic disconnect below must fire ONLY when no terminal frame arrived, otherwise every
        // successful reply is followed by a spurious "stream ended" error + reconnect in the TUI.
        let mut saw_terminal = false;
        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(bytes) => {
                    decoder.push(&bytes, &mut frames);
                    for (ev, data) in frames.drain(..) {
                        if ev == "done" || ev == "error" {
                            saw_terminal = true;
                        }
                        if let Some(item) = map(&ev, &data) {
                            if tx.send(item).is_err() {
                                return; // receiver dropped
                            }
                        }
                    }
                }
                Err(e) => {
                    let _ = tx.send(on_disconnect(format!("stream error: {e}")));
                    return;
                }
            }
        }
        // Clean EOF with no terminal frame — the daemon's run task finished without sending one (a
        // panic, or a graceful restart mid-run). Emit a synthetic disconnect so the consumer always
        // sees a terminal event and never hangs "streaming" forever. Suppressed after a real terminal.
        if !saw_terminal {
            let _ = tx.send(on_disconnect("stream ended before the run finished".into()));
        }
    });
}

/// Minimal percent-encoding for query values (RFC 3986 unreserved set passes through).
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(n).collect();
        out.push('…');
        out
    }
}

// ---- daemon discovery / spawning -----------------------------------------

/// Resolve the daemon base URL from an explicit override or `ENGRAM_ADDR`.
pub fn resolve_base(explicit: Option<&str>) -> String {
    if let Some(b) = explicit {
        return normalize_base(b);
    }
    if let Ok(addr) = std::env::var("ENGRAM_ADDR") {
        return normalize_base(&addr);
    }
    normalize_base(DEFAULT_ADDR)
}

fn normalize_base(s: &str) -> String {
    let s = s.trim();
    if s.starts_with("http://") || s.starts_with("https://") {
        s.trim_end_matches('/').to_string()
    } else {
        format!("http://{}", s.trim_end_matches('/'))
    }
}

/// Resolve the bearer token from an override or `ENGRAM_API_TOKEN`.
pub fn resolve_token(explicit: Option<&str>) -> Option<String> {
    explicit
        .map(|s| s.to_string())
        .or_else(|| std::env::var("ENGRAM_API_TOKEN").ok())
        .filter(|s| !s.is_empty())
}
