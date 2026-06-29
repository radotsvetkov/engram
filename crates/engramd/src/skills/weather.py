#!/usr/bin/env python3
"""weather — Engram seed skill (Process/python3, capability: Net).

Doubles as the reference `http_api` skill template: read JSON on stdin, call a
typed HTTP API with stdlib only (no `pip` in the sandbox), write JSON on stdout,
fail soft. Uses Open-Meteo, which is FREE and needs NO API KEY — so it works the
moment the skill is seeded, and is the pattern to copy when minting new API
skills (flights, maps, finance, ...).

Request (stdin):
    {"location": "Tangier, Morocco", "days": 3}
  or, to skip geocoding:
    {"latitude": 35.78, "longitude": -5.81, "name": "Tangier", "days": 3}

Output (stdout): JSON with resolved place, current conditions, daily forecast,
and a one-line summary.
"""

import json
import sys
import urllib.parse
import urllib.request

TIMEOUT = 20
UA = "engram-weather/1"

# WMO weather interpretation codes -> short text.
WMO = {
    0: "clear sky", 1: "mainly clear", 2: "partly cloudy", 3: "overcast",
    45: "fog", 48: "rime fog", 51: "light drizzle", 53: "drizzle",
    55: "dense drizzle", 61: "light rain", 63: "rain", 65: "heavy rain",
    66: "freezing rain", 67: "heavy freezing rain", 71: "light snow",
    73: "snow", 75: "heavy snow", 77: "snow grains", 80: "light showers",
    81: "showers", 82: "violent showers", 85: "snow showers",
    86: "heavy snow showers", 95: "thunderstorm", 96: "thunderstorm w/ hail",
    99: "thunderstorm w/ heavy hail",
}


def _get_json(url):
    req = urllib.request.Request(url)
    req.add_header("User-Agent", UA)
    req.add_header("Accept", "application/json")
    with urllib.request.urlopen(req, timeout=TIMEOUT) as resp:
        return json.loads(resp.read().decode("utf-8", "replace"))


def geocode(name):
    url = "https://geocoding-api.open-meteo.com/v1/search?" + urllib.parse.urlencode(
        {"name": name, "count": 1, "language": "en", "format": "json"}
    )
    raw = _get_json(url)
    results = raw.get("results") or []
    if not results:
        raise RuntimeError("no place matched %r" % name)
    r = results[0]
    label = ", ".join(
        x for x in [r.get("name"), r.get("admin1"), r.get("country")] if x
    )
    return r["latitude"], r["longitude"], label


def forecast(lat, lon, days):
    url = "https://api.open-meteo.com/v1/forecast?" + urllib.parse.urlencode(
        {
            "latitude": lat,
            "longitude": lon,
            "current": "temperature_2m,relative_humidity_2m,weather_code,wind_speed_10m",
            "daily": "weather_code,temperature_2m_max,temperature_2m_min,"
                     "precipitation_probability_max",
            "forecast_days": max(1, min(int(days or 3), 16)),
            "timezone": "auto",
        }
    )
    return _get_json(url)


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e}))
        return 0

    days = q.get("days", 3)
    try:
        if q.get("latitude") is not None and q.get("longitude") is not None:
            lat, lon, label = q["latitude"], q["longitude"], q.get("name", "")
        elif q.get("location"):
            lat, lon, label = geocode(q["location"])
        else:
            print(
                json.dumps(
                    {
                        "error": "provide 'location' (a place name) or 'latitude'+'longitude'",
                        "example": {"location": "Tangier, Morocco", "days": 3},
                    }
                )
            )
            return 0

        fc = forecast(lat, lon, days)
        cur = fc.get("current", {})
        cur_code = cur.get("weather_code")
        units = fc.get("current_units", {})
        daily = fc.get("daily", {})
        rows = []
        times = daily.get("time", [])
        for i, day in enumerate(times):
            rows.append(
                {
                    "date": day,
                    "min_c": daily.get("temperature_2m_min", [None] * len(times))[i],
                    "max_c": daily.get("temperature_2m_max", [None] * len(times))[i],
                    "precip_prob_pct": daily.get(
                        "precipitation_probability_max", [None] * len(times)
                    )[i],
                    "conditions": WMO.get(
                        daily.get("weather_code", [None] * len(times))[i], "unknown"
                    ),
                }
            )

        summary = "%s: now %s%s, %s; today %s-%s C" % (
            label or ("%.2f,%.2f" % (lat, lon)),
            cur.get("temperature_2m"),
            units.get("temperature_2m", "C"),
            WMO.get(cur_code, "unknown"),
            rows[0]["min_c"] if rows else "?",
            rows[0]["max_c"] if rows else "?",
        )
        print(
            json.dumps(
                {
                    "place": label,
                    "latitude": lat,
                    "longitude": lon,
                    "current": {
                        "temperature_c": cur.get("temperature_2m"),
                        "humidity_pct": cur.get("relative_humidity_2m"),
                        "wind_kmh": cur.get("wind_speed_10m"),
                        "conditions": WMO.get(cur_code, "unknown"),
                    },
                    "daily": rows,
                    "summary": summary,
                    "source": "open-meteo.com (free, no key)",
                },
                indent=2,
                default=str,
            )
        )
        return 0
    except Exception as e:
        print(json.dumps({"error": "weather lookup failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
