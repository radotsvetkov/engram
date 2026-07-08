#!/usr/bin/env python3
"""curl_builder — Engram skill (no network). Build a shell-safe curl command
string from a method/url/headers/body/query description (does not execute it).

Merges `query` params into the URL (preserving any existing query string),
JSON-encodes a dict `body` (auto-adding Content-Type: application/json if not
already set, case-insensitively), and shell-escapes every token with
shlex.quote so the result is safe to paste directly into a shell.

Request (stdin): {"method": "POST", "url": "https://api.example.com/x",
                   "headers": {"Authorization": "Bearer t"}, "body": {"a": 1},
                   "query": {"page": 2}}
Output (stdout): {command}
"""
import json
import shlex
import sys
import urllib.parse

_EXAMPLE = {"method": "POST", "url": "https://api.example.com/users", "body": {"name": "Ada"}}


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e})); return 0
    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object", "example": _EXAMPLE})); return 0

    url = q.get("url")
    if not isinstance(url, str) or not url.strip():
        print(json.dumps({
            "error": "missing required field 'url' (string)",
            "example": _EXAMPLE,
        })); return 0

    method = q.get("method") or "GET"
    if not isinstance(method, str) or not method.strip():
        method = "GET"
    method = method.strip().upper()

    headers = q.get("headers") or {}
    if not isinstance(headers, dict):
        print(json.dumps({"error": "'headers' must be an object of string:string", "example": _EXAMPLE})); return 0

    query = q.get("query") or {}
    if not isinstance(query, dict):
        print(json.dumps({"error": "'query' must be an object of param:value", "example": _EXAMPLE})); return 0

    body = q.get("body")

    try:
        final_url = url.strip()
        if query:
            parts = urllib.parse.urlsplit(final_url)
            existing = urllib.parse.parse_qsl(parts.query, keep_blank_values=True)
            existing.extend((str(k), str(v)) for k, v in query.items())
            new_query = urllib.parse.urlencode(existing)
            final_url = urllib.parse.urlunsplit((parts.scheme, parts.netloc, parts.path, new_query, parts.fragment))

        merged_headers = dict(headers)
        body_str = None
        if body is not None:
            if isinstance(body, (dict, list)):
                body_str = json.dumps(body)
                if not any(str(k).lower() == "content-type" for k in merged_headers):
                    merged_headers["Content-Type"] = "application/json"
            else:
                body_str = str(body)

        cmd = ["curl", "-X", shlex.quote(method), shlex.quote(final_url)]
        for k, v in merged_headers.items():
            cmd.extend(["-H", shlex.quote("%s: %s" % (k, v))])
        if body_str is not None:
            cmd.extend(["--data", shlex.quote(body_str)])

        command = " ".join(cmd)
        print(json.dumps({"command": command}, indent=2, default=str)); return 0
    except Exception as e:
        print(json.dumps({"error": "curl_builder failed: %s" % e})); return 1


if __name__ == "__main__":
    sys.exit(main())
