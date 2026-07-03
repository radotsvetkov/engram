//! Interactive browsing via the Chrome DevTools Protocol.
//!
//! A persistent headless-Chrome session the agent drives across tool calls: navigate,
//! click, type, extract, screenshot. It launches Chrome with a debugging port, attaches
//! to a page target over a websocket, and issues CDP commands - using `Runtime.evaluate`
//! for clicks/typing/extraction and `Page.captureScreenshot` for images. Deliberately
//! minimal (no heavyweight CDP crate); compiled only with `--features browser-cdp`.

use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::net::TcpStream;
use tokio::process::Child;
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};

use crate::tool::BrowserSession;

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

struct Conn {
    _child: Child,
    ws: Ws,
    next_id: u64,
}

/// A lazily-launched, single-page Chrome session.
pub struct CdpBrowser {
    chrome: String,
    port: u16,
    conn: Mutex<Option<Conn>>,
}

impl CdpBrowser {
    pub fn new(chrome: String, port: u16) -> Self {
        Self {
            chrome,
            port,
            conn: Mutex::new(None),
        }
    }

    async fn ensure<'a>(&self, guard: &'a mut Option<Conn>) -> Result<&'a mut Conn, String> {
        if guard.is_none() {
            // A DEDICATED profile dir is essential: without --user-data-dir, Chrome uses the default
            // profile, so if the user already has Chrome open the debug-port instance can't start
            // (the profile is locked) and the /json endpoint never appears → "opened browser" fails.
            // A unique per-process dir makes our CDP Chrome an independent instance every time.
            let udd = std::env::temp_dir().join(format!("engram-cdp-{}", std::process::id()));
            let _ = std::fs::create_dir_all(&udd);
            let child = tokio::process::Command::new(&self.chrome)
                .args([
                    "--headless=new",
                    "--disable-gpu",
                    "--no-sandbox",
                    "--no-first-run",
                    "--no-default-browser-check",
                    "--disable-extensions",
                    // Remove the most blatant automation tell at the source: this drops the
                    // `navigator.webdriver` flag Cloudflare/DataDome key on. The init script below is
                    // belt-and-braces for the other headless fingerprints.
                    "--disable-blink-features=AutomationControlled",
                ])
                .arg(format!("--user-data-dir={}", udd.display()))
                .arg(format!("--remote-debugging-port={}", self.port))
                .arg("about:blank")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .kill_on_drop(true)
                .spawn()
                .map_err(|e| format!("launch chrome: {e}"))?;
            let ws_url = self.discover_ws().await?;
            let (ws, _) = connect_async(ws_url.as_str())
                .await
                .map_err(|e| format!("cdp connect: {e}"))?;
            *guard = Some(Conn {
                _child: child,
                ws,
                next_id: 1,
            });
            // Apply anti-detection ONCE at session creation so every later navigation inherits it.
            Self::apply_stealth(guard.as_mut().unwrap()).await;
            // Arm the SSRF guard for the whole session (see `arm_ssrf_guard`). Enabling `Fetch`
            // request interception at session creation means every later navigation — including
            // HTTP 3xx redirects, meta-refresh, JS `location=`, and 0-TTL DNS rebinds — is
            // re-checked at the network layer, so the persistent CDP Chrome can't be steered to
            // 127.0.0.1:8088 (our own unauthenticated API) or 169.254.169.254 (cloud metadata).
            Self::arm_ssrf_guard(guard.as_mut().unwrap()).await;
        }
        Ok(guard.as_mut().unwrap())
    }

    /// Enable `Fetch` request interception for the session. Unlike the one-shot `chrome_dump_dom`
    /// helper (which pins the single target host with `--host-resolver-rules` at launch), the
    /// persistent session serves many hosts over its lifetime, so we cannot pin at launch. Instead
    /// we intercept EVERY request and re-run the SSRF address check on its host — the same check the
    /// one-shot pin enforces, but applied continuously. The `cmd` receive loop handles the
    /// `Fetch.requestPaused` events this produces (via `guard_paused_request`); here we just turn it on.
    async fn arm_ssrf_guard(conn: &mut Conn) {
        // `Request` stage only (no response interception) keeps overhead minimal: we just need the
        // URL to vet before the connection is made. An empty pattern set means "all requests".
        let _ = Self::cmd(
            conn,
            "Fetch.enable",
            json!({ "patterns": [ { "urlPattern": "*" } ] }),
        )
        .await;
    }

    /// Minimal anti-detection so the headless session isn't instantly flagged by Cloudflare/DataDome
    /// on the JS-heavy, bot-walled sites real data lives on (flights, prices, listings). Without this,
    /// the session ships a `HeadlessChrome` User-Agent and `navigator.webdriver===true`, so it reads a
    /// bot-wall/consent page and "answers" from it — the core reason web tasks silently fail.
    ///
    /// Best-effort by design: every command is fire-and-forget so a quirk degrades to the prior
    /// (fingerprintable) behaviour rather than breaking navigation. Three cheap, high-impact masks:
    ///   1. a real Chrome User-Agent (the default headless UA contains "HeadlessChrome"),
    ///   2. an on-new-document script hiding `navigator.webdriver` and the other headless tells,
    ///   3. a stable locale + timezone so the environment doesn't look blank/inconsistent.
    ///
    /// This is the in-tree minimum-viable stealth; a Camoufox-class sidecar is the heavier follow-up.
    async fn apply_stealth(conn: &mut Conn) {
        const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
            (KHTML, like Gecko) Chrome/138.0.0.0 Safari/537.36";
        // Runs before any page script on every new document, so the page never observes the tells.
        const STEALTH_JS: &str = "\
            Object.defineProperty(navigator,'webdriver',{get:()=>undefined});\
            Object.defineProperty(navigator,'languages',{get:()=>['en-US','en']});\
            Object.defineProperty(navigator,'plugins',{get:()=>[1,2,3,4,5]});\
            window.chrome=window.chrome||{runtime:{}};\
            const q=navigator.permissions&&navigator.permissions.query;\
            if(q){navigator.permissions.query=p=>p&&p.name==='notifications'?\
            Promise.resolve({state:Notification.permission}):q(p);}";
        // Page must be enabled for addScriptToEvaluateOnNewDocument; idempotent with open()'s enable.
        let _ = Self::cmd(conn, "Page.enable", json!({})).await;
        let _ = Self::cmd(
            conn,
            "Page.addScriptToEvaluateOnNewDocument",
            json!({ "source": STEALTH_JS }),
        )
        .await;
        let _ = Self::cmd(
            conn,
            "Emulation.setUserAgentOverride",
            json!({ "userAgent": UA, "acceptLanguage": "en-US,en;q=0.9", "platform": "Win32" }),
        )
        .await;
        let _ = Self::cmd(
            conn,
            "Emulation.setTimezoneOverride",
            json!({ "timezoneId": "Europe/Berlin" }),
        )
        .await;
    }

    async fn discover_ws(&self) -> Result<String, String> {
        let client = reqwest::Client::new();
        for _ in 0..50 {
            if let Ok(resp) = client
                .get(format!("http://127.0.0.1:{}/json", self.port))
                .send()
                .await
            {
                if let Ok(list) = resp.json::<Value>().await {
                    if let Some(arr) = list.as_array() {
                        if let Some(t) = arr
                            .iter()
                            .find(|t| t["type"] == "page" && t["webSocketDebuggerUrl"].is_string())
                        {
                            return Ok(t["webSocketDebuggerUrl"].as_str().unwrap().to_string());
                        }
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        Err("could not find a Chrome page target".into())
    }

    async fn cmd(conn: &mut Conn, method: &str, params: Value) -> Result<Value, String> {
        let id = conn.next_id;
        conn.next_id += 1;
        let payload = json!({ "id": id, "method": method, "params": params }).to_string();
        conn.ws
            .send(Message::Text(payload))
            .await
            .map_err(|e| e.to_string())?;
        // Bound the wait: a missing/never-arriving response must not hang the tool forever
        // while holding the browser session lock.
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return Err("cdp command timed out".into());
            }
            let next = match tokio::time::timeout(remaining, conn.ws.next()).await {
                Ok(n) => n,
                Err(_) => return Err("cdp command timed out".into()),
            };
            let msg = next
                .ok_or("cdp connection closed")?
                .map_err(|e| e.to_string())?;
            let txt = match msg.to_text() {
                Ok(t) => t,
                Err(_) => continue,
            };
            let v: Value = match serde_json::from_str(txt) {
                Ok(v) => v,
                Err(_) => continue,
            };
            // A paused request (Fetch domain is enabled for the whole session) MUST be answered or
            // Chrome stalls the navigation waiting for a decision. Vet its host through the SSRF
            // guard and continue or fail it — this is what closes the redirect/rebind hole, since
            // it runs on the ACTUAL request Chrome is about to make, not just the URL we asked for.
            if v.get("method").and_then(|m| m.as_str()) == Some("Fetch.requestPaused") {
                Self::guard_paused_request(conn, &v).await;
                continue;
            }
            if v.get("id").and_then(|x| x.as_u64()) == Some(id) {
                if let Some(err) = v.get("error") {
                    return Err(err.to_string());
                }
                return Ok(v.get("result").cloned().unwrap_or(Value::Null));
            }
        }
    }

    /// Answer a `Fetch.requestPaused` event: allow the request only if its URL passes the SSRF
    /// guard (public http(s) host that doesn't resolve to loopback/private/link-local/metadata),
    /// otherwise fail it. Fire-and-forget the continue/fail command (we don't await its response so
    /// we don't reenter `cmd` recursively); the guard's async DNS check is the only await.
    async fn guard_paused_request(conn: &mut Conn, event: &Value) {
        let rid = event["params"]["requestId"].as_str().unwrap_or("");
        if rid.is_empty() {
            return;
        }
        let url = event["params"]["request"]["url"].as_str().unwrap_or("");
        // Only http(s) requests hit the network (and thus could reach a private/metadata IP); pass
        // through non-network schemes (data:/blob:/about:/chrome-extension:) untouched so ordinary
        // in-page resources aren't broken. http(s) requests are re-checked through the SSRF guard.
        let is_http = url.starts_with("http://") || url.starts_with("https://");
        let allowed = !is_http || crate::tools::guard_url(url).await.is_ok();
        let (cmd_method, params) = if allowed {
            ("Fetch.continueRequest", json!({ "requestId": rid }))
        } else {
            (
                "Fetch.failRequest",
                json!({ "requestId": rid, "errorReason": "AddressUnreachable" }),
            )
        };
        let id = conn.next_id;
        conn.next_id += 1;
        let payload = json!({ "id": id, "method": cmd_method, "params": params }).to_string();
        let _ = conn.ws.send(Message::Text(payload)).await;
    }

    async fn eval(conn: &mut Conn, expr: &str) -> Result<Value, String> {
        let r = Self::cmd(
            conn,
            "Runtime.evaluate",
            json!({ "expression": expr, "returnByValue": true, "awaitPromise": true }),
        )
        .await?;
        if let Some(exc) = r.get("exceptionDetails") {
            return Err(format!("js error: {exc}"));
        }
        Ok(r["result"]["value"].clone())
    }
}

fn js_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

#[async_trait]
impl BrowserSession for CdpBrowser {
    async fn open(&self, url: &str) -> Result<String, String> {
        let mut guard = self.conn.lock().await;
        let conn = self.ensure(&mut guard).await?;
        Self::cmd(conn, "Page.enable", json!({})).await?;
        Self::cmd(conn, "Page.navigate", json!({ "url": url })).await?;
        // Wait until we're on the navigated page (not the initial about:blank) AND it has
        // finished loading - otherwise we'd read the blank page that was complete instantly.
        for _ in 0..80 {
            let ready = Self::eval(
                conn,
                "document.readyState==='complete' && location.href!=='about:blank'",
            )
            .await
            .ok()
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
            if ready {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        // Defence in depth: even with per-request Fetch guarding, re-check where we actually ENDED
        // up before returning any page text. A redirect/JS-navigation chain that landed on a
        // non-public origin (loopback API, cloud metadata) must never have its body handed back to
        // the model. `about:blank` is the benign initial page, so only a real http(s) location is
        // vetted; anything else (or a blocked host) yields no text.
        let href = Self::eval(conn, "location.href")
            .await
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default();
        if href.starts_with("http://") || href.starts_with("https://") {
            crate::tools::guard_url(&href).await.map_err(|e| {
                format!("refusing to read page: navigation landed on a non-public address ({e})")
            })?;
        }
        let text = Self::eval(conn, "document.body ? document.body.innerText : ''").await?;
        Ok(text.as_str().unwrap_or("").chars().take(6000).collect())
    }

    async fn wait_for(&self, selector: &str, timeout_ms: u64) -> Result<String, String> {
        let deadline = Duration::from_millis(timeout_ms.clamp(100, 30_000));
        let start = std::time::Instant::now();
        let expr = format!("!!document.querySelector({})", js_str(selector));
        loop {
            // Acquire the session lock for ONE poll only, then RELEASE it across the sleep - so a
            // long wait_for can't monopolize the shared browser for up to 30s and block every
            // other browser tool call (and the subagents sharing the session) in the meantime.
            let present = {
                let mut guard = self.conn.lock().await;
                let conn = self.ensure(&mut guard).await?;
                Self::eval(conn, &expr)
                    .await
                    .ok()
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
            };
            if present {
                return Ok("present".into());
            }
            if start.elapsed() >= deadline {
                return Err(format!(
                    "selector {selector} not found within {}ms",
                    deadline.as_millis()
                ));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    async fn scroll(&self, dy: i64) -> Result<String, String> {
        let mut guard = self.conn.lock().await;
        let conn = self.ensure(&mut guard).await?;
        Self::eval(
            conn,
            &format!("window.scrollBy(0,{dy}); String(window.scrollY)"),
        )
        .await?;
        Ok(format!("scrolled {dy}px"))
    }

    async fn click(&self, selector: &str) -> Result<String, String> {
        let mut guard = self.conn.lock().await;
        let conn = self.ensure(&mut guard).await?;
        // Auto-wait briefly so a click on a not-yet-rendered element doesn't spuriously fail.
        let expr = format!(
            "(async()=>{{for(let i=0;i<30;i++){{const e=document.querySelector({s}); \
             if(e){{ e.scrollIntoView({{block:'center'}}); e.click(); return 'clicked'; }} \
             await new Promise(r=>setTimeout(r,100));}} return 'not found';}})()",
            s = js_str(selector)
        );
        Ok(Self::eval(conn, &expr)
            .await?
            .as_str()
            .unwrap_or("ok")
            .to_string())
    }

    async fn type_text(&self, selector: &str, text: &str) -> Result<String, String> {
        let mut guard = self.conn.lock().await;
        let conn = self.ensure(&mut guard).await?;
        // Set the value through React's native setter and fire input+change+keyup so controlled
        // SPA inputs actually update (a bare `.value=` is ignored by React). Auto-wait for the field.
        let expr = format!(
            "(async()=>{{let e=null; for(let i=0;i<30;i++){{e=document.querySelector({s}); if(e) break; \
             await new Promise(r=>setTimeout(r,100));}} if(!e) return 'not found'; e.focus(); \
             const proto=e instanceof HTMLTextAreaElement?HTMLTextAreaElement.prototype:HTMLInputElement.prototype; \
             const set=Object.getOwnPropertyDescriptor(proto,'value'); if(set&&set.set){{set.set.call(e,{t});}}else{{e.value={t};}} \
             e.dispatchEvent(new Event('input',{{bubbles:true}})); e.dispatchEvent(new Event('change',{{bubbles:true}})); \
             e.dispatchEvent(new KeyboardEvent('keyup',{{bubbles:true}})); return 'typed';}})()",
            s = js_str(selector),
            t = js_str(text)
        );
        Ok(Self::eval(conn, &expr)
            .await?
            .as_str()
            .unwrap_or("ok")
            .to_string())
    }

    async fn extract(&self, selector: Option<&str>) -> Result<String, String> {
        let mut guard = self.conn.lock().await;
        let conn = self.ensure(&mut guard).await?;
        let expr = match selector {
            Some(s) => format!(
                "(()=>{{const e=document.querySelector({}); return e? e.innerText : 'not found';}})()",
                js_str(s)
            ),
            None => "document.body ? document.body.innerText : ''".to_string(),
        };
        Ok(Self::eval(conn, &expr)
            .await?
            .as_str()
            .unwrap_or("")
            .chars()
            .take(6000)
            .collect())
    }

    async fn screenshot(&self, path: &std::path::Path) -> Result<(), String> {
        let mut guard = self.conn.lock().await;
        let conn = self.ensure(&mut guard).await?;
        let r = Self::cmd(conn, "Page.captureScreenshot", json!({ "format": "png" })).await?;
        let data = r["data"].as_str().ok_or("no screenshot data")?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(data)
            .map_err(|e| e.to_string())?;
        tokio::fs::write(path, bytes)
            .await
            .map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "needs Chrome"]
    async fn navigates_types_clicks_and_extracts() {
        // A deterministic local page: typing into #in then clicking #btn copies the
        // value into #out - proving navigate + type + click + extract end to end.
        let dir = tempfile::tempdir().unwrap();
        let page = dir.path().join("t.html");
        std::fs::write(
            &page,
            "<html><body><input id='in'><button id='btn' onclick=\"document.getElementById('out').innerText=document.getElementById('in').value\">go</button><div id='out'>empty</div></body></html>",
        )
        .unwrap();
        let url = format!("file://{}", page.display());

        let chrome = crate::tools::find_chrome().expect("chrome");
        let b = CdpBrowser::new(chrome, 9338);
        let text = b.open(&url).await.unwrap();
        assert!(text.contains("empty"), "open got: {text:?}");

        // wait_for finds an existing selector immediately; a missing one times out (fast).
        b.wait_for("#in", 2000)
            .await
            .expect("wait_for an existing selector");
        assert!(
            b.wait_for("#nope", 300).await.is_err(),
            "missing selector must time out"
        );
        b.scroll(120).await.expect("scroll");

        b.type_text("#in", "engram-rocks").await.unwrap();
        b.click("#btn").await.unwrap();
        b.wait_for("#out", 2000).await.unwrap();
        let out = b.extract(Some("#out")).await.unwrap();
        assert_eq!(
            out.trim(),
            "engram-rocks",
            "click+type should populate #out"
        );
    }
}
