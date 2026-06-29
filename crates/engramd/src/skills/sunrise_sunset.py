#!/usr/bin/env python3
"""sunrise_sunset — Engram skill (keyless). Sunrise/sunset and twilight times for any place.

Takes {latitude, longitude, date?} OR {location}. When a location name is given it is
geocoded via Open-Meteo first, then sun times are fetched from sunrise-sunset.org.
Returns {place(if geocoded), date, sunrise, sunset, solar_noon, day_length_seconds}.
All sun times are UTC ISO-8601 strings.
"""
import json, sys, urllib.request, urllib.parse, urllib.error

UA = "engram-skill/sunrise_sunset (+https://engram.local)"


def _get_json(url):
    req = urllib.request.Request(url, headers={"User-Agent": UA})
    with urllib.request.urlopen(req, timeout=20) as resp:
        return json.loads(resp.read().decode("utf-8", "replace"))


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e})); return 0
    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"location": "Lisbon"},
        })); return 0

    lat = q.get("latitude")
    lon = q.get("longitude")
    location = q.get("location")
    date = q.get("date") or "today"
    place = None

    # Resolve coordinates: prefer explicit lat/lon, else geocode the location name.
    if lat is None or lon is None:
        if not location:
            print(json.dumps({
                "error": "provide either {latitude, longitude} or {location}",
                "example": {"location": "Tokyo", "date": "2026-06-21"},
            })); return 0
        try:
            geo_url = "https://geocoding-api.open-meteo.com/v1/search?" + urllib.parse.urlencode(
                {"name": str(location), "count": 1, "format": "json"}
            )
            geo = _get_json(geo_url)
            results = geo.get("results") or []
            if not results:
                print(json.dumps({
                    "error": "could not geocode location: %s" % location,
                    "how_to_fix": "try a more specific place name, or pass latitude/longitude directly",
                })); return 0
            top = results[0] or {}
            lat = top.get("latitude")
            lon = top.get("longitude")
            if lat is None or lon is None:
                print(json.dumps({"error": "geocoder returned no coordinates for: %s" % location})); return 0
            name = top.get("name")
            country = top.get("country")
            place = ", ".join([p for p in (name, country) if p]) or name
        except urllib.error.URLError as e:
            print(json.dumps({"error": "geocoding request failed: %s" % e})); return 0
        except Exception as e:
            print(json.dumps({"error": "geocoding failed: %s" % e})); return 0

    # Fetch sunrise/sunset (formatted=0 -> UTC ISO times, day_length in seconds).
    try:
        sun_url = "https://api.sunrise-sunset.org/json?" + urllib.parse.urlencode(
            {"lat": lat, "lng": lon, "date": date, "formatted": 0}
        )
        data = _get_json(sun_url)
    except urllib.error.URLError as e:
        print(json.dumps({"error": "sunrise-sunset request failed: %s" % e})); return 0
    except Exception as e:
        print(json.dumps({"error": "sunrise_sunset failed: %s" % e})); return 1

    status = data.get("status")
    if status != "OK":
        print(json.dumps({"error": status or "sunrise-sunset returned no status"})); return 0

    res = data.get("results") or {}
    out = {
        "date": date,
        "latitude": lat,
        "longitude": lon,
        "sunrise": res.get("sunrise"),
        "sunset": res.get("sunset"),
        "solar_noon": res.get("solar_noon"),
        "civil_twilight_begin": res.get("civil_twilight_begin"),
        "civil_twilight_end": res.get("civil_twilight_end"),
        "day_length_seconds": res.get("day_length"),
    }
    if place:
        out = dict([("place", place)] + list(out.items()))

    print(json.dumps(out, indent=2, default=str)); return 0


if __name__ == "__main__":
    sys.exit(main())
