//! The Settings view — a schema-driven, editable browser over the daemon's
//! config. Each row reads its current value from the live (redacted) config and
//! writes back a `{section:{field:value}}` patch. Text/secret/number fields open
//! the modal editor; booleans toggle; enums cycle. `t` tests the provider.

use crate::tui::app::App;
use crate::ui::format::one_line;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use serde_json::Value;

#[derive(Clone, Copy)]
pub enum Kind {
    Text,
    Secret,
    Bool,
    Number,
    Enum(&'static [&'static str]),
}

pub struct Row {
    pub section: &'static str,
    pub field: &'static str,
    pub label: &'static str,
    pub kind: Kind,
    /// Field name in the GET config to read the current value (differs from the
    /// patch `field` for secrets, which surface as a `*_set` bool).
    pub get: &'static str,
}

const PROVIDERS: &[&str] = &[
    "anthropic",
    "openai",
    "openrouter",
    "groq",
    "deepseek",
    "mistral",
    "together",
    "xai",
    "perplexity",
    "gemini",
    "ollama",
    "lmstudio",
    "vllm",
    "llamacpp",
    "mock",
];
const EFFORT: &[&str] = &["", "low", "medium", "high"];
const EMBED: &[&str] = &["trigram", "static", "gateway"];
// "" = host (no isolation), "sandbox" = built-in OS sandbox, then docker/ssh. Must include
// "sandbox" or cycling from a sandboxed config silently jumps to "docker" (removing OS sandboxing).
const SHELL: &[&str] = &["", "sandbox", "docker", "ssh"];

pub const ROWS: &[Row] = &[
    Row {
        section: "provider",
        field: "kind",
        label: "Provider",
        kind: Kind::Enum(PROVIDERS),
        get: "kind",
    },
    Row {
        section: "provider",
        field: "model",
        label: "Model",
        kind: Kind::Text,
        get: "model",
    },
    Row {
        section: "provider",
        field: "base_url",
        label: "Base URL",
        kind: Kind::Text,
        get: "base_url",
    },
    Row {
        section: "provider",
        field: "effort",
        label: "Reasoning effort",
        kind: Kind::Enum(EFFORT),
        get: "effort",
    },
    Row {
        section: "provider",
        field: "api_key",
        label: "API key",
        kind: Kind::Secret,
        get: "api_key_set",
    },
    Row {
        section: "embed",
        field: "kind",
        label: "Embedder",
        kind: Kind::Enum(EMBED),
        get: "kind",
    },
    Row {
        section: "embed",
        field: "model_dir",
        label: "Embedder model dir",
        kind: Kind::Text,
        get: "model_dir",
    },
    Row {
        section: "security",
        field: "allow_shell",
        label: "Allow shell",
        kind: Kind::Bool,
        get: "allow_shell",
    },
    Row {
        section: "security",
        field: "shell_backend",
        label: "Shell backend",
        kind: Kind::Enum(SHELL),
        get: "shell_backend",
    },
    Row {
        section: "security",
        field: "shell_target",
        label: "Shell target",
        kind: Kind::Text,
        get: "shell_target",
    },
    Row {
        section: "security",
        field: "enable_worktree_isolation",
        label: "Worktree isolation",
        kind: Kind::Bool,
        get: "enable_worktree_isolation",
    },
    Row {
        section: "security",
        field: "auto_distill_skills",
        label: "Auto-distill skills",
        kind: Kind::Bool,
        get: "auto_distill_skills",
    },
    Row {
        section: "security",
        field: "disable_skill_author",
        label: "Disable skill authoring",
        kind: Kind::Bool,
        get: "disable_skill_author",
    },
    Row {
        section: "security",
        field: "api_token",
        label: "API token",
        kind: Kind::Secret,
        get: "api_token_set",
    },
    Row {
        section: "security",
        field: "channel_secret",
        label: "Channel secret",
        kind: Kind::Secret,
        get: "channel_secret_set",
    },
    Row {
        section: "cost",
        field: "task_token_budget",
        label: "Task token budget",
        kind: Kind::Number,
        get: "task_token_budget",
    },
    Row {
        section: "web",
        field: "tavily_api_key",
        label: "Tavily key",
        kind: Kind::Secret,
        get: "tavily_key_set",
    },
    Row {
        section: "web",
        field: "brave_api_key",
        label: "Brave key",
        kind: Kind::Secret,
        get: "brave_key_set",
    },
    Row {
        section: "web",
        field: "searxng_url",
        label: "SearXNG URL",
        kind: Kind::Text,
        get: "searxng_url",
    },
    Row {
        section: "web",
        field: "travelpayouts_token",
        label: "Travelpayouts token",
        kind: Kind::Secret,
        get: "travelpayouts_set",
    },
    Row {
        section: "media",
        field: "vision_model",
        label: "Vision model",
        kind: Kind::Text,
        get: "vision_model",
    },
    Row {
        section: "media",
        field: "image_model",
        label: "Image model",
        kind: Kind::Text,
        get: "image_model",
    },
    Row {
        section: "media",
        field: "tts_model",
        label: "Text-to-speech model",
        kind: Kind::Text,
        get: "tts_model",
    },
    Row {
        section: "media",
        field: "stt_model",
        label: "Speech-to-text model",
        kind: Kind::Text,
        get: "stt_model",
    },
    Row {
        section: "browser",
        field: "chrome_path",
        label: "Chrome path",
        kind: Kind::Text,
        get: "chrome_path",
    },
    Row {
        section: "browser",
        field: "cdp_port",
        label: "CDP port",
        kind: Kind::Number,
        get: "cdp_port",
    },
    Row {
        section: "channels",
        field: "telegram_token",
        label: "Telegram token",
        kind: Kind::Secret,
        get: "telegram_set",
    },
    Row {
        section: "channels",
        field: "webhook_url",
        label: "Webhook URL",
        kind: Kind::Secret,
        get: "webhook_url_set",
    },
];

fn section_title(s: &str) -> &'static str {
    match s {
        "provider" => "Model provider",
        "embed" => "Embedding",
        "security" => "Security",
        "cost" => "Budget",
        "web" => "Web search",
        "media" => "Media models",
        "browser" => "Browser",
        "channels" => "Channels",
        _ => "Settings",
    }
}

/// Read a row's current value as a (display, value_for_edit) pair.
pub fn current(cfg: &Value, row: &Row) -> (String, String) {
    let v = cfg.get(row.section).and_then(|s| s.get(row.get));
    match row.kind {
        Kind::Bool => {
            let b = v.and_then(|x| x.as_bool()).unwrap_or(false);
            (b.to_string(), b.to_string())
        }
        Kind::Secret => {
            let set = v.and_then(|x| x.as_bool()).unwrap_or(false);
            (
                if set { "set".into() } else { "unset".into() },
                String::new(),
            )
        }
        Kind::Number => {
            let n = v.and_then(|x| x.as_u64()).unwrap_or(0);
            (n.to_string(), n.to_string())
        }
        _ => {
            let s = v.and_then(|x| x.as_str()).unwrap_or("").to_string();
            (s.clone(), s)
        }
    }
}

/// Number of MCP servers currently configured.
pub fn mcp_count(app: &App) -> usize {
    app.config_raw
        .as_ref()
        .and_then(|c| c.get("mcp"))
        .and_then(|m| m.as_array())
        .map(|a| a.len())
        .unwrap_or(0)
}

/// Total selectable items: config rows + MCP servers + the "add server" row +
/// the agent tools (each toggleable via `security.disabled_tools`).
pub fn total_items(app: &App) -> usize {
    ROWS.len() + mcp_count(app) + 1 + app.tools.len()
}

pub fn render(app: &mut App, f: &mut Frame, area: Rect) {
    let t = app.theme;
    let total = total_items(app);
    app.clamp_sel(total);
    app.list_len = total; // wheel scrolling spans config + MCP rows
    let block = super::panel(&t, "Settings", ROWS.len());
    let inner = block.inner(area);
    f.render_widget(block, area);

    let Some(cfg) = &app.config_raw else {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "  loading config…",
                Style::default().fg(t.muted),
            ))),
            inner,
        );
        return;
    };

    let mut lines: Vec<Line> = Vec::new();
    let mut sel_line = 0usize;
    let mut last_section = "";
    let label_w = 24usize;
    for (i, row) in ROWS.iter().enumerate() {
        if row.section != last_section {
            if !lines.is_empty() {
                lines.push(Line::default());
            }
            lines.push(Line::from(Span::styled(
                format!("  {}", section_title(row.section)),
                Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
            )));
            last_section = row.section;
        }
        let selected = i == app.sel;
        if selected {
            sel_line = lines.len();
        }
        let (display, _) = current(cfg, row);
        let bar = if selected { "▌ " } else { "  " };
        let val_style = match row.kind {
            Kind::Bool => {
                if display == "true" {
                    Style::default().fg(t.good)
                } else {
                    Style::default().fg(t.faint)
                }
            }
            Kind::Secret => {
                if display == "set" {
                    Style::default().fg(t.good)
                } else {
                    Style::default().fg(t.faint)
                }
            }
            Kind::Enum(_) => Style::default().fg(t.accent2),
            _ => Style::default().fg(t.fg),
        };
        let shown = match row.kind {
            Kind::Bool => {
                if display == "true" {
                    "● on".to_string()
                } else {
                    "○ off".to_string()
                }
            }
            Kind::Secret => {
                if display == "set" {
                    "● set".to_string()
                } else {
                    "○ unset".to_string()
                }
            }
            _ => {
                let d = one_line(&display);
                if d.is_empty() {
                    "—".to_string()
                } else {
                    crate::ui::format::ellipsize(
                        &d,
                        inner.width.saturating_sub(label_w as u16 + 6) as usize,
                    )
                }
            }
        };
        let mut line = Line::from(vec![
            Span::styled(bar, Style::default().fg(t.accent)),
            Span::styled(
                format!("{:<label_w$}", row.label),
                Style::default().fg(t.fg).add_modifier(if selected {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
            ),
            Span::styled(shown, val_style),
        ]);
        if selected {
            line.style = Style::default().bg(t.sel_bg);
            super::fill_row(&mut line, inner.width as usize);
        }
        lines.push(line);
    }

    // ---- MCP servers ----
    let mcp: Vec<Value> = cfg
        .get("mcp")
        .and_then(|m| m.as_array())
        .cloned()
        .unwrap_or_default();
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        format!("  MCP servers ({})", mcp.len()),
        Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
    )));
    let val_w = inner.width.saturating_sub(label_w as u16 + 6) as usize;
    for (j, srv) in mcp.iter().enumerate() {
        let selected = ROWS.len() + j == app.sel;
        if selected {
            sel_line = lines.len();
        }
        let name = srv.get("name").and_then(|v| v.as_str()).unwrap_or("?");
        let cmd = srv.get("command").and_then(|v| v.as_str()).unwrap_or("");
        let args = srv
            .get("args")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str())
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_default();
        let bar = if selected { "▌ " } else { "  " };
        let mut line = Line::from(vec![
            Span::styled(bar, Style::default().fg(t.accent)),
            Span::styled(
                format!("{:<label_w$}", crate::ui::format::ellipsize(name, label_w)),
                Style::default().fg(t.fg).add_modifier(if selected {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
            ),
            Span::styled(
                crate::ui::format::ellipsize(format!("{cmd} {args}").trim(), val_w),
                Style::default().fg(t.accent2),
            ),
        ]);
        if selected {
            line.style = Style::default().bg(t.sel_bg);
            super::fill_row(&mut line, inner.width as usize);
        }
        lines.push(line);
    }
    {
        let selected = ROWS.len() + mcp.len() == app.sel;
        if selected {
            sel_line = lines.len();
        }
        let bar = if selected { "▌ " } else { "  " };
        let mut line = Line::from(vec![
            Span::styled(bar, Style::default().fg(t.accent)),
            Span::styled(
                "+ add MCP server",
                Style::default().fg(t.good).add_modifier(if selected {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
            ),
        ]);
        if selected {
            line.style = Style::default().bg(t.sel_bg);
            super::fill_row(&mut line, inner.width as usize);
        }
        lines.push(line);
    }

    // ---- Agent tools (enable/disable via security.disabled_tools) ----
    let enabled_tools = app.tools.iter().filter(|x| !x.disabled).count();
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        format!("  Agent tools ({enabled_tools}/{} on)", app.tools.len()),
        Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
    )));
    let tool_base = ROWS.len() + mcp.len() + 1;
    for (j, tool) in app.tools.iter().enumerate() {
        let selected = tool_base + j == app.sel;
        if selected {
            sel_line = lines.len();
        }
        let bar = if selected { "▌ " } else { "  " };
        let dot = if tool.disabled {
            Span::styled("○ ", Style::default().fg(t.faint))
        } else {
            Span::styled("● ", Style::default().fg(t.good))
        };
        let mut line = Line::from(vec![
            Span::styled(bar, Style::default().fg(t.accent)),
            dot,
            Span::styled(
                format!(
                    "{:<width$}",
                    crate::ui::format::ellipsize(&tool.name, label_w - 2),
                    width = label_w - 2
                ),
                Style::default().fg(t.fg).add_modifier(if selected {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
            ),
            Span::styled(
                crate::ui::format::ellipsize(&one_line(&tool.description), val_w),
                Style::default().fg(t.muted),
            ),
        ]);
        if selected {
            line.style = Style::default().bg(t.sel_bg);
            super::fill_row(&mut line, inner.width as usize);
        }
        lines.push(line);
    }

    // Scroll so the selected row stays visible.
    let h = inner.height as usize;
    let total = lines.len();
    let offset = if total <= h || sel_line < h / 2 {
        0
    } else if sel_line + h / 2 >= total {
        total.saturating_sub(h)
    } else {
        sel_line - h / 2
    };
    f.render_widget(
        Paragraph::new(Text::from(lines)).scroll((offset as u16, 0)),
        inner,
    );
}

pub fn handle_key(app: &mut App, k: KeyEvent) -> bool {
    let len = ROWS.len();
    let mcp = mcp_count(app);
    let total = total_items(app);
    // Which region is the selection in?
    let in_config = app.sel < len;
    let mcp_index = if app.sel >= len && app.sel < len + mcp {
        Some(app.sel - len)
    } else {
        None
    };
    let on_add = app.sel == len + mcp;
    // Rows past the "+ add server" line are the agent tools.
    let tool_index = app.sel.checked_sub(len + mcp + 1);
    match k.code {
        KeyCode::Up | KeyCode::Char('k') => {
            app.move_sel(-1, total);
            true
        }
        KeyCode::Down | KeyCode::Char('j') => {
            app.move_sel(1, total);
            true
        }
        KeyCode::Char('r') => {
            app.load_view(app.view);
            true
        }
        KeyCode::Char('t') => {
            app.test_provider();
            true
        }
        // Delete the selected MCP server.
        KeyCode::Char('d') if mcp_index.is_some() => {
            app.delete_mcp(mcp_index.unwrap());
            true
        }
        // Clear a secret (the only way to *un*set a key from the TUI).
        KeyCode::Char('x') if in_config => {
            let row = &ROWS[app.sel.min(len - 1)];
            if matches!(row.kind, Kind::Secret) {
                app.clear_config_secret(row.section, row.field);
                app.toast(format!("· cleared {}", row.field));
            }
            true
        }
        KeyCode::Enter if mcp_index.is_some() => {
            app.open_mcp_form(mcp_index);
            true
        }
        KeyCode::Enter if on_add => {
            app.open_mcp_form(None);
            true
        }
        // Toggle an agent tool on/off (writes security.disabled_tools).
        KeyCode::Enter | KeyCode::Char(' ') if tool_index.is_some() => {
            app.toggle_tool(tool_index.unwrap());
            true
        }
        KeyCode::Enter => {
            let Some(cfg) = app.config_raw.clone() else {
                return true;
            };
            let row = &ROWS[app.sel.min(len - 1)];
            let (_, edit_val) = current(&cfg, row);
            match row.kind {
                Kind::Bool => {
                    let cur = cfg
                        .get(row.section)
                        .and_then(|s| s.get(row.get))
                        .and_then(|x| x.as_bool())
                        .unwrap_or(false);
                    app.toggle_config(row.section, row.field, cur);
                }
                Kind::Enum(opts) => {
                    if opts.is_empty() {
                        return true;
                    }
                    let cur = cfg
                        .get(row.section)
                        .and_then(|s| s.get(row.get))
                        .and_then(|x| x.as_str())
                        .unwrap_or("");
                    // If the live value isn't one of our options, land on the FIRST option rather
                    // than skipping past it — otherwise one keypress silently jumps two states
                    // (e.g. an unknown backend → index 0 → index 1 in a single Enter).
                    let next = match opts.iter().position(|o| *o == cur) {
                        Some(idx) => opts[(idx + 1) % opts.len()],
                        None => opts[0],
                    };
                    // Switching the provider kind resets the endpoint so the daemon
                    // picks the right default for the new provider (else base_url
                    // still points at the old one and the provider silently breaks).
                    if row.section == "provider" && row.field == "kind" {
                        app.config_set_patch(
                            serde_json::json!({ "provider": { "kind": next, "base_url": "" } }),
                        );
                    } else {
                        app.config_set_field(row.section, row.field, serde_json::json!(next));
                    }
                    app.toast(format!(
                        "· {} = {}",
                        row.field,
                        if next.is_empty() { "(default)" } else { next }
                    ));
                }
                Kind::Text => app.edit_config(row.section, row.field, false, false, &edit_val),
                Kind::Number => app.edit_config(row.section, row.field, false, true, &edit_val),
                Kind::Secret => app.edit_config(row.section, row.field, true, false, ""),
            }
            true
        }
        _ => false,
    }
}
