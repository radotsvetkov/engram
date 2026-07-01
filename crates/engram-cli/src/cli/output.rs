//! Pretty terminal output for the non-interactive CLI.
//!
//! The trick: we reuse the very same ratatui markdown renderer the TUI uses,
//! then transcode the styled `Line`/`Span` output into ANSI SGR sequences. One
//! renderer, two surfaces — the CLI's `engram ask` looks identical to the TUI's
//! chat pane. Respects `NO_COLOR` and non-tty pipes.

use crate::ui::markdown;
use crate::ui::theme::Theme;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use std::io::IsTerminal;
use std::sync::OnceLock;

pub fn color_enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        if std::env::var_os("NO_COLOR").is_some() {
            return false;
        }
        if std::env::var("CLICOLOR_FORCE")
            .map(|v| v != "0")
            .unwrap_or(false)
        {
            return true;
        }
        std::io::stdout().is_terminal()
    })
}

pub fn term_width() -> u16 {
    crossterm::terminal::size()
        .map(|(w, _)| w)
        .unwrap_or(100)
        .clamp(40, 120)
}

fn sgr_for(style: Style) -> (String, bool) {
    if !color_enabled() {
        return (String::new(), false);
    }
    let mut codes: Vec<String> = Vec::new();
    if let Some(fg) = style.fg {
        if let Some(c) = ansi_color(fg, false) {
            codes.push(c);
        }
    }
    if let Some(bg) = style.bg {
        if let Some(c) = ansi_color(bg, true) {
            codes.push(c);
        }
    }
    let m = style.add_modifier;
    if m.contains(Modifier::BOLD) {
        codes.push("1".into());
    }
    if m.contains(Modifier::DIM) {
        codes.push("2".into());
    }
    if m.contains(Modifier::ITALIC) {
        codes.push("3".into());
    }
    if m.contains(Modifier::UNDERLINED) {
        codes.push("4".into());
    }
    if m.contains(Modifier::CROSSED_OUT) {
        codes.push("9".into());
    }
    if codes.is_empty() {
        (String::new(), false)
    } else {
        (format!("\x1b[{}m", codes.join(";")), true)
    }
}

fn ansi_color(c: Color, bg: bool) -> Option<String> {
    let base = if bg { 48 } else { 38 };
    match c {
        Color::Rgb(r, g, b) => Some(format!("{base};2;{r};{g};{b}")),
        Color::Black => Some(format!("{};5;0", base)),
        Color::Red => Some(format!("{};5;1", base)),
        Color::Green => Some(format!("{};5;2", base)),
        Color::Yellow => Some(format!("{};5;3", base)),
        Color::Blue => Some(format!("{};5;4", base)),
        Color::Magenta => Some(format!("{};5;5", base)),
        Color::Cyan => Some(format!("{};5;6", base)),
        Color::Gray => Some(format!("{};5;7", base)),
        Color::DarkGray => Some(format!("{};5;8", base)),
        Color::Reset => None,
        _ => None,
    }
}

/// Transcode a single styled span into an ANSI-wrapped string.
pub fn span_to_ansi(span: &Span) -> String {
    let (open, on) = sgr_for(span.style);
    if on {
        format!("{open}{}\x1b[0m", span.content)
    } else {
        span.content.to_string()
    }
}

/// Transcode a styled line.
pub fn line_to_ansi(line: &Line) -> String {
    line.spans.iter().map(span_to_ansi).collect()
}

/// Render a Markdown document straight to stdout, ANSI-styled and wrapped.
pub fn print_markdown(src: &str) {
    let theme = Theme::dark();
    let width = term_width();
    for line in markdown::render(src, width, &theme) {
        println!("{}", line_to_ansi(&line));
    }
}

/// Apply a style to plain text → an ANSI string (no markdown).
pub fn paint(text: &str, style: Style) -> String {
    span_to_ansi(&Span::styled(text.to_string(), style))
}

// ---- semantic shortcuts ---------------------------------------------------

pub fn bold(s: &str) -> String {
    paint(s, Style::default().add_modifier(Modifier::BOLD))
}
pub fn dim(s: &str) -> String {
    paint(s, Style::default().fg(Theme::dark().muted))
}
pub fn accent(s: &str) -> String {
    paint(
        s,
        Style::default()
            .fg(Theme::dark().accent)
            .add_modifier(Modifier::BOLD),
    )
}
pub fn good(s: &str) -> String {
    paint(s, Style::default().fg(Theme::dark().good))
}
pub fn warn(s: &str) -> String {
    paint(s, Style::default().fg(Theme::dark().warn))
}
pub fn bad(s: &str) -> String {
    paint(s, Style::default().fg(Theme::dark().bad))
}
pub fn tool(s: &str) -> String {
    paint(s, Style::default().fg(Theme::dark().accent2))
}

/// A compact key/value line: `  key  value`.
pub fn kv(key: &str, value: &str) {
    println!("  {:<16} {}", dim(key), value);
}

/// Section header.
pub fn header(title: &str) {
    println!("\n{}", accent(&format!("▌ {title}")));
}

/// A simple fixed-width table with a dim header row.
pub fn table(headers: &[&str], rows: &[Vec<String>]) {
    if rows.is_empty() {
        println!("  {}", dim("(none)"));
        return;
    }
    let cols = headers.len();
    let mut w = vec![0usize; cols];
    for (i, h) in headers.iter().enumerate() {
        w[i] = h.chars().count();
    }
    for r in rows {
        for (i, c) in r.iter().enumerate().take(cols) {
            w[i] = w[i].max(c.chars().count().min(60));
        }
    }
    let hdr: Vec<String> = headers
        .iter()
        .enumerate()
        .map(|(i, h)| dim(&format!("{:<width$}", h, width = w[i])))
        .collect();
    println!("  {}", hdr.join("  "));
    for r in rows {
        let cells: Vec<String> = (0..cols)
            .map(|i| {
                let c = r.get(i).cloned().unwrap_or_default();
                let c = crate::ui::format::ellipsize(&c, 60);
                format!("{:<width$}", c, width = w[i])
            })
            .collect();
        println!("  {}", cells.join("  "));
    }
}
