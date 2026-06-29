#!/usr/bin/env python3
"""stackoverflow — Engram skill (keyless). Search Stack Overflow Q&A via the StackExchange API.

Queries the keyless StackExchange search/advanced endpoint and returns matching questions.
Request shape: {"query": "python asyncio gather", "limit": 5}. The StackExchange API
ALWAYS gzip-encodes its response body, so the raw bytes are gunzipped before json.loads.
Output: {query, results:[{title, score, answered, answers, link, tags}], quota_remaining?}.
"""
import gzip
import json
import sys
import urllib.parse
import urllib.request

TIMEOUT = 20
UA = "engram-stackoverflow/1"
BASE = "https://api.stackexchange.com/2.3/search/advanced"


def _get(url):
    req = urllib.request.Request(url, headers={"User-Agent": UA, "Accept": "application/json"})
    with urllib.request.urlopen(req, timeout=TIMEOUT) as r:
        raw = r.read()
    # The StackExchange API always gzip-encodes the body regardless of Accept-Encoding.
    try:
        raw = gzip.decompress(raw)
    except (OSError, EOFError):
        pass  # already-decoded body (e.g. transparent transport); use as-is
    return json.loads(raw.decode("utf-8", "replace"))


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e}))
        return 0
    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"query": "python asyncio gather", "limit": 5},
        }))
        return 0

    query = str(q.get("query") or "").strip()
    if not query:
        print(json.dumps({
            "error": "missing required field 'query'",
            "example": {"query": "python asyncio gather", "limit": 5},
        }))
        return 0

    # clamp limit to a sane range; ignore non-numeric input
    try:
        limit = int(q.get("limit", 5))
    except (TypeError, ValueError):
        limit = 5
    if limit < 1:
        limit = 1
    if limit > 50:
        limit = 50

    params = urllib.parse.urlencode({
        "order": "desc",
        "sort": "relevance",
        "q": query,
        "site": "stackoverflow",
        "pagesize": limit,
        "filter": "default",
    })

    try:
        data = _get("%s?%s" % (BASE, params))
        if not isinstance(data, dict):
            data = {}
        items = data.get("items") or []
        results = []
        for item in items:
            if not isinstance(item, dict):
                continue
            results.append({
                "title": item.get("title", ""),
                "score": item.get("score", 0),
                "answered": item.get("is_answered", False),
                "answers": item.get("answer_count", 0),
                "link": item.get("link", ""),
                "tags": item.get("tags", []),
            })
        out = {"query": query, "results": results}
        if "quota_remaining" in data:
            out["quota_remaining"] = data.get("quota_remaining")
        print(json.dumps(out, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "stackoverflow failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
