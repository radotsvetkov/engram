//! MCP client - connect to any Model Context Protocol server and borrow its tools.
//!
//! This is the parity multiplier. Rather than hand-coding 60+ integrations, Engram
//! speaks MCP (JSON-RPC 2.0): it launches/connects a server, lists its tools, and wraps
//! each as a native [`Tool`] that joins the agent's registry. Any community MCP server -
//! filesystem, GitHub, Slack, a browser driver, a database - becomes available to the
//! agent, audited through the same ledger as everything else.
//!
//! Two transports are supported:
//!   * **stdio** — launch a subprocess and speak newline-delimited JSON-RPC over its pipes.
//!   * **streamable-HTTP** — POST JSON-RPC to a remote server's URL (optionally with a
//!     bearer token), parsing either a plain `application/json` reply or a `text/event-stream`
//!     (SSE) body. This is where the ecosystem moved in 2025-26 (hosted GitHub / Notion /
//!     Linear / Sentry / Atlassian servers), and it needs no Node/npx on the box. Requires
//!     the `web` feature (default-on) for `reqwest`.

use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;

use crate::tool::{Tool, ToolCtx};

/// A hung MCP tool call must never wedge the run. `call_tool` bounds every request with this
/// ceiling *unless* the caller derives a tighter one from the run policy. It is deliberately
/// generous — a slow remote server or a big file read is legitimate — but finite, so the io
/// Mutex is always released and the halt kill-switch can make progress at the next step boundary.
const CALL_CEILING: Duration = Duration::from_secs(120);

/// A tool as advertised by an MCP server.
#[derive(Clone, Debug)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub schema: Value,
}

/// A resource as advertised by an MCP server's `resources/list`. A resource is a piece of
/// context the server can serve by URI (a file, a DB row, a wiki page) — the read side of MCP,
/// complementing tools (the act side).
#[derive(Clone, Debug)]
pub struct ResourceSpec {
    pub uri: String,
    pub name: String,
    pub description: String,
    /// MIME type the server advertises for the resource, if any (`text/plain`, `application/json`…).
    pub mime_type: Option<String>,
}

/// A prompt template as advertised by an MCP server's `prompts/list`. A prompt is a reusable,
/// parameterised message the server can expand into concrete conversation turns via `prompts/get`.
#[derive(Clone, Debug)]
pub struct PromptSpec {
    pub name: String,
    pub description: String,
    /// The named arguments the template accepts, mirrored verbatim from the server so a caller
    /// (or the UI) can prompt for them before `prompts/get`.
    pub arguments: Vec<PromptArgument>,
}

/// One argument a [`PromptSpec`] accepts.
#[derive(Clone, Debug)]
pub struct PromptArgument {
    pub name: String,
    pub description: String,
    pub required: bool,
}

/// The stdio side of a connection: a live subprocess we speak newline-delimited JSON-RPC to.
struct Io {
    _child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
    /// Set once a request times out (or the peer closes). A dead connection can no longer be
    /// trusted to id-match responses — a late reply to the abandoned request would corrupt the
    /// next call — so every subsequent request fails fast instead of reading stale frames.
    dead: bool,
}

/// The transport backing a [`McpClient`].
// One instance per connection, so the size gap between the stdio and HTTP variants is
// irrelevant to performance — not worth an extra allocation to equalize.
#[allow(clippy::large_enum_variant)]
enum Transport {
    /// A subprocess we own; each request grabs the Mutex, writes a line, and reads until the
    /// matching id. Kept behind a Mutex because stdio is a single duplex stream.
    Stdio(Mutex<Io>),
    /// A remote streamable-HTTP server. Stateless per request — each `request`/`notify` is its
    /// own POST — so no Mutex is needed. `next_id` is a monotonic counter for JSON-RPC ids.
    #[cfg(feature = "web")]
    Http(HttpTransport),
}

#[cfg(feature = "web")]
struct HttpTransport {
    client: reqwest::Client,
    url: String,
    bearer: Option<String>,
    next_id: std::sync::atomic::AtomicU64,
    /// The MCP session id handed back by the server on `initialize` (via the `Mcp-Session-Id`
    /// response header). Echoed on every subsequent request so the server can route stateful
    /// sessions. `None` for stateless servers that don't issue one.
    session: Mutex<Option<String>>,
}

/// A live connection to one MCP server.
pub struct McpClient {
    transport: Transport,
    server: String,
}

impl McpClient {
    /// Launch `command args…`, perform the MCP handshake, and return the client plus the
    /// tools it offers.
    pub async fn connect(
        server: &str,
        command: &str,
        args: &[String],
        env: &std::collections::BTreeMap<String, String>,
        cwd: Option<&str>,
    ) -> Result<(Arc<Self>, Vec<ToolSpec>), String> {
        let mut cmd = Command::new(command);
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        // Per-server secrets/config, scoped to THIS subprocess only (never the daemon's env).
        for (k, v) in env {
            cmd.env(k, v);
        }
        if let Some(dir) = cwd.filter(|d| !d.is_empty()) {
            cmd.current_dir(dir);
        }
        let mut child = cmd.spawn().map_err(|e| format!("spawn '{command}': {e}"))?;
        let stdin = child.stdin.take().ok_or("no stdin")?;
        let stdout = BufReader::new(child.stdout.take().ok_or("no stdout")?);
        let client = Arc::new(McpClient {
            transport: Transport::Stdio(Mutex::new(Io {
                _child: child,
                stdin,
                stdout,
                next_id: 1,
                dead: false,
            })),
            server: server.to_string(),
        });
        client.handshake().await?;
        let tools = client.list_tools().await?;
        Ok((client, tools))
    }

    /// Connect to a remote streamable-HTTP MCP server at `url`, optionally authenticated with a
    /// bearer token. Performs the same `initialize` + `tools/list` handshake as stdio.
    #[cfg(feature = "web")]
    pub async fn connect_http(
        server: &str,
        url: &str,
        bearer: Option<&str>,
    ) -> Result<(Arc<Self>, Vec<ToolSpec>), String> {
        let client = reqwest::Client::builder()
            .build()
            .map_err(|e| format!("build http client: {e}"))?;
        let mcp = Arc::new(McpClient {
            transport: Transport::Http(HttpTransport {
                client,
                url: url.to_string(),
                bearer: bearer.map(|b| b.to_string()),
                next_id: std::sync::atomic::AtomicU64::new(1),
                session: Mutex::new(None),
            }),
            server: server.to_string(),
        });
        mcp.handshake().await?;
        let tools = mcp.list_tools().await?;
        Ok((mcp, tools))
    }

    /// The MCP `initialize` handshake, bounded so a hung/misbehaving server can't block boot
    /// (`load_mcp` runs before the daemon serves) or a `--run-due` wake forever. 10s is generous
    /// for `initialize` + the `notifications/initialized` follow-up.
    async fn handshake(&self) -> Result<(), String> {
        let deadline = Duration::from_secs(10);
        tokio::time::timeout(
            deadline,
            self.request(
                "initialize",
                json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "clientInfo": { "name": "engram", "version": "0.1" }
                }),
            ),
        )
        .await
        .map_err(|_| "mcp initialize timed out".to_string())??;
        self.notify("notifications/initialized", json!({})).await?;
        Ok(())
    }

    /// Fetch and parse `tools/list`, bounded by the same 10s handshake deadline.
    async fn list_tools(&self) -> Result<Vec<ToolSpec>, String> {
        let deadline = Duration::from_secs(10);
        let listed = tokio::time::timeout(deadline, self.request("tools/list", json!({})))
            .await
            .map_err(|_| "mcp tools/list timed out".to_string())??;
        let tools = listed["tools"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|t| {
                        let name = t["name"].as_str()?.to_string();
                        let schema = match &t["inputSchema"] {
                            Value::Object(_) => t["inputSchema"].clone(),
                            _ => json!({ "type": "object" }),
                        };
                        Some(ToolSpec {
                            name,
                            description: t["description"].as_str().unwrap_or("").to_string(),
                            schema,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(tools)
    }

    async fn request(&self, method: &str, params: Value) -> Result<Value, String> {
        match &self.transport {
            Transport::Stdio(io) => self.request_stdio(io, method, params).await,
            #[cfg(feature = "web")]
            Transport::Http(http) => self.request_http(http, method, params).await,
        }
    }

    async fn request_stdio(
        &self,
        io: &Mutex<Io>,
        method: &str,
        params: Value,
    ) -> Result<Value, String> {
        let mut io = io.lock().await;
        if io.dead {
            return Err(
                "mcp connection is dead (a prior request timed out or the peer closed)".into(),
            );
        }
        let id = io.next_id;
        io.next_id += 1;
        let line = format!(
            "{}\n",
            json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
        );
        io.stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|e| e.to_string())?;
        io.stdin.flush().await.map_err(|e| e.to_string())?;
        loop {
            let mut buf = String::new();
            let n = io
                .stdout
                .read_line(&mut buf)
                .await
                .map_err(|e| e.to_string())?;
            if n == 0 {
                io.dead = true;
                return Err("mcp server closed the connection".into());
            }
            let v: Value = match serde_json::from_str(buf.trim()) {
                Ok(v) => v,
                Err(_) => continue, // skip non-JSON log lines
            };
            if v.get("id").and_then(|x| x.as_u64()) == Some(id) {
                if let Some(err) = v.get("error") {
                    return Err(err.to_string());
                }
                return Ok(v.get("result").cloned().unwrap_or(Value::Null));
            }
            // otherwise a notification or unrelated message - keep reading.
        }
    }

    #[cfg(feature = "web")]
    async fn request_http(
        &self,
        http: &HttpTransport,
        method: &str,
        params: Value,
    ) -> Result<Value, String> {
        use std::sync::atomic::Ordering;
        let id = http.next_id.fetch_add(1, Ordering::Relaxed);
        let body = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        let mut req = http
            .client
            .post(&http.url)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            // Streamable HTTP servers may reply with either JSON or an SSE stream; advertise both.
            .header(
                reqwest::header::ACCEPT,
                "application/json, text/event-stream",
            )
            .json(&body);
        if let Some(tok) = &http.bearer {
            req = req.bearer_auth(tok);
        }
        if let Some(sid) = http.session.lock().await.clone() {
            req = req.header("Mcp-Session-Id", sid);
        }
        let resp = req.send().await.map_err(|e| e.to_string())?;
        // Capture a session id the server may have minted on this exchange (typically initialize).
        if let Some(sid) = resp
            .headers()
            .get("Mcp-Session-Id")
            .and_then(|h| h.to_str().ok())
            .map(|s| s.to_string())
        {
            *http.session.lock().await = Some(sid);
        }
        if !resp.status().is_success() {
            let code = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("mcp http {code}: {text}"));
        }
        let ctype = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|h| h.to_str().ok())
            .unwrap_or("")
            .to_string();
        let text = resp.text().await.map_err(|e| e.to_string())?;
        let v = if ctype.contains("text/event-stream") {
            parse_sse_response(&text, id).ok_or("mcp sse: no matching response frame")?
        } else if text.trim().is_empty() {
            // A 202 with no body is a valid ack for a notification; nothing to parse.
            Value::Null
        } else {
            serde_json::from_str(&text).map_err(|e| format!("mcp http parse: {e}: {text}"))?
        };
        if v.is_null() {
            return Ok(Value::Null);
        }
        if let Some(err) = v.get("error") {
            return Err(err.to_string());
        }
        Ok(v.get("result").cloned().unwrap_or(Value::Null))
    }

    async fn notify(&self, method: &str, params: Value) -> Result<(), String> {
        match &self.transport {
            Transport::Stdio(io) => {
                let mut io = io.lock().await;
                if io.dead {
                    return Err("mcp connection is dead".into());
                }
                let line = format!(
                    "{}\n",
                    json!({ "jsonrpc": "2.0", "method": method, "params": params })
                );
                io.stdin
                    .write_all(line.as_bytes())
                    .await
                    .map_err(|e| e.to_string())?;
                io.stdin.flush().await.map_err(|e| e.to_string())
            }
            #[cfg(feature = "web")]
            Transport::Http(http) => {
                let body = json!({ "jsonrpc": "2.0", "method": method, "params": params });
                let mut req = http
                    .client
                    .post(&http.url)
                    .header(reqwest::header::CONTENT_TYPE, "application/json")
                    .header(
                        reqwest::header::ACCEPT,
                        "application/json, text/event-stream",
                    )
                    .json(&body);
                if let Some(tok) = &http.bearer {
                    req = req.bearer_auth(tok);
                }
                if let Some(sid) = http.session.lock().await.clone() {
                    req = req.header("Mcp-Session-Id", sid);
                }
                let resp = req.send().await.map_err(|e| e.to_string())?;
                if !resp.status().is_success() {
                    return Err(format!("mcp http notify {}", resp.status()));
                }
                Ok(())
            }
        }
    }

    /// Mark a stdio connection dead so every later request fails fast. The child is killed on
    /// drop (`kill_on_drop(true)`); we can't drop it here (the `McpClient` may be shared), but
    /// setting `dead` guarantees no more reads and releases the Mutex for other callers to see
    /// the failure immediately instead of blocking on a wedged read.
    async fn mark_dead(&self) {
        if let Transport::Stdio(io) = &self.transport {
            io.lock().await.dead = true;
        }
    }

    /// Call a remote tool and return its text content, bounded by `deadline` so a hung server
    /// (a stdio child that stopped responding, a stalled HTTP peer) can never wedge the run. On
    /// timeout the connection is marked dead — the io Mutex is released, subsequent calls fail
    /// fast, and the run can be halted at the next step boundary.
    pub async fn call_tool(
        &self,
        name: &str,
        args: &Value,
        deadline: Duration,
    ) -> Result<String, String> {
        let fut = self.request("tools/call", json!({ "name": name, "arguments": args }));
        let r = match tokio::time::timeout(deadline, fut).await {
            Ok(res) => res?,
            Err(_) => {
                self.mark_dead().await;
                return Err(format!(
                    "mcp tool '{name}' timed out after {}s (connection dropped)",
                    deadline.as_secs()
                ));
            }
        };
        if let Some(content) = r["content"].as_array() {
            let text = content
                .iter()
                .filter_map(|c| c["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n");
            if !text.is_empty() {
                return Ok(text);
            }
        }
        Ok(r.to_string())
    }

    /// List the resources this server serves (`resources/list`). Bounded by `deadline` — on
    /// timeout the connection is marked dead so the run can halt at the next step boundary, exactly
    /// like [`call_tool`]. A server that doesn't implement resources returns an empty list rather
    /// than an error (we simply find no `resources` array).
    pub async fn list_resources(&self, deadline: Duration) -> Result<Vec<ResourceSpec>, String> {
        let fut = self.request("resources/list", json!({}));
        let r = match tokio::time::timeout(deadline, fut).await {
            Ok(res) => res?,
            Err(_) => {
                self.mark_dead().await;
                return Err(format!(
                    "mcp resources/list timed out after {}s (connection dropped)",
                    deadline.as_secs()
                ));
            }
        };
        let resources = r["resources"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|res| {
                        let uri = res["uri"].as_str()?.to_string();
                        Some(ResourceSpec {
                            name: res["name"].as_str().unwrap_or(&uri).to_string(),
                            uri,
                            description: res["description"].as_str().unwrap_or("").to_string(),
                            mime_type: res["mimeType"].as_str().map(|s| s.to_string()),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(resources)
    }

    /// Read one resource by URI (`resources/read`) and return its text content, bounded by
    /// `deadline` (on timeout the connection is marked dead). A resource reply is a `contents`
    /// array of entries that carry either a `text` string or a base64 `blob`; we concatenate the
    /// text parts (the common case), falling back to the raw JSON when a resource is purely binary.
    pub async fn read_resource(&self, uri: &str, deadline: Duration) -> Result<String, String> {
        let fut = self.request("resources/read", json!({ "uri": uri }));
        let r = match tokio::time::timeout(deadline, fut).await {
            Ok(res) => res?,
            Err(_) => {
                self.mark_dead().await;
                return Err(format!(
                    "mcp resource '{uri}' timed out after {}s (connection dropped)",
                    deadline.as_secs()
                ));
            }
        };
        if let Some(contents) = r["contents"].as_array() {
            let text = contents
                .iter()
                .filter_map(|c| c["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n");
            if !text.is_empty() {
                return Ok(text);
            }
        }
        Ok(r.to_string())
    }

    /// List the prompt templates this server offers (`prompts/list`). Bounded by `deadline` (on
    /// timeout the connection is marked dead). A server without prompts returns an empty list.
    pub async fn list_prompts(&self, deadline: Duration) -> Result<Vec<PromptSpec>, String> {
        let fut = self.request("prompts/list", json!({}));
        let r = match tokio::time::timeout(deadline, fut).await {
            Ok(res) => res?,
            Err(_) => {
                self.mark_dead().await;
                return Err(format!(
                    "mcp prompts/list timed out after {}s (connection dropped)",
                    deadline.as_secs()
                ));
            }
        };
        let prompts = r["prompts"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|p| {
                        let name = p["name"].as_str()?.to_string();
                        let arguments = p["arguments"]
                            .as_array()
                            .map(|args| {
                                args.iter()
                                    .filter_map(|a| {
                                        Some(PromptArgument {
                                            name: a["name"].as_str()?.to_string(),
                                            description: a["description"]
                                                .as_str()
                                                .unwrap_or("")
                                                .to_string(),
                                            required: a["required"].as_bool().unwrap_or(false),
                                        })
                                    })
                                    .collect()
                            })
                            .unwrap_or_default();
                        Some(PromptSpec {
                            name,
                            description: p["description"].as_str().unwrap_or("").to_string(),
                            arguments,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(prompts)
    }

    /// Fetch and expand a prompt template by name (`prompts/get`), passing `args` as the template's
    /// named arguments (an empty object when the template takes none). Bounded by `deadline` (on
    /// timeout the connection is marked dead). Returns the expanded messages' text content joined
    /// with newlines — each message carries a `role` and a `content` block whose `text` we pull —
    /// falling back to the raw JSON when the reply has no text parts.
    pub async fn get_prompt(
        &self,
        name: &str,
        args: &Value,
        deadline: Duration,
    ) -> Result<String, String> {
        // `arguments` is optional in the spec; only send it when the caller supplied an object.
        let params = if args.is_object() {
            json!({ "name": name, "arguments": args })
        } else {
            json!({ "name": name })
        };
        let fut = self.request("prompts/get", params);
        let r = match tokio::time::timeout(deadline, fut).await {
            Ok(res) => res?,
            Err(_) => {
                self.mark_dead().await;
                return Err(format!(
                    "mcp prompt '{name}' timed out after {}s (connection dropped)",
                    deadline.as_secs()
                ));
            }
        };
        if let Some(messages) = r["messages"].as_array() {
            let text = messages
                .iter()
                .filter_map(|m| m["content"]["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n");
            if !text.is_empty() {
                return Ok(text);
            }
        }
        Ok(r.to_string())
    }

    pub fn server(&self) -> &str {
        &self.server
    }
}

/// Extract the JSON-RPC response with id `id` from an SSE (`text/event-stream`) body. Each event
/// carries one or more `data:` lines whose concatenation is a JSON-RPC message; we return the first
/// whose id matches (streamable-HTTP servers may interleave server-initiated notifications).
#[cfg(feature = "web")]
fn parse_sse_response(body: &str, id: u64) -> Option<Value> {
    let mut data = String::new();
    let consider = |data: &str| -> Option<Value> {
        let trimmed = data.trim();
        if trimmed.is_empty() {
            return None;
        }
        let v: Value = serde_json::from_str(trimmed).ok()?;
        if v.get("id").and_then(|x| x.as_u64()) == Some(id) {
            Some(v)
        } else {
            None
        }
    };
    for line in body.lines() {
        if let Some(rest) = line.strip_prefix("data:") {
            // Per the SSE spec a single leading space after the colon is stripped.
            data.push_str(rest.strip_prefix(' ').unwrap_or(rest));
            data.push('\n');
        } else if line.trim().is_empty() {
            // Blank line terminates an event: try to match, then reset for the next one.
            if let Some(v) = consider(&data) {
                return Some(v);
            }
            data.clear();
        }
    }
    // Trailing event with no terminating blank line.
    consider(&data)
}

/// An Engram tool backed by a remote MCP tool. Named `mcp_<server>_<tool>` to avoid
/// collisions with built-ins.
pub struct McpTool {
    client: Arc<McpClient>,
    engram_name: String,
    remote_name: String,
    description: String,
    schema: Value,
    /// A first-party server the user explicitly trusts. When false (the default), reading from
    /// this server is treated as untrusted AND sensitive, so it arms the no-egress guard - an
    /// attacker who can influence the server's content (a Gmail/scraper MCP) cannot launder it
    /// into a trusted run and then exfiltrate.
    trusted: bool,
}

impl McpTool {
    pub fn new(client: Arc<McpClient>, spec: ToolSpec) -> Self {
        let engram_name = format!("mcp_{}_{}", client.server(), spec.name);
        Self {
            client,
            engram_name,
            remote_name: spec.name,
            description: spec.description,
            schema: spec.schema,
            trusted: false,
        }
    }
    /// Mark this server as first-party trusted (its reads no longer taint / mark sensitive).
    pub fn trusted(mut self, trusted: bool) -> Self {
        self.trusted = trusted;
        self
    }
}

#[async_trait]
impl Tool for McpTool {
    fn name(&self) -> &str {
        &self.engram_name
    }
    fn description(&self) -> &str {
        &self.description
    }
    fn schema(&self) -> Value {
        self.schema.clone()
    }
    fn is_egress(&self) -> bool {
        // MCP tools are opaque external capabilities - treat them as egress so a tainted+
        // sensitive run cannot reach them (default-deny under the trifecta gate).
        true
    }
    fn taints(&self) -> bool {
        // An untrusted server's reply is attacker-influenceable content; reading it taints the
        // run (unless the server was explicitly marked first-party trusted).
        !self.trusted
    }
    fn reads_sensitive(&self) -> bool {
        // An authenticated MCP server (inbox, DB, drive) surfaces the user's private data
        // REGARDLESS of trust: marking a server first-party only means we trust its content not
        // to carry an attacker's instructions (it stops tainting), NOT that its data is public.
        // So sensitive stays armed - a trusted DB read of the user's records must still block
        // exfiltration if the run later reads untrusted content.
        true
    }
    fn side_effecting(&self) -> bool {
        // Opaque external capability - assume it can change the world (preview-gated).
        true
    }
    async fn run(&self, args: &Value, ctx: &ToolCtx) -> Result<String, String> {
        let _ = ctx.ledger.append(
            "agent.mcp",
            "agent",
            json!({ "server": self.client.server(), "tool": self.remote_name }),
        );
        self.client
            .call_tool(&self.remote_name, args, ingress_deadline(ctx))
            .await
    }
}

/// The per-call MCP deadline. Bounded above by [`CALL_CEILING`] so a hung server can't outlive the
/// run, but floored WELL ABOVE the shell/tool per-command timeout (which is tuned for local shell
/// commands, ~30s): an MCP server legitimately doing a slow DB query or API round-trip must not be
/// declared dead — a timeout marks the whole stdio connection dead for the rest of the session, so a
/// too-tight deadline would brick a merely-slow (not hung) server on its first slow call.
const MCP_MIN_DEADLINE_SECS: u64 = 60;
fn ingress_deadline(ctx: &ToolCtx) -> Duration {
    Duration::from_secs(ctx.policy.timeout_secs.max(MCP_MIN_DEADLINE_SECS)).min(CALL_CEILING)
}

/// An Engram tool that reads a resource from one MCP server (`resources/read`). This is the
/// **ingress** counterpart to [`McpTool`]: it pulls attacker-influenceable *content* into the run
/// (a file, a wiki page, a DB row the server serves by URI), so it carries the same taint/sensitive
/// posture as a tool call from the same server — an untrusted server's resource must taint the run
/// and count as sensitive so it can't be laundered into a trusted run and exfiltrated. Unlike
/// [`McpTool`] it is NOT egress (it only reads) and NOT side-effecting (it changes nothing).
pub struct McpResourceTool {
    client: Arc<McpClient>,
    engram_name: String,
    trusted: bool,
}

impl McpResourceTool {
    pub fn new(client: Arc<McpClient>) -> Self {
        let engram_name = format!("mcp_{}_read_resource", client.server());
        Self {
            client,
            engram_name,
            trusted: false,
        }
    }
    /// Mark the backing server first-party trusted (its reads no longer taint the run).
    pub fn trusted(mut self, trusted: bool) -> Self {
        self.trusted = trusted;
        self
    }
}

#[async_trait]
impl Tool for McpResourceTool {
    fn name(&self) -> &str {
        &self.engram_name
    }
    fn description(&self) -> &str {
        "Read one MCP resource from this server by its URI (as advertised in resources/list) and \
         return its text content."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "uri": { "type": "string", "description": "the resource URI to read (e.g. from resources/list)" }
            },
            "required": ["uri"]
        })
    }
    fn taints(&self) -> bool {
        // Same rule as McpTool: an untrusted server's content is attacker-influenceable.
        !self.trusted
    }
    fn reads_sensitive(&self) -> bool {
        // A resource is surfaced from an authenticated server's private context; sensitive stays
        // armed regardless of trust (trust only stops tainting), exactly like McpTool.
        true
    }
    async fn run(&self, args: &Value, ctx: &ToolCtx) -> Result<String, String> {
        let uri = crate::tools::arg_str(args, "uri")?;
        let _ = ctx.ledger.append(
            "agent.mcp_resource",
            "agent",
            json!({ "server": self.client.server(), "uri": uri }),
        );
        self.client.read_resource(uri, ingress_deadline(ctx)).await
    }
}

/// An Engram tool that expands a prompt template from one MCP server (`prompts/get`). Like
/// [`McpResourceTool`] this is **ingress**: the expanded messages are server-authored content, so it
/// carries the same taint/sensitive posture as a call from the same server (an untrusted server's
/// prompt expansion could smuggle instructions and must taint the run). NOT egress, NOT side-effecting.
pub struct McpPromptTool {
    client: Arc<McpClient>,
    engram_name: String,
    trusted: bool,
}

impl McpPromptTool {
    pub fn new(client: Arc<McpClient>) -> Self {
        let engram_name = format!("mcp_{}_get_prompt", client.server());
        Self {
            client,
            engram_name,
            trusted: false,
        }
    }
    /// Mark the backing server first-party trusted (its expansions no longer taint the run).
    pub fn trusted(mut self, trusted: bool) -> Self {
        self.trusted = trusted;
        self
    }
}

#[async_trait]
impl Tool for McpPromptTool {
    fn name(&self) -> &str {
        &self.engram_name
    }
    fn description(&self) -> &str {
        "Expand an MCP prompt template from this server by name (as advertised in prompts/list), \
         passing its named arguments, and return the expanded message text."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": { "type": "string", "description": "the prompt template name (e.g. from prompts/list)" },
                "arguments": { "type": "object", "description": "named arguments the template accepts (may be omitted if none)" }
            },
            "required": ["name"]
        })
    }
    fn taints(&self) -> bool {
        !self.trusted
    }
    fn reads_sensitive(&self) -> bool {
        true
    }
    async fn run(&self, args: &Value, ctx: &ToolCtx) -> Result<String, String> {
        let name = crate::tools::arg_str(args, "name")?;
        // `arguments` is optional; default to an empty object so a no-arg template still expands.
        let prompt_args = args.get("arguments").cloned().unwrap_or_else(|| json!({}));
        let _ = ctx.ledger.append(
            "agent.mcp_prompt",
            "agent",
            json!({ "server": self.client.server(), "prompt": name }),
        );
        self.client
            .get_prompt(name, &prompt_args, ingress_deadline(ctx))
            .await
    }
}

/// A configured MCP server: how to reach it, its per-server secrets/cwd, and whether the user
/// trusts it as first-party (so its reads don't taint/sensitise the run). A server is either a
/// local subprocess (`command`) or a remote streamable-HTTP endpoint (`url`); `url` wins when set.
#[derive(Clone, Debug, Default)]
pub struct McpServerSpec {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: std::collections::BTreeMap<String, String>,
    pub cwd: Option<String>,
    pub trusted: bool,
    /// Remote streamable-HTTP endpoint. When set (non-empty), the client connects over HTTP
    /// instead of spawning `command` — no Node/npx needed on the box.
    pub url: Option<String>,
    /// Optional bearer token for the remote endpoint's `Authorization` header.
    pub bearer: Option<String>,
}

/// A live MCP server connection reported back to the caller, so it can address the server directly
/// (list/read its resources and prompts) beyond the wrapped tools it already contributed. Held by the
/// daemon alongside the tool list; keeping the `Arc<McpClient>` alive keeps the subprocess/session up.
pub struct ConnectedServer {
    /// The server's configured name (the `mcp_<server>_…` tool prefix).
    pub server: String,
    /// The live client — call `list_resources` / `read_resource` / `list_prompts` / `get_prompt`.
    pub client: Arc<McpClient>,
    /// Whether the user marked this server first-party trusted (its reads don't taint the run).
    pub trusted: bool,
}

/// Connect a set of servers and return all their tools. A server that fails to connect is logged
/// and skipped, never fatal.
pub async fn connect_servers(specs: &[McpServerSpec]) -> Vec<Arc<dyn Tool>> {
    connect_servers_reported(specs).await.0
}

/// Like [`connect_servers`], but also returns the names of the servers that connected, so a
/// caller (the Settings panel) can tell the user which ones failed instead of silently
/// dropping them. The returned tool list already includes each server's resource/prompt ingress
/// tools alongside its call tools, so callers that only consume the `Vec<Arc<dyn Tool>>` get the
/// full surface unchanged — use [`connect_servers_full`] when you also need the client handles.
pub async fn connect_servers_reported(
    specs: &[McpServerSpec],
) -> (Vec<Arc<dyn Tool>>, Vec<String>) {
    let (tools, servers) = connect_servers_full(specs).await;
    let connected = servers.into_iter().map(|s| s.server).collect();
    (tools, connected)
}

/// The full connect result: every server's wrapped tools (call + resource + prompt) AND the live
/// [`ConnectedServer`] handles, so a caller can also drive a server's resources/prompts directly.
/// Each connected server contributes, in order: its `mcp_<server>_<tool>` call tools, then a single
/// `mcp_<server>_read_resource` and `mcp_<server>_get_prompt` ingress tool. A server that fails to
/// connect is logged and skipped, never fatal.
pub async fn connect_servers_full(
    specs: &[McpServerSpec],
) -> (Vec<Arc<dyn Tool>>, Vec<ConnectedServer>) {
    let mut out: Vec<Arc<dyn Tool>> = Vec::new();
    let mut servers: Vec<ConnectedServer> = Vec::new();
    for s in specs {
        let result = match s.url.as_deref().filter(|u| !u.is_empty()) {
            #[cfg(feature = "web")]
            Some(url) => McpClient::connect_http(&s.name, url, s.bearer.as_deref()).await,
            #[cfg(not(feature = "web"))]
            Some(_) => Err("remote HTTP MCP servers require the `web` feature".to_string()),
            None => {
                McpClient::connect(&s.name, &s.command, &s.args, &s.env, s.cwd.as_deref()).await
            }
        };
        match result {
            Ok((client, tool_specs)) => {
                tracing::info!(server = %s.name, tools = tool_specs.len(), trusted = s.trusted, remote = s.url.is_some(), "mcp server connected");
                for ts in tool_specs {
                    out.push(Arc::new(
                        McpTool::new(client.clone(), ts).trusted(s.trusted),
                    ));
                }
                // Surface the ingress side of the same server: read a resource by URI, expand a
                // prompt by name. Both inherit the server's trust so an untrusted server's content
                // taints the run exactly like its tool calls do.
                out.push(Arc::new(
                    McpResourceTool::new(client.clone()).trusted(s.trusted),
                ));
                out.push(Arc::new(
                    McpPromptTool::new(client.clone()).trusted(s.trusted),
                ));
                servers.push(ConnectedServer {
                    server: s.name.clone(),
                    client,
                    trusted: s.trusted,
                });
            }
            Err(e) => tracing::warn!(server = %s.name, error = %e, "mcp connect failed"),
        }
    }
    (out, servers)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MOCK_PY: &str = r#"
import sys, json
def send(o):
    sys.stdout.write(json.dumps(o) + "\n"); sys.stdout.flush()
for line in sys.stdin:
    line = line.strip()
    if not line: continue
    msg = json.loads(line)
    mid = msg.get("id"); method = msg.get("method")
    if method == "initialize":
        send({"jsonrpc":"2.0","id":mid,"result":{"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"mock","version":"0"}}})
    elif method == "tools/list":
        send({"jsonrpc":"2.0","id":mid,"result":{"tools":[{"name":"echo","description":"Echo text back","inputSchema":{"type":"object","properties":{"text":{"type":"string"}},"required":["text"]}}]}})
    elif method == "tools/call":
        args = msg["params"]["arguments"]
        send({"jsonrpc":"2.0","id":mid,"result":{"content":[{"type":"text","text":"echo: " + str(args.get("text",""))}]}})
    elif method == "resources/list":
        send({"jsonrpc":"2.0","id":mid,"result":{"resources":[{"uri":"mem://note/1","name":"note one","description":"a note","mimeType":"text/plain"}]}})
    elif method == "resources/read":
        uri = msg["params"]["uri"]
        send({"jsonrpc":"2.0","id":mid,"result":{"contents":[{"uri":uri,"mimeType":"text/plain","text":"body of " + uri}]}})
    elif method == "prompts/list":
        send({"jsonrpc":"2.0","id":mid,"result":{"prompts":[{"name":"greet","description":"greet a person","arguments":[{"name":"who","description":"who to greet","required":True}]}]}})
    elif method == "prompts/get":
        args = msg["params"].get("arguments", {})
        send({"jsonrpc":"2.0","id":mid,"result":{"description":"greeting","messages":[{"role":"user","content":{"type":"text","text":"hello " + str(args.get("who",""))}}]}})
"#;

    // A server that answers initialize/tools/list but then hangs forever on tools/call, to prove
    // the per-call timeout releases the run and marks the connection dead.
    const HANG_PY: &str = r#"
import sys, json, time
def send(o):
    sys.stdout.write(json.dumps(o) + "\n"); sys.stdout.flush()
for line in sys.stdin:
    line = line.strip()
    if not line: continue
    msg = json.loads(line)
    mid = msg.get("id"); method = msg.get("method")
    if method == "initialize":
        send({"jsonrpc":"2.0","id":mid,"result":{"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"hang","version":"0"}}})
    elif method == "tools/list":
        send({"jsonrpc":"2.0","id":mid,"result":{"tools":[{"name":"stall","description":"never replies","inputSchema":{"type":"object"}}]}})
    elif method == "tools/call":
        time.sleep(3600)
"#;

    #[tokio::test]
    async fn connects_lists_and_calls_an_mcp_tool() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("mock_mcp.py");
        std::fs::write(&script, MOCK_PY).unwrap();

        let (client, specs) = McpClient::connect(
            "mock",
            "python3",
            &[script.to_string_lossy().to_string()],
            &std::collections::BTreeMap::new(),
            None,
        )
        .await
        .expect("connect to mock mcp server");
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].name, "echo");

        let out = client
            .call_tool("echo", &json!({ "text": "hello mcp" }), CALL_CEILING)
            .await
            .unwrap();
        assert!(out.contains("echo: hello mcp"), "got: {out}");

        // And it wraps into a native tool with a namespaced name.
        let tool = McpTool::new(client, specs[0].clone());
        assert_eq!(tool.name(), "mcp_mock_echo");
    }

    #[tokio::test]
    async fn lists_and_reads_resources() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("mock_mcp.py");
        std::fs::write(&script, MOCK_PY).unwrap();

        let (client, _specs) = McpClient::connect(
            "mock",
            "python3",
            &[script.to_string_lossy().to_string()],
            &std::collections::BTreeMap::new(),
            None,
        )
        .await
        .expect("connect to mock mcp server");

        let resources = client
            .list_resources(CALL_CEILING)
            .await
            .expect("list resources");
        assert_eq!(resources.len(), 1);
        assert_eq!(resources[0].uri, "mem://note/1");
        assert_eq!(resources[0].name, "note one");
        assert_eq!(resources[0].mime_type.as_deref(), Some("text/plain"));

        let body = client
            .read_resource("mem://note/1", CALL_CEILING)
            .await
            .expect("read resource");
        assert_eq!(body, "body of mem://note/1");
    }

    #[tokio::test]
    async fn lists_and_gets_prompts() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("mock_mcp.py");
        std::fs::write(&script, MOCK_PY).unwrap();

        let (client, _specs) = McpClient::connect(
            "mock",
            "python3",
            &[script.to_string_lossy().to_string()],
            &std::collections::BTreeMap::new(),
            None,
        )
        .await
        .expect("connect to mock mcp server");

        let prompts = client
            .list_prompts(CALL_CEILING)
            .await
            .expect("list prompts");
        assert_eq!(prompts.len(), 1);
        assert_eq!(prompts[0].name, "greet");
        assert_eq!(prompts[0].arguments.len(), 1);
        assert_eq!(prompts[0].arguments[0].name, "who");
        assert!(prompts[0].arguments[0].required);

        let expanded = client
            .get_prompt("greet", &json!({ "who": "world" }), CALL_CEILING)
            .await
            .expect("get prompt");
        assert_eq!(expanded, "hello world");
    }

    #[tokio::test]
    async fn resource_read_times_out_and_marks_connection_dead() {
        // Reuse HANG_PY: it answers initialize/tools/list, then hangs on ANY other method
        // (its handler only special-cases those three), so resources/read never replies.
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("hang_mcp.py");
        std::fs::write(&script, HANG_PY).unwrap();

        let (client, _specs) = McpClient::connect(
            "hang",
            "python3",
            &[script.to_string_lossy().to_string()],
            &std::collections::BTreeMap::new(),
            None,
        )
        .await
        .expect("connect to hang mcp server");

        let err = client
            .read_resource("x://y", Duration::from_millis(300))
            .await
            .expect_err("hung read must time out");
        assert!(err.contains("timed out"), "got: {err}");

        // The connection is now dead: a second request fails fast rather than blocking.
        let err2 = tokio::time::timeout(
            Duration::from_secs(1),
            client.list_resources(Duration::from_secs(30)),
        )
        .await
        .expect("second call must not block on the poisoned Mutex")
        .expect_err("dead connection must reject");
        assert!(err2.contains("dead"), "got: {err2}");
    }

    #[tokio::test]
    async fn call_tool_times_out_and_marks_connection_dead() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("hang_mcp.py");
        std::fs::write(&script, HANG_PY).unwrap();

        let (client, specs) = McpClient::connect(
            "hang",
            "python3",
            &[script.to_string_lossy().to_string()],
            &std::collections::BTreeMap::new(),
            None,
        )
        .await
        .expect("connect to hang mcp server");
        assert_eq!(specs[0].name, "stall");

        let err = client
            .call_tool("stall", &json!({}), Duration::from_millis(300))
            .await
            .expect_err("hung call must time out");
        assert!(err.contains("timed out"), "got: {err}");

        // The connection is now dead: a second call fails fast rather than blocking on the wedged
        // read (proving the io Mutex was released).
        let err2 = tokio::time::timeout(
            Duration::from_secs(1),
            client.call_tool("stall", &json!({}), Duration::from_secs(30)),
        )
        .await
        .expect("second call must not block on the poisoned Mutex")
        .expect_err("dead connection must reject");
        assert!(err2.contains("dead"), "got: {err2}");
    }

    #[cfg(feature = "web")]
    #[test]
    fn sse_body_parses_matching_response_frame() {
        let body =
            "event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":7,\"result\":{\"ok\":true}}\n\n";
        let v = parse_sse_response(body, 7).expect("parse sse");
        assert_eq!(v["result"]["ok"], json!(true));
        // A non-matching id yields nothing.
        assert!(parse_sse_response(body, 99).is_none());
    }

    // --- Ingress-tool wrappers (McpResourceTool / McpPromptTool) --------------------------------

    use crate::tool::{Policy, ToolCtx};
    use engram_core::{Ledger, Taint};
    use engram_gateway::{Gateway, MockProvider};
    use engram_memory::{Memory, TrigramHashEmbedder};
    use engram_skills::{Registry, SkillSigner};

    /// A minimal, real `ToolCtx` so the ingress tools' `run` (which ledgers + derives the deadline
    /// from `ctx.policy`) can be exercised end-to-end. Mirrors `tools::file_tools_tests::ctx_in`.
    fn ctx_in(dir: &std::path::Path) -> ToolCtx {
        let ledger = Arc::new(Ledger::open(dir).unwrap());
        let memory = Arc::new(
            Memory::open(
                dir.join("b.db"),
                Arc::new(TrigramHashEmbedder::default()),
                ledger.clone(),
            )
            .unwrap(),
        );
        let signer = Arc::new(SkillSigner::load_or_create(dir.join("k")).unwrap());
        let skills = Arc::new(Registry::open(dir, signer, ledger.clone()).unwrap());
        let gateway = Arc::new(Gateway::new(Box::new(MockProvider), ledger.clone()));
        ToolCtx {
            memory,
            skills,
            gateway,
            ledger,
            taint: Taint::Trusted,
            sensitive: false,
            policy: Policy::default(),
            workdir: dir.to_path_buf(),
            model: "test".into(),
            depth: 0,
            browser: Arc::new(crate::tool::NoBrowser),
            scope: engram_core::ScopeCtx::any(),
            halt: None,
            spend_counter: None,
            token_budget: None,
            on_step: None,
            on_narration: None,
            allowed_tools: None,
            agent_actor: None,
        }
    }

    #[tokio::test]
    async fn resource_tool_reads_a_resource_by_uri() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("mock_mcp.py");
        std::fs::write(&script, MOCK_PY).unwrap();

        let (client, _specs) = McpClient::connect(
            "mock",
            "python3",
            &[script.to_string_lossy().to_string()],
            &std::collections::BTreeMap::new(),
            None,
        )
        .await
        .expect("connect to mock mcp server");

        let tool = McpResourceTool::new(client);
        // Namespaced, collision-free name and a `uri` arg.
        assert_eq!(tool.name(), "mcp_mock_read_resource");
        assert_eq!(tool.schema()["required"][0], json!("uri"));

        let ctx = ctx_in(dir.path());
        let out = tool
            .run(&json!({ "uri": "mem://note/1" }), &ctx)
            .await
            .expect("read the resource through the tool");
        assert_eq!(out, "body of mem://note/1");
    }

    #[tokio::test]
    async fn prompt_tool_expands_a_prompt_by_name() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("mock_mcp.py");
        std::fs::write(&script, MOCK_PY).unwrap();

        let (client, _specs) = McpClient::connect(
            "mock",
            "python3",
            &[script.to_string_lossy().to_string()],
            &std::collections::BTreeMap::new(),
            None,
        )
        .await
        .expect("connect to mock mcp server");

        let tool = McpPromptTool::new(client);
        assert_eq!(tool.name(), "mcp_mock_get_prompt");

        let ctx = ctx_in(dir.path());
        let out = tool
            .run(
                &json!({ "name": "greet", "arguments": { "who": "world" } }),
                &ctx,
            )
            .await
            .expect("expand the prompt through the tool");
        assert_eq!(out, "hello world");
    }

    #[tokio::test]
    async fn ingress_tools_carry_server_trust_into_taint_and_sensitive() {
        // Security parity with McpTool: an UNTRUSTED server's resource/prompt read taints the run
        // and is sensitive; a TRUSTED server's stops tainting but stays sensitive (trust only means
        // "content isn't an attacker's instructions", never "the data is public"). Neither is egress.
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("mock_mcp.py");
        std::fs::write(&script, MOCK_PY).unwrap();
        let (client, _specs) = McpClient::connect(
            "mock",
            "python3",
            &[script.to_string_lossy().to_string()],
            &std::collections::BTreeMap::new(),
            None,
        )
        .await
        .expect("connect to mock mcp server");

        let untrusted_res = McpResourceTool::new(client.clone());
        assert!(untrusted_res.taints(), "untrusted resource read taints");
        assert!(untrusted_res.reads_sensitive());
        assert!(!untrusted_res.is_egress(), "ingress, not egress");
        assert!(!untrusted_res.side_effecting());

        let trusted_res = McpResourceTool::new(client.clone()).trusted(true);
        assert!(!trusted_res.taints(), "trusted server does not taint");
        assert!(
            trusted_res.reads_sensitive(),
            "but its data stays sensitive"
        );

        let untrusted_prompt = McpPromptTool::new(client.clone());
        assert!(untrusted_prompt.taints());
        assert!(untrusted_prompt.reads_sensitive());
        assert!(!untrusted_prompt.is_egress());

        let trusted_prompt = McpPromptTool::new(client).trusted(true);
        assert!(!trusted_prompt.taints());
        assert!(trusted_prompt.reads_sensitive());
    }

    #[tokio::test]
    async fn connect_full_contributes_resource_and_prompt_tools_per_server() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("mock_mcp.py");
        std::fs::write(&script, MOCK_PY).unwrap();

        let spec = McpServerSpec {
            name: "mock".into(),
            command: "python3".into(),
            args: vec![script.to_string_lossy().to_string()],
            trusted: false,
            ..Default::default()
        };
        let (tools, servers) = connect_servers_full(std::slice::from_ref(&spec)).await;
        // One call tool (echo) + the two ingress tools.
        let names: Vec<&str> = tools.iter().map(|t| t.name()).collect();
        assert!(
            names.contains(&"mcp_mock_echo"),
            "call tool present: {names:?}"
        );
        assert!(
            names.contains(&"mcp_mock_read_resource"),
            "resource tool present: {names:?}"
        );
        assert!(
            names.contains(&"mcp_mock_get_prompt"),
            "prompt tool present: {names:?}"
        );
        // And the client handle is reported so a caller can drive resources/prompts directly.
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].server, "mock");
        assert!(!servers[0].trusted);
        let listed = servers[0]
            .client
            .list_resources(CALL_CEILING)
            .await
            .expect("list via the reported handle");
        assert_eq!(listed[0].uri, "mem://note/1");
    }
}
