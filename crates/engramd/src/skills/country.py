#!/usr/bin/env python3
"""country — Engram skill (keyless). Facts about a country.

Uses the World Bank API (free, no key, reliable). Stdlib only. Accepts a country
name or an ISO-2/ISO-3 code, and returns capital, region, income level, coords,
and the latest population.

Request (stdin): {"name": "Japan"}   (or {"name": "JP"} / {"name": "JPN"})
Output (stdout): {name, iso2, iso3, capital, region, income_level, latlng, population}
"""
import json
import sys
import urllib.request

TIMEOUT = 20
UA = "engram-country/1"


def _get(url):
    req = urllib.request.Request(url, headers={"User-Agent": UA, "Accept": "application/json"})
    with urllib.request.urlopen(req, timeout=TIMEOUT) as r:
        return json.loads(r.read().decode("utf-8", "replace"))


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e}))
        return 0
    name = (q.get("name") or q.get("country") or "").strip()
    if not name:
        print(json.dumps({"error": "provide 'name'", "example": {"name": "Japan"}}))
        return 0
    try:
        # The World Bank API has no name search, so fetch the country list once and match locally
        # (aggregates have region.id == 'NA'; real countries don't).
        raw = _get("https://api.worldbank.org/v2/country?format=json&per_page=400")
        rows = raw[1] if isinstance(raw, list) and len(raw) > 1 else []
        n = name.lower()
        match = None
        for c in rows:
            if (c.get("region") or {}).get("id") == "NA":
                continue  # skip aggregates (e.g. "World", "Euro area")
            if n in ((c.get("name") or "").lower(), (c.get("iso2Code") or "").lower(), (c.get("id") or "").lower()):
                match = c
                break
        if match is None:  # fall back to a substring match on the name
            for c in rows:
                if (c.get("region") or {}).get("id") != "NA" and n in (c.get("name") or "").lower():
                    match = c
                    break
        if match is None:
            print(json.dumps({"error": "no country matched %r" % name}))
            return 0

        iso3 = match.get("id", "")
        lat, lon = match.get("latitude"), match.get("longitude")
        population = None
        try:
            pop = _get("https://api.worldbank.org/v2/country/%s/indicator/SP.POP.TOTL?format=json&mrnev=1" % iso3)
            if isinstance(pop, list) and len(pop) > 1 and pop[1]:
                population = pop[1][0].get("value")
        except Exception:
            pass

        print(json.dumps({
            "name": match.get("name"),
            "iso2": match.get("iso2Code"),
            "iso3": iso3,
            "capital": match.get("capitalCity") or None,
            "region": (match.get("region") or {}).get("value"),
            "income_level": (match.get("incomeLevel") or {}).get("value"),
            "latlng": [float(lat), float(lon)] if lat and lon else None,
            "population": population,
        }, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "country lookup failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
