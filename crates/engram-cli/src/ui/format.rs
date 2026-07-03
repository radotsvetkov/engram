//! Small, dependency-free formatting helpers: timestamps, relative time,
//! human numbers, cost, and a spinner. Kept here so both the CLI and the TUI
//! present the same units the same way.

/// Current unix time in milliseconds.
pub fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// "3m ago", "in 2h", "just now" from a unix-ms timestamp.
pub fn rel_time(ts_ms: i64) -> String {
    if ts_ms <= 0 {
        return "—".into();
    }
    let delta = now_ms() - ts_ms;
    let future = delta < 0;
    let s = delta.unsigned_abs() / 1000;
    let label = if s < 5 {
        return if future {
            "soon".into()
        } else {
            "just now".into()
        };
    } else if s < 60 {
        format!("{s}s")
    } else if s < 3600 {
        format!("{}m", s / 60)
    } else if s < 86_400 {
        format!("{}h", s / 3600)
    } else if s < 86_400 * 30 {
        format!("{}d", s / 86_400)
    } else if s < 86_400 * 365 {
        format!("{}mo", s / (86_400 * 30))
    } else {
        format!("{}y", s / (86_400 * 365))
    };
    if future {
        format!("in {label}")
    } else {
        format!("{label} ago")
    }
}

/// Local-ish wall clock "HH:MM" from unix-ms (uses the process timezone offset
/// derived from `localtime` via a cheap heuristic — no chrono dependency).
pub fn hhmm(ts_ms: i64) -> String {
    let (h, m, _) = wall_clock(ts_ms);
    format!("{h:02}:{m:02}")
}

/// "Mon Jun 29 · 17:00"-style stamp.
pub fn stamp(ts_ms: i64) -> String {
    let (h, m, _) = wall_clock(ts_ms);
    let (y, mo, d) = civil_from_ms(ts_ms);
    let months = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let mon = months
        .get((mo as usize).saturating_sub(1))
        .unwrap_or(&"???");
    format!("{mon} {d} {y} · {h:02}:{m:02}")
}

/// Decompose unix-ms into local hour/min/sec, applying the system's current UTC offset.
fn wall_clock(ts_ms: i64) -> (i64, i64, i64) {
    let secs = (ts_ms / 1000) + local_offset_secs();
    let day_secs = secs.rem_euclid(86_400);
    (day_secs / 3600, (day_secs % 3600) / 60, day_secs % 60)
}

/// Civil date (year, month, day) for a unix-ms timestamp in local time.
fn civil_from_ms(ts_ms: i64) -> (i64, i64, i64) {
    let secs = (ts_ms / 1000) + local_offset_secs();
    let days = secs.div_euclid(86_400);
    // Howard Hinnant's days→civil algorithm.
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Best-effort local UTC offset in seconds, cached. Falls back to 0 (UTC).
fn local_offset_secs() -> i64 {
    use std::sync::OnceLock;
    static OFFSET: OnceLock<i64> = OnceLock::new();
    *OFFSET.get_or_init(|| {
        // Read TZ offset from the `date` command's %z once; cheap and avoids a tz crate.
        if let Ok(out) = std::process::Command::new("date").arg("+%z").output() {
            if let Ok(s) = String::from_utf8(out.stdout) {
                let s = s.trim();
                if s.len() >= 5 {
                    let sign = if s.starts_with('-') { -1 } else { 1 };
                    if let (Ok(h), Ok(m)) = (s[1..3].parse::<i64>(), s[3..5].parse::<i64>()) {
                        return sign * (h * 3600 + m * 60);
                    }
                }
            }
        }
        0
    })
}

/// "12.3k", "4.5M" style compact counts.
pub fn human_count(n: u64) -> String {
    if n < 1000 {
        n.to_string()
    } else if n < 1_000_000 {
        format!("{:.1}k", n as f64 / 1000.0)
    } else if n < 1_000_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else {
        format!("{:.1}B", n as f64 / 1_000_000_000.0)
    }
}

/// USD cost with a sensible number of digits.
pub fn cost(usd: f64) -> String {
    if usd <= 0.0 {
        "$0.00".into()
    } else if usd < 0.01 {
        format!("${usd:.4}")
    } else if usd < 1.0 {
        format!("${usd:.3}")
    } else {
        format!("${usd:.2}")
    }
}

/// Braille spinner frame for an animation tick.
pub fn spinner(tick: usize) -> char {
    const FRAMES: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
    FRAMES[tick % FRAMES.len()]
}

/// Truncate a string to `n` display columns with an ellipsis.
/// Left-align `s` into a field `n` DISPLAY columns wide. `format!("{:<n$}")`
/// pads by char count, so a wide-char (CJK) string would come out under-padded
/// and shift every column after it.
pub fn pad_display(s: &str, n: usize) -> String {
    use unicode_width::UnicodeWidthStr;
    let w = s.width();
    if w >= n {
        s.to_string()
    } else {
        format!("{s}{}", " ".repeat(n - w))
    }
}

pub fn ellipsize(s: &str, n: usize) -> String {
    use unicode_width::UnicodeWidthStr;
    if n == 0 {
        return String::new();
    }
    if s.width() <= n {
        return s.to_string();
    }
    let mut out = String::new();
    let mut used = 0;
    for ch in s.chars() {
        let w = UnicodeWidthStr::width(ch.to_string().as_str());
        if used + w + 1 > n {
            out.push('…');
            break;
        }
        out.push(ch);
        used += w;
    }
    out
}

/// Collapse a multi-line string into a single line for compact rows.
pub fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Truncate `s` in place to at most `max` bytes, never cutting a multi-byte
/// char (`String::truncate` panics on a non-boundary offset).
pub fn truncate_bytes(s: &mut String, max: usize) {
    if s.len() <= max {
        return;
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s.truncate(end);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_bytes_never_cuts_a_char() {
        // '€' is 3 bytes, so most cut offsets land mid-char.
        let base = "aa€bb€".repeat(50);
        for max in 0..=base.len() + 2 {
            let mut s = base.clone();
            truncate_bytes(&mut s, max); // must never panic
            assert!(s.len() <= base.len());
            assert!(std::str::from_utf8(s.as_bytes()).is_ok());
            if max <= base.len() {
                assert!(s.len() <= max);
            }
        }
    }
}
