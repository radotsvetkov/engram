//! Telegram inbound channel — run Engram as a bot.
//!
//! When `ENGRAM_TELEGRAM_TOKEN` is set, this long-polls Telegram's getUpdates, runs the
//! agent (full toolset, memory, persona) on each incoming message, and replies. It is
//! the messaging-gateway pattern: one transport, the same agent behind it. Other
//! platforms (Discord, Slack, …) follow the identical shape — poll/receive, run the
//! agent, send the reply.

use std::time::Duration;

use crate::App;

/// Spawn the Telegram polling loop as a background task.
pub fn spawn(app: App, token: String) {
    tokio::spawn(async move {
        let client = reqwest::Client::new();
        let base = format!("https://api.telegram.org/bot{token}");
        let mut offset: i64 = 0;
        loop {
            let url = format!("{base}/getUpdates?timeout=30&offset={offset}");
            let json: serde_json::Value = match client.get(&url).send().await.and_then(|r| r.error_for_status())
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

            let Some(updates) = json["result"].as_array() else { continue };
            for u in updates {
                if let Some(uid) = u["update_id"].as_i64() {
                    offset = uid + 1;
                }
                let (Some(text), Some(chat_id)) =
                    (u["message"]["text"].as_str(), u["message"]["chat"]["id"].as_i64())
                else {
                    continue;
                };
                // Inbound chat is untrusted: start the run tainted (no shell, no egress).
                let answer = match crate::run_agent_task_cb(&app, text, 8, engram_core::Taint::Untrusted, false, None).await {
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
    });
}
