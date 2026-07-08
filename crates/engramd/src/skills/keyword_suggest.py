#!/usr/bin/env python3
"""keyword_suggest — Engram skill (network). Keyword ideas from Google's autocomplete endpoint.

Fetches Google's public, keyless "suggest" endpoint (the same one browser
address bars use for autocomplete) and returns the raw suggestion list for a
query. This is a free, unofficial endpoint — not guaranteed or stable — so
failures are reported clearly rather than crashing. Stdlib only.

Request (stdin): {"query": "email marketing", "limit": 10}
Output (stdout): {query, suggestions: [...]}
"""
import json
import sys
import urllib.error
import urllib.parse
import urllib.request

TIMEOUT = 20
MAX_LIMIT = 50

HOW_TO_FIX = (
    "this is a free, unofficial Google Suggest endpoint (the same one used by "
    "browser address-bar autocomplete) — it is not guaranteed or stable "
    "(it can rate-limit, geo-block, or change format without notice); for "
    "production keyword research use a dedicated tool/API (e.g. Google "
    "Keyword Planner, Ahrefs, SEMrush)."
)


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0

    example = {"query": "email marketing", "limit": 10}

    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object", "example": example}))
        return 0

    query = q.get("query")
    if not isinstance(query, str) or not query.strip():
        print(json.dumps({
            "error": "missing required field 'query' (string)",
            "example": example,
        }))
        return 0
    query = query.strip()

    limit_raw = q.get("limit", 10)
    try:
        limit = int(limit_raw) if limit_raw is not None else 10
    except (TypeError, ValueError):
        print(json.dumps({"error": "'limit' must be an integer", "example": example}))
        return 0
    limit = max(1, min(limit, MAX_LIMIT))

    url = "https://suggestqueries.google.com/complete/search?client=firefox&q=" + urllib.parse.quote(query)

    try:
        req = urllib.request.Request(url, headers={"User-Agent": "engram-keyword_suggest/1"})
        with urllib.request.urlopen(req, timeout=TIMEOUT) as r:
            raw = r.read()
    except urllib.error.HTTPError as e:
        print(json.dumps({
            "error": "HTTP error %s from Google Suggest" % e.code,
            "how_to_fix": HOW_TO_FIX,
        }))
        return 0
    except urllib.error.URLError as e:
        print(json.dumps({
            "error": "could not reach Google Suggest: %s" % e.reason,
            "how_to_fix": HOW_TO_FIX,
        }))
        return 0
    except Exception as e:
        print(json.dumps({
            "error": "request to Google Suggest failed: %s" % e,
            "how_to_fix": HOW_TO_FIX,
        }))
        return 0

    try:
        data = json.loads(raw.decode("utf-8"))
        suggestions = data[1]
        if not isinstance(suggestions, list):
            raise ValueError("unexpected response shape (expected a list at index 1)")
        suggestions = [str(s) for s in suggestions][:limit]
    except Exception as e:
        print(json.dumps({
            "error": "could not parse Google Suggest response: %s" % e,
            "how_to_fix": HOW_TO_FIX,
        }))
        return 0

    print(json.dumps({"query": query, "suggestions": suggestions}, indent=2, default=str))
    return 0


if __name__ == "__main__":
    sys.exit(main())
