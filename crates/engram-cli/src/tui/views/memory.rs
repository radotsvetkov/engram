//! The Memory view — the brain's regions and tiers, the distilled self-model,
//! and a selectable list of recent memories you can forget.

use super::window_start;
use crate::tui::app::App;
use crate::ui::format::{one_line, rel_time};
use crate::ui::theme::region_color;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

pub fn render(app: &mut App, f: &mut Frame, area: Rect) {
    let t = app.theme;
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(3)])
        .split(area);

    // Stats strip.
    let stats = &app.memory_stats;
    let mut chips: Vec<Span> = vec![
        Span::styled("  ", Style::default()),
        Span::styled(
            format!("{} memories", stats.total),
            Style::default().fg(t.fg).add_modifier(Modifier::BOLD),
        ),
        Span::styled("   ", Style::default()),
    ];
    for (region, n) in &stats.by_region {
        chips.push(Span::styled(
            format!(" {region} {n} "),
            Style::default().fg(region_color(&t, region)).bg(t.code_bg),
        ));
        chips.push(Span::raw(" "));
    }
    for (tier, n) in &stats.by_tier {
        chips.push(Span::styled(
            format!("{tier}:{n}  "),
            Style::default().fg(t.muted),
        ));
    }
    let block = Block::default()
        .borders(Borders::BOTTOM)
        .border_style(Style::default().fg(t.faint));
    let inner = block.inner(rows[0]);
    f.render_widget(block, rows[0]);
    f.render_widget(Paragraph::new(Line::from(chips)), inner);

    // Body: self-model | recent.
    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Ratio(2, 5), Constraint::Ratio(3, 5)])
        .split(rows[1]);

    self_model(app, f, body[0]);
    recent(app, f, body[1]);
}

fn self_model(app: &App, f: &mut Frame, area: Rect) {
    let t = &app.theme;
    let count = app
        .consciousness
        .as_ref()
        .map(|c| c.lines.len())
        .unwrap_or(0);
    let block = super::panel(t, "Self-model", count);
    let inner = block.inner(area);
    f.render_widget(block, area);
    let mut lines: Vec<Line> = Vec::new();
    if let Some(c) = &app.consciousness {
        lines.push(Line::from(Span::styled(
            format!(
                "  distilled v{} · {}",
                c.version,
                rel_time(c.distilled_at_ms)
            ),
            Style::default().fg(t.faint),
        )));
        lines.push(Line::default());
        for l in &c.lines {
            lines.push(Line::from(vec![
                Span::styled(
                    format!(
                        " {} ",
                        l.region.chars().next().unwrap_or('?').to_ascii_uppercase()
                    ),
                    Style::default()
                        .fg(region_color(t, &l.region))
                        .bg(t.code_bg),
                ),
                Span::raw(" "),
                Span::styled(one_line(&l.text), Style::default().fg(t.fg)),
            ]));
        }
    } else {
        lines.push(Line::from(Span::styled(
            "  loading…",
            Style::default().fg(t.muted),
        )));
    }
    f.render_widget(Paragraph::new(Text::from(lines)), inner);
}

fn recent(app: &App, f: &mut Frame, area: Rect) {
    let t = &app.theme;
    let block = super::panel(t, "Recent memories", app.memory_recent.len());
    let inner = block.inner(area);
    f.render_widget(block, area);
    let h = inner.height as usize;
    let start = window_start(app.memory_recent.len(), h, app.sel);
    let mut lines: Vec<Line> = Vec::new();
    for (i, r) in app.memory_recent.iter().enumerate().skip(start).take(h) {
        let selected = i == app.sel;
        let bar = if selected { "▌" } else { " " };
        let spans = vec![
            Span::styled(format!("{bar} "), Style::default().fg(t.accent)),
            Span::styled(format!("{:>4} ", r.id), Style::default().fg(t.faint)),
            Span::styled(
                format!(
                    "{} ",
                    r.region.chars().next().unwrap_or('?').to_ascii_uppercase()
                ),
                Style::default().fg(region_color(t, &r.region)),
            ),
            Span::styled(
                crate::ui::format::ellipsize(
                    &one_line(&r.text),
                    inner.width.saturating_sub(10) as usize,
                ),
                Style::default().fg(if selected { t.fg } else { t.muted }),
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
}

pub fn handle_key(app: &mut App, k: KeyEvent) -> bool {
    let len = app.memory_recent.len();
    match k.code {
        KeyCode::Up | KeyCode::Char('k') => {
            app.confirm_forget = None;
            app.move_sel(-1, len);
            true
        }
        KeyCode::Down | KeyCode::Char('j') => {
            app.confirm_forget = None;
            app.move_sel(1, len);
            true
        }
        KeyCode::Char('r') => {
            app.load_view(app.view);
            true
        }
        // Forget is destructive, so require a confirming second `f` on the same row.
        KeyCode::Char('f') => {
            if let Some(rec) = app.memory_recent.get(app.sel) {
                let id = rec.id;
                if app.confirm_forget == Some(id) {
                    app.confirm_forget = None;
                    let client = app.client.clone();
                    let tx = app.tx.clone();
                    tokio::spawn(async move {
                        let _ = client.forget(id).await;
                        if let Ok(recs) = client.memory_recent(None, 40).await {
                            let _ = tx.send(crate::tui::app::Msg::MemoryRecent(recs));
                        }
                    });
                    app.toast(format!("· forgot memory {id}"));
                } else {
                    app.confirm_forget = Some(id);
                    app.toast(format!("press f again to forget memory {id}"));
                }
            }
            true
        }
        _ => false,
    }
}
