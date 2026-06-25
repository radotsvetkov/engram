//! Recurrence — when a job fires.
//!
//! A small, *deterministic* grammar covers the phrases people actually use: one-off
//! delays, fixed intervals, and daily / weekday / weekly clock times. Parsing never
//! calls a model, so it is free, instant, and testable; anything it cannot parse
//! returns [`ParseError`], which the agent can hand to the LLM as a fallback. All
//! times are computed in UTC against a caller-supplied "now", so it works correctly
//! even when the core was asleep.

use chrono::{DateTime, Datelike, Duration, NaiveTime, TimeZone, Utc, Weekday};
use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("could not parse schedule: {0}")]
pub struct ParseError(pub String);

/// How a job repeats.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Recurrence {
    /// Fire once at an absolute time (epoch millis).
    Once { at_ms: i64 },
    /// Fire every `secs` seconds.
    Interval { secs: i64 },
    /// Fire every day at `hour:min` UTC.
    Daily { hour: u32, min: u32 },
    /// Fire Mon–Fri at `hour:min` UTC.
    Weekdays { hour: u32, min: u32 },
    /// Fire weekly on `weekday` (0=Mon … 6=Sun) at `hour:min` UTC.
    Weekly { weekday: u8, hour: u32, min: u32 },
}

impl Recurrence {
    /// The next fire strictly after `after`, or `None` for a spent one-off.
    pub fn next_after(&self, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
        match self {
            Recurrence::Once { at_ms } => {
                let at = Utc.timestamp_millis_opt(*at_ms).single()?;
                (at > after).then_some(at)
            }
            Recurrence::Interval { secs } => Some(after + Duration::seconds(*secs)),
            Recurrence::Daily { hour, min } => Some(next_daily(after, *hour, *min)),
            Recurrence::Weekdays { hour, min } => {
                let mut c = next_daily(after, *hour, *min);
                while is_weekend(c) {
                    c = next_daily(c, *hour, *min);
                }
                Some(c)
            }
            Recurrence::Weekly { weekday, hour, min } => {
                let target = weekday_from_u8(*weekday);
                let mut c = next_daily(after, *hour, *min);
                while c.weekday() != target {
                    c += Duration::days(1);
                }
                Some(c)
            }
        }
    }
}

fn next_daily(after: DateTime<Utc>, hour: u32, min: u32) -> DateTime<Utc> {
    let t = NaiveTime::from_hms_opt(hour, min, 0).unwrap_or_default();
    let today = Utc.from_utc_datetime(&after.date_naive().and_time(t));
    if today > after {
        today
    } else {
        Utc.from_utc_datetime(&(after.date_naive() + Duration::days(1)).and_time(t))
    }
}

fn is_weekend(dt: DateTime<Utc>) -> bool {
    matches!(dt.weekday(), Weekday::Sat | Weekday::Sun)
}

fn weekday_from_u8(n: u8) -> Weekday {
    match n % 7 {
        0 => Weekday::Mon,
        1 => Weekday::Tue,
        2 => Weekday::Wed,
        3 => Weekday::Thu,
        4 => Weekday::Fri,
        5 => Weekday::Sat,
        _ => Weekday::Sun,
    }
}

/// Parse a natural-language schedule relative to `now`.
pub fn parse(input: &str, now: DateTime<Utc>) -> Result<Recurrence, ParseError> {
    let s = input.trim().to_lowercase();
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");

    if let Some(rest) = s.strip_prefix("in ") {
        let dur = parse_duration(rest).ok_or_else(|| ParseError(input.to_string()))?;
        return Ok(Recurrence::Once { at_ms: (now + dur).timestamp_millis() });
    }

    // Weekday-specific (check before the generic "every").
    for (name, n) in WEEKDAYS {
        if s.contains(name) {
            let (hour, min) = find_time(&s).unwrap_or((9, 0));
            return Ok(Recurrence::Weekly { weekday: *n, hour, min });
        }
    }

    if s.contains("weekday") {
        let (hour, min) = find_time(&s).unwrap_or((9, 0));
        return Ok(Recurrence::Weekdays { hour, min });
    }

    if s.contains("every day") || s.starts_with("daily") || (s.starts_with("at ") && find_time(&s).is_some()) {
        let (hour, min) = find_time(&s).unwrap_or((9, 0));
        return Ok(Recurrence::Daily { hour, min });
    }

    if let Some(rest) = s.strip_prefix("every ") {
        // "every 5 minutes", "every hour", "every 30 seconds"
        if let Some(dur) = parse_duration(rest) {
            return Ok(Recurrence::Interval { secs: dur.num_seconds().max(1) });
        }
        // "every <time>" with a clock time means daily at that time.
        if let Some((hour, min)) = find_time(&s) {
            return Ok(Recurrence::Daily { hour, min });
        }
    }

    Err(ParseError(input.to_string()))
}

const WEEKDAYS: &[(&str, u8)] = &[
    ("monday", 0),
    ("tuesday", 1),
    ("wednesday", 2),
    ("thursday", 3),
    ("friday", 4),
    ("saturday", 5),
    ("sunday", 6),
];

/// Parse "N unit" or "unit" durations: minutes/mins/m, hours/hr/h, seconds/secs/s, days.
fn parse_duration(s: &str) -> Option<Duration> {
    let toks: Vec<&str> = s.split_whitespace().collect();
    let (n, unit) = match toks.as_slice() {
        [num, unit, ..] if num.parse::<i64>().is_ok() => (num.parse::<i64>().ok()?, *unit),
        [unit, ..] => (1, *unit),
        _ => return None,
    };
    let n = n.max(1);
    let u = unit.trim_end_matches('s');
    match u {
        "second" | "sec" | "s" => Some(Duration::seconds(n)),
        "minute" | "min" | "m" => Some(Duration::minutes(n)),
        "hour" | "hr" | "h" => Some(Duration::hours(n)),
        "day" | "d" => Some(Duration::days(n)),
        _ => None,
    }
}

/// Find a clock time anywhere in the string: "9am", "9:30 pm", "17:00", "at 8".
fn find_time(s: &str) -> Option<(u32, u32)> {
    let toks: Vec<&str> = s.split_whitespace().collect();
    for (i, tok) in toks.iter().enumerate() {
        if let Some(t) = parse_clock(tok) {
            return Some(t);
        }
        // "at 9" / "at 9 am"
        if *tok == "at" {
            if let Some(next) = toks.get(i + 1) {
                let merged = match toks.get(i + 2) {
                    Some(ampm @ (&"am" | &"pm")) => format!("{next}{ampm}"),
                    _ => next.to_string(),
                };
                if let Some(t) = parse_clock(&merged) {
                    return Some(t);
                }
            }
        }
    }
    None
}

fn parse_clock(tok: &str) -> Option<(u32, u32)> {
    let t = tok.trim();
    let (body, ampm) = if let Some(b) = t.strip_suffix("am") {
        (b, Some(false))
    } else if let Some(b) = t.strip_suffix("pm") {
        (b, Some(true))
    } else {
        (t, None)
    };
    let body = body.trim();
    let (h_str, m_str) = match body.split_once(':') {
        Some((h, m)) => (h, m),
        None => (body, "0"),
    };
    let mut hour: u32 = h_str.parse().ok()?;
    let min: u32 = m_str.parse().ok()?;
    if min > 59 {
        return None;
    }
    match ampm {
        Some(true) if hour < 12 => hour += 12, // pm
        Some(false) if hour == 12 => hour = 0, // 12am = 00
        _ => {}
    }
    if hour > 23 {
        return None;
    }
    // Require an explicit time signal so a bare "every 5" is not read as 5 o'clock.
    if ampm.is_none() && !tok.contains(':') {
        return None;
    }
    Some((hour, min))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> DateTime<Utc> {
        // A fixed Wednesday 2026-06-24 12:00:00 UTC for deterministic tests.
        Utc.with_ymd_and_hms(2026, 6, 24, 12, 0, 0).unwrap()
    }

    #[test]
    fn parses_relative_once() {
        let r = parse("in 30 minutes", now()).unwrap();
        match r {
            Recurrence::Once { at_ms } => {
                assert_eq!(at_ms, (now() + Duration::minutes(30)).timestamp_millis());
            }
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn parses_daily_and_computes_next() {
        let r = parse("every day at 9am", now()).unwrap();
        assert_eq!(r, Recurrence::Daily { hour: 9, min: 0 });
        // 9am already passed today (now is 12:00), so next is tomorrow 09:00.
        let next = r.next_after(now()).unwrap();
        assert_eq!(next, Utc.with_ymd_and_hms(2026, 6, 25, 9, 0, 0).unwrap());
    }

    #[test]
    fn parses_weekday_skips_weekend() {
        let r = parse("every weekday at 09:00", now()).unwrap();
        assert_eq!(r, Recurrence::Weekdays { hour: 9, min: 0 });
        // From Friday, next weekday fire is Monday.
        let fri = Utc.with_ymd_and_hms(2026, 6, 26, 12, 0, 0).unwrap();
        let next = r.next_after(fri).unwrap();
        assert_eq!(next.weekday(), Weekday::Mon);
    }

    #[test]
    fn parses_weekly() {
        let r = parse("every monday at 8:30", now()).unwrap();
        assert_eq!(r, Recurrence::Weekly { weekday: 0, hour: 8, min: 30 });
        assert_eq!(r.next_after(now()).unwrap().weekday(), Weekday::Mon);
    }

    #[test]
    fn parses_intervals() {
        assert_eq!(parse("every 5 minutes", now()).unwrap(), Recurrence::Interval { secs: 300 });
        assert_eq!(parse("every hour", now()).unwrap(), Recurrence::Interval { secs: 3600 });
        assert_eq!(parse("every 30 seconds", now()).unwrap(), Recurrence::Interval { secs: 30 });
    }

    #[test]
    fn rejects_gibberish() {
        assert!(parse("sometime soon-ish", now()).is_err());
    }
}
