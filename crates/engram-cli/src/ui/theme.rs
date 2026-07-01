//! The visual language of the TUI — one cohesive palette plus the semantic
//! styles every view draws with. Calm, dark, high-signal: the brand indigo for
//! identity, green for the trust/ledger spine, amber for cost, cyan for tools.

use ratatui::style::{Color, Modifier, Style};

#[derive(Clone, Copy)]
pub struct Theme {
    /// Brand accent — the engram indigo. Headers, focus, the caret.
    pub accent: Color,
    /// Secondary accent — cyan. Tools, links, secondary emphasis.
    pub accent2: Color,
    /// Default foreground for body text.
    pub fg: Color,
    /// Muted text — timestamps, hints, secondary metadata.
    pub muted: Color,
    /// Very dim — borders at rest, scrollbar troughs.
    pub faint: Color,
    /// The user's voice in the chat transcript.
    pub user: Color,
    /// Trust / ledger-verified / success green.
    pub good: Color,
    /// Cost / caution amber.
    pub warn: Color,
    /// Errors and tamper.
    pub bad: Color,
    /// Background tint for code blocks and recessed panels.
    pub code_bg: Color,
    /// Background of the focused/selected row.
    pub sel_bg: Color,
    /// Surface tint for the header/footer bars.
    pub bar_bg: Color,
}

impl Default for Theme {
    fn default() -> Self {
        Theme::dark()
    }
}

impl Theme {
    pub fn dark() -> Self {
        Theme {
            accent: Color::Rgb(0x9d, 0x8c, 0xff),  // indigo
            accent2: Color::Rgb(0x5e, 0xd2, 0xd6), // teal/cyan
            fg: Color::Rgb(0xe8, 0xe8, 0xf0),
            muted: Color::Rgb(0x8a, 0x8a, 0x9c),
            faint: Color::Rgb(0x44, 0x44, 0x52),
            user: Color::Rgb(0x7d, 0xb4, 0xff), // soft blue
            good: Color::Rgb(0x59, 0xd9, 0x9a), // green
            warn: Color::Rgb(0xe7, 0xb4, 0x5b), // amber
            bad: Color::Rgb(0xf2, 0x6d, 0x6d),  // red
            code_bg: Color::Rgb(0x20, 0x20, 0x2c),
            sel_bg: Color::Rgb(0x2c, 0x2c, 0x3c),
            bar_bg: Color::Rgb(0x18, 0x18, 0x22),
        }
    }

    /// A higher-contrast variant for light terminals / accessibility.
    pub fn light() -> Self {
        Theme {
            accent: Color::Rgb(0x5b, 0x46, 0xe5),
            accent2: Color::Rgb(0x0d, 0x8f, 0x96),
            fg: Color::Rgb(0x1a, 0x1a, 0x22),
            muted: Color::Rgb(0x66, 0x66, 0x74),
            faint: Color::Rgb(0xc8, 0xc8, 0xd2),
            user: Color::Rgb(0x1f, 0x5f, 0xc4),
            good: Color::Rgb(0x1a, 0x9b, 0x63),
            warn: Color::Rgb(0xb5, 0x7d, 0x10),
            bad: Color::Rgb(0xcc, 0x36, 0x36),
            code_bg: Color::Rgb(0xee, 0xee, 0xf4),
            sel_bg: Color::Rgb(0xe2, 0xe2, 0xf0),
            bar_bg: Color::Rgb(0xf2, 0xf2, 0xf8),
        }
    }

    // ---- semantic styles --------------------------------------------------

    pub fn body(&self) -> Style {
        Style::default().fg(self.fg)
    }
    pub fn dim(&self) -> Style {
        Style::default().fg(self.muted)
    }
    pub fn faint_style(&self) -> Style {
        Style::default().fg(self.faint)
    }
    pub fn accent_style(&self) -> Style {
        Style::default().fg(self.accent)
    }
    pub fn title(&self) -> Style {
        Style::default()
            .fg(self.accent)
            .add_modifier(Modifier::BOLD)
    }
    pub fn good_style(&self) -> Style {
        Style::default().fg(self.good)
    }
    pub fn warn_style(&self) -> Style {
        Style::default().fg(self.warn)
    }
    pub fn bad_style(&self) -> Style {
        Style::default().fg(self.bad)
    }
    pub fn tool(&self) -> Style {
        Style::default().fg(self.accent2)
    }
    pub fn user_style(&self) -> Style {
        Style::default().fg(self.user).add_modifier(Modifier::BOLD)
    }
    pub fn selected(&self) -> Style {
        Style::default().bg(self.sel_bg).fg(self.fg)
    }
    pub fn bar(&self) -> Style {
        Style::default().bg(self.bar_bg).fg(self.fg)
    }
    pub fn link(&self) -> Style {
        Style::default()
            .fg(self.accent2)
            .add_modifier(Modifier::UNDERLINED)
    }
    pub fn code(&self) -> Style {
        Style::default().fg(self.accent2).bg(self.code_bg)
    }
}

/// A region's signature color (Identity / Semantic / Episodic / Procedural).
pub fn region_color(theme: &Theme, region: &str) -> Color {
    match region.to_ascii_lowercase().as_str() {
        "identity" => theme.accent,
        "semantic" => theme.accent2,
        "episodic" => theme.user,
        "procedural" => theme.good,
        _ => theme.muted,
    }
}
