//! Multi-platform messaging gateway - many platforms, one endpoint.
//!
//! `POST /v1/channel/{platform}` accepts a platform's webhook payload, normalizes it to
//! plain text, runs the agent, and replies in that platform's expected shape. This is
//! how "20+ platforms from one gateway" works: each platform is a tiny adapter over a
//! shared core. Webhook-style integrations (Slack, Discord interactions, Mattermost
//! outgoing webhooks, Teams, generic) reply synchronously in the HTTP response;
//! poll-based ones (Telegram) use the dedicated channel module.

use axum::extract::{Path, RawQuery, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};

use crate::{ApiError, App};

pub async fn channel_handler(
    State(app): State<App>,
    Path(platform): Path<String>,
    headers: HeaderMap,
    RawQuery(query): RawQuery,
    Json(body): Json<Value>,
) -> Response {
    // Optional shared-secret gate. When ENGRAM_CHANNEL_SECRET is set, an inbound webhook
    // must present it (X-Engram-Secret header or ?secret= query) before anything runs - so
    // a publicly-reachable channel endpoint can't be driven by strangers. (Runs are also
    // started Untrusted, so even past this gate they can't shell or exfiltrate.)
    if let Ok(secret) = std::env::var("ENGRAM_CHANNEL_SECRET") {
        if !secret.is_empty() {
            let provided = headers
                .get("x-engram-secret")
                .and_then(|h| h.to_str().ok())
                .map(str::to_string)
                .or_else(|| {
                    query
                        .as_deref()
                        .and_then(|q| q.split('&').find_map(|kv| kv.strip_prefix("secret=")))
                        .map(str::to_string)
                });
            if provided.as_deref() != Some(secret.as_str()) {
                return (StatusCode::UNAUTHORIZED, Json(json!({ "error": "bad or missing channel secret" })))
                    .into_response();
            }
        }
    }
    // Verification / keepalive handshakes that must be answered without running the agent.
    if let Some(resp) = handshake(&platform, &body) {
        return Json(resp).into_response();
    }
    let Some(text) = extract_text(&platform, &body) else {
        return Json(json!({ "ok": true, "ignored": true })).into_response();
    };
    let _ = app.ledger.append("channel.in", "user", json!({ "platform": platform }));
    // Inbound webhook content is untrusted: the run starts tainted, so it cannot run
    // shell or exfiltrate even though anyone can POST here.
    match crate::run_agent_task_cb(&app, &text, 8, engram_core::Taint::Untrusted, false, None).await {
        Ok(run) => Json(reply(&platform, &run.answer)).into_response(),
        Err(e) => ApiError(e).into_response(),
    }
}

/// Platform handshakes: Slack URL verification, Discord PING.
fn handshake(platform: &str, body: &Value) -> Option<Value> {
    match platform {
        "slack" if body["type"] == "url_verification" => Some(json!({ "challenge": body["challenge"] })),
        "discord" if body["type"] == 1 => Some(json!({ "type": 1 })),
        _ => None,
    }
}

/// Pull the user's text out of a platform's payload.
fn extract_text(platform: &str, body: &Value) -> Option<String> {
    let t = match platform {
        "slack" => body["text"].as_str().or_else(|| body["event"]["text"].as_str()),
        "discord" => body["content"]
            .as_str()
            .or_else(|| body["data"]["options"][0]["value"].as_str()),
        "telegram" => body["message"]["text"].as_str(),
        "mattermost" => body["text"].as_str(),
        // Teams, generic, and most webhook bridges send {text} or {message}.
        _ => body["text"].as_str().or_else(|| body["message"].as_str()),
    }?;
    let t = t.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}

/// Format the reply in the platform's expected shape.
fn reply(platform: &str, answer: &str) -> Value {
    match platform {
        // Discord interaction response: type 4 = channel message with source.
        "discord" => json!({ "type": 4, "data": { "content": answer } }),
        "slack" => json!({ "response_type": "in_channel", "text": answer }),
        // Mattermost, Teams, generic: {text}; include {content} too for compatibility.
        _ => json!({ "text": answer, "content": answer }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slack_url_verification_is_echoed() {
        let b = json!({ "type": "url_verification", "challenge": "abc123" });
        assert_eq!(handshake("slack", &b).unwrap()["challenge"], "abc123");
    }

    #[test]
    fn discord_ping_pongs() {
        assert_eq!(handshake("discord", &json!({ "type": 1 })).unwrap()["type"], 1);
    }

    #[test]
    fn extracts_text_per_platform() {
        assert_eq!(extract_text("slack", &json!({ "event": { "text": "hi slack" } })).as_deref(), Some("hi slack"));
        assert_eq!(extract_text("slack", &json!({ "text": "slash cmd" })).as_deref(), Some("slash cmd"));
        assert_eq!(
            extract_text("discord", &json!({ "data": { "options": [{ "value": "hi discord" }] } })).as_deref(),
            Some("hi discord")
        );
        assert_eq!(extract_text("telegram", &json!({ "message": { "text": "hi tg" } })).as_deref(), Some("hi tg"));
        assert_eq!(extract_text("mattermost", &json!({ "text": "hi mm" })).as_deref(), Some("hi mm"));
        assert_eq!(extract_text("whatsapp", &json!({ "message": "hi generic" })).as_deref(), Some("hi generic"));
        assert!(extract_text("slack", &json!({ "event": { "text": "   " } })).is_none());
    }

    #[test]
    fn reply_matches_platform_shape() {
        assert_eq!(reply("discord", "x")["type"], 4);
        assert_eq!(reply("discord", "x")["data"]["content"], "x");
        assert_eq!(reply("slack", "x")["text"], "x");
        assert_eq!(reply("teams", "x")["text"], "x");
    }
}
