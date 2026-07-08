#!/usr/bin/env python3
"""security_headers — Engram skill (network). Check a URL's security headers.

Fetches the URL with a plain GET and checks for the presence of six common
security-relevant response headers, then grades the result A-D. Headers are
still captured even from error responses (e.g. 404), since misconfigured
sites often still return security headers on error pages.

Request (stdin): {"url": "https://example.com"}
Output (stdout): {url, headers: {<name>: {present, value}, ...}, grade}
"""
import json
import sys
import urllib.error
import urllib.request

TIMEOUT = 20
UA = "engram-security-headers/1"
CHECKED_HEADERS = [
    "Strict-Transport-Security",
    "Content-Security-Policy",
    "X-Frame-Options",
    "X-Content-Type-Options",
    "Referrer-Policy",
    "Permissions-Policy",
]


def _grade(present_count):
    if present_count == 6:
        return "A"
    if present_count >= 4:
        return "B"
    if present_count >= 2:
        return "C"
    return "D"


def _build_result(url, headers_obj):
    headers_out = {}
    present_count = 0
    for name in CHECKED_HEADERS:
        value = headers_obj.get(name)
        present = value is not None
        if present:
            present_count += 1
        headers_out[name] = {"present": present, "value": value}
    return {
        "url": url,
        "headers": headers_out,
        "grade": _grade(present_count),
    }


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e}))
        return 0

    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object",
                          "example": {"url": "https://example.com"}}))
        return 0

    url = (q.get("url") or "").strip()
    if not url:
        print(json.dumps({"error": "provide a 'url'",
                          "example": {"url": "https://example.com"}}))
        return 0
    if not (url.startswith("http://") or url.startswith("https://")):
        url = "https://" + url

    try:
        req = urllib.request.Request(url, method="GET", headers={"User-Agent": UA})
        try:
            with urllib.request.urlopen(req, timeout=TIMEOUT) as resp:
                resp.read(1)
                result = _build_result(url, resp.headers)
                print(json.dumps(result, indent=2, default=str))
                return 0
        except urllib.error.HTTPError as e:
            # error responses can still carry security headers worth checking
            result = _build_result(url, e.headers or {})
            result["http_status"] = e.code
            print(json.dumps(result, indent=2, default=str))
            return 0
    except urllib.error.URLError as e:
        print(json.dumps({
            "error": "security_headers failed: network error: %s" % e.reason,
            "how_to_fix": "check the URL is reachable and the hostname resolves.",
        }))
        return 0
    except Exception as e:
        print(json.dumps({"error": "security_headers failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
