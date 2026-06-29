#!/usr/bin/env python3
"""holidays — Engram skill (keyless). Public holidays for a country and year via Nager.Date.

Request (stdin): {"country": "US", "year": 2026}  — country is an ISO-2 code (US, DE, MA);
year defaults to the current year. Fetches https://date.nager.at/api/v3/PublicHolidays.
Output (stdout): {country, year, count, holidays: [{date, name, localName}]}.
"""
import json
import sys
import datetime
import urllib.request
import urllib.error

TIMEOUT = 20
UA = "engram-holidays/1"


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e}))
        return 0

    country = str(q.get("country") or q.get("code") or "").strip().upper()
    if not country:
        print(json.dumps({
            "error": "provide 'country' as an ISO-2 code (e.g. US, DE, MA)",
            "example": {"country": "US", "year": datetime.datetime.now().year},
        }))
        return 0
    if not country.isalpha() or len(country) != 2:
        print(json.dumps({
            "error": "unknown country code %s (use ISO-2 like US, DE)" % country,
            "example": {"country": "DE", "year": datetime.datetime.now().year},
        }))
        return 0

    year = q.get("year")
    if year in (None, ""):
        year = datetime.datetime.now().year
    try:
        year = int(year)
    except Exception:
        print(json.dumps({
            "error": "year must be an integer (e.g. %d)" % datetime.datetime.now().year,
            "example": {"country": country, "year": datetime.datetime.now().year},
        }))
        return 0

    url = "https://date.nager.at/api/v3/PublicHolidays/%d/%s" % (year, country)
    try:
        req = urllib.request.Request(url, headers={"User-Agent": UA, "Accept": "application/json"})
        try:
            with urllib.request.urlopen(req, timeout=TIMEOUT) as r:
                data = json.loads(r.read().decode("utf-8", "replace"))
        except urllib.error.HTTPError as he:
            if he.code == 404:
                print(json.dumps({"error": "unknown country code %s (use ISO-2 like US, DE)" % country}))
                return 0
            print(json.dumps({"error": "holidays lookup failed: HTTP %s" % he.code}))
            return 0

        if not isinstance(data, list):
            print(json.dumps({"error": "unexpected response for %s %d" % (country, year)}))
            return 0

        holidays = []
        for h in data:
            if not isinstance(h, dict):
                continue
            holidays.append({
                "date": h.get("date", ""),
                "name": h.get("name", ""),
                "localName": h.get("localName", ""),
            })

        print(json.dumps({
            "country": country,
            "year": year,
            "count": len(holidays),
            "holidays": holidays,
        }, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "holidays failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
