//! The Tasks view — a three-column kanban board (To do / Running / Done) with
//! a glass-box detail modal showing the signed receipt for a finished run.

use super::window_start;
use crate::api::Task;
use crate::tui::app::App;
use crate::tui::ui::centered;
use crate::ui::format::{cost, rel_time, spinner};
use crate::ui::markdown;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

const COLUMNS: [(&str, &[&str]); 3] = [
    ("To do", &["todo", "scheduled"]),
    ("Running", &["doing"]),
    ("Done", &["done", "failed"]),
];

fn column_tasks(app: &App, col: usize) -> Vec<&Task> {
    let statuses = COLUMNS[col].1;
    app.tasks
        .iter()
        .filter(|t| statuses.contains(&t.status_or_todo()))
        .collect()
}

pub fn render(app: &mut App, f: &mut Frame, area: Rect) {
    let t = app.theme;
    if app.tasks.is_empty() {
        let b = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(t.faint));
        f.render_widget(b, area);
        crate::tui::ui::empty_state(
            f,
            &t,
            area,
            "No tasks yet — ask in Chat to kick one off (Ctrl-T), or run `engram tasks new \"…\"`.",
        );
        return;
    }

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Ratio(1, 3); 3])
        .split(area);

    app.board_col = app.board_col.min(2);
    // Keep the selection valid for the focused column after a background refresh.
    let col_len = column_tasks(app, app.board_col).len();
    app.clamp_sel(col_len);
    for (ci, area) in cols.iter().enumerate() {
        let tasks = column_tasks(app, ci);
        let focused = ci == app.board_col;
        let border = if focused { t.accent } else { t.faint };
        // Surface the halted state on the Running column so the user knows the kill switch is
        // engaged and that a second `c` releases it now.
        let title = if ci == 1 && app.task_halted {
            format!(
                " {} ({}) · halted — c to release ",
                COLUMNS[ci].0,
                tasks.len()
            )
        } else {
            format!(" {} ({}) ", COLUMNS[ci].0, tasks.len())
        };
        let title_fg = if ci == 1 && app.task_halted {
            t.warn
        } else if focused {
            t.accent
        } else {
            t.muted
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border))
            .title(Span::styled(
                title,
                Style::default().fg(title_fg).add_modifier(Modifier::BOLD),
            ));
        let inner = block.inner(*area);
        f.render_widget(block, *area);

        if tasks.is_empty() {
            f.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    "  —",
                    Style::default().fg(t.faint),
                ))),
                inner,
            );
            continue;
        }

        let card_h = 3usize;
        let visible = (inner.height as usize / card_h).max(1);
        let sel = if focused {
            app.sel.min(tasks.len() - 1)
        } else {
            usize::MAX
        };
        let start = if focused {
            window_start(tasks.len(), visible, app.sel.min(tasks.len() - 1))
        } else {
            0
        };

        for (row, task) in tasks.iter().enumerate().skip(start).take(visible) {
            let y = inner.y + ((row - start) as u16 * card_h as u16);
            if y + 2 > inner.y + inner.height {
                break;
            }
            let card = Rect {
                x: inner.x,
                y,
                width: inner.width,
                height: card_h as u16,
            };
            draw_card(f, app, card, task, row == sel, ci == 1);
        }
    }

    if app.detail_open {
        draw_detail(app, f, area);
    }
}

fn draw_card(f: &mut Frame, app: &App, area: Rect, task: &Task, selected: bool, running: bool) {
    let t = &app.theme;
    let title_style = if selected {
        Style::default().fg(t.fg).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(t.fg)
    };
    let bar = if selected {
        Span::styled("▌ ", Style::default().fg(t.accent))
    } else {
        Span::styled("  ", Style::default())
    };
    let title = crate::ui::format::ellipsize(&task.title, area.width.saturating_sub(4) as usize);
    let mut sub: Vec<Span> = vec![Span::raw("  ")];
    if running {
        sub.push(Span::styled(
            format!("{} ", spinner(app.tick)),
            Style::default().fg(t.accent),
        ));
        if let Some(p) = &task.progress {
            sub.push(Span::styled(
                crate::ui::format::ellipsize(p, area.width.saturating_sub(8) as usize),
                Style::default().fg(t.accent2),
            ));
        } else {
            sub.push(Span::styled("running…", Style::default().fg(t.accent2)));
        }
    } else {
        sub.push(Span::styled(
            format!("{} · {}", task.origin, rel_time(task.created_ms)),
            Style::default().fg(t.muted),
        ));
        if let Some(run) = &task.run {
            if run.cost_usd > 0.0 {
                sub.push(Span::styled(
                    format!(" · {}", cost(run.cost_usd)),
                    Style::default().fg(t.faint),
                ));
            }
        }
    }

    let lines = vec![
        Line::from(vec![bar, Span::styled(title, title_style)]),
        Line::from(sub),
    ];
    let mut p = Paragraph::new(lines);
    if selected {
        p = p.style(Style::default().bg(t.sel_bg));
    }
    f.render_widget(p, area);
}

fn draw_detail(app: &App, f: &mut Frame, area: Rect) {
    let t = &app.theme;
    let tasks = column_tasks(app, app.board_col);
    let Some(task) = tasks.get(app.sel) else {
        return;
    };
    let w = area.width.saturating_sub(8).min(100);
    let h = area.height.saturating_sub(4);
    let rect = centered(area, w, h);
    f.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(t.accent))
        .title(Span::styled(
            format!(
                " {} ",
                crate::ui::format::ellipsize(&task.title, w.saturating_sub(6) as usize)
            ),
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(t.bar_bg));
    let inner = block.inner(rect);
    f.render_widget(block, rect);

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(vec![
        Span::styled("status ", Style::default().fg(t.muted)),
        Span::styled(task.status_or_todo().to_string(), Style::default().fg(t.fg)),
        Span::styled("   id ", Style::default().fg(t.muted)),
        Span::styled(task.id.clone(), Style::default().fg(t.faint)),
    ]));
    if let Some(run) = &task.run {
        lines.push(Line::from(vec![
            Span::styled("stopped ", Style::default().fg(t.muted)),
            Span::styled(run.stopped.clone(), Style::default().fg(t.fg)),
            Span::styled("   cost ", Style::default().fg(t.muted)),
            Span::styled(cost(run.cost_usd), Style::default().fg(t.warn)),
        ]));
        lines.push(Line::default());
        for l in markdown::render(&run.answer, inner.width.saturating_sub(1), t) {
            lines.push(l);
        }
        if !run.steps.is_empty() {
            lines.push(Line::default());
            lines.push(crate::tui::ui::section(t, "Signed audit trail"));
            for s in &run.steps {
                lines.push(Line::from(vec![
                    Span::styled(
                        format!("#{:<5} ", s.ledger_seq),
                        Style::default().fg(t.faint),
                    ),
                    Span::styled(
                        if s.ok { "✓ " } else { "✗ " },
                        Style::default().fg(if s.ok { t.good } else { t.bad }),
                    ),
                    Span::styled(format!("{:<16}", s.tool), Style::default().fg(t.accent2)),
                    Span::styled(
                        s.ledger_hash.chars().take(16).collect::<String>(),
                        Style::default().fg(t.faint),
                    ),
                ]));
            }
        }
    } else {
        lines.push(Line::default());
        lines.push(Line::from(Span::styled(
            "Not run yet. Press ↵ on the card to run it.",
            Style::default().fg(t.muted),
        )));
    }
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        "↑↓ scroll · esc to close",
        Style::default().fg(t.faint),
    )));
    f.render_widget(
        Paragraph::new(Text::from(lines)).scroll((app.detail_scroll, 0)),
        inner,
    );
}

pub fn handle_key(app: &mut App, k: KeyEvent) -> bool {
    let tasks_len = column_tasks(app, app.board_col).len();
    // While the detail modal is open, arrow keys scroll it.
    if app.detail_open {
        match k.code {
            KeyCode::Esc => {
                app.detail_open = false;
                return true;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                app.detail_scroll = app.detail_scroll.saturating_sub(1);
                return true;
            }
            KeyCode::Down | KeyCode::Char('j') => {
                app.detail_scroll = app.detail_scroll.saturating_add(1);
                return true;
            }
            KeyCode::PageUp => {
                app.detail_scroll = app.detail_scroll.saturating_sub(10);
                return true;
            }
            KeyCode::PageDown => {
                app.detail_scroll = app.detail_scroll.saturating_add(10);
                return true;
            }
            _ => return true,
        }
    }
    match k.code {
        KeyCode::Left | KeyCode::Char('h') => {
            app.board_col = app.board_col.saturating_sub(1);
            app.sel = 0;
            true
        }
        KeyCode::Right | KeyCode::Char('l') => {
            app.board_col = (app.board_col + 1).min(2);
            app.sel = 0;
            true
        }
        KeyCode::Up | KeyCode::Char('k') => {
            app.move_sel(-1, tasks_len);
            true
        }
        KeyCode::Down | KeyCode::Char('j') => {
            app.move_sel(1, tasks_len);
            true
        }
        KeyCode::Char('r') => {
            app.load_view(app.view);
            true
        }
        // Cancel a running task with a TASK-SCOPED halt. The daemon registers each task run's flag
        // under "<task-id>#<n>" and matches it to session=<task-id>, so this stops JUST this run at
        // its next step boundary — it never touches other runs or the daemon-wide kill switch (the
        // old client-side global-pulse mitigation is no longer needed).
        KeyCode::Char('c') => {
            let task = column_tasks(app, app.board_col)
                .get(app.sel)
                .map(|t| (t.id.clone(), t.status_or_todo().to_string()));
            if let Some((id, status)) = task {
                if status == "doing" {
                    let client = app.client.clone();
                    let tx = app.tx.clone();
                    tokio::spawn(async move {
                        let _ = client.halt(Some(&id), true).await;
                        if let Ok(tasks) = client.tasks().await {
                            let _ = tx.send(crate::tui::app::Msg::Tasks(tasks));
                        }
                        let _ = tx.send(crate::tui::app::Msg::TaskHaltReleased);
                    });
                    app.toast("· stopping this task at its next step");
                }
            }
            true
        }
        KeyCode::Enter => {
            let task = column_tasks(app, app.board_col)
                .get(app.sel)
                .map(|t| (t.id.clone(), t.status_or_todo().to_string()));
            if let Some((id, status)) = task {
                if status == "todo" || status == "scheduled" {
                    // Fire the run; it will stream server-side and show up on refresh.
                    let client = app.client.clone();
                    let tx = app.tx.clone();
                    tokio::spawn(async move {
                        let _ = client.task_run(&id).await;
                        if let Ok(tasks) = client.tasks().await {
                            let _ = tx.send(crate::tui::app::Msg::Tasks(tasks));
                        }
                    });
                    app.toast("· running task");
                } else {
                    app.detail_open = true;
                    app.detail_scroll = 0;
                }
            }
            true
        }
        _ => false,
    }
}
