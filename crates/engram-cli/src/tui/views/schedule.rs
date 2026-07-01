//! The Schedule view — recurring jobs with their next-fire time. Fire on demand.

use super::window_start;
use crate::tui::app::App;
use crate::ui::format::{rel_time, stamp};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use serde_json::Value;

pub fn render(app: &mut App, f: &mut Frame, area: Rect) {
    let t = app.theme;
    app.clamp_sel(app.schedule.len());
    let block = super::panel(&t, "Schedule", app.schedule.len());
    let inner = block.inner(area);
    f.render_widget(block, area);

    if app.schedule.is_empty() {
        crate::tui::ui::empty_state(
            f,
            &t,
            inner,
            "No scheduled jobs. Press a to add one (name · when · task title).",
        );
        return;
    }

    let per = 3usize;
    let h = (inner.height as usize / per).max(1);
    let start = window_start(app.schedule.len(), h, app.sel);
    let mut lines: Vec<Line> = Vec::new();
    for (i, j) in app.schedule.iter().enumerate().skip(start).take(h) {
        let selected = i == app.sel;
        let bar = if selected { "▌ " } else { "  " };
        lines.push(Line::from(vec![
            Span::styled(bar, Style::default().fg(t.accent)),
            Span::styled("◷ ", Style::default().fg(t.accent2)),
            Span::styled(
                j.name.clone(),
                Style::default().fg(t.fg).add_modifier(if selected {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
            ),
            Span::styled(
                format!("   {}", describe(&j.recurrence)),
                Style::default().fg(t.muted),
            ),
        ]));
        let next = j
            .next_fire_ms
            .map(|ms| format!("next {} ({})", stamp(ms), rel_time(ms)))
            .unwrap_or_else(|| "—".into());
        let last = j
            .last_fire_ms
            .map(|ms| format!("  ·  last {}", rel_time(ms)))
            .unwrap_or_default();
        lines.push(Line::from(vec![
            Span::raw("    "),
            Span::styled(next, Style::default().fg(t.good)),
            Span::styled(last, Style::default().fg(t.faint)),
        ]));
        lines.push(Line::default());
    }
    f.render_widget(Paragraph::new(Text::from(lines)), inner);
}

fn describe(v: &Value) -> String {
    let kind = v.get("kind").and_then(|k| k.as_str()).unwrap_or("");
    match kind {
        "daily" => {
            let h = v.get("hour").and_then(|x| x.as_i64()).unwrap_or(0);
            let m = v.get("min").and_then(|x| x.as_i64()).unwrap_or(0);
            format!("daily at {h:02}:{m:02}")
        }
        "weekly" => "weekly".into(),
        "hourly" => "hourly".into(),
        "" => String::new(),
        other => other.to_string(),
    }
}

pub fn handle_key(app: &mut App, k: KeyEvent) -> bool {
    let len = app.schedule.len();
    match k.code {
        KeyCode::Up | KeyCode::Char('k') => {
            app.move_sel(-1, len);
            true
        }
        KeyCode::Down | KeyCode::Char('j') => {
            app.move_sel(1, len);
            true
        }
        KeyCode::Char('r') => {
            app.load_view(app.view);
            true
        }
        KeyCode::Char('a') => {
            app.add_schedule_form();
            true
        }
        KeyCode::Char('d') => {
            app.delete_selected_schedule();
            true
        }
        KeyCode::Enter => {
            if let Some(j) = app.schedule.get(app.sel) {
                let id = j.id.clone();
                let client = app.client.clone();
                tokio::spawn(async move {
                    let _ = client.schedule_run(&id).await;
                });
                app.toast("· fired job");
            }
            true
        }
        _ => false,
    }
}
