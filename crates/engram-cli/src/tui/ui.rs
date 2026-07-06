//! Top-level frame composition: the trust-spine header, the per-view body, the
//! context footer, and the floating command palette.

use super::app::{App, View, PALETTE};
use crate::ui::format::{cost, human_count, spinner};
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

pub fn draw(f: &mut Frame, app: &mut App) {
    // The boot splash owns the whole frame until dismissed.
    if app.splash {
        super::splash::render(f, app);
        return;
    }
    let area = f.area();
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // header / trust spine
            Constraint::Min(3),    // body
            Constraint::Length(1), // footer
        ])
        .split(area);

    header(f, app, rows[0]);
    super::views::render(app, f, rows[1]);
    footer(f, app, rows[2]);

    if app.palette.is_some() {
        palette(f, app, area);
    }
    if app.sessions_open {
        sessions_picker(f, app, area);
    }
    if app.project_picker_open {
        project_picker(f, app, area);
    }
    if app.prompt_modal.is_some() {
        prompt_modal(f, app, area);
    }
    if app.form.is_some() {
        form_modal(f, app, area);
    }
}

/// A multi-field form overlay (agent create/edit, autonomy policy, schedule add).
fn form_modal(f: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    let Some(form) = &app.form else { return };
    let w = 72u16.min(area.width.saturating_sub(4));
    let h = (form.fields.len() as u16 * 2 + 4).min(area.height.saturating_sub(2));
    let rect = centered(area, w, h);
    f.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(t.accent))
        .title(Span::styled(
            format!(" {} ", form.title),
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(t.bar_bg));
    let inner = block.inner(rect);
    f.render_widget(block, rect);

    let mut lines: Vec<Line> = Vec::new();
    let label_w = 18usize;
    for (i, field) in form.fields.iter().enumerate() {
        let focused = i == form.sel;
        let label_style = if focused {
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(t.muted)
        };
        let mut spans = vec![
            Span::styled(
                if focused { "▌ " } else { "  " },
                Style::default().fg(t.accent),
            ),
            Span::styled(format!("{:<label_w$}", field.label), label_style),
        ];
        if field.value.is_empty() && !focused {
            spans.push(Span::styled(field.hint, Style::default().fg(t.faint)));
        } else if focused {
            let chars: Vec<char> = field.value.chars().collect();
            let cur = form.cursor.min(chars.len());
            spans.push(Span::styled(
                chars[..cur].iter().collect::<String>(),
                Style::default().fg(t.fg),
            ));
            spans.push(Span::styled("▏", Style::default().fg(t.accent)));
            spans.push(Span::styled(
                chars[cur..].iter().collect::<String>(),
                Style::default().fg(t.fg),
            ));
        } else {
            spans.push(Span::styled(field.value.clone(), Style::default().fg(t.fg)));
        }
        lines.push(Line::from(spans));
        if focused && !field.hint.is_empty() {
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::raw(" ".repeat(label_w)),
                Span::styled(field.hint, Style::default().fg(t.faint)),
            ]));
        } else {
            lines.push(Line::default());
        }
    }
    lines.push(Line::from(Span::styled(
        "↵ submit   Tab/↑↓ move   esc cancel",
        Style::default().fg(t.faint),
    )));
    f.render_widget(Paragraph::new(lines), inner);
}

/// A centered single-line text-input modal (model switch, …).
fn prompt_modal(f: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    let Some(p) = &app.prompt_modal else { return };
    let w = 64u16.min(area.width.saturating_sub(4));
    let rect = centered(area, w, 5);
    f.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(t.accent))
        .title(Span::styled(
            format!(" {} ", p.title),
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(t.bar_bg));
    let inner = block.inner(rect);
    f.render_widget(block, rect);

    let chars: Vec<char> = p.buffer.chars().collect();
    let cursor = p.cursor.min(chars.len());
    let before: String = chars[..cursor].iter().collect();
    let after: String = chars[cursor..].iter().collect();
    let input = Line::from(vec![
        Span::styled("› ", Style::default().fg(t.accent)),
        Span::styled(before, Style::default().fg(t.fg)),
        Span::styled(
            "▏",
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
        ),
        Span::styled(after, Style::default().fg(t.fg)),
    ]);
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(1)])
        .split(inner);
    f.render_widget(Paragraph::new(input), rows[0]);
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "↵ apply   esc cancel",
            Style::default().fg(t.faint),
        ))),
        rows[1],
    );
}

/// The session-resume picker overlay.
fn project_picker(f: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    let h = (app.projects.len() as u16 + 4)
        .min(area.height.saturating_sub(2))
        .max(5);
    let w = 76u16.min(area.width.saturating_sub(4));
    let rect = centered(area, w, h);
    f.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(t.accent))
        .title(Span::styled(
            " switch project · enter=select · n=new · esc=cancel ",
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(t.bar_bg));
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    if app.projects.is_empty() {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "  loading…",
                Style::default().fg(t.muted),
            ))),
            inner,
        );
        return;
    }
    let view_h = inner.height as usize;
    let start = super::views::window_start(app.projects.len(), view_h, app.project_sel);
    let namew = w.saturating_sub(32) as usize;
    let mut lines: Vec<Line> = Vec::new();
    for (i, p) in app.projects.iter().enumerate().skip(start).take(view_h) {
        let selected = i == app.project_sel;
        let active = Some(&p.id) == app.active_project.as_ref();
        let name = crate::ui::format::ellipsize(&p.name, namew);
        let dir = crate::ui::format::ellipsize(p.workdir.as_deref().unwrap_or("(shared)"), 26);
        lines.push(Line::from(vec![
            Span::styled(
                if selected { "▌ " } else { "  " },
                Style::default().fg(t.accent),
            ),
            Span::styled(
                if active { "● " } else { "  " },
                Style::default().fg(t.good),
            ),
            Span::styled(
                format!("{name:<namew$}"),
                Style::default()
                    .fg(if selected { t.fg } else { t.muted })
                    .add_modifier(if selected {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    }),
            ),
            Span::styled(format!("  {dir}"), Style::default().fg(t.faint)),
        ]));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

fn sessions_picker(f: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    let h = (app.sessions.len() as u16 + 4)
        .min(area.height.saturating_sub(2))
        .max(5);
    let w = 72u16.min(area.width.saturating_sub(4));
    let rect = centered(area, w, h);
    f.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(t.accent))
        .title(Span::styled(
            " resume a session ",
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(t.bar_bg));
    let inner = block.inner(rect);
    f.render_widget(block, rect);

    if app.sessions.is_empty() {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "  no past sessions yet",
                Style::default().fg(t.muted),
            ))),
            inner,
        );
        return;
    }

    let view_h = inner.height as usize;
    let start = super::views::window_start(app.sessions.len(), view_h, app.sessions_sel);
    let mut lines: Vec<Line> = Vec::new();
    for (i, s) in app.sessions.iter().enumerate().skip(start).take(view_h) {
        let selected = i == app.sessions_sel;
        let marker = if selected { "▌ " } else { "  " };
        let title = crate::ui::format::ellipsize(
            if s.title.is_empty() {
                "(untitled)"
            } else {
                &s.title
            },
            w.saturating_sub(24) as usize,
        );
        let mut line = Line::from(vec![
            Span::styled(marker, Style::default().fg(t.accent)),
            Span::styled(if s.fav { "★ " } else { "  " }, Style::default().fg(t.warn)),
            Span::styled(
                format!("{title:<width$}", width = w.saturating_sub(24) as usize),
                Style::default()
                    .fg(if selected { t.fg } else { t.muted })
                    .add_modifier(if selected {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    }),
            ),
            Span::styled(
                format!("  {:>3} msg  ", s.messages),
                Style::default().fg(t.faint),
            ),
            Span::styled(
                crate::ui::format::rel_time(s.updated_ms),
                Style::default().fg(t.muted),
            ),
        ]);
        if selected {
            line.style = Style::default().bg(t.sel_bg);
            super::views::fill_row(&mut line, inner.width as usize);
        }
        lines.push(line);
    }
    f.render_widget(Paragraph::new(lines), inner);
}

// ---- header / trust spine -------------------------------------------------

fn header(f: &mut Frame, app: &mut App, area: Rect) {
    let t = app.theme;
    f.render_widget(Block::default().style(t.bar()), area);

    // Left: brand + tabs. Record each tab's x-range so the mouse can click it.
    let brand = [" ✦ ", "engram", "   "];
    let mut left: Vec<Span> = vec![
        Span::styled(brand[0], Style::default().fg(t.accent)),
        Span::styled(
            brand[1],
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
        ),
        Span::raw(brand[2]),
    ];
    let mut x: u16 = brand.iter().map(|s| s.width() as u16).sum();
    let active_view = app.view;
    let mut hits: Vec<(View, u16, u16)> = Vec::new();
    for (i, v) in View::TABS.iter().enumerate() {
        let active = *v == active_view;
        let style = if active {
            Style::default()
                .fg(t.accent)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
        } else {
            Style::default().fg(t.muted)
        };
        let title = v.title();
        let tw = title.width() as u16;
        hits.push((*v, x, x + tw));
        x += tw;
        left.push(Span::styled(title, style));
        if i + 1 < View::TABS.len() {
            left.push(Span::styled("  ", Style::default().fg(t.faint)));
            x += 2;
        }
    }
    // Drop tabs that fall off the visible width (the spine can trim the left on a
    // narrow terminal) so a click never lands on a phantom tab.
    hits.retain(|(_, x0, _)| *x0 < area.width);
    app.tab_hits = hits;

    // Right: model · tokens · cost · ledger · connection — built as independent
    // groups in *keep priority* (connection + ledger are most important) so the
    // spine sheds low-value segments on a narrow terminal instead of overrunning.
    let (dot, label, color) = match &app.health {
        Some(h) if h.ok && h.offline => ("●", "offline", t.warn),
        Some(h) if h.ok => ("●", "live", t.good),
        _ => ("○", "down", t.bad),
    };
    let conn = vec![Span::styled(
        format!("{dot} {label} "),
        Style::default().fg(color),
    )];
    let ledger = match &app.ledger {
        Some(l) if l.ok => vec![Span::styled(
            format!("✓ ledger {}", human_count(l.entries)),
            Style::default().fg(t.good),
        )],
        Some(_) => vec![Span::styled(
            "✗ TAMPER",
            Style::default()
                .fg(t.bad)
                .add_modifier(Modifier::BOLD | Modifier::RAPID_BLINK),
        )],
        None => vec![Span::styled("ledger …", Style::default().fg(t.faint))],
    };
    let cost_g = vec![Span::styled(
        cost(app.meter.cost_usd),
        Style::default().fg(t.warn),
    )];
    let tokens = vec![Span::styled(
        format!(
            "{}↑/{}↓",
            human_count(app.meter.tokens_in),
            human_count(app.meter.tokens_out)
        ),
        Style::default().fg(t.faint),
    )];
    let model = if app.model.is_empty() {
        vec![]
    } else {
        vec![Span::styled(
            crate::ui::format::ellipsize(&app.model, 24),
            Style::default().fg(t.muted),
        )]
    };

    // Highest keep-priority first.
    let groups: Vec<Vec<Span>> = vec![conn, ledger, cost_g, tokens, model]
        .into_iter()
        .filter(|g| !g.is_empty())
        .collect();
    let lw: usize = left.iter().map(|s| s.content.width()).sum();
    let budget = (area.width as usize).saturating_sub(lw.min(area.width as usize / 2) + 2);
    let sep_w = 3usize;
    let mut chosen: Vec<Vec<Span>> = Vec::new();
    let mut used = 0usize;
    for g in groups {
        let gw: usize = g.iter().map(|s| s.content.width()).sum();
        let add = gw + if chosen.is_empty() { 0 } else { sep_w };
        if used + add <= budget || chosen.is_empty() {
            used += add;
            chosen.push(g);
        }
    }
    // chosen is in keep-priority (conn first); display left→right is the reverse.
    chosen.reverse();
    let mut right: Vec<Span> = Vec::new();
    for (i, g) in chosen.into_iter().enumerate() {
        if i > 0 {
            right.push(sep(&t));
        }
        right.extend(g);
    }
    right.push(Span::raw(" "));

    render_split_bar(f, area, left, right, t.bar());
}

fn sep(t: &crate::ui::Theme) -> Span<'static> {
    Span::styled(" · ", Style::default().fg(t.faint))
}

/// Render `left` spans flush-left and `right` spans flush-right on one bar row.
fn render_split_bar(f: &mut Frame, area: Rect, left: Vec<Span>, right: Vec<Span>, bar: Style) {
    let w = area.width as usize;
    let lw: usize = left.iter().map(|s| s.content.width()).sum();
    let rw: usize = right.iter().map(|s| s.content.width()).sum();
    let mut spans = left;
    if lw + rw < w {
        spans.push(Span::styled(" ".repeat(w - lw - rw), bar));
        spans.extend(right);
    } else if rw >= w {
        // The right spine alone doesn't fit — keep only the rightmost (highest
        // priority) spans that fit, right-aligned, dropping the left entirely.
        let mut kept: Vec<Span> = Vec::new();
        let mut acc = 0;
        for s in right.into_iter().rev() {
            let sw = s.content.width();
            if acc + sw > w {
                break;
            }
            acc += sw;
            kept.push(s);
        }
        kept.reverse();
        let mut out: Vec<Span> = Vec::new();
        if acc < w {
            out.push(Span::styled(" ".repeat(w - acc), bar));
        }
        out.extend(kept);
        spans = out;
    } else {
        // Not enough room — keep the right spine (more important), trim the left.
        let keep = w.saturating_sub(rw + 1);
        let mut acc = 0;
        let mut trimmed = Vec::new();
        for s in spans {
            let sw = s.content.width();
            if acc + sw > keep {
                break;
            }
            acc += sw;
            trimmed.push(s);
        }
        trimmed.push(Span::styled(" ".repeat(w.saturating_sub(acc + rw)), bar));
        trimmed.extend(right);
        spans = trimmed;
    }
    f.render_widget(Paragraph::new(Line::from(spans)).style(bar), area);
}

// ---- footer ---------------------------------------------------------------

fn footer(f: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    f.render_widget(Block::default().style(t.bar()), area);

    let left: Vec<Span> = if let Some((msg, _)) = &app.toast {
        vec![Span::styled(
            format!(" {msg} "),
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
        )]
    } else {
        vec![Span::styled(footer_hint(app), Style::default().fg(t.muted))]
    };

    let mut right: Vec<Span> = Vec::new();
    // Always show the active project in chat, so it's clear which world the chat is scoped to
    // (^O switches it). This is the visible half of project isolation in the TUI.
    if app.view == View::Chat && !app.projects.is_empty() {
        right.push(Span::styled(
            format!(
                "⬡ {} ",
                crate::ui::format::ellipsize(&app.active_project_name(), 16)
            ),
            Style::default().fg(t.accent2).add_modifier(Modifier::BOLD),
        ));
    }
    if app.chat.streaming {
        right.push(Span::styled(
            format!("{} thinking ", spinner(app.tick)),
            Style::default().fg(t.accent),
        ));
    } else if let Some(sid) = &app.chat.session {
        if app.view == View::Chat {
            right.push(Span::styled(
                format!("{} ", crate::ui::format::ellipsize(sid, 18)),
                Style::default().fg(t.faint),
            ));
        }
    }
    render_split_bar(f, area, left, right, t.bar());
}

/// Per-view footer key hints, mirroring each view's real bindings.
fn footer_hint(app: &App) -> &'static str {
    match app.view {
        View::Chat => {
            if app.chat.streaming {
                " esc stop   ↑↓ scroll   ^P palette   ^C quit"
            } else {
                " ↵ send   ^O project   ^R sessions   ^T task   ^A attach   / cmds   ? help"
            }
        }
        View::Tasks => " ↑↓ move   ←→ column   ↵ run/open   r refresh   / cmds   ? help",
        View::Memory => " ↑↓ move   f forget (×2)   r refresh   / cmds   ? help",
        View::Skills => " ↑↓ move   ↵ toggle on/off   a adopt proposed   r refresh   ? help",
        View::Schedule => " ↑↓ move   a add   e edit   ↵ run   d delete   r refresh   ? help",
        View::Autonomy => " ↑↓ move   a approve   d deny   r refresh   ? help",
        View::Ledger => " ↑↓ move   r refresh   / cmds   ? help",
        View::Agents => {
            " ↑↓ move   n new   e edit   p policy   c self-model   d delete (×2)   r refresh"
        }
        View::Settings => {
            " ↑↓ move   ↵ edit/toggle (fields·MCP·tools)   x clear secret   d del MCP   t test"
        }
        View::Help => " esc back   ^C quit",
    }
}

// ---- command palette ------------------------------------------------------

fn palette(f: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    let items = app.palette_items();
    let Some(p) = &app.palette else { return };

    let h = (items.len() as u16 + 4)
        .min(area.height.saturating_sub(2))
        .max(5);
    let w = 60u16.min(area.width.saturating_sub(4));
    let rect = centered(area, w, h);

    f.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(t.accent))
        .title(Span::styled(
            " commands ",
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(t.bar_bg));
    let inner = block.inner(rect);
    f.render_widget(block, rect);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(1),
        ])
        .split(inner);

    // Query line.
    let query = Line::from(vec![
        Span::styled("› ", Style::default().fg(t.accent)),
        Span::styled(p.query.clone(), Style::default().fg(t.fg)),
        Span::styled("▏", Style::default().fg(t.accent)),
    ]);
    f.render_widget(Paragraph::new(query), chunks[0]);
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "─".repeat(inner.width as usize),
            Style::default().fg(t.faint),
        ))),
        chunks[1],
    );

    // Items.
    let mut lines: Vec<Line> = Vec::new();
    let view_h = chunks[2].height as usize;
    let start = super::views::window_start(items.len(), view_h, p.sel);
    for (row, &idx) in items.iter().enumerate().skip(start).take(view_h) {
        let it = &PALETTE[idx];
        let selected = row == p.sel;
        let marker = if selected { "▌ " } else { "  " };
        let label_style = if selected {
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(t.fg)
        };
        let spans = vec![
            Span::styled(marker, Style::default().fg(t.accent)),
            Span::styled(format!("{:<16}", it.label), label_style),
            Span::styled(it.hint, Style::default().fg(t.muted)),
        ];
        let mut line = Line::from(spans);
        if selected {
            line.style = Style::default().bg(t.sel_bg);
            super::views::fill_row(&mut line, inner.width as usize);
        }
        lines.push(line);
    }
    if items.is_empty() {
        lines.push(Line::from(Span::styled(
            "  no matching command",
            Style::default().fg(t.muted),
        )));
    }
    f.render_widget(Paragraph::new(lines), chunks[2]);
}

/// A centered sub-rect of `w`×`h`.
pub fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    Rect {
        x,
        y,
        width: w.min(area.width),
        height: h.min(area.height),
    }
}

/// Shared helper: a section title line.
pub fn section<'a>(t: &crate::ui::Theme, title: &'a str) -> Line<'a> {
    Line::from(vec![
        Span::styled("▌ ", Style::default().fg(t.accent)),
        Span::styled(
            title,
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
        ),
    ])
}

/// Shared helper: an empty-state placeholder centered-ish in `area`.
pub fn empty_state(f: &mut Frame, t: &crate::ui::Theme, area: Rect, msg: &str) {
    let p = Paragraph::new(Line::from(Span::styled(msg, Style::default().fg(t.muted))))
        .alignment(Alignment::Center);
    let mid = Rect {
        x: area.x,
        y: area.y + area.height / 2,
        width: area.width,
        height: 1,
    };
    f.render_widget(p, mid);
}
