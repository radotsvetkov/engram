#!/usr/bin/env python3
"""news — Engram skill (needs NEWSAPI_KEY). Recent news headlines for a query.

Searches NewsAPI's /v2/everything endpoint for recent articles matching a query,
newest first. Free dev tier: https://newsapi.org/register (set NEWSAPI_KEY in the
daemon env). Request (stdin): {"query": "...", "limit": 8, "language": "en"}.
Output (stdout): {"query", "articles": [{title, source, url, publishedAt, description}]}.
"""
import json
import os
import sys
import urllib.error
import urllib.parse
import urllib.request

TIMEOUT = 20
UA = "engram-news/1"
API = "https://newsapi.org/v2/everything"


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e}))
        return 0

    query = (q.get("query") or "").strip()
    if not query:
        print(json.dumps({
            "error": "'query' is required",
            "example": {"query": "artificial intelligence", "limit": 8, "language": "en"},
        }))
        return 0

    key = os.environ.get("NEWSAPI_KEY")
    if not key:
        print(json.dumps({
            "error": "no NewsAPI key configured",
            "how_to_fix": {"env": "NEWSAPI_KEY", "signup": "https://newsapi.org/register"},
        }))
        return 0

    # Clamp limit to NewsAPI's per-page bounds (1..100); default 8.
    try:
        limit = int(q.get("limit") or 8)
    except (TypeError, ValueError):
        limit = 8
    limit = max(1, min(limit, 100))
    language = (q.get("language") or "en").strip() or "en"

    params = urllib.parse.urlencode({
        "q": query,
        "language": language,
        "sortBy": "publishedAt",
        "pageSize": limit,
        "apiKey": key,
    })
    url = API + "?" + params

    try:
        req = urllib.request.Request(url, headers={
            "User-Agent": UA, "Accept": "application/json"})
        with urllib.request.urlopen(req, timeout=TIMEOUT) as resp:
            raw = json.loads(resp.read().decode("utf-8", "replace"))

        if raw.get("status") == "error":
            # NewsAPI returns 200/4xx with a structured error; surface it softly.
            print(json.dumps({
                "error": "NewsAPI error: %s" % raw.get("message", "unknown"),
                "code": raw.get("code"),
            }))
            return 0

        articles = []
        for a in (raw.get("articles") or []):
            articles.append({
                "title": a.get("title"),
                "source": (a.get("source") or {}).get("name"),
                "url": a.get("url"),
                "publishedAt": a.get("publishedAt"),
                "description": a.get("description"),
            })

        print(json.dumps({"query": query, "articles": articles}, indent=2, default=str))
        return 0
    except urllib.error.HTTPError as e:
        try:
            body = json.loads(e.read().decode("utf-8", "replace"))
            msg = body.get("message", "")
        except Exception:
            msg = ""
        if e.code == 401:
            print(json.dumps({
                "error": "invalid or unauthorized NewsAPI key",
                "how_to_fix": {"env": "NEWSAPI_KEY", "signup": "https://newsapi.org/register"},
            }))
            return 0
        if e.code == 429:
            print(json.dumps({"error": "NewsAPI rate limit reached — try again later"}))
            return 0
        print(json.dumps({"error": "NewsAPI error: HTTP %s %s" % (e.code, msg)}))
        return 0
    except Exception as e:
        print(json.dumps({"error": "news failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
