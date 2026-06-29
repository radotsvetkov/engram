#!/usr/bin/env python3
"""hackernews — Engram skill (keyless). Top Hacker News stories via the Firebase API.

Fetches the front-page story ids, then loads each item. Request shape:
{"limit": 10, "kind": "top"} where kind is one of top|new|best (defaults: 10, top).
Output: {kind, stories:[{title, url, score, by, comments, hn_url}]}. Items that
fail to load are skipped. Stdlib only; uses HN's free, keyless Firebase endpoints.
"""
import json
import sys
import urllib.request

TIMEOUT = 20
UA = "engram-hackernews/1"
KINDS = {"top", "new", "best"}
BASE = "https://hacker-news.firebaseio.com/v0"


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
        print(json.dumps({"error": "request must be a JSON object", "example": {"limit": 10, "kind": "top"}}))
        return 0

    kind = str(q.get("kind") or "top").strip().lower()
    if kind not in KINDS:
        print(json.dumps({
            "error": "invalid 'kind' %r; use one of top|new|best" % kind,
            "example": {"limit": 10, "kind": "top"},
        }))
        return 0

    # clamp limit to a sane range; ignore non-numeric input
    try:
        limit = int(q.get("limit", 10))
    except (TypeError, ValueError):
        limit = 10
    if limit < 1:
        limit = 1
    if limit > 50:
        limit = 50

    try:
        ids = _get("%s/%sstories.json" % (BASE, kind))
        if not isinstance(ids, list):
            ids = []
        stories = []
        for sid in ids[:limit]:
            try:
                item = _get("%s/item/%s.json" % (BASE, sid))
            except Exception:
                continue  # skip items that fail to load
            if not isinstance(item, dict):
                continue
            stories.append({
                "title": item.get("title", ""),
                "url": item.get("url", ""),
                "score": item.get("score", 0),
                "by": item.get("by", ""),
                "comments": item.get("descendants", 0),
                "hn_url": "https://news.ycombinator.com/item?id=%s" % sid,
            })
        print(json.dumps({"kind": kind, "stories": stories}, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "hackernews failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
