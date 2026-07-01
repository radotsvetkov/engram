//! The Agents view — the named, durable agents registered with the daemon,
//! each with its model, role, and autonomy posture.

use super::window_start;
use crate::tui::app::App;
use crate::ui::format::one_line;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

pub fn render(app: &mut App, f: &mut Frame, area: Rect) {
    let t = app.theme;
    app.clamp_sel(app.agents.len());
    let block = super::panel(&t, "Agents", app.agents.len());
    let inner = block.inner(area);
    f.render_widget(block, area);

    if app.agents.is_empty() {
        crate::tui::ui::empty_state(
            f,
            &t,
            inner,
            "No named agents yet — the default agent runs every chat and task. Press n to create one.",
        );
        return;
    }

    let per = 2usize;
    let h = (inner.height as usize / per).max(1);
    let start = window_start(app.agents.len(), h, app.sel);
    let mut lines: Vec<Line> = Vec::new();
    for (i, a) in app.agents.iter().enumerate().skip(start).take(h) {
        let selected = i == app.sel;
        let name = a.get("name").and_then(|v| v.as_str()).unwrap_or("?");
        let role = a.get("role").and_then(|v| v.as_str()).unwrap_or("");
        let emoji = a.get("emoji").and_then(|v| v.as_str()).unwrap_or("•");
        let model = a.get("model").and_then(|v| v.as_str()).unwrap_or("");
        let policy = a
            .get("autonomy_policy")
            .map(|p| !p.is_null())
            .unwrap_or(false);
        let bar = if selected { "▌ " } else { "  " };
        lines.push(Line::from(vec![
            Span::styled(bar, Style::default().fg(t.accent)),
            Span::styled(format!("{emoji} "), Style::default()),
            Span::styled(
                name.to_string(),
                Style::default().fg(t.fg).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                if model.is_empty() {
                    String::new()
                } else {
                    format!("   {model}")
                },
                Style::default().fg(t.accent2),
            ),
            Span::styled(
                if policy {
                    "   ⛨ autonomy".to_string()
                } else {
                    String::new()
                },
                Style::default().fg(t.good),
            ),
        ]));
        lines.push(Line::from(vec![
            Span::raw("    "),
            Span::styled(one_line(role), Style::default().fg(t.muted)),
        ]));
    }
    f.render_widget(Paragraph::new(Text::from(lines)), inner);
}

pub fn handle_key(app: &mut App, k: KeyEvent) -> bool {
    let len = app.agents.len();
    match k.code {
        KeyCode::Up | KeyCode::Char('k') => {
            app.confirm_agent = None;
            app.move_sel(-1, len);
            true
        }
        KeyCode::Down | KeyCode::Char('j') => {
            app.confirm_agent = None;
            app.move_sel(1, len);
            true
        }
        KeyCode::Char('r') => {
            app.load_view(app.view);
            true
        }
        KeyCode::Char('n') => {
            app.create_agent_prompt();
            true
        }
        KeyCode::Char('e') => {
            app.edit_selected_agent();
            true
        }
        KeyCode::Char('p') => {
            app.policy_selected_agent();
            true
        }
        // Destructive — requires a confirming second `d`.
        KeyCode::Char('d') => {
            app.delete_selected_agent();
            true
        }
        _ => false,
    }
}
