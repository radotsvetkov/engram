#!/usr/bin/env python3
"""air_quality — Engram skill (keyless | no network key). Current air quality for a place via the free Open-Meteo air-quality API.

Request: {"location": "<place name>"} OR {"latitude": <float>, "longitude": <float>}.
If a location name is given it is geocoded first via Open-Meteo's geocoding API.
Output: {place, us_aqi, category, pollutants:{pm2_5,pm10,ozone,...}, summary}.
"""
import json, sys
import urllib.request, urllib.parse, urllib.error

UA = "engram-air_quality/1"
GEOCODE_URL = "https://geocoding-api.open-meteo.com/v1/search"
AQ_URL = "https://air-quality-api.open-meteo.com/v1/air-quality"
POLLUTANTS = ["us_aqi", "pm2_5", "pm10", "ozone",
              "nitrogen_dioxide", "sulphur_dioxide", "carbon_monoxide"]


def _get_json(url, params):
    qs = urllib.parse.urlencode(params)
    full = "%s?%s" % (url, qs)
    req = urllib.request.Request(full, headers={"User-Agent": UA})
    with urllib.request.urlopen(req, timeout=20) as resp:
        return json.loads(resp.read().decode("utf-8", "replace"))


def _category(aqi):
    if aqi is None:
        return "Unknown"
    try:
        a = float(aqi)
    except (TypeError, ValueError):
        return "Unknown"
    if a <= 50:
        return "Good"
    if a <= 100:
        return "Moderate"
    if a <= 150:
        return "Unhealthy for sensitive"
    if a <= 200:
        return "Unhealthy"
    if a <= 300:
        return "Very unhealthy"
    return "Hazardous"


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e})); return 0
    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"location": "Sofia"},
        })); return 0

    lat = q.get("latitude")
    lon = q.get("longitude")
    location = q.get("location")
    place = None

    try:
        # Resolve coordinates: either from explicit lat/lon, or by geocoding a name.
        if lat is not None and lon is not None:
            try:
                lat = float(lat)
                lon = float(lon)
            except (TypeError, ValueError):
                print(json.dumps({
                    "error": "latitude/longitude must be numbers",
                    "example": {"latitude": 42.6977, "longitude": 23.3219},
                })); return 0
            place = q.get("place") or q.get("location") or "%.4f,%.4f" % (lat, lon)
        elif location:
            if not isinstance(location, str) or not location.strip():
                print(json.dumps({
                    "error": "location must be a non-empty place name",
                    "example": {"location": "Sofia"},
                })); return 0
            geo = _get_json(GEOCODE_URL, {
                "name": location.strip(),
                "count": 1,
                "format": "json",
            })
            results = geo.get("results") or []
            if not results:
                print(json.dumps({
                    "error": "no location found for %r" % location,
                    "how_to_fix": "Try a more specific place name, or pass latitude/longitude.",
                })); return 0
            top = results[0] or {}
            lat = top.get("latitude")
            lon = top.get("longitude")
            if lat is None or lon is None:
                print(json.dumps({
                    "error": "geocoder returned no coordinates for %r" % location,
                })); return 0
            name = top.get("name") or location.strip()
            country = top.get("country")
            place = "%s, %s" % (name, country) if country else name
        else:
            print(json.dumps({
                "error": "provide either {location} or {latitude, longitude}",
                "example": {"location": "Sofia"},
            })); return 0

        # Fetch current air quality.
        data = _get_json(AQ_URL, {
            "latitude": lat,
            "longitude": lon,
            "current": ",".join(POLLUTANTS),
            "timezone": "auto",
        })
        current = data.get("current") or {}
        units = data.get("current_units") or {}

        us_aqi = current.get("us_aqi")
        category = _category(us_aqi)

        pollutants = {}
        for key in ["pm2_5", "pm10", "ozone", "nitrogen_dioxide",
                    "sulphur_dioxide", "carbon_monoxide"]:
            val = current.get(key)
            if val is None:
                continue
            unit = units.get(key)
            pollutants[key] = {"value": val, "unit": unit} if unit else {"value": val}

        if us_aqi is None:
            summary = "Air quality for %s: US AQI unavailable." % place
        else:
            summary = "Air quality for %s: US AQI %s (%s)." % (place, us_aqi, category)

        result = {
            "place": place,
            "latitude": lat,
            "longitude": lon,
            "us_aqi": us_aqi,
            "category": category,
            "pollutants": pollutants,
            "observed_at": current.get("time"),
            "summary": summary,
        }
        print(json.dumps(result, indent=2, default=str)); return 0
    except urllib.error.HTTPError as e:
        print(json.dumps({"error": "air_quality failed: HTTP %s %s" % (e.code, e.reason)})); return 1
    except urllib.error.URLError as e:
        print(json.dumps({"error": "air_quality failed: network error: %s" % e.reason})); return 1
    except Exception as e:
        print(json.dumps({"error": "air_quality failed: %s" % e})); return 1


if __name__ == "__main__":
    sys.exit(main())
