//! Built-in tools - the actions an Engram agent can take.
//!
//! Each is small and auditable. Filesystem tools are confined to the workdir; the
//! shell is off by default and refused outright once the run is tainted; web tools
//! taint the run so anything they pull in can't later reach the shell or secrets.

use std::time::Duration;

use async_trait::async_trait;
use base64::Engine;
use engram_gateway::{Call, CompletionRequest, Message};
use engram_memory::{Region, WriteReq};
use serde_json::{json, Value};

use crate::tool::{confine, Tool, ToolCtx};

fn arg_str<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    args[key].as_str().ok_or_else(|| format!("missing string argument '{key}'"))
}

/// Reject non-public destinations (SSRF guard): only http(s), and never loopback,
/// private, link-local (incl. the 169.254.169.254 cloud-metadata IP), or unspecified
/// addresses - so the agent can't be tricked into reaching its own unauthenticated API
/// or a cloud metadata service. Resolves hostnames and checks every resulting IP.
pub(crate) async fn guard_url(url: &str) -> Result<(), String> {
    let u = url.trim();
    if !(u.starts_with("http://") || u.starts_with("https://")) {
        return Err("url must be http(s)".into());
    }
    let after = u.split_once("://").map(|(_, b)| b).unwrap_or("");
    let hostport = after.split(['/', '?', '#']).next().unwrap_or("");
    let hostport = hostport.rsplit('@').next().unwrap_or(hostport); // strip userinfo
    let host = if let Some(rest) = hostport.strip_prefix('[') {
        rest.split(']').next().unwrap_or("") // [ipv6]
    } else {
        hostport.split(':').next().unwrap_or(hostport)
    };
    if host.is_empty() {
        return Err("url has no host".into());
    }
    let ips: Vec<std::net::IpAddr> = if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        vec![ip]
    } else {
        tokio::net::lookup_host((host, 80u16))
            .await
            .map_err(|e| format!("could not resolve '{host}': {e}"))?
            .map(|sa| sa.ip())
            .collect()
    };
    if ips.is_empty() {
        return Err(format!("could not resolve '{host}'"));
    }
    for ip in ips {
        if is_blocked_ip(&ip) {
            return Err(format!("refusing to reach non-public address {ip}"));
        }
    }
    Ok(())
}

fn is_blocked_ip(ip: &std::net::IpAddr) -> bool {
    use std::net::IpAddr;
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback() || v4.is_private() || v4.is_link_local() || v4.is_unspecified()
                || v4.is_broadcast() || v4.octets()[0] == 0
        }
        IpAddr::V6(v6) => {
            v6.is_loopback() || v6.is_unspecified()
                || (v6.segments()[0] & 0xffc0) == 0xfe80 // link-local fe80::/10
                || (v6.segments()[0] & 0xfe00) == 0xfc00 // unique-local fc00::/7
        }
    }
}

/// Strip HTML to roughly readable text (drops script/style, collapses whitespace).
pub(crate) fn html_to_text(html: &str) -> String {
    let mut s = String::with_capacity(html.len() / 2);
    let mut in_tag = false;
    let mut skip_depth = 0i32;
    let lower = html.to_lowercase();
    let bytes = html.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if lower[i..].starts_with("<script") || lower[i..].starts_with("<style") {
            skip_depth += 1;
        }
        if lower[i..].starts_with("</script") || lower[i..].starts_with("</style") {
            skip_depth = (skip_depth - 1).max(0);
        }
        let c = bytes[i] as char;
        if c == '<' {
            in_tag = true;
        } else if c == '>' {
            in_tag = false;
            s.push(' ');
        } else if !in_tag && skip_depth == 0 {
            s.push(c);
        }
        i += 1;
    }
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

// ---------------------------------------------------------------------------
// Shell
// ---------------------------------------------------------------------------

/// Build the `(program, args)` to run `command` under the configured backend:
///
/// - `None` → local (`sh -c`)
/// - `Some("ssh:user@host")` → remote over SSH
/// - `Some("singularity:image")` → a `singularity exec` sandbox
/// - `Some(image)` → a sandboxed, network-isolated `docker run` against that image
///
/// Exposed (not crate-private) so the daemon's glass-box terminal runs human commands through
/// this exact same path as the agent.
pub fn shell_command(
    backend: Option<&str>,
    workdir: &std::path::Path,
    command: &str,
) -> (String, Vec<String>) {
    match backend {
        Some(b) if b.starts_with("ssh:") => {
            ("ssh".into(), vec![b[4..].to_string(), command.to_string()])
        }
        Some(b) if b.starts_with("singularity:") => (
            "singularity".into(),
            vec!["exec".into(), b[12..].to_string(), "sh".into(), "-c".into(), command.to_string()],
        ),
        Some(img) => {
            let mount = format!("{}:/work", workdir.display());
            (
                "docker".into(),
                vec![
                    "run".into(), "--rm".into(), "--network".into(), "none".into(),
                    "-v".into(), mount, "-w".into(), "/work".into(),
                    img.to_string(), "sh".into(), "-c".into(), command.to_string(),
                ],
            )
        }
        None => ("sh".into(), vec!["-c".into(), command.to_string()]),
    }
}

#[cfg(test)]
mod ssrf_guard_tests {
    use super::guard_url;

    #[tokio::test]
    async fn blocks_internal_and_non_http_targets() {
        // Loopback (the unauthenticated local API), cloud metadata, private, IPv6 loopback.
        assert!(guard_url("http://127.0.0.1:8088/v1/policy").await.is_err());
        assert!(guard_url("http://localhost/").await.is_err());
        assert!(guard_url("http://169.254.169.254/latest/meta-data/").await.is_err());
        assert!(guard_url("http://10.0.0.5/").await.is_err());
        assert!(guard_url("http://192.168.1.1/").await.is_err());
        assert!(guard_url("http://[::1]/").await.is_err());
        // Non-http(s) schemes (local file read, scheme confusion).
        assert!(guard_url("file:///etc/passwd").await.is_err());
        assert!(guard_url("ftp://example.com/").await.is_err());
        // A public literal IP passes (no DNS needed, so this is offline-stable).
        assert!(guard_url("https://1.1.1.1/").await.is_ok());
    }
}

#[cfg(test)]
mod shell_backend_tests {
    use super::shell_command;
    use std::path::Path;

    #[test]
    fn local_backend() {
        let (prog, args) = shell_command(None, Path::new("/w"), "ls -la");
        assert_eq!(prog, "sh");
        assert_eq!(args, vec!["-c".to_string(), "ls -la".to_string()]);
    }

    #[test]
    fn docker_backend_is_network_isolated() {
        let (prog, args) = shell_command(Some("alpine"), Path::new("/w"), "echo hi");
        assert_eq!(prog, "docker");
        assert!(args.contains(&"--network".to_string()) && args.contains(&"none".to_string()));
        assert!(args.contains(&"alpine".to_string()));
        assert!(args.contains(&"echo hi".to_string()));
        assert!(args.contains(&"/w:/work".to_string()));
    }

    #[test]
    fn ssh_backend_runs_remotely() {
        let (prog, args) = shell_command(Some("ssh:deploy@vps.example"), Path::new("/w"), "uptime");
        assert_eq!(prog, "ssh");
        assert_eq!(args, vec!["deploy@vps.example".to_string(), "uptime".to_string()]);
    }

    #[test]
    fn singularity_backend_execs_in_image() {
        let (prog, args) = shell_command(Some("singularity:img.sif"), Path::new("/w"), "id");
        assert_eq!(prog, "singularity");
        assert_eq!(args, vec!["exec".to_string(), "img.sif".to_string(), "sh".to_string(), "-c".to_string(), "id".to_string()]);
    }
}

pub struct ShellTool;

#[async_trait]
impl Tool for ShellTool {
    fn name(&self) -> &str {
        "shell"
    }
    fn side_effecting(&self) -> bool {
        true
    }
    fn description(&self) -> &str {
        "Run a shell command in the working directory and return its output. Disabled \
         unless explicitly enabled, and refused on a run that has read untrusted content."
    }
    fn schema(&self) -> Value {
        json!({ "type": "object", "properties": { "command": { "type": "string" } }, "required": ["command"] })
    }
    async fn run(&self, args: &Value, ctx: &ToolCtx) -> Result<String, String> {
        if !ctx.policy.allow_shell {
            return Err("shell tool is disabled (set ENGRAM_TOOLS_SHELL=1 to enable)".into());
        }
        if ctx.taint.is_untrusted() {
            return Err("shell refused: this run read untrusted content (injection guard)".into());
        }
        let command = arg_str(args, "command")?;
        let backend = ctx.policy.shell_backend.as_deref();
        let _ = ctx.ledger.append(
            "agent.shell",
            "agent",
            json!({ "command": command, "backend": backend.unwrap_or("local") }),
        );
        let (program, cmd_args) = shell_command(backend, &ctx.workdir, command);
        let fut = tokio::process::Command::new(&program)
            .args(&cmd_args)
            .current_dir(&ctx.workdir)
            .output();
        let out = tokio::time::timeout(Duration::from_secs(ctx.policy.timeout_secs), fut)
            .await
            .map_err(|_| "command timed out".to_string())?
            .map_err(|e| e.to_string())?;
        Ok(format!(
            "exit={}\n--- stdout ---\n{}\n--- stderr ---\n{}",
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ))
    }
}

// ---------------------------------------------------------------------------
// Files
// ---------------------------------------------------------------------------

pub struct ReadFileTool;

#[async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &str {
        "read_file"
    }
    fn description(&self) -> &str {
        "Read a UTF-8 text file inside the working directory."
    }
    fn schema(&self) -> Value {
        json!({ "type": "object", "properties": { "path": { "type": "string" } }, "required": ["path"] })
    }
    async fn run(&self, args: &Value, ctx: &ToolCtx) -> Result<String, String> {
        let path = confine(&ctx.workdir, arg_str(args, "path")?)?;
        tokio::fs::read_to_string(&path).await.map_err(|e| e.to_string())
    }
}

pub struct WriteFileTool;

#[async_trait]
impl Tool for WriteFileTool {
    fn name(&self) -> &str {
        "write_file"
    }
    fn side_effecting(&self) -> bool {
        true
    }
    fn description(&self) -> &str {
        "Create or overwrite a text file inside the working directory."
    }
    fn schema(&self) -> Value {
        json!({ "type": "object",
            "properties": { "path": { "type": "string" }, "content": { "type": "string" } },
            "required": ["path", "content"] })
    }
    async fn run(&self, args: &Value, ctx: &ToolCtx) -> Result<String, String> {
        if !ctx.policy.allow_write {
            return Err("file writing is disabled".into());
        }
        let path = confine(&ctx.workdir, arg_str(args, "path")?)?;
        let content = arg_str(args, "content")?;
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|e| e.to_string())?;
        }
        tokio::fs::write(&path, content).await.map_err(|e| e.to_string())?;
        let _ = ctx.ledger.append(
            "agent.write",
            "agent",
            json!({ "path": path.to_string_lossy(), "bytes": content.len() }),
        );
        Ok(format!("wrote {} bytes", content.len()))
    }
}

pub struct ListDirTool;

#[async_trait]
impl Tool for ListDirTool {
    fn name(&self) -> &str {
        "list_dir"
    }
    fn description(&self) -> &str {
        "List entries of a directory inside the working directory."
    }
    fn schema(&self) -> Value {
        json!({ "type": "object", "properties": { "path": { "type": "string" } } })
    }
    async fn run(&self, args: &Value, ctx: &ToolCtx) -> Result<String, String> {
        let rel = args["path"].as_str().unwrap_or(".");
        let path = confine(&ctx.workdir, rel)?;
        let mut rd = tokio::fs::read_dir(&path).await.map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        while let Some(e) = rd.next_entry().await.map_err(|e| e.to_string())? {
            let kind = if e.path().is_dir() { "dir " } else { "file" };
            out.push(format!("{kind}  {}", e.file_name().to_string_lossy()));
        }
        out.sort();
        Ok(if out.is_empty() { "(empty)".into() } else { out.join("\n") })
    }
}

// ---------------------------------------------------------------------------
// Planning
// ---------------------------------------------------------------------------

/// An explicit, ledgered to-do list the agent maintains across a multi-step task -
/// frontier-harness planning, surfaced in the glass-box receipt so the user sees intent.
pub struct UpdatePlanTool;

#[async_trait]
impl Tool for UpdatePlanTool {
    fn name(&self) -> &str {
        "update_plan"
    }
    fn description(&self) -> &str {
        "Record or update your step-by-step plan for a multi-step task. Call it early to \
         outline the steps, then again to mark progress as you go - it keeps you on track and \
         shows the user your plan. Each step has a 'title' and a 'status' (todo, doing, done)."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "steps": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "title": { "type": "string" },
                            "status": { "type": "string", "enum": ["todo", "doing", "done"] }
                        },
                        "required": ["title"]
                    }
                }
            },
            "required": ["steps"]
        })
    }
    async fn run(&self, args: &Value, ctx: &ToolCtx) -> Result<String, String> {
        let steps = args["steps"].as_array().ok_or("steps must be an array")?;
        let total = steps.len();
        let done = steps.iter().filter(|s| s["status"] == "done").count();
        let _ = ctx
            .ledger
            .append("agent.plan", "agent", json!({ "steps": steps, "total": total, "done": done }));
        Ok(format!("plan updated: {done}/{total} steps done"))
    }
}

// ---------------------------------------------------------------------------
// Memory
// ---------------------------------------------------------------------------

pub struct MemoryRecallTool;

#[async_trait]
impl Tool for MemoryRecallTool {
    fn name(&self) -> &str {
        "memory_recall"
    }
    fn description(&self) -> &str {
        "Search the agent's long-term memory (semantic + keyword) for relevant facts."
    }
    fn schema(&self) -> Value {
        json!({ "type": "object",
            "properties": { "query": { "type": "string" }, "k": { "type": "integer" } },
            "required": ["query"] })
    }
    async fn run(&self, args: &Value, ctx: &ToolCtx) -> Result<String, String> {
        let query = arg_str(args, "query")?;
        let k = args["k"].as_u64().unwrap_or(5) as usize;
        // A trusted run gets trusted-provenance memories only (injected web/memory content
        // can't poison it). An already-tainted run may see all - its egress is blocked
        // anyway and it can legitimately use what it just researched.
        let hits = if ctx.taint.is_untrusted() {
            ctx.memory.recall(query, &[], k)
        } else {
            ctx.memory.recall_trusted(query, &[], k)
        }
        .map_err(|e| e.to_string())?;
        if hits.is_empty() {
            return Ok("(no relevant memories)".into());
        }
        Ok(hits.iter().map(|h| format!("- [{}] {}", h.record.region, h.record.text)).collect::<Vec<_>>().join("\n"))
    }
}

pub struct MemoryRememberTool;

#[async_trait]
impl Tool for MemoryRememberTool {
    fn name(&self) -> &str {
        "memory_remember"
    }
    fn description(&self) -> &str {
        "Store a fact in long-term memory (region: semantic, identity, episodic, or procedural)."
    }
    fn schema(&self) -> Value {
        json!({ "type": "object",
            "properties": { "text": { "type": "string" }, "region": { "type": "string" } },
            "required": ["text"] })
    }
    async fn run(&self, args: &Value, ctx: &ToolCtx) -> Result<String, String> {
        let text = arg_str(args, "text")?;
        let region = match args["region"].as_str() {
            Some("identity") => Region::Identity,
            Some("episodic") => Region::Episodic,
            Some("procedural") => Region::Procedural,
            _ => Region::Semantic,
        };
        // Writes inherit the run's taint, so injected content can't launder into a trusted fact.
        let rec = ctx
            .memory
            .remember(WriteReq::new(region, text).taint(ctx.taint).actor("agent"))
            .map_err(|e| e.to_string())?;
        Ok(format!("remembered as #{}", rec.id))
    }
}

// ---------------------------------------------------------------------------
// Media (vision, image generation, speech) - through the gateway
// ---------------------------------------------------------------------------

pub struct VisionAnalyzeTool;

#[async_trait]
impl Tool for VisionAnalyzeTool {
    fn name(&self) -> &str {
        "vision_analyze"
    }
    fn description(&self) -> &str {
        "Look at an image file in the workdir (e.g. a screenshot) and answer a question about it."
    }
    fn schema(&self) -> Value {
        json!({ "type": "object",
            "properties": { "path": { "type": "string" }, "question": { "type": "string" } },
            "required": ["path", "question"] })
    }
    async fn run(&self, args: &Value, ctx: &ToolCtx) -> Result<String, String> {
        let path = confine(&ctx.workdir, arg_str(args, "path")?)?;
        let question = arg_str(args, "question")?;
        let bytes = tokio::fs::read(&path).await.map_err(|e| e.to_string())?;
        let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
        let model = std::env::var("ENGRAM_VISION_MODEL").unwrap_or_else(|_| ctx.model.clone());
        let req = CompletionRequest::new(model, vec![Message::user_with_image(question, b64)]);
        let completion = ctx
            .gateway
            .complete(Call::new(req).actor("agent").tainted(ctx.taint))
            .await
            .map_err(|e| e.to_string())?;
        Ok(completion.text)
    }
}

pub struct ImageGenerateTool;

#[async_trait]
impl Tool for ImageGenerateTool {
    fn name(&self) -> &str {
        "image_generate"
    }
    fn side_effecting(&self) -> bool {
        true
    }
    fn description(&self) -> &str {
        "Generate an image from a text prompt and save it as a PNG in the workdir."
    }
    fn schema(&self) -> Value {
        json!({ "type": "object",
            "properties": { "prompt": { "type": "string" }, "path": { "type": "string" } },
            "required": ["prompt", "path"] })
    }
    async fn run(&self, args: &Value, ctx: &ToolCtx) -> Result<String, String> {
        let prompt = arg_str(args, "prompt")?;
        let path = confine(&ctx.workdir, arg_str(args, "path")?)?;
        let bytes = ctx.gateway.generate_image(prompt, "agent").await.map_err(|e| e.to_string())?;
        tokio::fs::write(&path, &bytes).await.map_err(|e| e.to_string())?;
        Ok(format!("saved {}-byte image to {}", bytes.len(), path.display()))
    }
}

pub struct TextToSpeechTool;

#[async_trait]
impl Tool for TextToSpeechTool {
    fn name(&self) -> &str {
        "text_to_speech"
    }
    fn side_effecting(&self) -> bool {
        true
    }
    fn description(&self) -> &str {
        "Synthesize speech from text and save the audio file in the workdir."
    }
    fn schema(&self) -> Value {
        json!({ "type": "object",
            "properties": { "text": { "type": "string" }, "path": { "type": "string" }, "voice": { "type": "string" } },
            "required": ["text", "path"] })
    }
    async fn run(&self, args: &Value, ctx: &ToolCtx) -> Result<String, String> {
        let text = arg_str(args, "text")?;
        let path = confine(&ctx.workdir, arg_str(args, "path")?)?;
        let voice = args["voice"].as_str().unwrap_or("alloy");
        let bytes = ctx.gateway.tts(text, voice, "agent").await.map_err(|e| e.to_string())?;
        tokio::fs::write(&path, &bytes).await.map_err(|e| e.to_string())?;
        Ok(format!("saved {}-byte audio to {}", bytes.len(), path.display()))
    }
}

pub struct TranscribeTool;

#[async_trait]
impl Tool for TranscribeTool {
    fn name(&self) -> &str {
        "transcribe"
    }
    fn description(&self) -> &str {
        "Transcribe an audio file in the workdir to text (speech-to-text)."
    }
    fn schema(&self) -> Value {
        json!({ "type": "object",
            "properties": { "path": { "type": "string" }, "format": { "type": "string" } },
            "required": ["path"] })
    }
    async fn run(&self, args: &Value, ctx: &ToolCtx) -> Result<String, String> {
        let path = confine(&ctx.workdir, arg_str(args, "path")?)?;
        let format = args["format"]
            .as_str()
            .map(String::from)
            .or_else(|| path.extension().and_then(|e| e.to_str()).map(String::from))
            .unwrap_or_else(|| "mp3".into());
        let bytes = tokio::fs::read(&path).await.map_err(|e| e.to_string())?;
        ctx.gateway.transcribe(&bytes, &format, "agent").await.map_err(|e| e.to_string())
    }
}

// ---------------------------------------------------------------------------
// Delegation (subagents)
// ---------------------------------------------------------------------------

pub struct DelegateTool;

#[async_trait]
impl Tool for DelegateTool {
    fn name(&self) -> &str {
        "delegate_task"
    }
    fn description(&self) -> &str {
        "Delegate a focused subtask to an isolated subagent and return its result. Use \
         for parallelizable or self-contained work that deserves its own reasoning."
    }
    fn schema(&self) -> Value {
        json!({ "type": "object", "properties": { "task": { "type": "string" } }, "required": ["task"] })
    }
    async fn run(&self, args: &Value, ctx: &ToolCtx) -> Result<String, String> {
        if ctx.depth >= 2 {
            return Err("maximum delegation depth (2) reached".into());
        }
        let task = arg_str(args, "task")?;
        let _ = ctx.ledger.append("agent.delegate", "agent", json!({ "task": task, "depth": ctx.depth }));
        // The subagent gets the base toolset (no further delegation by default) and a
        // deeper context, but inherits taint - an untrusted parent yields an untrusted child.
        let agent = crate::agent::Agent::new(ctx.gateway.clone(), crate::sub_tools(), ctx.model.clone());
        let mut sub = ctx.clone();
        sub.depth = ctx.depth + 1;
        let run = agent.run(task, sub).await.map_err(|e| e.to_string())?;
        Ok(run.answer)
    }
}

// ---------------------------------------------------------------------------
// Browser (headless Chrome via subprocess - real JS rendering, no extra deps)
// ---------------------------------------------------------------------------

pub(crate) fn find_chrome() -> Option<String> {
    if let Ok(p) = std::env::var("ENGRAM_CHROME") {
        if std::path::Path::new(&p).exists() {
            return Some(p);
        }
    }
    const CANDIDATES: &[&str] = &[
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        "/Applications/Chromium.app/Contents/MacOS/Chromium",
        "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser",
        "/usr/bin/google-chrome",
        "/usr/bin/chromium",
        "/usr/bin/chromium-browser",
    ];
    CANDIDATES.iter().find(|p| std::path::Path::new(p).exists()).map(|s| s.to_string())
}

const CHROME_FLAGS: &[&str] =
    &["--headless", "--disable-gpu", "--no-sandbox", "--no-first-run", "--disable-extensions"];

async fn chrome_dump_dom(url: &str, timeout: u64) -> Result<String, String> {
    let chrome = find_chrome().ok_or("no Chrome/Chromium found (set ENGRAM_CHROME)")?;
    let fut = tokio::process::Command::new(&chrome)
        .args(CHROME_FLAGS)
        .arg("--dump-dom")
        .arg(url)
        .output();
    let out = tokio::time::timeout(Duration::from_secs(timeout), fut)
        .await
        .map_err(|_| "browser timed out".to_string())?
        .map_err(|e| e.to_string())?;
    if out.stdout.is_empty() {
        return Err(format!(
            "browser produced no output: {}",
            String::from_utf8_lossy(&out.stderr).chars().take(200).collect::<String>()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

async fn chrome_screenshot(url: &str, out_path: &std::path::Path, timeout: u64) -> Result<(), String> {
    let chrome = find_chrome().ok_or("no Chrome/Chromium found (set ENGRAM_CHROME)")?;
    let fut = tokio::process::Command::new(&chrome)
        .args(CHROME_FLAGS)
        .arg("--hide-scrollbars")
        .arg("--window-size=1280,1024")
        .arg(format!("--screenshot={}", out_path.display()))
        .arg(url)
        .output();
    tokio::time::timeout(Duration::from_secs(timeout), fut)
        .await
        .map_err(|_| "browser timed out".to_string())?
        .map_err(|e| e.to_string())?;
    if !out_path.exists() {
        return Err("screenshot was not produced".into());
    }
    Ok(())
}

pub struct BrowserReadTool;

#[async_trait]
impl Tool for BrowserReadTool {
    fn name(&self) -> &str {
        "browser_read"
    }
    fn description(&self) -> &str {
        "Open a URL in a headless browser (running its JavaScript) and return the rendered \
         page text. Use for JS-heavy pages and SPAs where web_fetch returns little."
    }
    fn schema(&self) -> Value {
        json!({ "type": "object", "properties": { "url": { "type": "string" } }, "required": ["url"] })
    }
    fn taints(&self) -> bool {
        true
    }
    fn is_egress(&self) -> bool {
        true
    }
    async fn run(&self, args: &Value, ctx: &ToolCtx) -> Result<String, String> {
        let url = arg_str(args, "url")?;
        guard_url(url).await?;
        let _ = ctx.ledger.append("agent.browser_read", "agent", json!({ "url": url }));
        let html = chrome_dump_dom(url, ctx.policy.timeout_secs.max(30)).await?;
        Ok(html_to_text(&html))
    }
}

pub struct BrowserScreenshotTool;

#[async_trait]
impl Tool for BrowserScreenshotTool {
    fn name(&self) -> &str {
        "browser_screenshot"
    }
    fn description(&self) -> &str {
        "Open a URL in a headless browser and save a PNG screenshot to a file in the workdir."
    }
    fn schema(&self) -> Value {
        json!({ "type": "object",
            "properties": { "url": { "type": "string" }, "path": { "type": "string" } },
            "required": ["url", "path"] })
    }
    fn taints(&self) -> bool {
        true
    }
    fn is_egress(&self) -> bool {
        true
    }
    async fn run(&self, args: &Value, ctx: &ToolCtx) -> Result<String, String> {
        let url = arg_str(args, "url")?;
        guard_url(url).await?;
        let path = confine(&ctx.workdir, arg_str(args, "path")?)?;
        let _ = ctx.ledger.append("agent.browser_screenshot", "agent", json!({ "url": url }));
        chrome_screenshot(url, &path, ctx.policy.timeout_secs.max(30)).await?;
        Ok(format!("saved screenshot to {}", path.display()))
    }
}

// Interactive browser tools - drive a persistent session via ctx.browser.

pub struct BrowserOpenTool;

#[async_trait]
impl Tool for BrowserOpenTool {
    fn name(&self) -> &str {
        "browser_open"
    }
    fn description(&self) -> &str {
        "Navigate the persistent interactive browser to a URL and return the page text. \
         Follow with browser_click / browser_type / browser_extract for multi-step tasks."
    }
    fn schema(&self) -> Value {
        json!({ "type": "object", "properties": { "url": { "type": "string" } }, "required": ["url"] })
    }
    fn taints(&self) -> bool {
        true
    }
    fn is_egress(&self) -> bool {
        true
    }
    async fn run(&self, args: &Value, ctx: &ToolCtx) -> Result<String, String> {
        let url = arg_str(args, "url")?;
        guard_url(url).await?;
        let _ = ctx.ledger.append("agent.browser_open", "agent", json!({ "url": url }));
        ctx.browser.open(url).await
    }
}

pub struct BrowserClickTool;

#[async_trait]
impl Tool for BrowserClickTool {
    fn name(&self) -> &str {
        "browser_click"
    }
    fn side_effecting(&self) -> bool {
        true
    }
    fn description(&self) -> &str {
        "Click the first element matching a CSS selector in the current browser page."
    }
    fn schema(&self) -> Value {
        json!({ "type": "object", "properties": { "selector": { "type": "string" } }, "required": ["selector"] })
    }
    fn taints(&self) -> bool {
        true
    }
    fn is_egress(&self) -> bool {
        true
    }
    async fn run(&self, args: &Value, ctx: &ToolCtx) -> Result<String, String> {
        let _ = ctx.ledger.append("agent.browser_click", "agent", json!({}));
        ctx.browser.click(arg_str(args, "selector")?).await
    }
}

pub struct BrowserTypeTool;

#[async_trait]
impl Tool for BrowserTypeTool {
    fn name(&self) -> &str {
        "browser_type"
    }
    fn side_effecting(&self) -> bool {
        true
    }
    fn description(&self) -> &str {
        "Type text into the first element matching a CSS selector in the current page."
    }
    fn schema(&self) -> Value {
        json!({ "type": "object",
            "properties": { "selector": { "type": "string" }, "text": { "type": "string" } },
            "required": ["selector", "text"] })
    }
    fn taints(&self) -> bool {
        true
    }
    fn is_egress(&self) -> bool {
        true
    }
    async fn run(&self, args: &Value, ctx: &ToolCtx) -> Result<String, String> {
        let _ = ctx.ledger.append("agent.browser_type", "agent", json!({}));
        ctx.browser.type_text(arg_str(args, "selector")?, arg_str(args, "text")?).await
    }
}

pub struct BrowserExtractTool;

#[async_trait]
impl Tool for BrowserExtractTool {
    fn name(&self) -> &str {
        "browser_extract"
    }
    fn description(&self) -> &str {
        "Extract text from the current browser page, optionally limited to a CSS selector."
    }
    fn schema(&self) -> Value {
        json!({ "type": "object", "properties": { "selector": { "type": "string" } } })
    }
    fn taints(&self) -> bool {
        true
    }
    fn is_egress(&self) -> bool {
        true
    }
    async fn run(&self, args: &Value, ctx: &ToolCtx) -> Result<String, String> {
        ctx.browser.extract(args["selector"].as_str()).await
    }
}

#[cfg(test)]
mod browser_tests {
    use super::*;

    #[tokio::test]
    #[ignore = "needs Chrome"]
    async fn reads_a_real_page_with_js() {
        let html = chrome_dump_dom("https://example.com", 30).await.unwrap();
        assert!(html_to_text(&html).contains("Example Domain"));
    }

    #[tokio::test]
    #[ignore = "needs Chrome"]
    async fn screenshots_a_page() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("shot.png");
        chrome_screenshot("https://example.com", &out, 30).await.unwrap();
        assert!(out.exists() && std::fs::metadata(&out).unwrap().len() > 0);
    }
}

// ---------------------------------------------------------------------------
// Web (feature-gated)
// ---------------------------------------------------------------------------

#[cfg(feature = "web")]
pub use web::{SendMessageTool, WebFetchTool, WebSearchTool};

#[cfg(feature = "web")]
mod web {
    use super::*;

    async fn get_text(url: &str, timeout: u64) -> Result<String, String> {
        let client = reqwest::Client::builder()
            .user_agent("engram-agent/0.1")
            .timeout(Duration::from_secs(timeout))
            .build()
            .map_err(|e| e.to_string())?;
        let resp = client.get(url).send().await.map_err(|e| e.to_string())?;
        resp.text().await.map_err(|e| e.to_string())
    }

    pub struct WebFetchTool;

    #[async_trait]
    impl Tool for WebFetchTool {
        fn name(&self) -> &str {
            "web_fetch"
        }
        fn description(&self) -> &str {
            "Fetch a URL and return its readable text content."
        }
        fn schema(&self) -> Value {
            json!({ "type": "object", "properties": { "url": { "type": "string" } }, "required": ["url"] })
        }
        fn taints(&self) -> bool {
            true
        }
        async fn run(&self, args: &Value, ctx: &ToolCtx) -> Result<String, String> {
            let url = arg_str(args, "url")?;
            super::guard_url(url).await?;
            let _ = ctx.ledger.append("agent.web_fetch", "agent", json!({ "url": url }));
            let html = get_text(url, ctx.policy.timeout_secs).await?;
            Ok(super::html_to_text(&html))
        }
    }

    pub struct WebSearchTool;

    #[async_trait]
    impl Tool for WebSearchTool {
        fn name(&self) -> &str {
            "web_search"
        }
        fn description(&self) -> &str {
            "Search the web and return the top result titles and URLs."
        }
        fn schema(&self) -> Value {
            json!({ "type": "object", "properties": { "query": { "type": "string" } }, "required": ["query"] })
        }
        fn taints(&self) -> bool {
            true
        }
        async fn run(&self, args: &Value, ctx: &ToolCtx) -> Result<String, String> {
            let query = arg_str(args, "query")?;
            let _ = ctx.ledger.append("agent.web_search", "agent", json!({ "query": query }));
            let url = format!(
                "https://html.duckduckgo.com/html/?q={}",
                urlencoding(query)
            );
            let html = get_text(&url, ctx.policy.timeout_secs).await?;
            let results = extract_results(&html);
            if results.is_empty() {
                Ok("(no results)".into())
            } else {
                Ok(results.into_iter().take(8).collect::<Vec<_>>().join("\n"))
            }
        }
    }

    pub struct SendMessageTool;

    #[async_trait]
    impl Tool for SendMessageTool {
        fn name(&self) -> &str {
            "send_message"
        }
        fn description(&self) -> &str {
            "Send a message to a chat channel via an incoming webhook (Slack / Discord / \
             Mattermost style). Pass 'url' or set ENGRAM_WEBHOOK_URL."
        }
        fn is_egress(&self) -> bool {
            true
        }
        fn side_effecting(&self) -> bool {
            true
        }
        fn schema(&self) -> Value {
            json!({ "type": "object",
                "properties": { "text": { "type": "string" }, "url": { "type": "string" } },
                "required": ["text"] })
        }
        async fn run(&self, args: &Value, ctx: &ToolCtx) -> Result<String, String> {
            let text = arg_str(args, "text")?;
            let url = args["url"]
                .as_str()
                .map(String::from)
                .or_else(|| std::env::var("ENGRAM_WEBHOOK_URL").ok())
                .ok_or("no webhook url (pass 'url' or set ENGRAM_WEBHOOK_URL)")?;
            super::guard_url(&url).await?;
            let _ = ctx.ledger.append("agent.send_message", "agent", json!({ "chars": text.len() }));
            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(ctx.policy.timeout_secs))
                .build()
                .map_err(|e| e.to_string())?;
            // Include both "text" (Slack/Mattermost) and "content" (Discord) for compatibility.
            let resp = client
                .post(&url)
                .json(&json!({ "text": text, "content": text }))
                .send()
                .await
                .map_err(|e| e.to_string())?;
            Ok(format!("sent (http {})", resp.status().as_u16()))
        }
    }

    /// Minimal percent-encoding for a query string.
    fn urlencoding(s: &str) -> String {
        let mut out = String::new();
        for b in s.bytes() {
            match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
                b' ' => out.push('+'),
                _ => out.push_str(&format!("%{b:02X}")),
            }
        }
        out
    }

    /// Pull `result__a` anchors (title + href) out of DuckDuckGo's HTML results.
    fn extract_results(html: &str) -> Vec<String> {
        let mut out = Vec::new();
        for chunk in html.split("result__a").skip(1) {
            let href = chunk
                .split_once("href=\"")
                .and_then(|(_, r)| r.split_once('"'))
                .map(|(h, _)| h.to_string());
            let title = chunk
                .split_once('>')
                .and_then(|(_, r)| r.split_once('<'))
                .map(|(t, _)| t.trim().to_string());
            if let (Some(href), Some(title)) = (href, title) {
                if !title.is_empty() {
                    out.push(format!("- {title} :: {href}"));
                }
            }
        }
        out
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn strips_html_to_text() {
            let html = "<html><head><style>x{}</style></head><body><h1>Hi</h1><script>bad()</script><p>there</p></body></html>";
            let text = super::super::html_to_text(html);
            assert!(text.contains("Hi") && text.contains("there"));
            assert!(!text.contains("bad") && !text.contains('{'));
        }

        #[tokio::test]
        #[ignore = "network"]
        async fn fetches_a_real_page() {
            let html = get_text("https://example.com", 15).await.unwrap();
            assert!(html_to_text(&html).contains("Example Domain"));
        }

        #[tokio::test]
        #[ignore = "network"]
        async fn searches_the_real_web() {
            let html = get_text("https://html.duckduckgo.com/html/?q=rust+programming", 15).await.unwrap();
            assert!(!extract_results(&html).is_empty(), "expected search results");
        }
    }
}
