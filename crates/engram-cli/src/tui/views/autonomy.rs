//! The Autonomy view — the egress policy report plus staged actions awaiting
//! your approval. Approve (a / ↵) or deny (d) a destination inline.

use crate::tui::app::App;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

pub fn render(app: &mut App, f: &mut Frame, area: Rect) {
    let t = app.theme;
    app.clamp_sel(app.egress.len());
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(8), Constraint::Min(3)])
        .split(area);

    // Report.
    let block = super::panel(&t, "Autonomy report", 0);
    let inner = block.inner(rows[0]);
    f.render_widget(block, rows[0]);
    let mut lines: Vec<Line> = Vec::new();
    if let Some(r) = &app.autonomy {
        let tt = &r.totals;
        let stat = |label: &str, n: u64, color| {
            Span::styled(format!(" {label} {n}  "), Style::default().fg(color))
        };
        lines.push(Line::from(vec![
            Span::raw("  "),
            stat("sent", tt.autonomous_sends, t.good),
            stat("allowlisted", tt.allowlisted, t.accent2),
            stat("staged", tt.staged, t.warn),
            stat("refused", tt.refused, t.muted),
            stat("denied", tt.denied, t.bad),
        ]));
        lines.push(Line::default());
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled("one-time approvals ", Style::default().fg(t.muted)),
            Span::styled(r.one_time_approvals.to_string(), Style::default().fg(t.fg)),
        ]));
        lines.push(Line::default());
        lines.push(Line::from(Span::styled(
            "  Egress is gated on the lethal trifecta: a run that read untrusted input",
            Style::default().fg(t.faint),
        )));
        lines.push(Line::from(Span::styled(
            "  and holds private data cannot send — staged actions wait for you here.",
            Style::default().fg(t.faint),
        )));
    } else {
        lines.push(Line::from(Span::styled(
            "  loading…",
            Style::default().fg(t.muted),
        )));
    }
    f.render_widget(Paragraph::new(Text::from(lines)), inner);

    // Pending approvals.
    let block = super::panel(&t, "Pending egress", app.egress.len());
    let inner = block.inner(rows[1]);
    f.render_widget(block, rows[1]);
    if app.egress.is_empty() {
        crate::tui::ui::empty_state(
            f,
            &t,
            inner,
            "Nothing staged — the agent isn't waiting on any approvals.",
        );
        return;
    }
    let mut lines: Vec<Line> = Vec::new();
    for (i, e) in app.egress.iter().enumerate() {
        let selected = i == app.sel;
        let bar = if selected { "▌ " } else { "  " };
        lines.push(Line::from(vec![
            Span::styled(bar, Style::default().fg(t.accent)),
            Span::styled("⚠ ", Style::default().fg(t.warn)),
            Span::styled(format!("{} ", e.tool), Style::default().fg(t.accent2)),
            Span::styled("→ ", Style::default().fg(t.muted)),
            Span::styled(
                e.dest.clone(),
                Style::default().fg(t.fg).add_modifier(if selected {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
            ),
        ]));
        lines.push(Line::from(vec![
            Span::raw("    "),
            Span::styled(
                format!("{}  ·  {}", e.scope, e.reason),
                Style::default().fg(t.faint),
            ),
        ]));
    }
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        "  a / ↵ approve (allowlist)    d deny",
        Style::default().fg(t.muted),
    )));
    f.render_widget(Paragraph::new(Text::from(lines)), inner);
}

pub fn handle_key(app: &mut App, k: KeyEvent) -> bool {
    let len = app.egress.len();
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
        KeyCode::Enter | KeyCode::Char('a') => {
            resolve(app, true);
            true
        }
        KeyCode::Char('d') => {
            resolve(app, false);
            true
        }
        _ => false,
    }
}

fn resolve(app: &mut App, approve: bool) {
    if let Some(e) = app.egress.get(app.sel) {
        let (scope, dest) = (e.scope.clone(), e.dest.clone());
        let client = app.client.clone();
        let tx = app.tx.clone();
        tokio::spawn(async move {
            if approve {
                let _ = client.egress_approve(&scope, &dest).await;
            } else {
                let _ = client.egress_deny(&scope, &dest).await;
            }
            if let Ok(p) = client.egress_pending().await {
                let _ = tx.send(crate::tui::app::Msg::Egress(p.pending));
            }
        });
        app.toast(if approve { "· approved" } else { "· denied" });
    }
}
