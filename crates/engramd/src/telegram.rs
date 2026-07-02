//! Telegram inbound channel - run Engram as a bot.
//!
//! When `ENGRAM_TELEGRAM_TOKEN` is set, this long-polls Telegram's getUpdates, runs the
//! agent (full toolset, memory, persona) on each incoming message, and replies. It is
//! the messaging-gateway pattern: one transport, the same agent behind it. Other
//! platforms (Discord, Slack, …) follow the identical shape - poll/receive, run the
//! agent, send the reply.

use std::time::Duration;

use crate::App;

/// The bot's identity from getMe - proof the token is valid and which bot it names.
pub struct Identity {
    pub username: String,
    pub name: String,
}

/// Validate a bot token against Telegram's getMe. Returns the bot identity, or a human-readable
/// reason it failed (bad token, or Telegram unreachable - e.g. offline). This is the live check
/// the desktop's Connect flow runs before it claims a connection, so the UI never bluffs.
pub async fn validate(token: &str) -> Result<Identity, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())?;
    let url = format!("https://api.telegram.org/bot{token}/getMe");
    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|_| "couldn't reach Telegram - are you offline?".to_string())?;
    let ok_status = resp.status().is_success();
    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|_| "unexpected response from Telegram".to_string())?;
    if !ok_status || !json["ok"].as_bool().unwrap_or(false) {
        return Err(json["description"]
            .as_str()
            .unwrap_or("invalid bot token")
            .to_string());
    }
    let r = &json["result"];
    Ok(Identity {
        username: r["username"].as_str().unwrap_or("").to_string(),
        name: r["first_name"].as_str().unwrap_or("bot").to_string(),
    })
}

/// The configured owner chat id, if any. Telegram replies go back to whoever messaged the bot, and
/// the run answer can carry the user's private memory — so a bot whose @username leaks becomes an
/// exfiltration surface for any stranger. When an owner chat id is set, only that chat is served;
/// all other chat ids are ignored (fail-closed). When unset, the bot answers anyone (legacy
/// behavior) — but the run is still started Untrusted and the memory tools are stripped for
/// untrusted-origin runs (see channel exfil hardening), so private memory is not returned.
///
/// The configured owner chat id: `channels.telegram_owner_chat_id` (0 = unset) wins, then the
/// `ENGRAM_TELEGRAM_OWNER` env var as a headless fallback. `None` = legacy any-sender behavior.
fn owner_chat_id(app: &App) -> Option<i64> {
    let cfg_id = app.cfg().channels.telegram_owner_chat_id;
    if cfg_id != 0 {
        return Some(cfg_id);
    }
    std::env::var("ENGRAM_TELEGRAM_OWNER")
        .ok()
        .and_then(|s| s.trim().parse::<i64>().ok())
        .filter(|&id| id != 0)
}

/// Spawn the Telegram polling loop as a background task. Returns an [`AbortHandle`] so the
/// desktop's Disconnect can stop it live, without a restart.
pub fn spawn(app: App, token: String) -> tokio::task::AbortHandle {
    tokio::spawn(async move {
        let client = reqwest::Client::new();
        let base = format!("https://api.telegram.org/bot{token}");
        let owner = owner_chat_id(&app);
        let mut offset: i64 = 0;
        loop {
            let url = format!("{base}/getUpdates?timeout=30&offset={offset}");
            let json: serde_json::Value = match client
                .get(&url)
                .send()
                .await
                .and_then(|r| r.error_for_status())
            {
                Ok(r) => match r.json().await {
                    Ok(j) => j,
                    Err(_) => continue,
                },
                Err(e) => {
                    tracing::warn!(error = %e, "telegram getUpdates failed");
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    continue;
                }
            };

            let Some(updates) = json["result"].as_array() else {
                continue;
            };
            for u in updates {
                if let Some(uid) = u["update_id"].as_i64() {
                    offset = uid + 1;
                }
                let (Some(text), Some(chat_id)) = (
                    u["message"]["text"].as_str(),
                    u["message"]["chat"]["id"].as_i64(),
                ) else {
                    continue;
                };
                // Owner allowlist: if an owner chat id is configured, silently ignore every other
                // chat. Telegram replies are an egress surface (the answer goes back to the sender),
                // so an unlisted stranger must never even trigger a run.
                if let Some(owner_id) = owner {
                    if chat_id != owner_id {
                        tracing::warn!(chat_id, "telegram message from non-owner chat ignored");
                        continue;
                    }
                }
                // Inbound chat is untrusted: start the run tainted (no shell, no egress).
                let answer = match crate::run_agent_task_cb(
                    &app,
                    text,
                    8,
                    engram_core::Taint::Untrusted,
                    false,
                    None,
                    None,
                    None,
                    None,
                    false, // approved: inbound message never auto-approves its own egress
                    false, // attended: unattended inbound surface
                    app.halt.clone(),
                    engram_core::ScopeCtx::user_only(), // inbound has no project → user-global only
                )
                .await
                {
                    Ok(run) => run.answer,
                    Err(e) => format!("error: {e}"),
                };
                let reply: String = answer.chars().take(4000).collect();
                let _ = client
                    .post(format!("{base}/sendMessage"))
                    .json(&serde_json::json!({ "chat_id": chat_id, "text": reply }))
                    .send()
                    .await;
            }
        }
    })
    .abort_handle()
}
