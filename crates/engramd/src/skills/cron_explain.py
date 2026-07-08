#!/usr/bin/env python3
"""cron_explain — Engram skill (no network). Explain a standard 5-field cron
expression ("minute hour day month weekday") in plain English, and compute
the next fire times by brute-force forward simulation.

Request (stdin): {"expr": "*/5 * * * *", "count"?: 5}
Output (stdout): {expr, description, next_runs: ["2026-07-08T12:05:00", ...]}
"""
import json
import re
import sys
from datetime import datetime, timedelta

_FIELD_SPECS = [
    ("minute", 0, 59),
    ("hour", 0, 23),
    ("day", 1, 31),
    ("month", 1, 12),
    ("weekday", 0, 6),
]

_WEEKDAY_NAMES = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"]

_TOKEN_RE = re.compile(r"^\d+$")

_MAX_MINUTES = 4 * 366 * 24 * 60  # ~4 years safety cap


def _parse_field(token_str, lo, hi, name):
    values = set()
    for part in token_str.split(","):
        part = part.strip()
        if not part:
            raise ValueError("empty token in %s field %r" % (name, token_str))
        if "/" in part:
            base, step_s = part.split("/", 1)
            if not _TOKEN_RE.match(step_s):
                raise ValueError("invalid step %r in %s field" % (part, name))
            step = int(step_s)
            if step <= 0:
                raise ValueError("step must be positive in %s field: %r" % (name, part))
        else:
            base, step = part, 1

        if base == "*":
            start, end = lo, hi
        elif "-" in base:
            a_s, b_s = base.split("-", 1)
            if not (_TOKEN_RE.match(a_s) and _TOKEN_RE.match(b_s)):
                raise ValueError("invalid range %r in %s field" % (part, name))
            start, end = int(a_s), int(b_s)
            if start > end:
                raise ValueError("invalid range %r in %s field (start > end)" % (part, name))
        else:
            if not _TOKEN_RE.match(base):
                raise ValueError("invalid token %r in %s field" % (part, name))
            start = int(base)
            end = hi if "/" in part else start

        for v in range(start, end + 1, step):
            if v < lo or v > hi:
                raise ValueError(
                    "value %d out of range for %s field (%d-%d)" % (v, name, lo, hi))
            values.add(v)

    if not values:
        raise ValueError("%s field parsed to no values: %r" % (name, token_str))
    return values


def _list_desc(vals, lo, hi):
    vals = sorted(vals)
    if vals == list(range(lo, hi + 1)):
        return None
    if len(vals) == 1:
        return str(vals[0])
    return "%s and %s" % (", ".join(str(v) for v in vals[:-1]), vals[-1])


def _describe(tokens, sets):
    minute_tok, hour_tok, day_tok, month_tok, weekday_tok = tokens
    minute_set, hour_set, day_set, month_set, weekday_set = sets

    all_hour = hour_tok == "*"
    all_day = day_tok == "*"
    all_month = month_tok == "*"
    all_weekday = weekday_tok == "*"

    m = re.fullmatch(r"\*/(\d+)", minute_tok)
    if m and all_hour and all_day and all_month and all_weekday:
        return "Every %s minutes" % m.group(1)

    if minute_tok == "*" and all_hour and all_day and all_month and all_weekday:
        return "Every minute"

    if minute_tok.isdigit() and all_hour and all_day and all_month and all_weekday:
        return "At minute %s past every hour" % minute_tok

    if minute_tok.isdigit() and hour_tok.isdigit() and all_day and all_month and all_weekday:
        return "At %02d:%02d every day" % (int(hour_tok), int(minute_tok))

    if minute_tok.isdigit() and hour_tok.isdigit() and all_day and all_month and weekday_tok.isdigit():
        return "At %02d:%02d on %s" % (
            int(hour_tok), int(minute_tok), _WEEKDAY_NAMES[int(weekday_tok) % 7])

    if minute_tok.isdigit() and hour_tok.isdigit() and day_tok.isdigit() and all_month and all_weekday:
        return "At %02d:%02d on day %s of every month" % (
            int(hour_tok), int(minute_tok), day_tok)

    # Mechanical fallback for complex combinations.
    minute_desc = _list_desc(minute_set, 0, 59)
    hour_desc = _list_desc(hour_set, 0, 23)
    day_desc = _list_desc(day_set, 1, 31)
    month_desc = _list_desc(month_set, 1, 12)
    weekday_desc = _list_desc(weekday_set, 0, 6)

    segments = [
        "At minute %s" % minute_desc if minute_desc else "Every minute",
        "past hour %s" % hour_desc if hour_desc else "every hour",
        "on day %s" % day_desc if day_desc else "every day",
    ]
    if month_desc:
        segments.append("in month %s" % month_desc)
    if weekday_desc:
        segments.append("on weekday %s" % weekday_desc)
    return ", ".join(segments)


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e})); return 0
    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"expr": "*/5 * * * *", "count": 5},
        })); return 0

    expr = q.get("expr")
    if not isinstance(expr, str) or not expr.strip():
        print(json.dumps({
            "error": "missing required field 'expr' (5-field cron expression)",
            "example": {"expr": "0 9 * * 1"},
        })); return 0
    expr = expr.strip()

    count = q.get("count", 5)
    try:
        count = int(count)
        if count < 1 or count > 500:
            raise ValueError
    except (TypeError, ValueError):
        print(json.dumps({"error": "'count' must be a positive integer (<= 500)"})); return 0

    fields = expr.split()
    if len(fields) != 5:
        print(json.dumps({
            "error": "expected exactly 5 fields (minute hour day month weekday), got %d: %r" % (
                len(fields), expr),
            "example": {"expr": "0 9 * * 1"},
        })); return 0

    try:
        sets = []
        for (name, lo, hi), token in zip(_FIELD_SPECS, fields):
            sets.append(_parse_field(token, lo, hi, name))
        minute_set, hour_set, day_set, month_set, weekday_set = sets
    except ValueError as e:
        print(json.dumps({"error": str(e), "expr": expr})); return 0

    try:
        description = _describe(fields, sets)

        day_wild = fields[2] == "*"
        weekday_wild = fields[4] == "*"

        t = datetime.now().replace(second=0, microsecond=0) + timedelta(minutes=1)
        next_runs = []
        steps = 0
        while len(next_runs) < count and steps < _MAX_MINUTES:
            steps += 1
            if t.month in month_set:
                cron_wd = (t.weekday() + 1) % 7  # python Mon=0..Sun=6 -> cron Sun=0..Sat=6
                if day_wild and weekday_wild:
                    dom_ok = True
                elif day_wild:
                    dom_ok = cron_wd in weekday_set
                elif weekday_wild:
                    dom_ok = t.day in day_set
                else:
                    # Standard cron OR-rule: when both day-of-month and
                    # weekday are restricted, a match on either is enough.
                    dom_ok = (t.day in day_set) or (cron_wd in weekday_set)
                if dom_ok and t.hour in hour_set and t.minute in minute_set:
                    next_runs.append(t.isoformat())
            t += timedelta(minutes=1)

        if len(next_runs) < count:
            print(json.dumps({
                "error": "no matching time found in the search window — check your expression",
                "expr": expr,
                "description": description,
            })); return 0

        print(json.dumps({
            "expr": expr, "description": description, "next_runs": next_runs,
        }, indent=2, default=str)); return 0
    except Exception as e:
        print(json.dumps({"error": "cron_explain failed: %s" % e})); return 1


if __name__ == "__main__":
    sys.exit(main())
