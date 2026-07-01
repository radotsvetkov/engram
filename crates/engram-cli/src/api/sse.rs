//! Server-Sent-Events decoding and the typed events the daemon streams.
//!
//! engramd streams three things over SSE: a live chat turn
//! (`/v1/converse/stream`), a task run (`/v1/tasks/{id}/run/stream`), and the
//! global spike bus (`/v1/events`). We parse the raw `event:`/`data:` frames
//! once here and lift them into strongly typed enums the UI can match on.

use super::types::{ConverseDone, Task};
use serde_json::Value;

/// A minimal, allocation-light SSE frame decoder.
///
/// Feed it raw byte chunks as they arrive off the wire; it hands back complete
/// `(event, data)` frames. Bytes are buffered until a full line (`\n`-terminated)
/// is available before decoding — because `\n` (0x0A) never appears inside a
/// multi-byte UTF-8 sequence, a complete line always ends on a char boundary, so
/// a character split across two network chunks is never corrupted. Multiple
/// `data:` lines in one frame are joined with newlines, per the SSE spec;
/// comment lines (`:`) and unknown fields are ignored.
#[derive(Default)]
pub struct SseDecoder {
    buf: Vec<u8>,
    event: String,
    data: String,
}

impl SseDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Push a raw byte chunk, draining any completed frames into `out`.
    pub fn push(&mut self, chunk: &[u8], out: &mut Vec<(String, String)>) {
        self.buf.extend_from_slice(chunk);
        // Process whole lines; keep the trailing partial line buffered.
        while let Some(nl) = self.buf.iter().position(|&b| b == b'\n') {
            let mut raw: Vec<u8> = self.buf.drain(..=nl).collect();
            raw.pop(); // drop the '\n'
            if raw.last() == Some(&b'\r') {
                raw.pop();
            }
            // A complete line ends on a char boundary, so this is lossless.
            let line = String::from_utf8_lossy(&raw).into_owned();
            if line.is_empty() {
                // Blank line terminates a frame.
                if !self.data.is_empty() || !self.event.is_empty() {
                    let ev = if self.event.is_empty() {
                        "message".to_string()
                    } else {
                        std::mem::take(&mut self.event)
                    };
                    let data = std::mem::take(&mut self.data);
                    out.push((ev, data));
                }
                self.event.clear();
                self.data.clear();
                continue;
            }
            if let Some(rest) = line.strip_prefix("event:") {
                self.event = rest.trim_start().to_string();
            } else if let Some(rest) = line.strip_prefix("data:") {
                if !self.data.is_empty() {
                    self.data.push('\n');
                }
                self.data.push_str(rest.strip_prefix(' ').unwrap_or(rest));
            }
            // `id:`, `retry:`, comments — ignored.
        }
    }
}

/// A live chat turn, lifted from `/v1/converse/stream`.
#[derive(Debug, Clone)]
pub enum ChatEvent {
    /// A tool the agent invoked, with its observation.
    Step {
        index: usize,
        tool: String,
        ok: bool,
        observation: String,
        args: Value,
    },
    /// Interim model commentary streamed while it works.
    Narration(String),
    /// The finished turn — the answer plus grounding/learning metadata.
    Done(Box<ConverseDone>),
    /// The run failed.
    Error(String),
    /// The HTTP stream itself broke (connection dropped, decode error, …).
    Disconnected(String),
}

impl ChatEvent {
    /// Map a raw `(event, data)` SSE frame into a typed [`ChatEvent`].
    pub fn from_frame(event: &str, data: &str) -> Option<ChatEvent> {
        match event {
            "step" => {
                let v: Value = serde_json::from_str(data).ok()?;
                Some(ChatEvent::Step {
                    index: v.get("index").and_then(|x| x.as_u64()).unwrap_or(0) as usize,
                    tool: v
                        .get("tool")
                        .and_then(|x| x.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    ok: v.get("ok").and_then(|x| x.as_bool()).unwrap_or(true),
                    observation: v
                        .get("observation")
                        .and_then(|x| x.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    args: v.get("args").cloned().unwrap_or(Value::Null),
                })
            }
            "narration" => {
                let v: Value = serde_json::from_str(data).ok()?;
                let text = v
                    .get("text")
                    .and_then(|x| x.as_str())
                    .unwrap_or(data)
                    .to_string();
                Some(ChatEvent::Narration(text))
            }
            "done" => {
                let done: ConverseDone = serde_json::from_str(data).unwrap_or_default();
                Some(ChatEvent::Done(Box::new(done)))
            }
            "error" => {
                // `error` may be a bare string or `{ "error": "..." }`.
                let msg = serde_json::from_str::<Value>(data)
                    .ok()
                    .and_then(|v| {
                        v.get("error")
                            .and_then(|e| e.as_str())
                            .map(|s| s.to_string())
                    })
                    .unwrap_or_else(|| data.to_string());
                Some(ChatEvent::Error(msg))
            }
            _ => None,
        }
    }
}

/// A live task run, lifted from `/v1/tasks/{id}/run/stream`.
#[derive(Debug, Clone)]
pub enum TaskEvent {
    Step(Value),
    Done(Box<Task>),
    Error(String),
    Disconnected(String),
}

impl TaskEvent {
    pub fn from_frame(event: &str, data: &str) -> Option<TaskEvent> {
        match event {
            "step" => Some(TaskEvent::Step(
                serde_json::from_str(data).unwrap_or(Value::String(data.to_string())),
            )),
            "done" => Some(TaskEvent::Done(Box::new(
                serde_json::from_str(data).unwrap_or_default(),
            ))),
            "error" => {
                let msg = serde_json::from_str::<Value>(data)
                    .ok()
                    .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(String::from))
                    .unwrap_or_else(|| data.to_string());
                Some(TaskEvent::Error(msg))
            }
            _ => None,
        }
    }
}

/// A spike off the global bus (`/v1/events`).
#[derive(Debug, Clone)]
pub struct Spike {
    pub topic: String,
    pub payload: Value,
}

impl Spike {
    pub fn from_frame(event: &str, data: &str) -> Option<Spike> {
        if event != "spike" {
            return None;
        }
        let v: Value = serde_json::from_str(data).ok()?;
        Some(Spike {
            topic: v
                .get("topic")
                .and_then(|x| x.as_str())
                .unwrap_or_default()
                .to_string(),
            payload: v.get("payload").cloned().unwrap_or(Value::Null),
        })
    }
}
