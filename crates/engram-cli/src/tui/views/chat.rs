//! The chat view: a streaming transcript with live tool steps, a recall ribbon
//! under each grounded answer, and a composer at the bottom.

use crate::tui::app::{App, PlanStep, Role};
use crate::ui::format::{one_line, spinner};
use crate::ui::markdown;
use crate::ui::theme::region_color;
use crate::ui::Theme;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

pub fn render(app: &mut App, f: &mut Frame, area: Rect) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(3)])
        .split(area);
    let transcript = rows[0];
    let composer = rows[1];

    let t = app.theme;
    let width = transcript.width.saturating_sub(3) as usize;
    let mut lines: Vec<Line<'static>> = Vec::new();

    if app.chat.turns.is_empty() && !app.chat.streaming {
        welcome(&mut lines, app);
    }

    for turn in &app.chat.turns {
        match turn.role {
            Role::User => {
                lines.push(Line::from(vec![
                    Span::styled("▌ ", Style::default().fg(t.user)),
                    Span::styled(
                        "you",
                        Style::default().fg(t.user).add_modifier(Modifier::BOLD),
                    ),
                ]));
                for l in markdown::render(&turn.text, width as u16, &t) {
                    lines.push(indent(l, 2));
                }
            }
            Role::Engram => {
                let label_style = if turn.error {
                    Style::default().fg(t.bad).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(t.accent).add_modifier(Modifier::BOLD)
                };
                lines.push(Line::from(vec![
                    Span::styled("▌ ", Style::default().fg(t.accent)),
                    Span::styled("engram", label_style),
                ]));
                for pl in plan_lines(&turn.plan, &t) {
                    lines.push(pl);
                }
                for l in markdown::render(&turn.text, width as u16, &t) {
                    lines.push(indent(l, 2));
                }
                // Recall ribbon.
                if !turn.recalled.is_empty() {
                    let mut chips: Vec<Span> = vec![
                        Span::raw("  "),
                        Span::styled("grounded on ", Style::default().fg(t.muted)),
                    ];
                    for r in turn.recalled.iter().take(6) {
                        chips.push(Span::styled(
                            format!(" {}:{} ", region_letter(&r.region), r.id),
                            Style::default()
                                .fg(region_color(&t, &r.region))
                                .bg(t.code_bg),
                        ));
                        chips.push(Span::raw(" "));
                    }
                    lines.push(Line::from(chips));
                }
                if !turn.learned.is_empty() {
                    lines.push(Line::from(vec![
                        Span::raw("  "),
                        Span::styled("learned ", Style::default().fg(t.muted)),
                        Span::styled(
                            one_line(&turn.learned.join("; ")),
                            Style::default().fg(t.good),
                        ),
                    ]));
                }
                if !turn.steps.is_empty() {
                    let last = turn
                        .steps
                        .last()
                        .map(|s| s.tool.clone())
                        .unwrap_or_default();
                    lines.push(Line::from(vec![
                        Span::raw("  "),
                        Span::styled(
                            format!(
                                "⚙ {} tool step{} · last {last}",
                                turn.steps.len(),
                                if turn.steps.len() == 1 { "" } else { "s" }
                            ),
                            Style::default().fg(t.faint),
                        ),
                    ]));
                }
            }
        }
        lines.push(Line::default());
    }

    // Live, in-progress turn.
    if app.chat.streaming {
        lines.push(Line::from(vec![
            Span::styled("▌ ", Style::default().fg(t.accent)),
            Span::styled(
                "engram",
                Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
            ),
        ]));
        for pl in plan_lines(&app.chat.live_plan, &t) {
            lines.push(pl);
        }
        for note in &app.chat.live_narration {
            for l in markdown::render(note, width as u16, &t) {
                lines.push(indent(l, 2));
            }
        }
        for step in &app.chat.live_steps {
            let mark = if step.ok { "✓" } else { "✗" };
            let color = if step.ok { t.good } else { t.bad };
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(format!("{mark} "), Style::default().fg(color)),
                Span::styled(format!("{} ", step.tool), Style::default().fg(t.accent2)),
                Span::styled(
                    crate::ui::format::ellipsize(
                        &one_line(&step.observation),
                        width.saturating_sub(8),
                    ),
                    Style::default().fg(t.muted),
                ),
            ]));
        }
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(
                format!("{} thinking…", spinner(app.tick)),
                Style::default().fg(t.accent),
            ),
        ]));
        lines.push(Line::default());
    }

    // Scroll math: stick to bottom unless the user scrolled up.
    let total = lines.len() as u16;
    let viewport = transcript.height;
    app.chat.last_total = total;
    app.chat.last_viewport = viewport;
    let max_scroll = total.saturating_sub(viewport);
    if app.chat.stick || app.chat.scroll > max_scroll {
        app.chat.scroll = max_scroll;
    }

    let para = Paragraph::new(Text::from(lines)).scroll((app.chat.scroll, 0));
    f.render_widget(para, transcript);

    // Composer.
    draw_composer(app, f, composer);
}

fn draw_composer(app: &App, f: &mut Frame, area: Rect) {
    let t = &app.theme;
    let border = if app.chat.streaming {
        Style::default().fg(t.faint)
    } else {
        Style::default().fg(t.accent)
    };
    let title = if app.chat.streaming {
        Span::styled(" streaming — esc to stop ", Style::default().fg(t.muted))
    } else if !app.chat.pending_attachments.is_empty() {
        let names: Vec<&str> = app
            .chat
            .pending_attachments
            .iter()
            .map(|a| a.name.as_str())
            .collect();
        Span::styled(
            format!(" message · 📎 {} ", names.join(", ")),
            Style::default().fg(t.accent2).add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled(
            " message ",
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
        )
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border)
        .title(title);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let avail = inner.width.saturating_sub(1) as usize;
    let content: Line = if app.chat.composer.is_empty() {
        Line::from(Span::styled(
            "Message Engram…   ( / for commands )",
            Style::default().fg(t.faint),
        ))
    } else {
        // Horizontal scroll so the caret stays visible.
        let chars: Vec<char> = app.chat.composer.chars().collect();
        let cursor = app.chat.cursor.min(chars.len());
        let start = cursor.saturating_sub(avail.saturating_sub(1));
        let mut spans: Vec<Span> = Vec::new();
        let visible: String = chars[start..cursor].iter().collect();
        spans.push(Span::styled(visible, Style::default().fg(t.fg)));
        spans.push(Span::styled(
            "▏",
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
        ));
        let after: String = chars[cursor..].iter().collect();
        let after = crate::ui::format::ellipsize(&after, avail.saturating_sub(cursor - start));
        spans.push(Span::styled(after, Style::default().fg(t.fg)));
        Line::from(spans)
    };
    f.render_widget(Paragraph::new(content), inner);
}

fn welcome(lines: &mut Vec<Line<'static>>, app: &App) {
    let t = &app.theme;
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        "  ✦ Engram",
        Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(Span::styled(
        "  Your personal agent — it remembers you, and every action is signed.",
        Style::default().fg(t.muted),
    )));
    lines.push(Line::default());
    let bullets = [
        (
            "Ask anything",
            "it runs the same tool-using agent the task board does",
        ),
        (
            "/ for commands",
            "jump to Tasks, Memory, Skills, Ledger, and more",
        ),
        (
            "Watch it think",
            "tool steps stream live; answers are grounded in memory",
        ),
        (
            "Trust the spine",
            "the header shows cost and a live ledger-verified chip",
        ),
    ];
    for (head, body) in bullets {
        lines.push(Line::from(vec![
            Span::styled("  • ", Style::default().fg(t.accent2)),
            Span::styled(head, Style::default().fg(t.fg).add_modifier(Modifier::BOLD)),
            Span::styled(format!("  {body}"), Style::default().fg(t.muted)),
        ]));
    }
    lines.push(Line::default());
}

fn region_letter(region: &str) -> char {
    region.chars().next().unwrap_or('?').to_ascii_uppercase()
}

/// Render the agent's plan as a live checklist (empty input → no lines).
fn plan_lines(plan: &[PlanStep], t: &Theme) -> Vec<Line<'static>> {
    if plan.is_empty() {
        return Vec::new();
    }
    let done = plan.iter().filter(|s| s.status == "done").count();
    let mut out = vec![Line::from(vec![
        Span::raw("  "),
        Span::styled(
            format!("plan · {done}/{} done", plan.len()),
            Style::default().fg(t.muted),
        ),
    ])];
    for s in plan {
        let (sym, color) = match s.status.as_str() {
            "done" => ("✓", t.good),
            "doing" => ("◐", t.accent),
            _ => ("○", t.muted),
        };
        let title_style = match s.status.as_str() {
            "done" => Style::default()
                .fg(t.muted)
                .add_modifier(Modifier::CROSSED_OUT),
            "doing" => Style::default().fg(t.fg).add_modifier(Modifier::BOLD),
            _ => Style::default().fg(t.muted),
        };
        out.push(Line::from(vec![
            Span::raw("   "),
            Span::styled(format!("{sym} "), Style::default().fg(color)),
            Span::styled(s.title.clone(), title_style),
        ]));
    }
    out
}

/// Left-pad a rendered line by `n` columns.
fn indent(line: Line<'static>, n: usize) -> Line<'static> {
    let mut spans = vec![Span::raw(" ".repeat(n))];
    spans.extend(line.spans);
    Line::from(spans)
}
