#!/usr/bin/env python3
"""arxiv — Engram skill (keyless). Search arXiv papers by query.

Reads {"query": str, "max"?: int (default 5, capped 1..50)} on stdin, hits the
public arXiv Atom API (http://export.arxiv.org/api/query), and parses the XML.
Outputs {"query", "count", "papers":[{title, authors, summary(<=400 chars),
url, published}]}.
"""
import json, sys, os, urllib.request, urllib.parse, urllib.error, re
import xml.etree.ElementTree as ET

ATOM = "{http://www.w3.org/2005/Atom}"


def _clean(text):
    # Collapse newlines/whitespace runs into single spaces and trim.
    return re.sub(r"\s+", " ", (text or "")).strip()


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e})); return 0

    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"query": "transformer attention", "max": 5},
        })); return 0

    query = q.get("query")
    if not query or not isinstance(query, str) or not query.strip():
        print(json.dumps({
            "error": "missing required field 'query'",
            "example": {"query": "transformer attention", "max": 5},
        })); return 0
    query = query.strip()

    # Clamp max_results to a sane range; tolerate bad types.
    try:
        max_results = int(q.get("max", 5))
    except (TypeError, ValueError):
        max_results = 5
    if max_results < 1:
        max_results = 1
    if max_results > 50:
        max_results = 50

    params = urllib.parse.urlencode({
        "search_query": "all:" + query,
        "start": 0,
        "max_results": max_results,
        "sortBy": "relevance",
    })
    url = "http://export.arxiv.org/api/query?" + params

    try:
        req = urllib.request.Request(url, headers={"User-Agent": "engram-arxiv/1"})
        with urllib.request.urlopen(req, timeout=20) as resp:
            raw = resp.read()

        root = ET.fromstring(raw)

        papers = []
        for entry in root.findall(ATOM + "entry"):
            title = _clean((entry.findtext(ATOM + "title") or ""))

            summary = _clean((entry.findtext(ATOM + "summary") or ""))
            if len(summary) > 400:
                summary = summary[:400].rstrip() + "..."

            authors = []
            for a in entry.findall(ATOM + "author"):
                name = _clean(a.findtext(ATOM + "name") or "")
                if name:
                    authors.append(name)

            link = _clean(entry.findtext(ATOM + "id") or "")
            published = _clean(entry.findtext(ATOM + "published") or "")

            papers.append({
                "title": title,
                "authors": authors,
                "summary": summary,
                "url": link,
                "published": published,
            })

        result = {
            "query": query,
            "count": len(papers),
            "papers": papers,
        }
        print(json.dumps(result, indent=2, default=str)); return 0
    except urllib.error.HTTPError as e:
        print(json.dumps({"error": "arxiv failed: HTTP %s %s" % (e.code, e.reason)})); return 1
    except urllib.error.URLError as e:
        print(json.dumps({"error": "arxiv failed: network error: %s" % e.reason})); return 1
    except ET.ParseError as e:
        print(json.dumps({"error": "arxiv failed: could not parse XML response: %s" % e})); return 1
    except Exception as e:
        print(json.dumps({"error": "arxiv failed: %s" % e})); return 1


if __name__ == "__main__":
    sys.exit(main())
