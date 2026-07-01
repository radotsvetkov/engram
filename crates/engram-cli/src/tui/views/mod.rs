//! Per-view rendering and key handling. `render` paints the body for the active
//! view; `handle_key` lets list views consume navigation keys.

mod agents;
mod autonomy;
mod chat;
mod help;
mod ledger;
mod memory;
mod schedule;
mod settings;
mod skills;
mod tasks;

use super::app::{App, View};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::Frame;

pub fn render(app: &mut App, f: &mut Frame, area: Rect) {
    // Record the active list's length for wheel-scroll clamping.
    app.list_len = match app.view {
        View::Memory => app.memory_recent.len(),
        View::Skills => app.skills.len(),
        View::Schedule => app.schedule.len(),
        View::Autonomy => app.egress.len(),
        View::Ledger => app.ledger_tail.len(),
        View::Agents => app.agents.len(),
        View::Settings => settings::ROWS.len(),
        View::Tasks => app.tasks.len(),
        _ => 0,
    };
    match app.view {
        View::Chat => chat::render(app, f, area),
        View::Tasks => tasks::render(app, f, area),
        View::Memory => memory::render(app, f, area),
        View::Skills => skills::render(app, f, area),
        View::Schedule => schedule::render(app, f, area),
        View::Autonomy => autonomy::render(app, f, area),
        View::Ledger => ledger::render(app, f, area),
        View::Agents => agents::render(app, f, area),
        View::Settings => settings::render(app, f, area),
        View::Help => help::render(app, f, area),
    }
}

/// Returns true if the key was consumed by the view.
pub fn handle_key(app: &mut App, k: KeyEvent) -> bool {
    // Universal: '/' opens the palette and '?' opens Help from any list view.
    if let KeyCode::Char('/') = k.code {
        app.open_palette();
        return true;
    }
    if let KeyCode::Char('?') = k.code {
        app.set_view(View::Help);
        return true;
    }
    match app.view {
        View::Tasks => tasks::handle_key(app, k),
        View::Memory => memory::handle_key(app, k),
        View::Skills => skills::handle_key(app, k),
        View::Schedule => schedule::handle_key(app, k),
        View::Autonomy => autonomy::handle_key(app, k),
        View::Ledger => ledger::handle_key(app, k),
        View::Agents => agents::handle_key(app, k),
        View::Settings => settings::handle_key(app, k),
        View::Help => false,
        View::Chat => false,
    }
}

/// Extend a line with trailing spaces so its background (e.g. a selection
/// highlight) paints the full panel width, not just under the text.
pub fn fill_row(line: &mut ratatui::text::Line<'static>, width: usize) {
    use unicode_width::UnicodeWidthStr;
    let cur: usize = line.spans.iter().map(|s| s.content.width()).sum();
    if cur < width {
        line.spans
            .push(ratatui::text::Span::raw(" ".repeat(width - cur)));
    }
}

/// The first visible row so the selection stays on screen.
pub fn window_start(len: usize, height: usize, sel: usize) -> usize {
    if len <= height || height == 0 {
        return 0;
    }
    let half = height / 2;
    if sel < half {
        0
    } else if sel + (height - half) >= len {
        len - height
    } else {
        sel - half
    }
}

/// A standard bordered panel for a list view. The title is materialised into an
/// owned span, so the returned block borrows nothing (`'static`).
pub fn panel(t: &crate::ui::Theme, title: &str, count: usize) -> ratatui::widgets::Block<'static> {
    use ratatui::style::{Modifier, Style};
    use ratatui::text::Span;
    use ratatui::widgets::{Block, Borders};
    Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(t.faint))
        .title(Span::styled(
            format!(" {title} ({count}) "),
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
        ))
}
