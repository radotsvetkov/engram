//! Built-in tools — the actions an Engram agent can take.
//!
//! Each is small and auditable. Filesystem tools are confined to the workdir; the
//! shell is off by default and refused outright once the run is tainted; web tools
//! taint the run so anything they pull in can't later reach the shell or secrets.

use std::time::Duration;

use async_trait::async_trait;
use engram_memory::{Region, WriteReq};
use serde_json::{json, Value};

use crate::tool::{confine, Tool, ToolCtx};

fn arg_str<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    args[key].as_str().ok_or_else(|| format!("missing string argument '{key}'"))
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

/// Build the (program, args) to run `command` under the configured backend. `Some(image)`
/// runs it sandboxed in a network-isolated container; `None` runs it locally.
pub(crate) fn shell_command(
    image: Option<&str>,
    workdir: &std::path::Path,
    command: &str,
) -> (String, Vec<String>) {
    match image {
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
}

pub struct ShellTool;

#[async_trait]
impl Tool for ShellTool {
    fn name(&self) -> &str {
        "shell"
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
        let hits = ctx.memory.recall(query, &[], k).map_err(|e| e.to_string())?;
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
        // deeper context, but inherits taint — an untrusted parent yields an untrusted child.
        let agent = crate::agent::Agent::new(ctx.gateway.clone(), crate::sub_tools(), ctx.model.clone());
        let mut sub = ctx.clone();
        sub.depth = ctx.depth + 1;
        let run = agent.run(task, sub).await.map_err(|e| e.to_string())?;
        Ok(run.answer)
    }
}

// ---------------------------------------------------------------------------
// Browser (headless Chrome via subprocess — real JS rendering, no extra deps)
// ---------------------------------------------------------------------------

fn find_chrome() -> Option<String> {
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
    async fn run(&self, args: &Value, ctx: &ToolCtx) -> Result<String, String> {
        let url = arg_str(args, "url")?;
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return Err("url must be http(s)".into());
        }
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
    async fn run(&self, args: &Value, ctx: &ToolCtx) -> Result<String, String> {
        let url = arg_str(args, "url")?;
        let path = confine(&ctx.workdir, arg_str(args, "path")?)?;
        let _ = ctx.ledger.append("agent.browser_screenshot", "agent", json!({ "url": url }));
        chrome_screenshot(url, &path, ctx.policy.timeout_secs.max(30)).await?;
        Ok(format!("saved screenshot to {}", path.display()))
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
pub use web::{WebFetchTool, WebSearchTool};

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
            if !(url.starts_with("http://") || url.starts_with("https://")) {
                return Err("url must be http(s)".into());
            }
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
