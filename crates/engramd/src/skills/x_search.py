#!/usr/bin/env python3
"""x_search — Engram skill (needs X_BEARER_TOKEN). Search recent posts on X/Twitter.

Calls the X (Twitter) API v2 "recent search" endpoint, which requires an
app-only Bearer Token from a developer.x.com app. NOTE: the free tier has
historically NOT included access to search endpoints — a paid tier
(Basic/Pro) is typically required. Set X_BEARER_TOKEN in the daemon
environment; see https://developer.x.com/en/portal/dashboard to create an
app and generate a token.

Request (stdin): {"query": "climate change", "limit"?: 10}
Output (stdout): {query, results: [{id, text, created_at, likes, retweets,
replies}], result_count}
"""
import json
import os
import sys
import urllib.error
import urllib.parse
import urllib.request

TIMEOUT = 20
UA = "engram-x_search/1"
API = "https://api.x.com/2/tweets/search/recent"


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e}))
        return 0

    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"query": "climate change", "limit": 10},
        }))
        return 0

    query = q.get("query")
    if not query or not str(query).strip():
        print(json.dumps({
            "error": "missing required field: query",
            "example": {"query": "climate change", "limit": 10},
        }))
        return 0
    query = str(query).strip()

    token = os.environ.get("X_BEARER_TOKEN", "").strip()
    if not token:
        print(json.dumps({
            "error": "X_BEARER_TOKEN is required to search X/Twitter (needs an X "
                     "Developer Portal app; the free tier has very limited quota)",
            "how_to_fix": "create an app at developer.x.com, generate a Bearer "
                          "Token, and set X_BEARER_TOKEN in the daemon environment",
        }))
        return 0

    # Clamp limit to the API's actual max_results bounds (10..100); default 10.
    try:
        limit = int(q.get("limit") or 10)
    except (TypeError, ValueError):
        limit = 10
    limit = max(10, min(limit, 100))

    params = urllib.parse.urlencode({
        "query": query,
        "max_results": limit,
        "tweet.fields": "created_at,public_metrics,author_id",
    })
    url = API + "?" + params

    try:
        req = urllib.request.Request(url, headers={
            "User-Agent": UA,
            "Accept": "application/json",
            "Authorization": "Bearer " + token,
        })
        try:
            with urllib.request.urlopen(req, timeout=TIMEOUT) as resp:
                raw = resp.read().decode("utf-8", "replace")
        except urllib.error.HTTPError as he:
            body = ""
            try:
                body = he.read().decode("utf-8", "replace")
            except Exception:
                pass
            detail = ""
            try:
                parsed = json.loads(body)
                detail = parsed.get("detail") or parsed.get("title") or ""
            except Exception:
                detail = body[:300] if body else ""

            if he.code == 401:
                out = {
                    "error": "X API rejected the bearer token (401 unauthorized) — "
                             "it may be invalid, expired, or revoked",
                    "how_to_fix": "generate a new Bearer Token at "
                                  "developer.x.com and update X_BEARER_TOKEN",
                }
            elif he.code == 403:
                out = {
                    "error": "X API refused access (403 forbidden) — this endpoint "
                             "may require a paid X API tier; the free tier has "
                             "historically excluded search access",
                    "how_to_fix": "upgrade your X Developer Portal project to a "
                                  "tier (Basic/Pro) that includes recent search",
                }
            elif he.code == 429:
                out = {
                    "error": "X API rate limit reached (429) — try again later "
                             "or reduce request frequency",
                }
            else:
                out = {"error": "X API request failed: HTTP %s" % he.code}
            if detail:
                out["detail"] = detail
            out["query"] = query
            print(json.dumps(out))
            return 0
        except urllib.error.URLError as ue:
            print(json.dumps({
                "error": "could not reach X API: %s" % ue.reason,
                "query": query,
            }))
            return 0

        data = json.loads(raw) if raw else {}
        results = []
        for item in data.get("data", []) or []:
            metrics = item.get("public_metrics", {}) or {}
            results.append({
                "id": item.get("id"),
                "text": item.get("text"),
                "created_at": item.get("created_at"),
                "likes": metrics.get("like_count"),
                "retweets": metrics.get("retweet_count"),
                "replies": metrics.get("reply_count"),
            })

        result = {"query": query, "results": results, "result_count": len(results)}
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "x_search failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
