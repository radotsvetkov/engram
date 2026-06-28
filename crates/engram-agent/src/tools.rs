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

pub(crate) fn arg_str<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    args[key]
        .as_str()
        .ok_or_else(|| format!("missing string argument '{key}'"))
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
    // CRITICAL: unmap IPv4-mapped/-compatible IPv6 BEFORE classifying, so a literal like
    // ::ffff:169.254.169.254 (which parses as V6) is routed through the V4 checks instead of
    // sneaking past them straight to cloud metadata or the loopback control plane.
    if let IpAddr::V6(v6) = ip {
        if let Some(v4) = v6.to_ipv4_mapped().or_else(|| v6.to_ipv4()) {
            return is_blocked_ip(&IpAddr::V4(v4));
        }
    }
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_multicast()
                || v4.octets()[0] == 0
                // CGNAT 100.64.0.0/10, benchmark 198.18.0.0/15, IETF protocol 192.0.0.0/24.
                || (v4.octets()[0] == 100 && (v4.octets()[1] & 0xc0) == 0x40)
                || (v4.octets()[0] == 198 && (v4.octets()[1] & 0xfe) == 18)
                || (v4.octets()[0] == 192 && v4.octets()[1] == 0 && v4.octets()[2] == 0)
        }
        IpAddr::V6(v6) if v6.is_multicast() => true,
        IpAddr::V6(v6) => {
            let s = v6.segments();
            v6.is_loopback() || v6.is_unspecified()
                || (s[0] & 0xffc0) == 0xfe80 // link-local fe80::/10
                || (s[0] & 0xfe00) == 0xfc00 // unique-local fc00::/7
                // Block the v4-embedding transition prefixes outright (they could wrap a private/
                // metadata v4 that to_ipv4_mapped/to_ipv4 don't unwrap): 6to4 2002::/16 and the
                // NAT64 well-known prefix 64:ff9b::/96.
                || s[0] == 0x2002
                || (s[0] == 0x0064 && s[1] == 0xff9b)
        }
    }
}

/// A URL resolved to a set of vetted public socket addresses, ready to be **pinned** so the
/// connection can only ever reach the address we validated (defeating DNS-rebinding TOCTOU).
pub(crate) struct GuardedTarget {
    pub host: String,
    pub addrs: Vec<std::net::SocketAddr>,
}

/// Parse + resolve + SSRF-check a URL, returning the vetted address(es) to pin the connection
/// to. Like [`guard_url`] but it hands back the resolved IPs so the caller connects to exactly
/// those, closing the gap where reqwest/Chrome would re-resolve the hostname after the check.
pub(crate) async fn resolve_guarded(url: &str) -> Result<GuardedTarget, String> {
    let u = url.trim();
    let scheme = if u.starts_with("https://") {
        "https"
    } else if u.starts_with("http://") {
        "http"
    } else {
        return Err("url must be http(s)".into());
    };
    let after = u.split_once("://").map(|(_, b)| b).unwrap_or("");
    let hostport = after.split(['/', '?', '#']).next().unwrap_or("");
    let hostport = hostport.rsplit('@').next().unwrap_or(hostport); // strip userinfo
    let (host, port): (String, u16) = if let Some(rest) = hostport.strip_prefix('[') {
        // [ipv6] or [ipv6]:port
        let h = rest.split(']').next().unwrap_or("").to_string();
        let p = rest.split("]:").nth(1).and_then(|s| s.parse().ok());
        (h, p.unwrap_or(if scheme == "https" { 443 } else { 80 }))
    } else {
        let mut it = hostport.splitn(2, ':');
        let h = it.next().unwrap_or("").to_string();
        let p = it.next().and_then(|s| s.parse().ok());
        (h, p.unwrap_or(if scheme == "https" { 443 } else { 80 }))
    };
    if host.is_empty() {
        return Err("url has no host".into());
    }
    let ips: Vec<std::net::IpAddr> = if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        vec![ip]
    } else {
        tokio::net::lookup_host((host.as_str(), port))
            .await
            .map_err(|e| format!("could not resolve '{host}': {e}"))?
            .map(|sa| sa.ip())
            .collect()
    };
    if ips.is_empty() {
        return Err(format!("could not resolve '{host}'"));
    }
    let mut addrs = Vec::new();
    for ip in ips {
        if is_blocked_ip(&ip) {
            return Err(format!("refusing to reach non-public address {ip}"));
        }
        addrs.push(std::net::SocketAddr::new(ip, port));
    }
    Ok(GuardedTarget { host, addrs })
}

/// Turn a possibly-relative redirect `Location` into an absolute URL against `base`.
pub(crate) fn absolutize(base: &str, loc: &str) -> String {
    if loc.contains("://") {
        return loc.to_string();
    }
    // scheme://authority of the base
    let (scheme, rest) = base.split_once("://").unwrap_or(("https", base));
    let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    if let Some(stripped) = loc.strip_prefix("//") {
        return format!("{scheme}://{stripped}"); // protocol-relative
    }
    if loc.starts_with('/') {
        return format!("{scheme}://{authority}{loc}"); // root-relative
    }
    // path-relative: drop the base's last path segment and append
    let path = &rest[authority.len()..];
    let dir = path.rfind('/').map(|i| &path[..=i]).unwrap_or("/");
    format!("{scheme}://{authority}{dir}{loc}")
}

/// Strip HTML to roughly readable text (drops script/style, collapses whitespace).
///
/// Iterates by CHARACTER, not raw bytes. The old version sliced a separately-lowercased copy at a
/// byte index and did `bytes[i] as char`; on real web content a byte index lands inside a multibyte
/// char (e.g. the curly apostrophe '’', 3 bytes) and the slice PANICKED — which, with panic=abort,
/// took the whole daemon down and surfaced as "Couldn't reach Engram / Load failed". `char_indices`
/// gives valid boundaries, and pushing the real `char` also stops multibyte text being mangled.
pub(crate) fn html_to_text(html: &str) -> String {
    let mut s = String::with_capacity(html.len() / 2);
    let mut in_tag = false;
    let mut skip_depth = 0i32;
    for (i, c) in html.char_indices() {
        if c == '<' {
            // `html[i..]` is always a valid boundary here; lowercase a tiny ASCII lookahead (tag
            // names are ASCII, so this never shifts byte positions) for case-insensitive detection.
            let look = html[i..]
                .chars()
                .take(8)
                .collect::<String>()
                .to_ascii_lowercase();
            if look.starts_with("<script") || look.starts_with("<style") {
                skip_depth += 1;
            } else if look.starts_with("</script") || look.starts_with("</style") {
                skip_depth = (skip_depth - 1).max(0);
            }
            in_tag = true;
        } else if c == '>' {
            in_tag = false;
            s.push(' ');
        } else if !in_tag && skip_depth == 0 {
            s.push(c);
        }
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
            vec![
                "exec".into(),
                b[12..].to_string(),
                "sh".into(),
                "-c".into(),
                command.to_string(),
            ],
        ),
        Some(img) => {
            let mount = format!("{}:/work", workdir.display());
            (
                "docker".into(),
                vec![
                    "run".into(),
                    "--rm".into(),
                    "--network".into(),
                    "none".into(),
                    "-v".into(),
                    mount,
                    "-w".into(),
                    "/work".into(),
                    img.to_string(),
                    "sh".into(),
                    "-c".into(),
                    command.to_string(),
                ],
            )
        }
        None => ("sh".into(), vec!["-c".into(), command.to_string()]),
    }
}

#[cfg(test)]
mod ssrf_guard_tests {
    use super::{absolutize, guard_url, resolve_guarded};

    #[tokio::test]
    async fn blocks_internal_and_non_http_targets() {
        // Loopback (the unauthenticated local API), cloud metadata, private, IPv6 loopback.
        assert!(guard_url("http://127.0.0.1:8088/v1/policy").await.is_err());
        assert!(guard_url("http://localhost/").await.is_err());
        assert!(guard_url("http://169.254.169.254/latest/meta-data/")
            .await
            .is_err());
        assert!(guard_url("http://10.0.0.5/").await.is_err());
        assert!(guard_url("http://192.168.1.1/").await.is_err());
        assert!(guard_url("http://[::1]/").await.is_err());
        // Non-http(s) schemes (local file read, scheme confusion).
        assert!(guard_url("file:///etc/passwd").await.is_err());
        assert!(guard_url("ftp://example.com/").await.is_err());
        // A public literal IP passes (no DNS needed, so this is offline-stable).
        assert!(guard_url("https://1.1.1.1/").await.is_ok());
        // IPv4-mapped IPv6 must be UNMAPPED and blocked (the metadata/loopback bypass).
        assert!(guard_url("http://[::ffff:127.0.0.1]/").await.is_err());
        assert!(guard_url("http://[::ffff:169.254.169.254]/").await.is_err());
        assert!(guard_url("http://[::ffff:10.0.0.5]/").await.is_err());
        // CGNAT and IPv6 multicast.
        assert!(guard_url("http://100.64.1.1/").await.is_err());
        assert!(guard_url("http://[ff02::1]/").await.is_err());
        // v4-embedding transition prefixes (6to4 / NAT64) are blocked outright.
        assert!(guard_url("http://[2002:a9fe:a9fe::1]/").await.is_err());
        assert!(guard_url("http://[64:ff9b::a9fe:a9fe]/").await.is_err());
        // A normal public IPv6 is still allowed (no over-block).
        assert!(guard_url("http://[2606:4700::1]/").await.is_ok());
    }

    #[test]
    fn glob_match_is_linear_and_correct() {
        use super::glob_match;
        assert!(glob_match("**/*.rs", "src/inner/b.rs"));
        assert!(glob_match("src/*.rs", "src/a.rs"));
        assert!(!glob_match("src/*.rs", "src/inner/b.rs"));
        assert!(glob_match("a*b*c", "axxbyyc"));
        assert!(!glob_match("a*b*c", "axxbyy"));
        assert!(glob_match("a?c", "abc"));
        // A pathological pattern that would hang the OLD exponential matcher returns promptly.
        assert!(!glob_match("a*a*a*a*a*a*a*a*b", &"a".repeat(64)));
    }

    // resolve_guarded is what every redirect hop re-runs, so a 3xx to a private/metadata IP is
    // rejected at the next hop. It must reject the same internal targets and pin public ones.
    #[tokio::test]
    async fn resolve_guarded_pins_public_and_rejects_internal() {
        assert!(resolve_guarded("http://169.254.169.254/latest/meta-data/")
            .await
            .is_err());
        assert!(resolve_guarded("http://127.0.0.1/").await.is_err());
        assert!(resolve_guarded("http://10.0.0.5/").await.is_err());
        assert!(resolve_guarded("http://[::1]/").await.is_err());
        assert!(resolve_guarded("ftp://example.com/").await.is_err());
        let ok = resolve_guarded("https://1.1.1.1/path")
            .await
            .expect("public IP must pass");
        assert_eq!(ok.host, "1.1.1.1");
        assert_eq!(ok.addrs[0].port(), 443, "https default port");
        assert!(ok.addrs.iter().all(|a| a.ip().to_string() == "1.1.1.1"));
        // Explicit port is honored (so the pin connects to the right port).
        let p = resolve_guarded("http://1.1.1.1:8080/").await.unwrap();
        assert_eq!(p.addrs[0].port(), 8080);
    }

    #[test]
    fn absolutize_handles_every_redirect_shape() {
        assert_eq!(
            absolutize("https://a.com/x/y", "https://b.com/z"),
            "https://b.com/z"
        );
        assert_eq!(
            absolutize("https://a.com/x/y", "//b.com/z"),
            "https://b.com/z"
        );
        assert_eq!(absolutize("https://a.com/x/y", "/z"), "https://a.com/z");
        assert_eq!(absolutize("https://a.com/x/y", "z"), "https://a.com/x/z");
    }
}

#[cfg(test)]
mod html_text_tests {
    use super::html_to_text;
    #[test]
    fn multibyte_content_does_not_panic_and_is_preserved() {
        // The exact crash class that aborted the daemon ("Couldn't reach Engram"): a multibyte char
        // ('’', emoji) in a long page, where a byte index lands inside the codepoint.
        let big = format!(
            "<html><body><p>{}it’s a test 🌍 with — dashes</p></body></html>",
            "word ".repeat(40000)
        );
        let out = html_to_text(&big); // must not panic
        assert!(out.contains("it’s"), "multibyte preserved, not mangled");
        assert!(out.contains("🌍"));
        assert!(!out.contains('<'));
    }
    #[test]
    fn drops_script_and_style_keeps_text() {
        let h = "<style>body{color:red}</style><p>Hello</p><script>alert(1)</script>";
        assert_eq!(html_to_text(h), "Hello");
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
        assert_eq!(
            args,
            vec!["deploy@vps.example".to_string(), "uptime".to_string()]
        );
    }

    #[test]
    fn singularity_backend_execs_in_image() {
        let (prog, args) = shell_command(Some("singularity:img.sif"), Path::new("/w"), "id");
        assert_eq!(prog, "singularity");
        assert_eq!(
            args,
            vec![
                "exec".to_string(),
                "img.sif".to_string(),
                "sh".to_string(),
                "-c".to_string(),
                "id".to_string()
            ]
        );
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
        "Read a UTF-8 text file inside the working directory. Use the optional 0-based `offset` \
         (start line) and `limit` (number of lines) to page through a large file; the response \
         header reports the file's total line and byte count so you know how much remains."
    }
    fn schema(&self) -> Value {
        json!({ "type": "object", "properties": {
            "path": { "type": "string" },
            "offset": { "type": "integer", "description": "0-based first line to return" },
            "limit": { "type": "integer", "description": "max lines to return (default 250)" }
        }, "required": ["path"] })
    }
    // Local files may hold private data, so reading one marks the run sensitive (arms no-egress).
    fn reads_sensitive(&self) -> bool {
        true
    }
    async fn run(&self, args: &Value, ctx: &ToolCtx) -> Result<String, String> {
        let path = confine(&ctx.workdir, arg_str(args, "path")?)?;
        let text = tokio::fs::read_to_string(&path)
            .await
            .map_err(|e| e.to_string())?;
        let total_bytes = text.len();
        let lines: Vec<&str> = text.lines().collect();
        let total_lines = lines.len();
        let offset = args["offset"].as_u64().unwrap_or(0) as usize;
        let limit = (args["limit"].as_u64().unwrap_or(250) as usize).clamp(1, 2000);
        if total_lines == 0 {
            return Ok("(empty file)".into());
        }
        if offset >= total_lines {
            return Err(format!(
                "offset {offset} is past the end ({total_lines} lines)"
            ));
        }
        let end = (offset + limit).min(total_lines);
        let body = lines[offset..end].join("\n");
        let more = if end < total_lines {
            format!(
                " — {} more line(s), read with offset {end}",
                total_lines - end
            )
        } else {
            String::new()
        };
        Ok(format!(
            "[lines {}-{} of {total_lines} · {total_bytes} bytes{more}]\n{body}",
            offset + 1,
            end
        ))
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
        // Accept common synonyms a model might use for the body, and give a clear, recoverable error
        // (the old one fired even when the model DID send content under a different key, or when a
        // huge value got truncated — and the model just retried the same broken call until stopped).
        let content = ["content", "text", "body", "data", "contents"]
            .iter()
            .find_map(|k| args[*k].as_str())
            .ok_or("write_file needs the file text in a 'content' string argument")?;
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| e.to_string())?;
        }
        tokio::fs::write(&path, content)
            .await
            .map_err(|e| e.to_string())?;
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
        let mut rd = tokio::fs::read_dir(&path)
            .await
            .map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        while let Some(e) = rd.next_entry().await.map_err(|e| e.to_string())? {
            let kind = if e.path().is_dir() { "dir " } else { "file" };
            out.push(format!("{kind}  {}", e.file_name().to_string_lossy()));
        }
        out.sort();
        Ok(if out.is_empty() {
            "(empty)".into()
        } else {
            out.join("\n")
        })
    }
}

/// Surgical, single-occurrence text replacement - the best-in-class harness edit primitive. The
/// model sends the exact `old` text and its `new` replacement; the edit is rejected unless `old`
/// occurs exactly once, so it can never silently change the wrong place or drop the rest of a file.
pub struct EditFileTool;

#[async_trait]
impl Tool for EditFileTool {
    fn name(&self) -> &str {
        "edit_file"
    }
    fn side_effecting(&self) -> bool {
        true
    }
    fn description(&self) -> &str {
        "Replace an exact unique substring in a file inside the working directory. `old` must \
         match verbatim (including whitespace) and occur exactly once, else the edit is refused. \
         This is the safe way to change part of a file without re-sending the whole thing."
    }
    fn schema(&self) -> Value {
        json!({ "type": "object",
            "properties": {
                "path": { "type": "string" },
                "old": { "type": "string", "description": "exact text to replace (must be unique)" },
                "new": { "type": "string", "description": "replacement text" }
            },
            "required": ["path", "old", "new"] })
    }
    async fn run(&self, args: &Value, ctx: &ToolCtx) -> Result<String, String> {
        if !ctx.policy.allow_write {
            return Err("file writing is disabled".into());
        }
        let path = confine(&ctx.workdir, arg_str(args, "path")?)?;
        let old = arg_str(args, "old")?;
        let new = args["new"].as_str().unwrap_or("");
        if old.is_empty() {
            return Err("'old' must not be empty".into());
        }
        let text = tokio::fs::read_to_string(&path)
            .await
            .map_err(|e| e.to_string())?;
        let count = text.matches(old).count();
        if count == 0 {
            return Err("'old' text was not found in the file".into());
        }
        if count > 1 {
            return Err(format!(
                "'old' matched {count} times - include more surrounding context so it is unique"
            ));
        }
        let updated = text.replacen(old, new, 1);
        tokio::fs::write(&path, &updated)
            .await
            .map_err(|e| e.to_string())?;
        let _ = ctx.ledger.append(
            "agent.edit",
            "agent",
            json!({ "path": path.to_string_lossy(), "removed": old.len(), "added": new.len() }),
        );
        Ok(format!(
            "edited {} (−{} +{} bytes)",
            arg_str(args, "path")?,
            old.len(),
            new.len()
        ))
    }
}

/// Append text to a file (creating it if needed) - cheaper than read+rewrite for logs/notes.
pub struct AppendFileTool;

#[async_trait]
impl Tool for AppendFileTool {
    fn name(&self) -> &str {
        "append_file"
    }
    fn side_effecting(&self) -> bool {
        true
    }
    fn description(&self) -> &str {
        "Append text to the end of a file inside the working directory (creates it if absent)."
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
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| e.to_string())?;
        }
        use tokio::io::AsyncWriteExt;
        let mut f = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await
            .map_err(|e| e.to_string())?;
        f.write_all(content.as_bytes())
            .await
            .map_err(|e| e.to_string())?;
        // Flush before returning so a subsequent read observes the append deterministically (the
        // tokio File buffers, and dropping it doesn't guarantee the bytes are visible yet).
        f.flush().await.map_err(|e| e.to_string())?;
        let _ = ctx.ledger.append(
            "agent.append",
            "agent",
            json!({ "path": path.to_string_lossy(), "bytes": content.len() }),
        );
        Ok(format!("appended {} bytes", content.len()))
    }
}

/// Filesystem housekeeping confined to the workdir: make_dir / move_file / copy_file / delete_file.
pub struct MakeDirTool;

#[async_trait]
impl Tool for MakeDirTool {
    fn name(&self) -> &str {
        "make_dir"
    }
    fn side_effecting(&self) -> bool {
        true
    }
    fn description(&self) -> &str {
        "Create a directory (and parents) inside the working directory."
    }
    fn schema(&self) -> Value {
        json!({ "type": "object", "properties": { "path": { "type": "string" } }, "required": ["path"] })
    }
    async fn run(&self, args: &Value, ctx: &ToolCtx) -> Result<String, String> {
        if !ctx.policy.allow_write {
            return Err("file writing is disabled".into());
        }
        let path = confine(&ctx.workdir, arg_str(args, "path")?)?;
        tokio::fs::create_dir_all(&path)
            .await
            .map_err(|e| e.to_string())?;
        let _ = ctx.ledger.append(
            "agent.mkdir",
            "agent",
            json!({ "path": path.to_string_lossy() }),
        );
        Ok(format!("created {}", arg_str(args, "path")?))
    }
}

pub struct MoveFileTool;

#[async_trait]
impl Tool for MoveFileTool {
    fn name(&self) -> &str {
        "move_file"
    }
    fn side_effecting(&self) -> bool {
        true
    }
    fn description(&self) -> &str {
        "Move or rename a file/directory within the working directory (`from` -> `to`)."
    }
    fn schema(&self) -> Value {
        json!({ "type": "object",
            "properties": { "from": { "type": "string" }, "to": { "type": "string" } },
            "required": ["from", "to"] })
    }
    async fn run(&self, args: &Value, ctx: &ToolCtx) -> Result<String, String> {
        if !ctx.policy.allow_write {
            return Err("file writing is disabled".into());
        }
        let from = confine(&ctx.workdir, arg_str(args, "from")?)?;
        let to = confine(&ctx.workdir, arg_str(args, "to")?)?;
        if from == ctx.workdir || to == ctx.workdir {
            return Err("refusing to move the working directory itself".into());
        }
        if let Some(parent) = to.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| e.to_string())?;
        }
        tokio::fs::rename(&from, &to)
            .await
            .map_err(|e| e.to_string())?;
        let _ = ctx.ledger.append(
            "agent.move",
            "agent",
            json!({ "from": from.to_string_lossy(), "to": to.to_string_lossy() }),
        );
        Ok(format!(
            "moved {} -> {}",
            arg_str(args, "from")?,
            arg_str(args, "to")?
        ))
    }
}

pub struct CopyFileTool;

#[async_trait]
impl Tool for CopyFileTool {
    fn name(&self) -> &str {
        "copy_file"
    }
    fn side_effecting(&self) -> bool {
        true
    }
    fn description(&self) -> &str {
        "Copy a file within the working directory (`from` -> `to`)."
    }
    fn schema(&self) -> Value {
        json!({ "type": "object",
            "properties": { "from": { "type": "string" }, "to": { "type": "string" } },
            "required": ["from", "to"] })
    }
    async fn run(&self, args: &Value, ctx: &ToolCtx) -> Result<String, String> {
        if !ctx.policy.allow_write {
            return Err("file writing is disabled".into());
        }
        let from = confine(&ctx.workdir, arg_str(args, "from")?)?;
        let to = confine(&ctx.workdir, arg_str(args, "to")?)?;
        if let Some(parent) = to.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| e.to_string())?;
        }
        let n = tokio::fs::copy(&from, &to)
            .await
            .map_err(|e| e.to_string())?;
        let _ = ctx.ledger.append(
            "agent.copy",
            "agent",
            json!({ "from": from.to_string_lossy(), "to": to.to_string_lossy(), "bytes": n }),
        );
        Ok(format!("copied {n} bytes -> {}", arg_str(args, "to")?))
    }
}

pub struct DeleteFileTool;

#[async_trait]
impl Tool for DeleteFileTool {
    fn name(&self) -> &str {
        "delete_file"
    }
    fn side_effecting(&self) -> bool {
        true
    }
    fn description(&self) -> &str {
        "Delete a file (or an empty directory) inside the working directory."
    }
    fn schema(&self) -> Value {
        json!({ "type": "object", "properties": { "path": { "type": "string" } }, "required": ["path"] })
    }
    async fn run(&self, args: &Value, ctx: &ToolCtx) -> Result<String, String> {
        if !ctx.policy.allow_write {
            return Err("file writing is disabled".into());
        }
        let path = confine(&ctx.workdir, arg_str(args, "path")?)?;
        if path == ctx.workdir {
            return Err("refusing to delete the working directory itself".into());
        }
        let meta = tokio::fs::metadata(&path)
            .await
            .map_err(|e| e.to_string())?;
        if meta.is_dir() {
            tokio::fs::remove_dir(&path)
                .await
                .map_err(|e| e.to_string())?;
        } else {
            tokio::fs::remove_file(&path)
                .await
                .map_err(|e| e.to_string())?;
        }
        let _ = ctx.ledger.append(
            "agent.delete",
            "agent",
            json!({ "path": path.to_string_lossy() }),
        );
        Ok(format!("deleted {}", arg_str(args, "path")?))
    }
}

/// Find files by glob pattern (`*`, `?`, `**`) recursively within the workdir - the discovery
/// primitive a real harness needs (non-recursive `list_dir` alone can't find anything).
pub struct GlobTool;

#[async_trait]
impl Tool for GlobTool {
    fn name(&self) -> &str {
        "glob"
    }
    fn description(&self) -> &str {
        "Find files matching a glob pattern (e.g. `**/*.rs`, `src/*.md`) within the working \
         directory. `*` matches within a path segment, `**` matches across segments, `?` one char."
    }
    fn schema(&self) -> Value {
        json!({ "type": "object",
            "properties": {
                "pattern": { "type": "string" },
                "path": { "type": "string", "description": "subdir to search under (default '.')" }
            },
            "required": ["pattern"] })
    }
    fn reads_sensitive(&self) -> bool {
        true
    }
    async fn run(&self, args: &Value, ctx: &ToolCtx) -> Result<String, String> {
        let pattern = arg_str(args, "pattern")?;
        let root = confine(&ctx.workdir, args["path"].as_str().unwrap_or("."))?;
        let base = std::fs::canonicalize(&ctx.workdir).unwrap_or_else(|_| ctx.workdir.clone());
        let mut out = Vec::new();
        let mut stack = vec![root];
        let limit = 500usize;
        while let Some(dir) = stack.pop() {
            let mut rd = match tokio::fs::read_dir(&dir).await {
                Ok(rd) => rd,
                Err(_) => continue,
            };
            while let Some(e) = rd.next_entry().await.map_err(|e| e.to_string())? {
                let p = e.path();
                let ft = e.file_type().await.map_err(|e| e.to_string())?;
                if ft.is_dir() {
                    // skip the usual heavy/noise dirs
                    let name = e.file_name();
                    let n = name.to_string_lossy();
                    if n == ".git" || n == "node_modules" || n == "target" {
                        continue;
                    }
                    stack.push(p);
                } else {
                    let rel = p
                        .strip_prefix(&base)
                        .or_else(|_| p.strip_prefix(&ctx.workdir))
                        .unwrap_or(&p);
                    let rels = rel.to_string_lossy().replace('\\', "/");
                    // Don't surface a symlink that escapes the workdir (resolve + prefix-check).
                    if std::fs::canonicalize(&p)
                        .map(|c| !c.starts_with(&base))
                        .unwrap_or(true)
                    {
                        continue;
                    }
                    if glob_match(pattern, &rels) {
                        out.push(rels);
                        if out.len() >= limit {
                            out.push(format!("(stopped at {limit} matches)"));
                            out.sort();
                            return Ok(out.join("\n"));
                        }
                    }
                }
            }
        }
        out.sort();
        Ok(if out.is_empty() {
            "(no matches)".into()
        } else {
            out.join("\n")
        })
    }
}

/// Search file contents for a fixed string (optionally case-insensitive), returning `file:line`
/// hits - the content-discovery primitive (`grep`) every capable agent harness exposes.
pub struct GrepTool;

#[async_trait]
impl Tool for GrepTool {
    fn name(&self) -> &str {
        "grep"
    }
    fn description(&self) -> &str {
        "Search file contents within the working directory for a literal string and return \
         `path:line: text` hits. Optional `glob` to restrict files and `ignore_case`."
    }
    fn schema(&self) -> Value {
        json!({ "type": "object",
            "properties": {
                "query": { "type": "string" },
                "glob": { "type": "string", "description": "only search files matching this glob" },
                "ignore_case": { "type": "boolean" }
            },
            "required": ["query"] })
    }
    fn reads_sensitive(&self) -> bool {
        true
    }
    async fn run(&self, args: &Value, ctx: &ToolCtx) -> Result<String, String> {
        let query = arg_str(args, "query")?;
        let ignore_case = args["ignore_case"].as_bool().unwrap_or(false);
        let needle = if ignore_case {
            query.to_lowercase()
        } else {
            query.to_string()
        };
        let glob = args["glob"].as_str();
        let base = std::fs::canonicalize(&ctx.workdir).unwrap_or_else(|_| ctx.workdir.clone());
        let mut out = Vec::new();
        let mut stack = vec![ctx.workdir.clone()];
        let limit = 200usize;
        while let Some(dir) = stack.pop() {
            let mut rd = match tokio::fs::read_dir(&dir).await {
                Ok(rd) => rd,
                Err(_) => continue,
            };
            while let Some(e) = rd.next_entry().await.map_err(|e| e.to_string())? {
                let p = e.path();
                let ft = e.file_type().await.map_err(|e| e.to_string())?;
                if ft.is_dir() {
                    let n = e.file_name();
                    let n = n.to_string_lossy();
                    if n == ".git" || n == "node_modules" || n == "target" {
                        continue;
                    }
                    stack.push(p);
                    continue;
                }
                let rel = p
                    .strip_prefix(&base)
                    .or_else(|_| p.strip_prefix(&ctx.workdir))
                    .unwrap_or(&p);
                let rels = rel.to_string_lossy().replace('\\', "/");
                if let Some(g) = glob {
                    if !glob_match(g, &rels) {
                        continue;
                    }
                }
                // Don't follow an in-workdir symlink that points OUTSIDE the workdir (confine bypass):
                // resolve the real path and require it to stay under the canonical workdir.
                if std::fs::canonicalize(&p)
                    .map(|c| !c.starts_with(&base))
                    .unwrap_or(true)
                {
                    continue;
                }
                // Read as text; skip binaries / unreadable files quietly.
                let Ok(text) = tokio::fs::read_to_string(&p).await else {
                    continue;
                };
                for (i, line) in text.lines().enumerate() {
                    let hay = if ignore_case {
                        line.to_lowercase()
                    } else {
                        line.to_string()
                    };
                    if hay.contains(&needle) {
                        let shown: String = line.chars().take(200).collect();
                        out.push(format!("{rels}:{}: {}", i + 1, shown.trim()));
                        if out.len() >= limit {
                            out.push(format!("(stopped at {limit} hits)"));
                            return Ok(out.join("\n"));
                        }
                    }
                }
            }
        }
        Ok(if out.is_empty() {
            "(no matches)".into()
        } else {
            out.join("\n")
        })
    }
}

/// Minimal glob matcher over a `/`-separated relative path. Supports `*` (within a segment),
/// `**` (across segments), and `?` (one char). Good enough for find-by-pattern without a dep.
pub(crate) fn glob_match(pattern: &str, path: &str) -> bool {
    // Iterative two-pointer wildcard match with a single backtrack anchor - O(len(p)*len(t)) worst
    // case, NO exponential backtracking (the old recursive `*` could hang a core on a crafted
    // pattern, and tool calls have no timeout). `*`/`?` never cross '/' (segments split by `rec`).
    fn seg_match(p: &[u8], t: &[u8]) -> bool {
        let (mut pi, mut ti) = (0usize, 0usize);
        let (mut star, mut mark) = (None, 0usize);
        while ti < t.len() {
            if pi < p.len() && (p[pi] == t[ti] || (p[pi] == b'?' && t[ti] != b'/')) {
                pi += 1;
                ti += 1;
            } else if pi < p.len() && p[pi] == b'*' {
                star = Some(pi);
                mark = ti;
                pi += 1;
            } else if let Some(sp) = star {
                // Backtrack: let the last `*` consume one more char (never crossing '/').
                if t[mark] == b'/' {
                    return false;
                }
                pi = sp + 1;
                mark += 1;
                ti = mark;
            } else {
                return false;
            }
        }
        while pi < p.len() && p[pi] == b'*' {
            pi += 1;
        }
        pi == p.len()
    }
    // Handle `**` by matching the rest of the pattern against any suffix of the path segments.
    fn rec(pp: &[&str], tt: &[&str]) -> bool {
        match pp.first() {
            None => tt.is_empty(),
            Some(&"**") => {
                // `**` matches zero or more path segments.
                (0..=tt.len()).any(|i| rec(&pp[1..], &tt[i..]))
            }
            Some(seg) => {
                !tt.is_empty()
                    && seg_match(seg.as_bytes(), tt[0].as_bytes())
                    && rec(&pp[1..], &tt[1..])
            }
        }
    }
    let pp: Vec<&str> = pattern.split('/').filter(|s| !s.is_empty()).collect();
    let tt: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    rec(&pp, &tt)
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
        let _ = ctx.ledger.append(
            "agent.plan",
            "agent",
            json!({ "steps": steps, "total": total, "done": done }),
        );
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
    // Recall surfaces the user's private knowledge into the run, so it marks the run sensitive:
    // combined with reading untrusted content, that arms the no-egress exfiltration guard.
    fn reads_sensitive(&self) -> bool {
        true
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
        Ok(hits
            .iter()
            .map(|h| format!("- [{}] {}", h.record.region, h.record.text))
            .collect::<Vec<_>>()
            .join("\n"))
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
    // The image is attacker-influenceable content interpreted into the model (a screenshot of a
    // hostile page, a downloaded picture), so looking at it taints the run; the local file may
    // also be private, so it sensitises too - both arms of the trifecta gate.
    fn taints(&self) -> bool {
        true
    }
    fn reads_sensitive(&self) -> bool {
        true
    }
    async fn run(&self, args: &Value, ctx: &ToolCtx) -> Result<String, String> {
        let path = confine(&ctx.workdir, arg_str(args, "path")?)?;
        let question = arg_str(args, "question")?;
        let bytes = tokio::fs::read(&path).await.map_err(|e| e.to_string())?;
        let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
        let model = ctx
            .policy
            .vision_model
            .clone()
            .or_else(|| std::env::var("ENGRAM_VISION_MODEL").ok())
            .unwrap_or_else(|| ctx.model.clone());
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
        let bytes = ctx
            .gateway
            .generate_image(prompt, "agent")
            .await
            .map_err(|e| e.to_string())?;
        tokio::fs::write(&path, &bytes)
            .await
            .map_err(|e| e.to_string())?;
        Ok(format!(
            "saved {}-byte image to {}",
            bytes.len(),
            path.display()
        ))
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
        let bytes = ctx
            .gateway
            .tts(text, voice, "agent")
            .await
            .map_err(|e| e.to_string())?;
        tokio::fs::write(&path, &bytes)
            .await
            .map_err(|e| e.to_string())?;
        Ok(format!(
            "saved {}-byte audio to {}",
            bytes.len(),
            path.display()
        ))
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
    // Transcribed audio is untrusted, model-interpreted content (an attacker can speak an
    // injection); the source file may be private too.
    fn taints(&self) -> bool {
        true
    }
    fn reads_sensitive(&self) -> bool {
        true
    }
    async fn run(&self, args: &Value, ctx: &ToolCtx) -> Result<String, String> {
        let path = confine(&ctx.workdir, arg_str(args, "path")?)?;
        let format = args["format"]
            .as_str()
            .map(String::from)
            .or_else(|| path.extension().and_then(|e| e.to_str()).map(String::from))
            .unwrap_or_else(|| "mp3".into());
        let bytes = tokio::fs::read(&path).await.map_err(|e| e.to_string())?;
        ctx.gateway
            .transcribe(&bytes, &format, "agent")
            .await
            .map_err(|e| e.to_string())
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
        let _ = ctx.ledger.append(
            "agent.delegate",
            "agent",
            json!({ "task": task, "depth": ctx.depth }),
        );
        // The subagent gets the base toolset (no further delegation by default) and a
        // deeper context, but inherits taint - an untrusted parent yields an untrusted child.
        let agent =
            crate::agent::Agent::new(ctx.gateway.clone(), crate::sub_tools(), ctx.model.clone());
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
    CANDIDATES
        .iter()
        .find(|p| std::path::Path::new(p).exists())
        .map(|s| s.to_string())
}

const CHROME_FLAGS: &[&str] = &[
    "--headless=new",
    "--disable-gpu",
    "--no-sandbox",
    "--no-first-run",
    "--no-default-browser-check",
    "--disable-extensions",
];

/// A dedicated, per-process Chrome profile dir. Without `--user-data-dir` Chrome uses the default
/// profile, so when the user already has Chrome open our headless launch can't start (locked
/// profile) and produces no output. A unique dir makes every Engram Chrome an independent instance.
fn chrome_profile_dir() -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("engram-chrome-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&d);
    d
}

/// Build a Chrome `--host-resolver-rules` flag that PINS the URL's host to the single vetted IP
/// (and fails every other resolution), so Chrome's own resolver/redirects can't be DNS-rebound or
/// 302'd to a private/metadata address. Validates the URL through the SSRF guard along the way.
async fn resolver_rule(url: &str) -> Result<String, String> {
    let t = resolve_guarded(url).await?;
    let ip = t.addrs.first().ok_or("no vetted address")?.ip();
    Ok(format!(
        "--host-resolver-rules=MAP {} {ip},MAP * ^NOTFOUND",
        t.host
    ))
}

async fn chrome_dump_dom(url: &str, timeout: u64) -> Result<String, String> {
    let chrome = find_chrome().ok_or("no Chrome/Chromium found (set ENGRAM_CHROME)")?;
    let pin = resolver_rule(url).await?;
    let fut = tokio::process::Command::new(&chrome)
        .args(CHROME_FLAGS)
        .arg(format!("--user-data-dir={}", chrome_profile_dir().display()))
        .arg(&pin)
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
            String::from_utf8_lossy(&out.stderr)
                .chars()
                .take(200)
                .collect::<String>()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

async fn chrome_screenshot(
    url: &str,
    out_path: &std::path::Path,
    timeout: u64,
) -> Result<(), String> {
    let chrome = find_chrome().ok_or("no Chrome/Chromium found (set ENGRAM_CHROME)")?;
    let pin = resolver_rule(url).await?;
    let fut = tokio::process::Command::new(&chrome)
        .args(CHROME_FLAGS)
        .arg(format!("--user-data-dir={}", chrome_profile_dir().display()))
        .arg(&pin)
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
        false
    }
    async fn run(&self, args: &Value, ctx: &ToolCtx) -> Result<String, String> {
        let url = arg_str(args, "url")?;
        guard_url(url).await?;
        let _ = ctx
            .ledger
            .append("agent.browser_read", "agent", json!({ "url": url }));
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
        "Screenshot the LIVE interactive browser session (the page you navigated to, with its \
         cookies/login intact) to a PNG in the workdir. Pass an optional `url` to navigate there \
         first, and an optional `question` to have the agent SEE and describe the screenshot."
    }
    fn schema(&self) -> Value {
        json!({ "type": "object",
            "properties": {
                "url": { "type": "string", "description": "navigate here first (optional)" },
                "path": { "type": "string", "description": "save path (default screenshot.png)" },
                "question": { "type": "string", "description": "if set, look at the shot and answer this" }
            } })
    }
    fn taints(&self) -> bool {
        true
    }
    fn is_egress(&self) -> bool {
        false
    }
    async fn run(&self, args: &Value, ctx: &ToolCtx) -> Result<String, String> {
        let rel = args["path"].as_str().unwrap_or("screenshot.png");
        let path = confine(&ctx.workdir, rel)?;
        // Navigate the LIVE session first when a url is given, so the shot reflects the real,
        // authenticated, multi-step page (not a throwaway cookieless Chrome).
        if let Some(url) = args["url"].as_str() {
            guard_url(url).await?;
            ctx.browser.open(url).await?;
        }
        let _ = ctx
            .ledger
            .append("agent.browser_screenshot", "agent", json!({ "path": rel }));
        // Capture from the live CDP session. If interactive browsing isn't built in but a url was
        // given, fall back to the standalone headless capture so the tool still works.
        if let Err(e) = ctx.browser.screenshot(&path).await {
            match args["url"].as_str() {
                Some(url) => chrome_screenshot(url, &path, ctx.policy.timeout_secs.max(30)).await?,
                None => return Err(e),
            }
        }
        // Vision-in-the-loop: when asked a question, the agent actually SEES the page by routing
        // the PNG through the multimodal gateway, returning the description as its observation.
        if let Some(question) = args["question"].as_str().filter(|q| !q.is_empty()) {
            let bytes = tokio::fs::read(&path).await.map_err(|e| e.to_string())?;
            let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
            let model = ctx
                .policy
                .vision_model
                .clone()
                .or_else(|| std::env::var("ENGRAM_VISION_MODEL").ok())
                .unwrap_or_else(|| ctx.model.clone());
            let req = CompletionRequest::new(model, vec![Message::user_with_image(question, b64)]);
            let c = ctx
                .gateway
                .complete(Call::new(req).actor("agent").tainted(ctx.taint))
                .await
                .map_err(|e| e.to_string())?;
            return Ok(format!("[saved {}] {}", path.display(), c.text));
        }
        Ok(format!(
            "saved screenshot to {} (use vision_analyze or pass a question to read it)",
            path.display()
        ))
    }
}

pub struct BrowserWaitTool;

#[async_trait]
impl Tool for BrowserWaitTool {
    fn name(&self) -> &str {
        "browser_wait"
    }
    fn description(&self) -> &str {
        "Wait until a CSS selector appears in the live browser page (for content that renders \
         after navigation or an interaction). Returns once present or times out."
    }
    fn schema(&self) -> Value {
        json!({ "type": "object",
            "properties": {
                "selector": { "type": "string" },
                "timeout_ms": { "type": "integer", "description": "default 8000" }
            },
            "required": ["selector"] })
    }
    fn side_effecting(&self) -> bool {
        true
    }
    async fn run(&self, args: &Value, ctx: &ToolCtx) -> Result<String, String> {
        let selector = arg_str(args, "selector")?;
        let timeout = args["timeout_ms"].as_u64().unwrap_or(8000);
        ctx.browser.wait_for(selector, timeout).await
    }
}

pub struct BrowserScrollTool;

#[async_trait]
impl Tool for BrowserScrollTool {
    fn name(&self) -> &str {
        "browser_scroll"
    }
    fn description(&self) -> &str {
        "Scroll the live browser page by a number of pixels (negative scrolls up) to reveal \
         lazy-loaded or below-the-fold content."
    }
    fn schema(&self) -> Value {
        json!({ "type": "object", "properties": { "dy": { "type": "integer" } }, "required": ["dy"] })
    }
    fn side_effecting(&self) -> bool {
        true
    }
    async fn run(&self, args: &Value, ctx: &ToolCtx) -> Result<String, String> {
        let dy = args["dy"].as_i64().unwrap_or(600);
        ctx.browser.scroll(dy).await
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
        false
    }
    async fn run(&self, args: &Value, ctx: &ToolCtx) -> Result<String, String> {
        let url = arg_str(args, "url")?;
        guard_url(url).await?;
        let _ = ctx
            .ledger
            .append("agent.browser_open", "agent", json!({ "url": url }));
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
        false
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
        false
    }
    async fn run(&self, args: &Value, ctx: &ToolCtx) -> Result<String, String> {
        let _ = ctx.ledger.append("agent.browser_type", "agent", json!({}));
        ctx.browser
            .type_text(arg_str(args, "selector")?, arg_str(args, "text")?)
            .await
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
        false
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
        chrome_screenshot("https://example.com", &out, 30)
            .await
            .unwrap();
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

    /// Fetch text with SSRF protection that survives redirects and DNS-rebinding: every hop is
    /// re-validated by [`resolve_guarded`] and the connection is **pinned** to the vetted IP via
    /// `resolve_to_addrs`, so a public host can't 302 to (or rebind to) a private/metadata IP.
    async fn get_text(url: &str, timeout: u64) -> Result<String, String> {
        let mut current = url.to_string();
        for _hop in 0..6 {
            let target = super::resolve_guarded(&current).await?;
            let client = reqwest::Client::builder()
                // A real browser UA: search engines and many sites drop/deny a bot UA like
                // "engram-agent/0.1", which made web_search/web_fetch silently fail.
                .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
                .timeout(Duration::from_secs(timeout))
                // Follow redirects manually so each hop is SSRF-checked before we connect.
                .redirect(reqwest::redirect::Policy::none())
                // Pin the hostname to the exact IPs we just validated (no re-resolution).
                .resolve_to_addrs(&target.host, &target.addrs)
                .build()
                .map_err(|e| e.to_string())?;
            let resp = client
                .get(&current)
                .send()
                .await
                .map_err(|e| e.to_string())?;
            if resp.status().is_redirection() {
                if let Some(loc) = resp
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|v| v.to_str().ok())
                {
                    current = super::absolutize(&current, loc);
                    continue;
                }
            }
            return resp.text().await.map_err(|e| e.to_string());
        }
        Err("too many redirects".into())
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
        // Fetching a URL to READ its content is research/ingress (SSRF-guarded), not exfiltration,
        // so it is NOT egress - otherwise the trifecta blocked all web research the moment the run
        // had recalled any memory. The clear data-out channel (send_message) stays egress-gated.
        fn is_egress(&self) -> bool {
            false
        }
        async fn run(&self, args: &Value, ctx: &ToolCtx) -> Result<String, String> {
            let url = arg_str(args, "url")?;
            super::guard_url(url).await?;
            let _ = ctx
                .ledger
                .append("agent.web_fetch", "agent", json!({ "url": url }));
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
        // Searching the web to find sources is research/ingress, not exfiltration (a short query is
        // not a meaningful data-out channel), so it is NOT egress - this is what kept blocking
        // research tasks. The explicit send channel (send_message) remains egress-gated.
        fn is_egress(&self) -> bool {
            false
        }
        async fn run(&self, args: &Value, ctx: &ToolCtx) -> Result<String, String> {
            let query = arg_str(args, "query")?;
            let _ = ctx
                .ledger
                .append("agent.web_search", "agent", json!({ "query": query }));
            let q = urlencoding(query);
            let to = ctx.policy.timeout_secs;
            // Resilient search: DuckDuckGo's scrape endpoints block/timeout from many networks, which
            // left web_search returning nothing. Try DDG, then Bing, then DDG-lite — take the first
            // that yields results, so a single dead backend doesn't break research.
            let attempts: [(&str, fn(&str) -> Vec<String>); 3] = [
                ("https://search.brave.com/search?q=", extract_brave_results),
                ("https://html.duckduckgo.com/html/?q=", extract_results),
                ("https://lite.duckduckgo.com/lite/?q=", extract_results),
            ];
            let mut last_err = String::new();
            for (base, parse) in attempts {
                match get_text(&format!("{base}{q}"), to).await {
                    Ok(html) => {
                        let results = parse(&html);
                        if !results.is_empty() {
                            return Ok(results.into_iter().take(8).collect::<Vec<_>>().join("\n"));
                        }
                    }
                    Err(e) => last_err = e,
                }
            }
            if last_err.is_empty() {
                Ok("(no results)".into())
            } else {
                Err(format!("web search unavailable (all engines failed): {last_err}"))
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
            // Precedence: explicit per-call url > the default from settings (Integrations) >
            // the ENGRAM_WEBHOOK_URL env var. resolve_guarded below SSRF-validates the winner.
            let url = args["url"]
                .as_str()
                .map(String::from)
                .or_else(|| ctx.policy.webhook_url.clone())
                .or_else(|| std::env::var("ENGRAM_WEBHOOK_URL").ok())
                .ok_or("no webhook url (pass 'url', set one in Integrations, or set ENGRAM_WEBHOOK_URL)")?;
            let target = super::resolve_guarded(&url).await?;
            let _ = ctx.ledger.append(
                "agent.send_message",
                "agent",
                json!({ "chars": text.len() }),
            );
            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(ctx.policy.timeout_secs))
                // Pin to the vetted IP and never auto-follow a redirect to a private address.
                .redirect(reqwest::redirect::Policy::none())
                .resolve_to_addrs(&target.host, &target.addrs)
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
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    out.push(b as char)
                }
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

    /// Parse Brave Search HTML: each web result follows `data-type="web"` and carries an
    /// `href="URL"` plus a `class="title…">Title`. Brave returns server-rendered results without an
    /// API key and is the most reliable scrape target — DuckDuckGo blocks many networks outright and
    /// Bing serves a JS-only shell with no parseable results. Titles are tag-stripped.
    fn extract_brave_results(html: &str) -> Vec<String> {
        let mut out = Vec::new();
        for block in html.split("data-type=\"web\"").skip(1) {
            let block = &block[..block.len().min(4000)];
            // First external (non-Brave) http link in the block is the result URL.
            let href = block
                .split("href=\"")
                .skip(1)
                .filter_map(|s| s.split_once('"').map(|(h, _)| h))
                .find(|h| h.starts_with("http") && !h.contains("brave.com"))
                .map(|h| h.to_string());
            let title = block
                .split_once("class=\"title")
                .and_then(|(_, r)| r.split_once('>'))
                .and_then(|(_, r)| r.split_once("</"))
                .map(|(t, _)| super::html_to_text(t).trim().to_string())
                .filter(|t| !t.is_empty());
            if let (Some(href), Some(title)) = (href, title) {
                out.push(format!("- {title} :: {href}"));
            }
        }
        // Brave varies its markup under bot-detection (sometimes the structured blocks don't carry a
        // direct href). Fall back to a broad scan of external result anchors so we still return
        // something usable instead of "(no results)".
        if out.is_empty() {
            out = broad_result_links(html);
        }
        out
    }

    /// Last-resort extractor: pull external (non-search-engine, non-asset) `<a href="https://…">Text`
    /// links from any results HTML, deduped. Less precise than a per-engine parser but resilient to
    /// markup changes / shell variants.
    fn broad_result_links(html: &str) -> Vec<String> {
        let skip = [
            "brave.com",
            "duckduckgo",
            "bing.com",
            "google.",
            "microsoft.com",
            "gstatic",
            "w3.org",
            "schema.org",
            "/cdn",
            "javascript:",
        ];
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        for chunk in html.split("href=\"https").skip(1) {
            let Some((rest, after)) = chunk.split_once('"') else {
                continue;
            };
            let href = format!("https{rest}");
            if skip.iter().any(|s| href.contains(s)) || !seen.insert(href.clone()) {
                continue;
            }
            // Title = the anchor's visible text (tags stripped), if any.
            let title = after
                .split_once('>')
                .and_then(|(_, r)| r.split_once("</a>"))
                .map(|(t, _)| super::html_to_text(t).trim().to_string())
                .filter(|t| t.len() >= 8)
                .unwrap_or_default();
            if !title.is_empty() {
                out.push(format!("- {title} :: {href}"));
            }
            if out.len() >= 10 {
                break;
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
            let html = get_text("https://html.duckduckgo.com/html/?q=rust+programming", 15)
                .await
                .unwrap();
            assert!(
                !extract_results(&html).is_empty(),
                "expected search results"
            );
        }
    }
}

#[cfg(test)]
mod file_tools_tests {
    use super::*;
    use crate::tool::{Policy, ToolCtx};
    use engram_core::{Ledger, Taint};
    use engram_gateway::{Gateway, MockProvider};
    use engram_memory::{Memory, TrigramHashEmbedder};
    use engram_skills::{Registry, SkillSigner};
    use std::sync::Arc;

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
        }
    }

    #[tokio::test]
    async fn edit_file_replaces_unique_and_rejects_ambiguous() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_in(dir.path());
        std::fs::write(dir.path().join("f.txt"), "alpha\nbeta\nalpha\n").unwrap();
        // non-unique -> refused, file unchanged
        let e = EditFileTool
            .run(&json!({"path":"f.txt","old":"alpha","new":"X"}), &ctx)
            .await;
        assert!(e.is_err() && e.unwrap_err().contains("matched 2"));
        // absent -> refused
        assert!(EditFileTool
            .run(&json!({"path":"f.txt","old":"zzz","new":"X"}), &ctx)
            .await
            .is_err());
        // unique context -> replaced exactly once
        EditFileTool
            .run(&json!({"path":"f.txt","old":"beta","new":"BETA"}), &ctx)
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("f.txt")).unwrap(),
            "alpha\nBETA\nalpha\n"
        );
    }

    #[tokio::test]
    async fn read_file_pages_with_offset_limit() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_in(dir.path());
        let body: String = (1..=10)
            .map(|i| format!("line{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(dir.path().join("big.txt"), &body).unwrap();
        let out = ReadFileTool
            .run(&json!({"path":"big.txt","offset":3,"limit":2}), &ctx)
            .await
            .unwrap();
        assert!(out.contains("of 10"), "header reports total: {out}");
        assert!(out.contains("line4") && out.contains("line5"));
        assert!(!out.contains("line3") && !out.contains("line6"));
        assert!(out.contains("more line"), "advertises remaining lines");
    }

    #[tokio::test]
    async fn append_make_move_copy_delete_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_in(dir.path());
        MakeDirTool.run(&json!({"path":"sub"}), &ctx).await.unwrap();
        assert!(dir.path().join("sub").is_dir());
        AppendFileTool
            .run(&json!({"path":"sub/log.txt","content":"a\n"}), &ctx)
            .await
            .unwrap();
        AppendFileTool
            .run(&json!({"path":"sub/log.txt","content":"b\n"}), &ctx)
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("sub/log.txt")).unwrap(),
            "a\nb\n"
        );
        CopyFileTool
            .run(&json!({"from":"sub/log.txt","to":"copy.txt"}), &ctx)
            .await
            .unwrap();
        assert!(dir.path().join("copy.txt").exists());
        MoveFileTool
            .run(&json!({"from":"copy.txt","to":"moved.txt"}), &ctx)
            .await
            .unwrap();
        assert!(!dir.path().join("copy.txt").exists() && dir.path().join("moved.txt").exists());
        DeleteFileTool
            .run(&json!({"path":"moved.txt"}), &ctx)
            .await
            .unwrap();
        assert!(!dir.path().join("moved.txt").exists());
        // escape attempts are refused by confinement
        assert!(MoveFileTool
            .run(&json!({"from":"sub/log.txt","to":"../escape"}), &ctx)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn glob_and_grep_find_within_workdir() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_in(dir.path());
        std::fs::create_dir_all(dir.path().join("src/inner")).unwrap();
        std::fs::write(dir.path().join("src/a.rs"), "fn main() { needle(); }\n").unwrap();
        std::fs::write(
            dir.path().join("src/inner/b.rs"),
            "// other\nlet NEEDLE = 1;\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("notes.md"), "no match here\n").unwrap();
        let g = GlobTool
            .run(&json!({"pattern":"**/*.rs"}), &ctx)
            .await
            .unwrap();
        assert!(g.contains("src/a.rs") && g.contains("src/inner/b.rs") && !g.contains("notes.md"));
        let g2 = GlobTool
            .run(&json!({"pattern":"src/*.rs"}), &ctx)
            .await
            .unwrap();
        assert!(g2.contains("src/a.rs") && !g2.contains("inner/b.rs"));
        let hits = GrepTool
            .run(&json!({"query":"needle","ignore_case":true}), &ctx)
            .await
            .unwrap();
        assert!(hits.contains("src/a.rs:1") && hits.contains("src/inner/b.rs:2"));
        let none = GrepTool
            .run(
                &json!({"query":"needle","ignore_case":false,"glob":"*.md"}),
                &ctx,
            )
            .await
            .unwrap();
        assert!(none.contains("no matches"));
    }

    #[test]
    fn glob_match_semantics() {
        assert!(glob_match("**/*.rs", "src/inner/b.rs"));
        assert!(glob_match("src/*.rs", "src/a.rs"));
        assert!(!glob_match("src/*.rs", "src/inner/b.rs"));
        assert!(glob_match("*.md", "notes.md"));
        assert!(!glob_match("*.md", "src/notes.md"));
        assert!(glob_match("a?c", "abc"));
        assert!(glob_match("**", "any/deep/path.txt"));
    }
}
