//! The boot splash — the Engram logomark as terminal pixel art.
//!
//! A faithful pixelation of `assets/brand/logo.svg`: the neuron (an engram is
//! a memory trace) — soma with nucleus, three synapses equally spaced about
//! it, the lower-right one firing in the brand teal with a spark halo. Drawn
//! while the client connects to the daemon; any key (or the daemon answering)
//! dismisses it. The bitmap renders at two vertical pixels per terminal row
//! using half-block glyphs, so it stays crisp in any font and never depends on
//! the terminal's background color.

use crate::tui::app::App;
use crate::ui::format::spinner;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

/// The neuron mark, pixelated from the 24×24 SVG. `w` = stroke (foreground),
/// `g` = the firing synapse (brand teal #45c8a8), `h` = its spark halo,
/// `.` = transparent. Row pairs collapse into half-block terminal rows.
const NEURON: [&str; 20] = [
    "............www............", // top synapse
    "...........w...w...........",
    "...........w...w...........",
    "...........w...w...........",
    "............www............",
    ".............w.............", // axon
    ".............w.............",
    "............www............", // soma
    "...........w...w...........",
    "..........w.....w..........",
    ".........w...w...w.........", // nucleus row
    ".........w..www..w...h.....",
    ".........w...w...w......h..",
    "....www...w.....w...ggg....", // dendrites reach the synapses
    "...w...w.w.w...w.w.ggggg...",
    "...w...ww...www...wggggg.h.",
    "...w...w...........ggggg...",
    "....www.............ggg....",
    "........................h..",
    ".....................h.....",
];

/// The wordmark, pre-drawn in block glyphs (2 rows × 26 cols).
const WORDMARK: [&str; 2] = ["█▀▀ █▄ █ █▀▀ █▀█ ▄▀█ █▀▄▀█", "██▄ █ ▀█ █▄█ █▀▄ █▀█ █ ▀ █"];

/// The brand accent from the logo — the firing synapse teal.
const SYNAPSE: Color = Color::Rgb(0x45, 0xc8, 0xa8);

pub fn render(f: &mut Frame, app: &App) {
    let t = &app.theme;
    let area = f.area();
    // The halo reads as a soft glow: darker teal on dark, minted on light.
    let halo = if app.light {
        Color::Rgb(0x9f, 0xdc, 0xcc)
    } else {
        Color::Rgb(0x2f, 0x74, 0x63)
    };

    // Assemble the content lines top-to-bottom, then center the block.
    let mut lines: Vec<Line> = Vec::new();
    // Two bitmap rows collapse into one terminal row; the full layout is
    // mark + blank + 2 wordmark + blank + tagline + blank + status + blank +
    // hint = mark_rows + 9 lines.
    let mark_rows = NEURON.len() / 2;
    let want_full = mark_rows + 9;
    let show_mark = area.height as usize >= want_full && area.width >= 30;
    let show_wordmark = area.height >= 8 && area.width >= 30;

    if show_mark {
        for pair in NEURON.chunks(2) {
            lines.push(halfblock_row(pair[0], pair[1], t.fg, halo));
        }
        lines.push(Line::default());
    }
    if show_wordmark {
        for row in WORDMARK {
            lines.push(Line::from(Span::styled(
                row,
                Style::default().fg(t.fg).add_modifier(Modifier::BOLD),
            )));
        }
    } else {
        lines.push(Line::from(vec![
            Span::styled("✦ ", Style::default().fg(SYNAPSE)),
            Span::styled(
                "engram",
                Style::default().fg(t.fg).add_modifier(Modifier::BOLD),
            ),
        ]));
    }
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        "your self-improving personal agent",
        Style::default().fg(t.muted),
    )));

    // Connection status: live once the daemon has answered, else a spinner.
    lines.push(Line::default());
    lines.push(match &app.health {
        Some(h) if h.ok => Line::from(Span::styled(
            format!("● connected · engramd v{}", h.version),
            Style::default().fg(t.good),
        )),
        _ => Line::from(Span::styled(
            format!("{} waking the daemon…", spinner(app.tick)),
            Style::default().fg(t.muted),
        )),
    });
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        "press any key",
        Style::default().fg(t.faint),
    )));

    let h = lines.len() as u16;
    let y = area.y + area.height.saturating_sub(h) / 2;
    let rect = Rect {
        x: area.x,
        y,
        width: area.width,
        height: h.min(area.height),
    };
    f.render_widget(Paragraph::new(lines).alignment(Alignment::Center), rect);
}

/// Render two bitmap rows as one terminal row of half-blocks. `top`/`bottom`
/// must be the same length; a `.` is transparent (terminal background).
fn halfblock_row(top: &str, bottom: &str, stroke: Color, halo: Color) -> Line<'static> {
    let color = |c: char| -> Option<Color> {
        match c {
            'w' => Some(stroke),
            'g' => Some(SYNAPSE),
            'h' => Some(halo),
            _ => None,
        }
    };
    let mut spans: Vec<Span> = Vec::with_capacity(top.len());
    for (tc, bc) in top.chars().zip(bottom.chars()) {
        let span = match (color(tc), color(bc)) {
            (None, None) => Span::raw(" "),
            (Some(a), None) => Span::styled("▀", Style::default().fg(a)),
            (None, Some(b)) => Span::styled("▄", Style::default().fg(b)),
            (Some(a), Some(b)) => Span::styled("▀", Style::default().fg(a).bg(b)),
        };
        spans.push(span);
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bitmap must pair cleanly into half-block rows: an even row count,
    /// every row the same width, and only known palette characters.
    #[test]
    fn neuron_bitmap_is_well_formed() {
        assert_eq!(NEURON.len() % 2, 0, "row count must be even");
        let w = NEURON[0].chars().count();
        for row in NEURON {
            assert_eq!(row.chars().count(), w, "ragged row: {row:?}");
            assert!(
                row.chars().all(|c| matches!(c, '.' | 'w' | 'g' | 'h')),
                "unknown palette char in {row:?}"
            );
        }
        // Both wordmark rows are the same width too.
        assert_eq!(
            WORDMARK[0].chars().count(),
            WORDMARK[1].chars().count(),
            "ragged wordmark"
        );
    }
}
