#!/usr/bin/env python3
"""rhyme_finder — Engram skill (network). Find rhymes for a word via the free,
keyless Datamuse API.

Queries `rel_rhy` (perfect rhymes) first. If that comes back empty or thin
(fewer than 5 results), also queries `rel_nry` (near rhymes) and merges the
two lists, deduping while preserving order, so songwriters/poets still get
useful suggestions for words with few or no perfect rhymes.

Request (stdin): {"word": "orange", "limit": 15}
Output (stdout): {word, rhymes: ["word1", "word2", ...], near_rhymes_used: bool}
"""
import json
import sys
import urllib.error
import urllib.parse
import urllib.request

TIMEOUT = 20
UA = "engram-rhyme_finder/1"
BASE = "https://api.datamuse.com/words"


def _get(params):
    url = BASE + "?" + urllib.parse.urlencode(params)
    req = urllib.request.Request(url, headers={"User-Agent": UA})
    with urllib.request.urlopen(req, timeout=TIMEOUT) as r:
        data = json.loads(r.read().decode("utf-8", "replace"))
    if not isinstance(data, list):
        return []
    words = []
    for item in data:
        if isinstance(item, dict) and isinstance(item.get("word"), str):
            words.append(item["word"])
    return words


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0
    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object",
                          "example": {"word": "orange"}}))
        return 0

    word = (q.get("word") or "").strip()
    if not word:
        print(json.dumps({
            "error": "provide 'word'",
            "example": {"word": "orange", "limit": 15},
        }))
        return 0

    limit = q.get("limit", 15)
    try:
        limit = int(limit)
    except (TypeError, ValueError):
        print(json.dumps({"error": "'limit' must be an integer", "example": {"word": "orange", "limit": 15}}))
        return 0
    if limit <= 0:
        limit = 15
    limit = min(limit, 100)

    try:
        rhymes = _get({"rel_rhy": word, "max": limit})
        near_used = False
        if len(rhymes) < 5:
            near = _get({"rel_nry": word, "max": limit})
            near_used = bool(near)
            seen = set(rhymes)
            for w in near:
                if w not in seen:
                    seen.add(w)
                    rhymes.append(w)
            rhymes = rhymes[:limit]
    except urllib.error.HTTPError as e:
        print(json.dumps({"error": "Datamuse HTTP error %s: %s" % (e.code, e.reason)}))
        return 0
    except urllib.error.URLError as e:
        print(json.dumps({"error": "network error reaching Datamuse: %s" % e.reason}))
        return 0
    except Exception as e:
        print(json.dumps({"error": "rhyme lookup failed: %s" % e}))
        return 1

    if not rhymes:
        print(json.dumps({
            "word": word,
            "rhymes": [],
            "near_rhymes_used": False,
            "note": "no rhymes or near-rhymes found for %r" % word,
        }, indent=2, default=str))
        return 0

    print(json.dumps({
        "word": word,
        "rhymes": rhymes,
        "near_rhymes_used": near_used,
    }, indent=2, default=str))
    return 0


if __name__ == "__main__":
    sys.exit(main())
