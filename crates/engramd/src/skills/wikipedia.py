#!/usr/bin/env python3
"""wikipedia — Engram skill (keyless). Look up a topic on Wikipedia.

Search the encyclopedia and return the best match's summary + link. Stdlib only;
uses Wikipedia's free, keyless REST/Action APIs — works the moment it's seeded.

Request (stdin): {"query": "Alan Turing", "lang": "en"}
Output (stdout): {title, extract, url, also: [related titles]}
"""
import json
import sys
import urllib.parse
import urllib.request

TIMEOUT = 20
UA = "engram-wikipedia/1 (https://github.com/)"


def _get(url):
    req = urllib.request.Request(url, headers={"User-Agent": UA, "Accept": "application/json"})
    with urllib.request.urlopen(req, timeout=TIMEOUT) as r:
        return json.loads(r.read().decode("utf-8", "replace"))


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0
    query = (q.get("query") or q.get("q") or "").strip()
    if not query:
        print(json.dumps({"error": "provide 'query'", "example": {"query": "Alan Turing"}}))
        return 0
    lang = (q.get("lang") or "en").strip() or "en"
    base = "https://%s.wikipedia.org" % lang
    try:
        # opensearch: resolve the query to the closest article title(s)
        s = _get(base + "/w/api.php?" + urllib.parse.urlencode(
            {"action": "opensearch", "search": query, "limit": 5, "namespace": 0, "format": "json"}))
        titles = s[1] if len(s) > 1 else []
        if not titles:
            print(json.dumps({"error": "no Wikipedia article matched %r" % query}))
            return 0
        title = titles[0]
        summary = _get(base + "/api/rest_v1/page/summary/" + urllib.parse.quote(title.replace(" ", "_")))
        print(json.dumps({
            "title": summary.get("title", title),
            "extract": summary.get("extract", ""),
            "url": summary.get("content_urls", {}).get("desktop", {}).get("page", base + "/wiki/" + urllib.parse.quote(title)),
            "also": titles[1:],
        }, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "wikipedia lookup failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
