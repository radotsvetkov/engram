#!/usr/bin/env python3
"""ip_lookup — Engram skill (keyless). Geolocate / get info for an IP address.

Uses the free, keyless ip-api.com service. Request: {"ip": "8.8.8.8"} — omit
"ip" to geolocate the caller's own public IP. GET http://ip-api.com/json/<ip>.
Output: {ip, country, region, city, lat, lon, timezone, isp, org}.
"""
import json
import sys
import urllib.parse
import urllib.request

TIMEOUT = 20
UA = "engram-ip_lookup/1"
FIELDS = "status,message,query,country,regionName,city,zip,lat,lon,timezone,isp,org,as"


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
    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object", "example": {"ip": "8.8.8.8"}}))
        return 0
    ip = (q.get("ip") or q.get("address") or q.get("query") or "").strip()
    try:
        # Empty path -> ip-api looks up the caller's own public IP.
        url = "http://ip-api.com/json/" + urllib.parse.quote(ip) + "?" + urllib.parse.urlencode({"fields": FIELDS})
        data = _get(url)
        if data.get("status") != "success":
            print(json.dumps({"error": data.get("message") or "IP lookup failed", "ip": ip or None}))
            return 0
        result = {
            "ip": data.get("query"),
            "country": data.get("country"),
            "region": data.get("regionName"),
            "city": data.get("city"),
            "lat": data.get("lat"),
            "lon": data.get("lon"),
            "timezone": data.get("timezone"),
            "isp": data.get("isp"),
            "org": data.get("org") or data.get("as"),
        }
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "ip_lookup failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
