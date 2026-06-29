#!/usr/bin/env python3
"""wayback — Engram skill (keyless). Find an archived snapshot of a URL in the Internet Archive Wayback Machine.

Queries the Wayback Machine "available" API for the closest archived snapshot of a URL.
Request: {"url": "<url>", "timestamp": "20200101"?}  (timestamp optional, near which to search).
Output: {url, archived, snapshot_url, snapshot_time(ISO), status} when found, else
{url, archived:false, message}. Keyless, no signup required.
"""
import json, sys, urllib.request, urllib.parse, urllib.error

API = "http://archive.org/wayback/available"
UA = "Engram/1.0 (+https://archive.org wayback skill)"


def _parse_ts(ts):
    """Parse a 14-digit YYYYMMDDhhmmss Wayback timestamp into an ISO 8601 string."""
    ts = str(ts or "").strip()
    if len(ts) != 14 or not ts.isdigit():
        return ts or None
    try:
        return "%s-%s-%sT%s:%s:%sZ" % (
            ts[0:4], ts[4:6], ts[6:8], ts[8:10], ts[10:12], ts[12:14]
        )
    except Exception:
        return ts


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e})); return 0

    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"url": "https://example.com", "timestamp": "20200101"},
        })); return 0

    url = q.get("url")
    if not url or not isinstance(url, str) or not url.strip():
        print(json.dumps({
            "error": "missing required field 'url'",
            "example": {"url": "https://example.com", "timestamp": "20200101"},
        })); return 0
    url = url.strip()

    timestamp = q.get("timestamp") or ""
    if not isinstance(timestamp, (str, int)):
        timestamp = ""
    timestamp = str(timestamp).strip()

    try:
        params = {"url": url}
        if timestamp:
            params["timestamp"] = timestamp
        full_url = API + "?" + urllib.parse.urlencode(params)
        req = urllib.request.Request(full_url, headers={"User-Agent": UA})
        with urllib.request.urlopen(req, timeout=20) as resp:
            raw = resp.read().decode("utf-8", "replace")
        data = json.loads(raw or "{}")
        if not isinstance(data, dict):
            data = {}

        snapshots = data.get("archived_snapshots") or {}
        if not isinstance(snapshots, dict):
            snapshots = {}
        closest = snapshots.get("closest") or {}
        if not isinstance(closest, dict):
            closest = {}

        available = bool(closest.get("available"))
        snapshot_url = closest.get("url")

        if closest and available and snapshot_url:
            result = {
                "url": url,
                "archived": True,
                "snapshot_url": snapshot_url,
                "snapshot_time": _parse_ts(closest.get("timestamp")),
                "status": closest.get("status"),
            }
        else:
            result = {
                "url": url,
                "archived": False,
                "message": "no archived snapshot found",
            }
        print(json.dumps(result, indent=2, default=str)); return 0
    except urllib.error.HTTPError as e:
        print(json.dumps({"error": "wayback failed: HTTP %s %s" % (e.code, e.reason)})); return 1
    except urllib.error.URLError as e:
        print(json.dumps({"error": "wayback failed: network error: %s" % e.reason})); return 1
    except Exception as e:
        print(json.dumps({"error": "wayback failed: %s" % e})); return 1


if __name__ == "__main__":
    sys.exit(main())
