#!/usr/bin/env python3
"""dns — Engram skill (keyless). Resolve DNS records via Google DNS-over-HTTPS.

Request: {"name": "example.com", "type": "A"} where type is one of
A|AAAA|MX|TXT|NS|CNAME|SOA (defaults to "A"). GETs https://dns.google/resolve
with the Accept: application/dns-json header. Output:
{name, type, status, records:[{data, ttl}, ...]} where status is "NOERROR"
when the lookup succeeds, else the numeric status as a string.
"""
import json
import sys
import urllib.parse
import urllib.request

TIMEOUT = 20
UA = "engram-dns/1"
TYPES = {"A", "AAAA", "MX", "TXT", "NS", "CNAME", "SOA"}


def _get(url):
    req = urllib.request.Request(
        url, headers={"User-Agent": UA, "Accept": "application/dns-json"}
    )
    with urllib.request.urlopen(req, timeout=TIMEOUT) as r:
        return json.loads(r.read().decode("utf-8", "replace"))


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e}))
        return 0
    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"name": "example.com", "type": "A"},
        }))
        return 0
    name = (q.get("name") or q.get("domain") or q.get("host") or "").strip()
    if not name:
        print(json.dumps({
            "error": "missing 'name'",
            "example": {"name": "example.com", "type": "A"},
        }))
        return 0
    rtype = (q.get("type") or "A").strip().upper()
    if rtype not in TYPES:
        print(json.dumps({
            "error": "unsupported record type: %s" % rtype,
            "how_to_fix": {"type": "one of " + ", ".join(sorted(TYPES))},
            "example": {"name": "example.com", "type": "A"},
        }))
        return 0
    try:
        url = "https://dns.google/resolve?" + urllib.parse.urlencode(
            {"name": name, "type": rtype}
        )
        data = _get(url)
        if not isinstance(data, dict):
            print(json.dumps({"error": "unexpected DNS response", "name": name, "type": rtype}))
            return 0
        status_code = data.get("Status")
        status = "NOERROR" if status_code == 0 else str(status_code)
        records = []
        for ans in (data.get("Answer") or []):
            if not isinstance(ans, dict):
                continue
            records.append({"data": ans.get("data"), "ttl": ans.get("TTL")})
        result = {
            "name": name,
            "type": rtype,
            "status": status,
            "records": records,
        }
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "dns failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
