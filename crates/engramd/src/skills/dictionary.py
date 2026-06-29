#!/usr/bin/env python3
"""dictionary — Engram skill (keyless). Define a word.

Uses dictionaryapi.dev (free, no key). Stdlib only.

Request (stdin): {"word": "serendipity", "lang": "en"}
Output (stdout): {word, phonetic, meanings: [{part_of_speech, definitions, ...}]}
"""
import json
import sys
import urllib.parse
import urllib.request

TIMEOUT = 20


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0
    word = (q.get("word") or q.get("term") or "").strip()
    if not word:
        print(json.dumps({"error": "provide 'word'", "example": {"word": "serendipity"}}))
        return 0
    lang = (q.get("lang") or "en").strip() or "en"
    url = "https://api.dictionaryapi.dev/api/v2/entries/%s/%s" % (lang, urllib.parse.quote(word))
    try:
        req = urllib.request.Request(url, headers={"User-Agent": "engram-dictionary/1"})
        with urllib.request.urlopen(req, timeout=TIMEOUT) as r:
            data = json.loads(r.read().decode("utf-8", "replace"))
    except urllib.error.HTTPError as e:
        if e.code == 404:
            print(json.dumps({"error": "no definition found for %r" % word}))
            return 0
        print(json.dumps({"error": "lookup failed: HTTP %s" % e.code}))
        return 1
    except Exception as e:
        print(json.dumps({"error": "lookup failed: %s" % e}))
        return 1
    if not isinstance(data, list) or not data:
        print(json.dumps({"error": "no definition found for %r" % word}))
        return 0
    entry = data[0]
    meanings = []
    for m in entry.get("meanings", []):
        defs = [d.get("definition") for d in m.get("definitions", [])[:3] if d.get("definition")]
        meanings.append({"part_of_speech": m.get("partOfSpeech"), "definitions": defs,
                         "synonyms": (m.get("synonyms") or [])[:6]})
    phonetic = entry.get("phonetic") or next((p.get("text") for p in entry.get("phonetics", []) if p.get("text")), "")
    print(json.dumps({"word": entry.get("word", word), "phonetic": phonetic, "meanings": meanings},
                     indent=2, default=str))
    return 0


if __name__ == "__main__":
    sys.exit(main())
