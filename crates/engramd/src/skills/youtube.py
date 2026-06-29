#!/usr/bin/env python3
"""youtube — Engram skill (needs YOUTUBE_API_KEY). Search YouTube videos.

Reads YOUTUBE_API_KEY from the environment (enable "YouTube Data API v3" in
Google Cloud). Request: {"query": "...", "limit"?: 5}. Calls the YouTube
Data API v3 search endpoint and returns {"query", "videos": [...]} where each
video has title, channel, published, url, and description.
"""
import json, sys, os, urllib.request, urllib.parse, urllib.error


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e})); return 0

    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"query": "lo-fi beats", "limit": 5},
        })); return 0

    query = q.get("query")
    if not query or not str(query).strip():
        print(json.dumps({
            "error": "missing required field: query",
            "example": {"query": "lo-fi beats", "limit": 5},
        })); return 0
    query = str(query).strip()

    key = os.environ.get("YOUTUBE_API_KEY", "").strip()
    if not key:
        print(json.dumps({
            "error": "no YouTube API key configured",
            "how_to_fix": {
                "env": "YOUTUBE_API_KEY",
                "signup": "https://console.cloud.google.com/apis/library/youtube.googleapis.com",
            },
        })); return 0

    # Clamp limit to YouTube's allowed 1..50 range; default 5.
    try:
        limit = int(q.get("limit", 5))
    except Exception:
        limit = 5
    if limit < 1:
        limit = 1
    if limit > 50:
        limit = 50

    try:
        params = urllib.parse.urlencode({
            "part": "snippet",
            "q": query,
            "type": "video",
            "maxResults": limit,
            "key": key,
        })
        url = "https://www.googleapis.com/youtube/v3/search?" + params
        req = urllib.request.Request(url, headers={"User-Agent": "engram-youtube-skill/1.0"})
        try:
            with urllib.request.urlopen(req, timeout=20) as resp:
                raw = resp.read().decode("utf-8", "replace")
        except urllib.error.HTTPError as he:
            body = ""
            try:
                body = he.read().decode("utf-8", "replace")
            except Exception:
                pass
            msg = "HTTP %s" % he.code
            try:
                err = json.loads(body).get("error", {})
                if err.get("message"):
                    msg += ": " + err["message"]
            except Exception:
                if body:
                    msg += ": " + body[:300]
            out = {"error": "YouTube API request failed (%s)" % msg, "query": query}
            if he.code in (400, 401, 403):
                out["how_to_fix"] = {
                    "env": "YOUTUBE_API_KEY",
                    "hint": "verify the key is valid and 'YouTube Data API v3' is enabled / not over quota",
                    "signup": "https://console.cloud.google.com/apis/library/youtube.googleapis.com",
                }
            print(json.dumps(out)); return 0

        data = json.loads(raw) if raw else {}
        videos = []
        for item in data.get("items", []) or []:
            snippet = item.get("snippet", {}) or {}
            vid = (item.get("id", {}) or {}).get("videoId", "")
            videos.append({
                "title": snippet.get("title", ""),
                "channel": snippet.get("channelTitle", ""),
                "published": snippet.get("publishedAt", ""),
                "url": ("https://youtube.com/watch?v=" + vid) if vid else "",
                "description": snippet.get("description", ""),
            })

        result = {"query": query, "videos": videos}
        print(json.dumps(result, indent=2, default=str)); return 0
    except Exception as e:
        print(json.dumps({"error": "youtube failed: %s" % e})); return 1


if __name__ == "__main__":
    sys.exit(main())
