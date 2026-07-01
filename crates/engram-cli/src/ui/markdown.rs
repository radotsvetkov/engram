//! A compact Markdown → ratatui renderer.
//!
//! Not a spec-complete parser — a pragmatic one tuned for what an agent
//! actually emits: headings, bold/italic/strike, inline + fenced code, bullet
//! and ordered lists, block quotes, horizontal rules, links, and light tables.
//! Everything is width-aware: logical lines wrap to the available columns with
//! a hanging indent so lists and quotes stay aligned.

use super::theme::Theme;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

/// Render a Markdown document into wrapped, styled lines for `width` columns.
pub fn render(src: &str, width: u16, theme: &Theme) -> Vec<Line<'static>> {
    let width = width.max(8) as usize;
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut in_fence = false;
    let mut fence_lang = String::new();
    let mut fence_diff = false;
    let mut table_buf: Vec<String> = Vec::new();

    let flush_table = |buf: &mut Vec<String>, out: &mut Vec<Line<'static>>| {
        if !buf.is_empty() {
            render_table(buf, width, theme, out);
            buf.clear();
        }
    };

    for raw in src.replace('\t', "    ").lines() {
        let line = raw.to_string();
        let trimmed = line.trim_start();

        // Fenced code blocks. Every row of the block tints the same `cap` columns.
        let cap = width.min(80);
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            if in_fence {
                in_fence = false;
                out.push(Line::from(Span::styled(
                    " ".repeat(cap),
                    Style::default().bg(theme.code_bg),
                )));
            } else {
                flush_table(&mut table_buf, &mut out);
                in_fence = true;
                fence_lang = trimmed.trim_start_matches(['`', '~']).trim().to_string();
                fence_diff = matches!(fence_lang.as_str(), "diff" | "patch");
                let label = format!(" {fence_lang} ");
                let pad = cap.saturating_sub(label.width());
                out.push(Line::from(Span::styled(
                    format!("{label}{}", " ".repeat(pad)),
                    Style::default().fg(theme.muted).bg(theme.code_bg),
                )));
            }
            continue;
        }
        if in_fence {
            // Syntax/diff highlight the line; the line's background tints the gutter.
            let mut spans = vec![Span::raw(" ")];
            if fence_diff {
                spans.extend(diff_spans(&line, theme));
            } else {
                spans.extend(highlight_code(&line, &fence_lang, theme));
            }
            let used: usize = spans.iter().map(|s| s.content.width()).sum();
            spans.push(Span::raw(" ".repeat(cap.saturating_sub(used))));
            let mut l = Line::from(spans);
            l.style = Style::default().fg(theme.fg).bg(theme.code_bg);
            out.push(l);
            continue;
        }

        // Table accumulation: consecutive lines that look like table rows.
        if is_table_row(trimmed) {
            table_buf.push(trimmed.to_string());
            continue;
        } else {
            flush_table(&mut table_buf, &mut out);
        }

        // Blank line.
        if trimmed.is_empty() {
            out.push(Line::default());
            continue;
        }

        // Horizontal rule.
        if is_hr(trimmed) {
            out.push(Line::from(Span::styled(
                "─".repeat(width.min(60)),
                Style::default().fg(theme.faint),
            )));
            continue;
        }

        // Headings.
        if let Some((level, text)) = heading(trimmed) {
            let prefix = match level {
                1 => "▌ ",
                2 => "▎ ",
                _ => "· ",
            };
            let style = match level {
                1 => Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
                2 => Style::default()
                    .fg(theme.accent2)
                    .add_modifier(Modifier::BOLD),
                _ => Style::default().fg(theme.fg).add_modifier(Modifier::BOLD),
            };
            let first = vec![Span::styled(prefix, Style::default().fg(theme.accent))];
            let words = to_words(parse_inline(text, theme, style));
            wrap(&words, width, first, vec![Span::raw("  ")], &mut out);
            continue;
        }

        // Block quote.
        if let Some(rest) = trimmed.strip_prefix('>') {
            let q = rest.trim_start();
            let bar = Span::styled("▏ ", Style::default().fg(theme.accent));
            let words = to_words(parse_inline(q, theme, Style::default().fg(theme.muted)));
            wrap(
                &words,
                width,
                vec![bar.clone()],
                vec![Span::styled("▏ ", Style::default().fg(theme.accent))],
                &mut out,
            );
            continue;
        }

        // List items (bullet or ordered), preserving nesting indent.
        if let Some((indent, marker, text)) = list_item(&line) {
            let pad = " ".repeat(indent);
            let bullet = Span::styled(
                format!("{pad}{marker} "),
                Style::default().fg(theme.accent2),
            );
            let cont = Span::raw(format!("{pad}{} ", " ".repeat(marker.width())));
            let words = to_words(parse_inline(text, theme, theme.body()));
            wrap(&words, width, vec![bullet], vec![cont], &mut out);
            continue;
        }

        // Paragraph.
        let words = to_words(parse_inline(trimmed, theme, theme.body()));
        wrap(&words, width, vec![], vec![], &mut out);
    }
    flush_table(&mut table_buf, &mut out);
    out
}

// ---- block detectors ------------------------------------------------------

fn heading(s: &str) -> Option<(usize, &str)> {
    if !s.starts_with('#') {
        return None;
    }
    let level = s.chars().take_while(|&c| c == '#').count();
    if level == 0 || level > 6 {
        return None;
    }
    let rest = s[level..].trim_start();
    if rest.is_empty() {
        return None;
    }
    Some((level, rest))
}

fn is_hr(s: &str) -> bool {
    let t: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    t.len() >= 3
        && (t.chars().all(|c| c == '-')
            || t.chars().all(|c| c == '*')
            || t.chars().all(|c| c == '_'))
}

fn list_item(line: &str) -> Option<(usize, String, &str)> {
    let indent = line.len() - line.trim_start().len();
    let indent = indent.min(8);
    let t = line.trim_start();
    // Bullet.
    for m in ['-', '*', '+'] {
        if let Some(rest) = t.strip_prefix(m) {
            if rest.starts_with(' ') {
                return Some((indent, "•".to_string(), rest.trim_start()));
            }
        }
    }
    // Ordered: digits then '.' or ')'.
    let digits: String = t.chars().take_while(|c| c.is_ascii_digit()).collect();
    if !digits.is_empty() {
        let after = &t[digits.len()..];
        if let Some(rest) = after
            .strip_prefix(". ")
            .or_else(|| after.strip_prefix(") "))
        {
            return Some((indent, format!("{digits}."), rest.trim_start()));
        }
    }
    None
}

fn is_table_row(s: &str) -> bool {
    let s = s.trim();
    s.starts_with('|') && s.matches('|').count() >= 2
}

// ---- table ----------------------------------------------------------------

fn render_table(rows: &[String], width: usize, theme: &Theme, out: &mut Vec<Line<'static>>) {
    // Parse cells.
    let parsed: Vec<Vec<String>> = rows
        .iter()
        .map(|r| {
            r.trim()
                .trim_matches('|')
                .split('|')
                .map(|c| c.trim().to_string())
                .collect()
        })
        .collect();
    // Identify the separator row (---|---).
    let sep_idx = parsed.iter().position(|r| {
        !r.is_empty()
            && r.iter()
                .all(|c| !c.is_empty() && c.chars().all(|ch| matches!(ch, '-' | ':' | ' ')))
    });
    let cols = parsed.iter().map(|r| r.len()).max().unwrap_or(0);
    if cols == 0 {
        return;
    }
    // Column widths.
    let mut col_w = vec![3usize; cols];
    for (i, r) in parsed.iter().enumerate() {
        if Some(i) == sep_idx {
            continue;
        }
        for (c, cell) in r.iter().enumerate() {
            col_w[c] = col_w[c].max(cell.width().min(40));
        }
    }
    // Shrink to fit terminal width.
    let total: usize = col_w.iter().sum::<usize>() + cols * 3 + 1;
    if total > width {
        let overflow = total - width;
        // Take from the widest columns first.
        let mut remaining = overflow;
        while remaining > 0 {
            if let Some((idx, _)) = col_w
                .iter()
                .enumerate()
                .max_by_key(|(_, w)| **w)
                .map(|(i, w)| (i, *w))
            {
                if col_w[idx] <= 4 {
                    break;
                }
                col_w[idx] -= 1;
                remaining -= 1;
            } else {
                break;
            }
        }
    }
    // If every column is at its floor and it still doesn't fit, drop trailing
    // columns so rows never render wider than the terminal.
    let mut cols = cols;
    while cols > 1 && col_w.iter().sum::<usize>() + cols * 3 + 1 > width {
        col_w.pop();
        cols -= 1;
    }

    for (i, r) in parsed.iter().enumerate() {
        if Some(i) == sep_idx {
            let mut spans = vec![Span::styled("├", Style::default().fg(theme.faint))];
            for (c, w) in col_w.iter().enumerate() {
                spans.push(Span::styled(
                    "─".repeat(w + 2),
                    Style::default().fg(theme.faint),
                ));
                spans.push(Span::styled(
                    if c + 1 == cols { "┤" } else { "┼" },
                    Style::default().fg(theme.faint),
                ));
            }
            out.push(Line::from(spans));
            continue;
        }
        let is_header = sep_idx.map(|s| i < s).unwrap_or(i == 0);
        let mut spans = vec![Span::styled("│ ", Style::default().fg(theme.faint))];
        for (c, w) in col_w.iter().enumerate() {
            let cell = r.get(c).cloned().unwrap_or_default();
            let cell = clip(&cell, *w);
            let pad = w.saturating_sub(cell.width());
            let style = if is_header {
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                theme.body()
            };
            spans.push(Span::styled(cell, style));
            spans.push(Span::raw(" ".repeat(pad)));
            spans.push(Span::styled(" │ ", Style::default().fg(theme.faint)));
        }
        out.push(Line::from(spans));
    }
}

fn clip(s: &str, w: usize) -> String {
    if w == 0 {
        return String::new();
    }
    if s.width() <= w {
        return s.to_string();
    }
    let mut acc = String::new();
    let mut used = 0;
    for ch in s.chars() {
        let cw = UnicodeWidthStr::width(ch.to_string().as_str());
        if used + cw + 1 > w {
            acc.push('…');
            break;
        }
        acc.push(ch);
        used += cw;
    }
    acc
}

// ---- inline ---------------------------------------------------------------

/// Parse inline markup into styled spans relative to `base`.
pub fn parse_inline(s: &str, theme: &Theme, base: Style) -> Vec<Span<'static>> {
    let chars: Vec<char> = s.chars().collect();
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut buf = String::new();
    let mut i = 0;
    let push_buf = |buf: &mut String, spans: &mut Vec<Span<'static>>| {
        if !buf.is_empty() {
            spans.push(Span::styled(std::mem::take(buf), base));
        }
    };
    while i < chars.len() {
        let c = chars[i];
        // Inline code.
        if c == '`' {
            if let Some(end) = find_from(&chars, i + 1, '`') {
                push_buf(&mut buf, &mut spans);
                let code: String = chars[i + 1..end].iter().collect();
                spans.push(Span::styled(code, theme.code()));
                i = end + 1;
                continue;
            }
        }
        // Bold (** or __).
        if (c == '*' || c == '_') && i + 1 < chars.len() && chars[i + 1] == c {
            let marker = [c, c];
            if let Some(end) = find_seq(&chars, i + 2, &marker) {
                push_buf(&mut buf, &mut spans);
                let inner: String = chars[i + 2..end].iter().collect();
                let st = base.add_modifier(Modifier::BOLD);
                spans.extend(parse_inline(&inner, theme, st));
                i = end + 2;
                continue;
            }
        }
        // Strikethrough (~~).
        if c == '~' && i + 1 < chars.len() && chars[i + 1] == '~' {
            if let Some(end) = find_seq(&chars, i + 2, &['~', '~']) {
                push_buf(&mut buf, &mut spans);
                let inner: String = chars[i + 2..end].iter().collect();
                spans.push(Span::styled(
                    inner,
                    base.add_modifier(Modifier::CROSSED_OUT).fg(theme.muted),
                ));
                i = end + 2;
                continue;
            }
        }
        // Italic (single * or _), avoid matching inside words for _.
        if c == '*' || (c == '_' && (i == 0 || chars[i - 1].is_whitespace())) {
            if let Some(end) = find_from(&chars, i + 1, c) {
                if end > i + 1 {
                    push_buf(&mut buf, &mut spans);
                    let inner: String = chars[i + 1..end].iter().collect();
                    spans.push(Span::styled(inner, base.add_modifier(Modifier::ITALIC)));
                    i = end + 1;
                    continue;
                }
            }
        }
        // Link [text](url).
        if c == '[' {
            if let Some(close) = find_from(&chars, i + 1, ']') {
                if close + 1 < chars.len() && chars[close + 1] == '(' {
                    if let Some(paren) = find_from(&chars, close + 2, ')') {
                        push_buf(&mut buf, &mut spans);
                        let label: String = chars[i + 1..close].iter().collect();
                        let url: String = chars[close + 2..paren].iter().collect();
                        // Parse markup inside the label (so `[**bold** link](…)` styles
                        // correctly) on top of the link base style.
                        spans.extend(parse_inline(&label, theme, theme.link()));
                        if !url.is_empty() {
                            spans.push(Span::styled(
                                format!(" ({url})"),
                                Style::default().fg(theme.faint),
                            ));
                        }
                        i = paren + 1;
                        continue;
                    }
                }
            }
        }
        buf.push(c);
        i += 1;
    }
    push_buf(&mut buf, &mut spans);
    spans
}

fn find_from(chars: &[char], start: usize, target: char) -> Option<usize> {
    (start..chars.len()).find(|&j| chars[j] == target)
}

fn find_seq(chars: &[char], start: usize, seq: &[char; 2]) -> Option<usize> {
    let mut j = start;
    while j + 1 < chars.len() {
        if chars[j] == seq[0] && chars[j + 1] == seq[1] {
            return Some(j);
        }
        j += 1;
    }
    None
}

// ---- wrapping -------------------------------------------------------------

/// Split styled spans into word tokens (code spans stay whole and unbreakable).
/// Separators are normalised: never leading, trailing, or doubled.
fn to_words(spans: Vec<Span<'static>>) -> Vec<(String, Style)> {
    let mut words: Vec<(String, Style)> = Vec::new();
    let is_sep = |w: &str| w == " ";
    let push = |w: String, st: Style, words: &mut Vec<(String, Style)>| {
        if is_sep(&w) {
            // No leading separator, and never two in a row.
            if words.is_empty() || words.last().map(|(t, _)| is_sep(t)).unwrap_or(false) {
                return;
            }
        }
        words.push((w, st));
    };
    for sp in spans {
        let style = sp.style;
        if style.bg.is_some() {
            // Code span: an unbreakable token.
            push(sp.content.to_string(), style, &mut words);
            continue;
        }
        for (k, part) in sp.content.split(' ').enumerate() {
            if k > 0 {
                push(" ".to_string(), style, &mut words);
            }
            if !part.is_empty() {
                push(part.to_string(), style, &mut words);
            }
        }
    }
    // Drop any trailing separator.
    while words.last().map(|(t, _)| is_sep(t)).unwrap_or(false) {
        words.pop();
    }
    words
}

/// Greedily wrap `words` to `width`, with span prefixes on the first and
/// continuation lines, pushing finished [`Line`]s into `out`.
fn wrap(
    words: &[(String, Style)],
    width: usize,
    first_prefix: Vec<Span<'static>>,
    cont_prefix: Vec<Span<'static>>,
    out: &mut Vec<Line<'static>>,
) {
    let prefix_w = |p: &[Span]| p.iter().map(|s| s.content.width()).sum::<usize>();
    let first_w = prefix_w(&first_prefix);
    let cont_w = prefix_w(&cont_prefix);

    if words.is_empty() {
        if !first_prefix.is_empty() {
            out.push(Line::from(first_prefix));
        }
        return;
    }

    let mut line: Vec<Span<'static>> = first_prefix;
    let mut used = first_w;
    let mut first = true;
    let avail = |first: bool| {
        width
            .saturating_sub(if first { first_w } else { cont_w })
            .max(1)
    };

    let mut at_line_start = true;
    for (w, style) in words {
        // Skip leading spaces at the start of a wrapped line.
        if w == " " {
            if at_line_start {
                continue;
            }
            // Defer the space; only add if a word follows that fits.
            // Simpler: add the space, it will be trimmed by width check next word.
            line.push(Span::styled(" ".to_string(), *style));
            used += 1;
            continue;
        }
        let ww = w.width();
        if !at_line_start && used + ww > width && used > if first { first_w } else { cont_w } {
            // Trim any trailing separator spaces before breaking.
            while line
                .last()
                .map(|s| s.content.as_ref() == " ")
                .unwrap_or(false)
            {
                line.pop();
            }
            out.push(Line::from(std::mem::take(&mut line)));
            line = cont_prefix.clone();
            used = cont_w;
            first = false;
            at_line_start = true;
        }
        // Hard-break a single token longer than the available width.
        let cap = avail(first);
        if ww > cap {
            for piece in hard_split(w, cap) {
                let pw = piece.width();
                if !at_line_start && used + pw > width {
                    out.push(Line::from(std::mem::take(&mut line)));
                    line = cont_prefix.clone();
                    used = cont_w;
                    first = false;
                }
                line.push(Span::styled(piece.clone(), *style));
                used += pw;
                at_line_start = false;
            }
            continue;
        }
        line.push(Span::styled(w.clone(), *style));
        used += ww;
        at_line_start = false;
    }
    while line
        .last()
        .map(|s| s.content.as_ref() == " ")
        .unwrap_or(false)
    {
        line.pop();
    }
    if !line.is_empty() {
        out.push(Line::from(line));
    }
}

fn hard_split(s: &str, cap: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut w = 0;
    for ch in s.chars() {
        let cw = UnicodeWidthStr::width(ch.to_string().as_str());
        if w + cw > cap && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
            w = 0;
        }
        cur.push(ch);
        w += cw;
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

// ---- code highlighting ----------------------------------------------------

/// Colour a unified-diff line by its leading marker.
fn diff_spans(line: &str, theme: &Theme) -> Vec<Span<'static>> {
    let style = if line.starts_with("+++") || line.starts_with("---") {
        Style::default()
            .fg(theme.muted)
            .add_modifier(Modifier::BOLD)
    } else if line.starts_with("@@") {
        Style::default().fg(theme.accent2)
    } else if line.starts_with('+') {
        Style::default().fg(theme.good)
    } else if line.starts_with('-') {
        Style::default().fg(theme.bad)
    } else {
        Style::default().fg(theme.fg)
    };
    vec![Span::styled(line.to_string(), style)]
}

/// A pragmatic, dependency-free highlighter: strings, line comments, numbers,
/// and per-language keywords. Not a full lexer — tuned to read well in a TUI.
fn highlight_code(line: &str, lang: &str, theme: &Theme) -> Vec<Span<'static>> {
    let lang = lang.to_ascii_lowercase();
    let kw = keywords(&lang);
    let lc: Option<Vec<char>> = line_comment(&lang).map(|s| s.chars().collect());
    let kw_style = Style::default().fg(theme.accent);
    let str_style = Style::default().fg(theme.good);
    let num_style = Style::default().fg(theme.warn);
    let com_style = Style::default()
        .fg(theme.muted)
        .add_modifier(Modifier::ITALIC);
    let def = Style::default().fg(theme.fg);

    let chars: Vec<char> = line.chars().collect();
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut buf = String::new();
    let flush = |buf: &mut String, spans: &mut Vec<Span<'static>>| {
        if buf.is_empty() {
            return;
        }
        let t = std::mem::take(buf);
        let style = if kw.contains(&t.as_str()) {
            kw_style
        } else {
            def
        };
        spans.push(Span::styled(t, style));
    };

    let mut i = 0;
    while i < chars.len() {
        // Line comment to end of line.
        if let Some(c) = &lc {
            if chars[i..].starts_with(c.as_slice()) {
                flush(&mut buf, &mut spans);
                spans.push(Span::styled(
                    chars[i..].iter().collect::<String>(),
                    com_style,
                ));
                return spans;
            }
        }
        let ch = chars[i];
        // String literal.
        if ch == '"' || ch == '\'' || ch == '`' {
            flush(&mut buf, &mut spans);
            let quote = ch;
            let mut s = String::from(ch);
            i += 1;
            while i < chars.len() {
                if chars[i] == '\\' && i + 1 < chars.len() {
                    s.push(chars[i]);
                    s.push(chars[i + 1]);
                    i += 2;
                    continue;
                }
                s.push(chars[i]);
                let closed = chars[i] == quote;
                i += 1;
                if closed {
                    break;
                }
            }
            spans.push(Span::styled(s, str_style));
            continue;
        }
        // Number at the start of a token. Stops before a method call (`1.foo`)
        // and a digit-led identifier, and handles 0x/0o/0b prefixes + a fraction.
        if ch.is_ascii_digit() && buf.is_empty() {
            let mut n = String::from(ch);
            i += 1;
            if n == "0" && i < chars.len() && matches!(chars[i], 'x' | 'X' | 'o' | 'O' | 'b' | 'B')
            {
                n.push(chars[i]);
                i += 1;
                while i < chars.len() && (chars[i].is_ascii_hexdigit() || chars[i] == '_') {
                    n.push(chars[i]);
                    i += 1;
                }
            } else {
                while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '_') {
                    n.push(chars[i]);
                    i += 1;
                }
                // Fraction: only a '.' that's actually followed by a digit.
                if i + 1 < chars.len() && chars[i] == '.' && chars[i + 1].is_ascii_digit() {
                    n.push('.');
                    i += 1;
                    while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '_') {
                        n.push(chars[i]);
                        i += 1;
                    }
                }
            }
            spans.push(Span::styled(n, num_style));
            continue;
        }
        // Identifier character.
        if ch.is_alphanumeric() || ch == '_' {
            buf.push(ch);
            i += 1;
            continue;
        }
        // Punctuation / whitespace.
        flush(&mut buf, &mut spans);
        spans.push(Span::styled(ch.to_string(), def));
        i += 1;
    }
    flush(&mut buf, &mut spans);
    if spans.is_empty() {
        spans.push(Span::styled(line.to_string(), def));
    }
    spans
}

fn line_comment(lang: &str) -> Option<&'static str> {
    match lang {
        "py" | "python" | "sh" | "bash" | "zsh" | "ruby" | "rb" | "yaml" | "yml" | "toml" | "r"
        | "perl" | "makefile" | "dockerfile" | "ini" | "conf" => Some("#"),
        "sql" => Some("--"),
        "lisp" | "clojure" | "scheme" | "el" => Some(";"),
        "" => None,      // unknown language: don't guess (avoid mangling plain text)
        _ => Some("//"), // rust/js/ts/go/c/cpp/java/kotlin/swift/…
    }
}

fn keywords(lang: &str) -> &'static [&'static str] {
    match lang {
        "rust" | "rs" => &[
            "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum",
            "extern", "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod",
            "move", "mut", "pub", "ref", "return", "self", "Self", "static", "struct", "super",
            "trait", "true", "type", "unsafe", "use", "where", "while",
        ],
        "py" | "python" => &[
            "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del",
            "elif", "else", "except", "False", "finally", "for", "from", "global", "if", "import",
            "in", "is", "lambda", "None", "nonlocal", "not", "or", "pass", "raise", "return",
            "True", "try", "while", "with", "yield",
        ],
        "js" | "javascript" | "ts" | "typescript" | "jsx" | "tsx" => &[
            "async",
            "await",
            "break",
            "case",
            "catch",
            "class",
            "const",
            "continue",
            "default",
            "delete",
            "do",
            "else",
            "export",
            "extends",
            "false",
            "finally",
            "for",
            "function",
            "if",
            "import",
            "in",
            "instanceof",
            "interface",
            "let",
            "new",
            "null",
            "of",
            "private",
            "public",
            "readonly",
            "return",
            "super",
            "switch",
            "this",
            "throw",
            "true",
            "try",
            "type",
            "typeof",
            "var",
            "void",
            "while",
            "yield",
        ],
        "go" => &[
            "break",
            "case",
            "chan",
            "const",
            "continue",
            "default",
            "defer",
            "else",
            "fallthrough",
            "false",
            "for",
            "func",
            "go",
            "goto",
            "if",
            "import",
            "interface",
            "map",
            "nil",
            "package",
            "range",
            "return",
            "select",
            "struct",
            "switch",
            "true",
            "type",
            "var",
        ],
        "c" | "cpp" | "c++" | "java" | "kotlin" | "swift" | "cs" | "csharp" => &[
            "auto",
            "bool",
            "break",
            "case",
            "catch",
            "char",
            "class",
            "const",
            "continue",
            "default",
            "do",
            "double",
            "else",
            "enum",
            "extern",
            "false",
            "final",
            "float",
            "for",
            "if",
            "import",
            "int",
            "long",
            "new",
            "null",
            "private",
            "protected",
            "public",
            "return",
            "short",
            "static",
            "struct",
            "switch",
            "this",
            "throw",
            "true",
            "try",
            "void",
            "while",
        ],
        "sh" | "bash" | "zsh" => &[
            "case", "do", "done", "echo", "elif", "else", "esac", "export", "fi", "for",
            "function", "if", "in", "local", "return", "then", "while",
        ],
        "json" => &["true", "false", "null"],
        _ => &[],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line_w(line: &Line) -> usize {
        line.spans.iter().map(|s| s.content.width()).sum()
    }

    const DOC: &str = "# Title with a long heading that surely wraps somewhere in the middle\n\n\
        Para with **bold**, _italic_, `code span with spaces`, ~~strike~~ and a \
        [**bold** label](https://example.com/very/long/url/that/keeps/going/forever).\n\n\
        - bullet a\n- bullet with a supercalifragilisticexpialidocioussupercalifragilistic token\n\n\
        1. first\n2. second\n\n\
        | Column One | Column Two | Column Three | Column Four |\n|---|---|---|---|\n\
        | aaaaaaaaaa | bbbbbbbbbb | cccccccccc | dddddddddd |\n\n\
        > a block quote that is also quite long and should wrap with the quote bar kept\n\n\
        ```rust\nfn main() { let x = \"a fairly long line of code that exceeds width\"; }\n```\n\n\
        Some CJK 你好世界 and emoji 🚀🎉 mixed in for width measurement.";

    #[test]
    fn never_panics_at_any_width() {
        let theme = Theme::dark();
        for w in [1u16, 2, 5, 8, 12, 20, 40, 80, 120, 200] {
            let _ = render(DOC, w, &theme); // a panic here fails the test
        }
    }

    #[test]
    fn wrapping_lines_fit_width() {
        // Code blocks and tables intentionally don't wrap (the terminal clips
        // them), so the fit-invariant applies to ordinary wrapped text only.
        let theme = Theme::dark();
        for w in [20u16, 40, 80, 120, 200] {
            for line in render(DOC, w, &theme) {
                let is_code_or_rule = line.style.bg.is_some()
                    || line.spans.iter().any(|s| s.style.bg.is_some())
                    || line
                        .spans
                        .iter()
                        .any(|s| s.content.starts_with('│') || s.content.starts_with('├'));
                if is_code_or_rule {
                    continue;
                }
                assert!(
                    line_w(&line) <= w as usize,
                    "wrapped line width {} exceeds target {w}: {:?}",
                    line_w(&line),
                    line.spans
                        .iter()
                        .map(|s| s.content.as_ref())
                        .collect::<String>()
                );
            }
        }
    }

    #[test]
    fn no_doubled_or_leading_spaces() {
        let words = to_words(parse_inline("a    b   c", &Theme::dark(), Style::default()));
        let seps = words.iter().filter(|(w, _)| w == " ").count();
        // three words → at most two separators, none leading/trailing.
        assert_eq!(seps, 2, "{words:?}");
        assert_ne!(words.first().map(|(w, _)| w.as_str()), Some(" "));
        assert_ne!(words.last().map(|(w, _)| w.as_str()), Some(" "));
    }

    #[test]
    fn code_highlight_colours_tokens() {
        let theme = Theme::dark();
        let spans = highlight_code("let x = \"hi\"; // note", "rust", &theme);
        assert!(spans
            .iter()
            .any(|s| s.content == "let" && s.style.fg == Some(theme.accent)));
        assert!(spans
            .iter()
            .any(|s| s.content == "\"hi\"" && s.style.fg == Some(theme.good)));
        assert!(spans
            .iter()
            .any(|s| s.content.contains("// note") && s.style.fg == Some(theme.muted)));
    }

    #[test]
    fn number_scan_stops_at_method_call() {
        let theme = Theme::dark();
        let spans = highlight_code("let n = 1.to_string();", "rust", &theme);
        // '1' is a number…
        assert!(spans
            .iter()
            .any(|s| s.content == "1" && s.style.fg == Some(theme.warn)));
        // …and 'to_string' is NOT swallowed into the number.
        assert!(spans.iter().any(|s| s.content == "to_string"));
        assert!(!spans.iter().any(|s| s.content.contains("1.to")));
    }

    #[test]
    fn diff_lines_coloured() {
        let theme = Theme::dark();
        assert_eq!(diff_spans("+added", &theme)[0].style.fg, Some(theme.good));
        assert_eq!(diff_spans("-removed", &theme)[0].style.fg, Some(theme.bad));
        assert_eq!(
            diff_spans("@@ -1 +1 @@", &theme)[0].style.fg,
            Some(theme.accent2)
        );
    }

    #[test]
    fn link_label_markup_is_parsed() {
        let spans = parse_inline("[**bold**](http://x)", &Theme::dark(), Style::default());
        // The bold label should be its own styled span carrying the BOLD modifier.
        assert!(
            spans
                .iter()
                .any(|s| s.content.as_ref() == "bold"
                    && s.style.add_modifier.contains(Modifier::BOLD))
        );
    }
}
