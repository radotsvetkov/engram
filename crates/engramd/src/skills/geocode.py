#!/usr/bin/env python3
"""geocode — Engram skill (keyless). Forward + reverse geocoding via OpenStreetMap Nominatim.

Resolves a place name to coordinates, or coordinates back to an address. Keyless;
Nominatim requires a descriptive User-Agent header or it rejects the request.
Request (stdin): EITHER {"query": "Eiffel Tower"} OR {"latitude": 48.8584, "longitude": 2.2945}.
Output (stdout): forward -> {query, lat, lon, display_name, type, address}; reverse -> {lat, lon, display_name, address}.
"""
import json
import sys
import urllib.parse
import urllib.request

TIMEOUT = 20
# Nominatim demands a real, identifying User-Agent; a generic one gets a 403.
UA = "engram-geocode/1 (engram skill)"
BASE = "https://nominatim.openstreetmap.org"


def _get(url):
    req = urllib.request.Request(url, headers={"User-Agent": UA, "Accept": "application/json"})
    with urllib.request.urlopen(req, timeout=TIMEOUT) as r:
        return json.loads(r.read().decode("utf-8", "replace"))


def _to_float(v):
    try:
        return float(v)
    except (TypeError, ValueError):
        return None


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e}))
        return 0
    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object",
                          "example": {"query": "Eiffel Tower"}}))
        return 0

    query = (q.get("query") or q.get("q") or "").strip()
    # Accept lat/lon under several common spellings.
    lat = _to_float(q.get("latitude", q.get("lat")))
    lon = _to_float(q.get("longitude", q.get("lon", q.get("lng", q.get("long")))))

    if not query and (lat is None or lon is None):
        print(json.dumps({
            "error": "provide either 'query' (place name) or both 'latitude' and 'longitude'",
            "example": {"query": "Eiffel Tower"},
            "example_reverse": {"latitude": 48.8584, "longitude": 2.2945},
        }))
        return 0

    try:
        if query:
            # Forward geocoding: name -> coordinates.
            url = BASE + "/search?" + urllib.parse.urlencode({
                "q": query,
                "format": "jsonv2",
                "limit": 1,
                "addressdetails": 1,
            })
            data = _get(url)
            if not isinstance(data, list) or not data:
                print(json.dumps({"error": "no place matched %r" % query}))
                return 0
            hit = data[0] if isinstance(data[0], dict) else {}
            print(json.dumps({
                "query": query,
                "lat": _to_float(hit.get("lat")),
                "lon": _to_float(hit.get("lon")),
                "display_name": hit.get("display_name", ""),
                "type": hit.get("type", ""),
                "address": hit.get("address", {}),
            }, indent=2, default=str))
            return 0
        else:
            # Reverse geocoding: coordinates -> address.
            url = BASE + "/reverse?" + urllib.parse.urlencode({
                "lat": lat,
                "lon": lon,
                "format": "jsonv2",
                "addressdetails": 1,
            })
            data = _get(url)
            if not isinstance(data, dict) or data.get("error"):
                err = data.get("error") if isinstance(data, dict) else None
                print(json.dumps({"error": "no address matched %s,%s%s" % (
                    lat, lon, (": %s" % err) if err else "")}))
                return 0
            print(json.dumps({
                "lat": _to_float(data.get("lat")) if data.get("lat") is not None else lat,
                "lon": _to_float(data.get("lon")) if data.get("lon") is not None else lon,
                "display_name": data.get("display_name", ""),
                "address": data.get("address", {}),
            }, indent=2, default=str))
            return 0
    except Exception as e:
        print(json.dumps({"error": "geocode failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
