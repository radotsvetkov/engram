//! The Skills view — the self-improving program library, with enable toggles
//! and a detail pane showing capabilities, runtime, and learning history.

use super::window_start;
use crate::tui::app::App;
use crate::ui::format::one_line;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

pub fn render(app: &mut App, f: &mut Frame, area: Rect) {
    let t = app.theme;
    app.clamp_sel(app.skills.len());
    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Ratio(3, 5), Constraint::Ratio(2, 5)])
        .split(area);

    // List.
    let enabled = app.skills.iter().filter(|s| s.enabled).count();
    let proposed = app.skills.iter().filter(|s| s.proposed).count();
    let title = if proposed > 0 {
        format!("Skills · {enabled} on · {proposed} proposed")
    } else {
        format!("Skills · {enabled} on")
    };
    let block = super::panel(&t, &title, app.skills.len());
    let inner = block.inner(body[0]);
    f.render_widget(block, body[0]);
    let h = inner.height as usize;
    let start = window_start(app.skills.len(), h, app.sel);
    let mut lines: Vec<Line> = Vec::new();
    for (i, s) in app.skills.iter().enumerate().skip(start).take(h) {
        let selected = i == app.sel;
        // Three states: proposed (◆ amber, adoptable), enabled (● green), off (○ dim).
        let dot = if s.proposed {
            Span::styled("◆ ", Style::default().fg(t.warn))
        } else if s.enabled {
            Span::styled("● ", Style::default().fg(t.good))
        } else {
            Span::styled("○ ", Style::default().fg(t.faint))
        };
        let bar = if selected { "▌" } else { " " };
        // Pad by DISPLAY width (ellipsize budgets columns, format! counts
        // chars) — else a wide-char id/category shifts everything after it.
        let mut spans = vec![
            Span::styled(bar, Style::default().fg(t.accent)),
            dot,
            Span::styled(
                crate::ui::format::pad_display(&crate::ui::format::ellipsize(&s.id, 18), 18),
                Style::default().fg(t.fg).add_modifier(if selected {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
            ),
            Span::styled(
                crate::ui::format::pad_display(&crate::ui::format::ellipsize(&s.category, 12), 12),
                Style::default().fg(t.accent2),
            ),
        ];
        if s.proposed {
            spans.push(Span::styled(" proposed", Style::default().fg(t.warn)));
        } else if !s.learn.is_empty() {
            spans.push(Span::styled(
                format!(" ↗{}", s.learn.len()),
                Style::default().fg(t.good),
            ));
        }
        let mut line = Line::from(spans);
        if selected {
            line.style = Style::default().bg(t.sel_bg);
            super::fill_row(&mut line, inner.width as usize);
        }
        lines.push(line);
    }
    f.render_widget(Paragraph::new(Text::from(lines)), inner);

    // Detail.
    let block = super::panel(&t, "Detail", 0);
    let inner = block.inner(body[1]);
    f.render_widget(block, body[1]);
    let mut dl: Vec<Line> = Vec::new();
    if let Some(s) = app.skills.get(app.sel) {
        dl.push(Line::from(Span::styled(
            s.id.clone(),
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
        )));
        dl.push(Line::default());
        for l in crate::ui::markdown::render(&s.description, inner.width.saturating_sub(1), &t) {
            dl.push(l);
        }
        dl.push(Line::default());
        if let Some(w) = &s.when_to_use {
            dl.push(Line::from(Span::styled(
                "when to use",
                Style::default().fg(t.muted),
            )));
            for l in crate::ui::markdown::render(w, inner.width.saturating_sub(1), &t) {
                dl.push(l);
            }
            dl.push(Line::default());
        }
        kv(
            &mut dl,
            &t,
            "runtime",
            &format!(
                "{} {}",
                s.runtime,
                s.interpreter.clone().unwrap_or_default()
            ),
        );
        kv(
            &mut dl,
            &t,
            "capabilities",
            &if s.capabilities.is_empty() {
                "none".into()
            } else {
                s.capabilities.join(", ")
            },
        );
        kv(
            &mut dl,
            &t,
            "version",
            &match s.active {
                Some(v) => format!("v{} of {}", v, s.versions.len()),
                None => format!("proposed · {} version(s), none active", s.versions.len()),
            },
        );
        kv(
            &mut dl,
            &t,
            "gold runs",
            &if s.runs == 0 {
                "0 (unverified)".to_string()
            } else {
                s.runs.to_string()
            },
        );
        kv(&mut dl, &t, "improvements", &s.learn.len().to_string());
        dl.push(Line::default());
        if s.proposed {
            dl.push(Line::from(Span::styled(
                "a adopt (replays gold examples, activates on pass)",
                Style::default().fg(t.warn),
            )));
        }
        dl.push(Line::from(Span::styled(
            "↵/space toggle enable",
            Style::default().fg(t.faint),
        )));
    } else {
        dl.push(Line::from(Span::styled(
            "  no skills",
            Style::default().fg(t.muted),
        )));
    }
    f.render_widget(Paragraph::new(Text::from(dl)), inner);
}

fn kv(out: &mut Vec<Line>, t: &crate::ui::Theme, k: &str, v: &str) {
    out.push(Line::from(vec![
        Span::styled(format!("{k:<14}"), Style::default().fg(t.muted)),
        Span::styled(one_line(v), Style::default().fg(t.fg)),
    ]));
}

pub fn handle_key(app: &mut App, k: KeyEvent) -> bool {
    let len = app.skills.len();
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
            app.adopt_selected_skill();
            true
        }
        KeyCode::Enter | KeyCode::Char(' ') => {
            if let Some(s) = app.skills.get(app.sel) {
                let (id, want) = (s.id.clone(), !s.enabled);
                let client = app.client.clone();
                let tx = app.tx.clone();
                tokio::spawn(async move {
                    let _ = client.skill_set_enabled(&id, want).await;
                    if let Ok(resp) = client.skills().await {
                        let _ = tx.send(crate::tui::app::Msg::Skills(resp.skills));
                    }
                });
                app.toast(if want { "· enabled" } else { "· disabled" });
            }
            true
        }
        _ => false,
    }
}
