//! The Help view — keys, the command palette, and what makes Engram different.

use crate::tui::app::App;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

pub fn render(app: &mut App, f: &mut Frame, area: Rect) {
    let t = &app.theme;
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(t.faint))
        .title(Span::styled(
            " Help ",
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut lines: Vec<Line> = Vec::new();
    let head = |s: &str| {
        Line::from(Span::styled(
            s.to_string(),
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
        ))
    };
    let key = |k: &str, d: &str| {
        Line::from(vec![
            Span::styled(format!("  {k:<14}"), Style::default().fg(t.accent2)),
            Span::styled(d.to_string(), Style::default().fg(t.fg)),
        ])
    };

    lines.push(head("Global"));
    lines.push(key("Ctrl-P / Ctrl-K", "open the command palette"));
    lines.push(key(
        "/",
        "open the palette (from chat when empty, or any list)",
    ));
    lines.push(key("Alt-1 … Alt-9", "jump to a tab"));
    lines.push(key("F1", "this help"));
    lines.push(key("Ctrl-C / Ctrl-Q", "quit"));
    lines.push(Line::default());

    lines.push(head("Chat"));
    lines.push(key("Enter", "send your message"));
    lines.push(key("Ctrl-T", "turn the composer into a task and run it"));
    lines.push(key("Ctrl-A", "attach a file to the next message"));
    lines.push(key("Ctrl-R", "resume a past session"));
    lines.push(key("Esc", "stop a streaming run"));
    lines.push(key("↑ ↓ / PgUp PgDn", "scroll the transcript"));
    lines.push(key("Ctrl-U / Ctrl-W", "clear line / delete word"));
    lines.push(Line::default());

    lines.push(head("Lists (Tasks, Memory, Skills, …)"));
    lines.push(key("↑ ↓ / j k", "move selection"));
    lines.push(key("← → / h l", "switch kanban column (Tasks)"));
    lines.push(key("Enter", "run / open / approve / edit"));
    lines.push(key("r", "refresh"));
    lines.push(key("f", "forget the selected memory (×2)"));
    lines.push(key(
        "Skills: a",
        "adopt a ◆ proposed skill (replays its gold examples)",
    ));
    lines.push(key("Autonomy: a / d", "approve / deny a staged egress"));
    lines.push(Line::default());

    lines.push(head("Settings & Agents"));
    lines.push(key(
        "Settings: Enter",
        "edit a value, toggle a flag/tool, or cycle an option",
    ));
    lines.push(key("Settings: t", "test the model provider"));
    lines.push(key(
        "Settings: tools",
        "the Agent tools section toggles each tool on/off",
    ));
    lines.push(key("Agents: n / d", "create a new agent / delete (×2)"));
    lines.push(Line::default());

    lines.push(head("What makes Engram different"));
    for s in [
        "• Every action is signed into an append-only ledger — the header chip proves it's intact.",
        "• Answers are grounded in a hybrid (keyword + semantic) memory; the recall ribbon shows why.",
        "• Skills are small programs that rewrite themselves when a measured A/B beats the incumbent.",
        "• Egress is gated on the lethal trifecta, so a prompt-injected run can't exfiltrate your data.",
        "• It sleeps to zero when idle — nothing resident between requests on a socket-activated host.",
    ] {
        lines.push(Line::from(Span::styled(
            format!("  {s}"),
            Style::default().fg(t.muted),
        )));
    }
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        "  press esc or a tab key to return",
        Style::default().fg(t.faint),
    )));

    f.render_widget(Paragraph::new(Text::from(lines)), inner);
}
