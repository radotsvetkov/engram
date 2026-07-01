//! The Ledger view — the signed, append-only audit chain, newest last, with the
//! live verification chip and a payload preview for the selected entry.

use super::window_start;
use crate::tui::app::App;
use crate::ui::format::{hhmm, one_line};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

pub fn render(app: &mut App, f: &mut Frame, area: Rect) {
    let t = app.theme;
    app.clamp_sel(app.ledger_tail.len());
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(6)])
        .split(area);

    // Verification banner + list.
    let (chip, chip_style) = match &app.ledger {
        Some(l) if l.ok => (
            format!("✓ chain intact · {} entries signed", l.entries),
            Style::default().fg(t.good).add_modifier(Modifier::BOLD),
        ),
        Some(_) => (
            "✗ TAMPER DETECTED".to_string(),
            Style::default().fg(t.bad).add_modifier(Modifier::BOLD),
        ),
        None => ("verifying…".to_string(), Style::default().fg(t.muted)),
    };
    let block = super::panel(&t, "Audit ledger", app.ledger_tail.len())
        .title_bottom(Span::styled(format!(" {chip} "), chip_style));
    let inner = block.inner(rows[0]);
    f.render_widget(block, rows[0]);

    let h = inner.height as usize;
    let start = window_start(app.ledger_tail.len(), h, app.sel);
    let mut lines: Vec<Line> = Vec::new();
    for (i, e) in app.ledger_tail.iter().enumerate().skip(start).take(h) {
        let selected = i == app.sel;
        let bar = if selected { "▌" } else { " " };
        let spans = vec![
            Span::styled(format!("{bar} "), Style::default().fg(t.accent)),
            Span::styled(format!("#{:<6}", e.seq), Style::default().fg(t.faint)),
            Span::styled(format!("{} ", hhmm(e.ts_ms)), Style::default().fg(t.muted)),
            Span::styled(
                format!("{:<22}", crate::ui::format::ellipsize(&e.kind, 22)),
                Style::default().fg(t.accent2),
            ),
            Span::styled(
                format!("{:<8} ", crate::ui::format::ellipsize(&e.actor, 8)),
                Style::default().fg(t.muted),
            ),
            Span::styled(
                e.hash.chars().take(12).collect::<String>(),
                Style::default().fg(t.faint),
            ),
        ];
        let mut line = Line::from(spans);
        if selected {
            line.style = Style::default().bg(t.sel_bg);
            super::fill_row(&mut line, inner.width as usize);
        }
        lines.push(line);
    }
    f.render_widget(Paragraph::new(Text::from(lines)), inner);

    // Payload of the selected entry.
    let block = super::panel(&t, "Payload", 0);
    let inner = block.inner(rows[1]);
    f.render_widget(block, rows[1]);
    let mut dl: Vec<Line> = Vec::new();
    if let Some(e) = app.ledger_tail.get(app.sel) {
        dl.push(Line::from(vec![
            Span::styled("hash ", Style::default().fg(t.muted)),
            Span::styled(e.hash.clone(), Style::default().fg(t.good)),
        ]));
        let pretty = serde_json::to_string(&e.payload).unwrap_or_default();
        for chunk in textwrap(
            &one_line(&pretty),
            inner.width.saturating_sub(1) as usize,
            3,
        ) {
            dl.push(Line::from(Span::styled(chunk, Style::default().fg(t.fg))));
        }
    }
    f.render_widget(Paragraph::new(Text::from(dl)), inner);
}

fn textwrap(s: &str, width: usize, max_lines: usize) -> Vec<String> {
    let mut out = Vec::new();
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() && out.len() < max_lines {
        let end = (i + width.max(1)).min(chars.len());
        out.push(chars[i..end].iter().collect());
        i = end;
    }
    out
}

pub fn handle_key(app: &mut App, k: KeyEvent) -> bool {
    let len = app.ledger_tail.len();
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
            app.refresh_spine();
            true
        }
        _ => false,
    }
}
