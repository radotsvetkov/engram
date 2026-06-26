//! MCP client — connect to any Model Context Protocol server and borrow its tools.
//!
//! This is the parity multiplier. Rather than hand-coding 60+ integrations, Engram
//! speaks MCP (JSON-RPC 2.0 over a subprocess's stdio): it launches a server, lists
//! its tools, and wraps each as a native [`Tool`] that joins the agent's registry. Any
//! community MCP server — filesystem, GitHub, Slack, a browser driver, a database —
//! becomes available to the agent, audited through the same ledger as everything else.

use std::process::Stdio;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;

use crate::tool::{Tool, ToolCtx};

/// A tool as advertised by an MCP server.
#[derive(Clone, Debug)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub schema: Value,
}

struct Io {
    _child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

/// A live connection to one MCP server.
pub struct McpClient {
    io: Mutex<Io>,
    server: String,
}

impl McpClient {
    /// Launch `command args…`, perform the MCP handshake, and return the client plus the
    /// tools it offers.
    pub async fn connect(
        server: &str,
        command: &str,
        args: &[String],
    ) -> Result<(Arc<Self>, Vec<ToolSpec>), String> {
        let mut child = Command::new(command)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| format!("spawn '{command}': {e}"))?;
        let stdin = child.stdin.take().ok_or("no stdin")?;
        let stdout = BufReader::new(child.stdout.take().ok_or("no stdout")?);
        let client = Arc::new(McpClient {
            io: Mutex::new(Io { _child: child, stdin, stdout, next_id: 1 }),
            server: server.to_string(),
        });

        client
            .request(
                "initialize",
                json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "clientInfo": { "name": "engram", "version": "0.1" }
                }),
            )
            .await?;
        client.notify("notifications/initialized", json!({})).await?;

        let listed = client.request("tools/list", json!({})).await?;
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
        Ok((client, tools))
    }

    async fn request(&self, method: &str, params: Value) -> Result<Value, String> {
        let mut io = self.io.lock().await;
        let id = io.next_id;
        io.next_id += 1;
        let line = format!("{}\n", json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        io.stdin.write_all(line.as_bytes()).await.map_err(|e| e.to_string())?;
        io.stdin.flush().await.map_err(|e| e.to_string())?;
        loop {
            let mut buf = String::new();
            let n = io.stdout.read_line(&mut buf).await.map_err(|e| e.to_string())?;
            if n == 0 {
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
            // otherwise a notification or unrelated message — keep reading.
        }
    }

    async fn notify(&self, method: &str, params: Value) -> Result<(), String> {
        let mut io = self.io.lock().await;
        let line = format!("{}\n", json!({ "jsonrpc": "2.0", "method": method, "params": params }));
        io.stdin.write_all(line.as_bytes()).await.map_err(|e| e.to_string())?;
        io.stdin.flush().await.map_err(|e| e.to_string())
    }

    /// Call a remote tool and return its text content.
    pub async fn call_tool(&self, name: &str, args: &Value) -> Result<String, String> {
        let r = self.request("tools/call", json!({ "name": name, "arguments": args })).await?;
        if let Some(content) = r["content"].as_array() {
            let text = content.iter().filter_map(|c| c["text"].as_str()).collect::<Vec<_>>().join("\n");
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

/// An Engram tool backed by a remote MCP tool. Named `mcp_<server>_<tool>` to avoid
/// collisions with built-ins.
pub struct McpTool {
    client: Arc<McpClient>,
    engram_name: String,
    remote_name: String,
    description: String,
    schema: Value,
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
        }
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
        // MCP tools are opaque external capabilities — treat them as egress so a tainted
        // run cannot reach them (default-deny under taint).
        true
    }
    async fn run(&self, args: &Value, ctx: &ToolCtx) -> Result<String, String> {
        let _ = ctx.ledger.append(
            "agent.mcp",
            "agent",
            json!({ "server": self.client.server(), "tool": self.remote_name }),
        );
        self.client.call_tool(&self.remote_name, args).await
    }
}

/// Connect a set of `(name, command, args)` servers and return all their tools. A
/// server that fails to connect is logged and skipped, never fatal.
pub async fn connect_servers(configs: &[(String, String, Vec<String>)]) -> Vec<Arc<dyn Tool>> {
    let mut out: Vec<Arc<dyn Tool>> = Vec::new();
    for (name, command, args) in configs {
        match McpClient::connect(name, command, args).await {
            Ok((client, specs)) => {
                tracing::info!(server = %name, tools = specs.len(), "mcp server connected");
                for spec in specs {
                    out.push(Arc::new(McpTool::new(client.clone(), spec)));
                }
            }
            Err(e) => tracing::warn!(server = %name, error = %e, "mcp connect failed"),
        }
    }
    out
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
"#;

    #[tokio::test]
    async fn connects_lists_and_calls_an_mcp_tool() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("mock_mcp.py");
        std::fs::write(&script, MOCK_PY).unwrap();

        let (client, specs) =
            McpClient::connect("mock", "python3", &[script.to_string_lossy().to_string()])
                .await
                .expect("connect to mock mcp server");
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].name, "echo");

        let out = client.call_tool("echo", &json!({ "text": "hello mcp" })).await.unwrap();
        assert!(out.contains("echo: hello mcp"), "got: {out}");

        // And it wraps into a native tool with a namespaced name.
        let tool = McpTool::new(client, specs[0].clone());
        assert_eq!(tool.name(), "mcp_mock_echo");
    }
}
