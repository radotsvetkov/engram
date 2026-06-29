#!/usr/bin/env python3
"""datetime — Engram skill (no network). Time across timezones + date math.

Stdlib only (zoneinfo). Actions:
  {"action": "now", "tz": "Asia/Tokyo"}                       -> current time there
  {"action": "convert", "time": "2026-07-01 15:00", "from": "Europe/Berlin", "to": "America/New_York"}
  {"action": "diff", "a": "2026-07-01", "b": "2026-12-25"}    -> days between two dates
"""
import json
import sys
from datetime import datetime, timezone

try:
    from zoneinfo import ZoneInfo
except Exception:
    ZoneInfo = None


def _parse(s):
    s = (s or "").strip().replace("T", " ").replace("Z", "")
    for fmt in ("%Y-%m-%d %H:%M:%S", "%Y-%m-%d %H:%M", "%Y-%m-%d", "%d.%m.%Y", "%m/%d/%Y"):
        try:
            return datetime.strptime(s, fmt)
        except ValueError:
            continue
    raise ValueError("could not parse date/time %r (try YYYY-MM-DD or YYYY-MM-DD HH:MM)" % s)


def _zone(name):
    if not name:
        return timezone.utc
    if ZoneInfo is None:
        raise ValueError("timezone database unavailable on this host")
    try:
        return ZoneInfo(name)
    except Exception:
        raise ValueError("unknown timezone %r (use IANA names like Europe/Berlin)" % name)


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0
    action = (q.get("action") or "now").lower()
    try:
        if action == "now":
            tz = _zone(q.get("tz"))
            now = datetime.now(tz)
            print(json.dumps({"tz": str(q.get("tz") or "UTC"),
                              "iso": now.isoformat(timespec="seconds"),
                              "pretty": now.strftime("%A, %d %B %Y, %H:%M %Z")}, indent=2))
        elif action == "convert":
            src = _parse(q.get("time")).replace(tzinfo=_zone(q.get("from")))
            dst = src.astimezone(_zone(q.get("to")))
            print(json.dumps({
                "from": {"tz": q.get("from") or "UTC", "time": src.isoformat(timespec="minutes")},
                "to": {"tz": q.get("to") or "UTC", "time": dst.isoformat(timespec="minutes"),
                       "pretty": dst.strftime("%A, %d %B %Y, %H:%M %Z")}}, indent=2))
        elif action == "diff":
            a, b = _parse(q.get("a")), _parse(q.get("b"))
            secs = (b - a).total_seconds()
            print(json.dumps({"a": a.date().isoformat(), "b": b.date().isoformat(),
                              "days": round(secs / 86400, 2), "seconds": int(secs)}, indent=2))
        else:
            print(json.dumps({"error": "unknown action %r" % action, "actions": ["now", "convert", "diff"]}))
        return 0
    except Exception as e:
        print(json.dumps({"error": str(e)}))
        return 0


if __name__ == "__main__":
    sys.exit(main())
