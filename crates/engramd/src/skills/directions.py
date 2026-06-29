#!/usr/bin/env python3
"""directions — Engram skill (keyless). Driving distance + time between two places via OSRM.

Resolves driving distance and travel time between two points using only keyless
public services: Open-Meteo geocoding for place names and the OSRM public router.
Request (stdin): {"from": "Berlin", "to": "Hamburg"} OR coords {"from": [lon, lat], "to": [lon, lat]}.
Output (stdout): {from, to, distance_km, distance_mi, duration_min}.
"""
import json
import math
import sys
import urllib.parse
import urllib.request

TIMEOUT = 20
UA = "engram-directions/1 (engram skill)"
GEOCODE = "https://geocoding-api.open-meteo.com/v1/search"
# OSRM public router. The CDN in front of it rejects some Python TLS handshakes (curl succeeds,
# urllib may not), so we try HTTPS first and fall back to HTTP — the request carries only coords.
OSRM_HOST = "router.project-osrm.org/route/v1/driving"


def _get(url):
    req = urllib.request.Request(url, headers={"User-Agent": UA, "Accept": "application/json"})
    with urllib.request.urlopen(req, timeout=TIMEOUT) as r:
        return json.loads(r.read().decode("utf-8", "replace"))


def _osrm(coords):
    """Fetch the OSRM route, trying HTTPS then HTTP. Returns parsed JSON or raises the last error."""
    path = "%s/%s?%s" % (OSRM_HOST, coords, urllib.parse.urlencode({"overview": "false"}))
    last = None
    for scheme in ("https://", "http://"):
        try:
            return _get(scheme + path)
        except Exception as e:  # try the next scheme
            last = e
    raise last


def _haversine(a, b):
    """Great-circle distance in km between [lon, lat] points a and b."""
    lon1, lat1, lon2, lat2 = map(math.radians, [a[0], a[1], b[0], b[1]])
    h = math.sin((lat2 - lat1) / 2) ** 2 + math.cos(lat1) * math.cos(lat2) * math.sin((lon2 - lon1) / 2) ** 2
    return 2 * 6371.0 * math.asin(math.sqrt(h))


def _to_float(v):
    try:
        return float(v)
    except (TypeError, ValueError):
        return None


def _resolve(place):
    """Turn a place spec into [lon, lat]. Accepts a name string or a [lon, lat] pair.

    Returns (coords, label) on success or (None, error_message) on failure.
    """
    # Coordinate pair: accept list/tuple [lon, lat] (also dicts with lon/lat keys).
    if isinstance(place, (list, tuple)):
        if len(place) != 2:
            return None, "coordinate pair must be [lon, lat]"
        lon, lat = _to_float(place[0]), _to_float(place[1])
        if lon is None or lat is None:
            return None, "coordinate pair must be numeric [lon, lat]"
        return [lon, lat], "%.5f,%.5f" % (lon, lat)
    if isinstance(place, dict):
        lon = _to_float(place.get("lon", place.get("lng", place.get("longitude"))))
        lat = _to_float(place.get("lat", place.get("latitude")))
        if lon is None or lat is None:
            return None, "coordinate object needs 'lon' and 'lat'"
        return [lon, lat], "%.5f,%.5f" % (lon, lat)

    # Place name: geocode it via Open-Meteo (keyless).
    name = ("" if place is None else str(place)).strip()
    if not name:
        return None, "empty place"
    url = GEOCODE + "?" + urllib.parse.urlencode(
        {"name": name, "count": 1, "format": "json", "language": "en"})
    data = _get(url)
    results = data.get("results") or []
    if not results:
        return None, "could not geocode %r" % name
    top = results[0]
    lon, lat = _to_float(top.get("longitude")), _to_float(top.get("latitude"))
    if lon is None or lat is None:
        return None, "geocoder returned no coordinates for %r" % name
    return [lon, lat], name


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e}))
        return 0
    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object",
                          "example": {"from": "Berlin", "to": "Hamburg"}}))
        return 0

    # "from" is a Python keyword, so always read it via .get().
    frm = q.get("from")
    to = q.get("to")
    if frm is None or to is None:
        print(json.dumps({
            "error": "provide both 'from' and 'to' (place names or [lon, lat] pairs)",
            "example": {"from": "Berlin", "to": "Hamburg"},
            "example_coords": {"from": [13.405, 52.52], "to": [9.993, 53.551]},
        }))
        return 0

    try:
        from_coords, from_label = _resolve(frm)
        if from_coords is None:
            print(json.dumps({"error": "from: %s" % from_label}))
            return 0
        to_coords, to_label = _resolve(to)
        if to_coords is None:
            print(json.dumps({"error": "to: %s" % to_label}))
            return 0

        coords = "%f,%f;%f,%f" % (
            from_coords[0], from_coords[1], to_coords[0], to_coords[1])

        # Driving route via OSRM (HTTPS→HTTP). If it's unreachable, fall back to the straight-line
        # (great-circle) distance so the skill still returns something useful instead of failing.
        r = None
        try:
            route = _osrm(coords)
            routes = route.get("routes") or []
            r = routes[0] if route.get("code") == "Ok" and routes else None
        except Exception:
            r = None

        if r is not None:
            dist = _to_float(r.get("distance"))
            dur = _to_float(r.get("duration"))
            if dist is not None and dur is not None:
                print(json.dumps({
                    "from": from_label, "to": to_label, "mode": "driving",
                    "distance_km": round(dist / 1000.0, 1),
                    "distance_mi": round(dist / 1609.344, 1),
                    "duration_min": round(dur / 60),
                }, indent=2, default=str))
                return 0

        # Fallback: straight-line distance (always available once both points are geocoded).
        km = _haversine(from_coords, to_coords)
        print(json.dumps({
            "from": from_label, "to": to_label, "mode": "straight_line",
            "distance_km": round(km, 1), "distance_mi": round(km / 1.609344, 1),
            "note": "driving-route service unavailable — showing great-circle (straight-line) distance",
        }, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "directions failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
