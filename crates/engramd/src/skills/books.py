#!/usr/bin/env python3
"""books — Engram skill (keyless). Search books via the OpenLibrary catalog.

Queries OpenLibrary's free, keyless search API for a title/author/keyword.
Request (stdin): {"query": "the hobbit", "limit": 5}
Output (stdout): {query, count, books:[{title, authors:[...], year, key, isbn}]}
"""
import json
import sys
import urllib.parse
import urllib.request

TIMEOUT = 20
UA = "engram-books/1 (https://github.com/)"
SEARCH = "https://openlibrary.org/search.json"


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
    query = (q.get("query") or q.get("q") or "").strip()
    if not query:
        print(json.dumps({"error": "provide 'query'", "example": {"query": "the hobbit", "limit": 5}}))
        return 0
    # clamp limit to a sane positive integer (default 5)
    try:
        limit = int(q.get("limit", 5))
    except Exception:
        limit = 5
    if limit < 1:
        limit = 1
    if limit > 50:
        limit = 50
    try:
        url = SEARCH + "?" + urllib.parse.urlencode({
            "q": query,
            "limit": limit,
            "fields": "title,author_name,first_publish_year,key,isbn,publisher",
        })
        data = _get(url)
        docs = data.get("docs") or []
        books = []
        for doc in docs[:limit]:
            if not isinstance(doc, dict):
                continue
            authors = doc.get("author_name") or []
            if not isinstance(authors, list):
                authors = [authors]
            isbns = doc.get("isbn") or []
            isbn = isbns[0] if isinstance(isbns, list) and isbns else None
            books.append({
                "title": doc.get("title"),
                "authors": authors,
                "year": doc.get("first_publish_year"),
                "key": doc.get("key"),
                "isbn": isbn,
            })
        print(json.dumps({
            "query": query,
            "count": len(docs),
            "books": books,
        }, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "books failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
