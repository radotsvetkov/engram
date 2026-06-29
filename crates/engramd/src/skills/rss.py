#!/usr/bin/env python3
"""rss — Engram skill (keyless). Read the latest items from any RSS/Atom feed.

News sites, blogs, podcasts, release feeds, subreddits (.rss), YouTube channel
feeds — anything with a feed URL. Stdlib only (xml.etree), no key.

Request (stdin): {"url": "https://hnrss.org/frontpage", "limit": 10}
Output (stdout): {feed, items: [{title, link, published, summary}]}
"""
import json
import re
import sys
import urllib.request
import xml.etree.ElementTree as ET

TIMEOUT = 20


def _text(el):
    if el is None:
        return ""
    t = "".join(el.itertext())
    t = re.sub(r"<[^>]+>", " ", t)  # strip any HTML in the text
    return re.sub(r"\s+", " ", t).strip()


def _tag(el):
    return el.tag.split("}")[-1] if el is not None else ""


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0
    url = (q.get("url") or q.get("feed") or "").strip()
    if not url:
        print(json.dumps({"error": "provide a feed 'url'", "example": {"url": "https://hnrss.org/frontpage"}}))
        return 0
    limit = max(1, min(int(q.get("limit", 10) or 10), 50))
    try:
        req = urllib.request.Request(url, headers={"User-Agent": "engram-rss/1", "Accept": "application/rss+xml, application/atom+xml, application/xml, text/xml"})
        with urllib.request.urlopen(req, timeout=TIMEOUT) as r:
            root = ET.fromstring(r.read())
    except Exception as e:
        print(json.dumps({"error": "could not read feed: %s" % e}))
        return 1

    # RSS: channel/item with <link> text. Atom: feed/entry with <link href=...>.
    items = []
    entries = [e for e in root.iter() if _tag(e) in ("item", "entry")]
    feed_title = ""
    for e in root.iter():
        if _tag(e) == "title":
            feed_title = _text(e)
            break
    for e in entries[:limit]:
        d = {"title": "", "link": "", "published": "", "summary": ""}
        for c in e:
            tag = _tag(c)
            if tag == "title":
                d["title"] = _text(c)
            elif tag == "link":
                d["link"] = (c.get("href") or _text(c) or "").strip()
            elif tag in ("pubDate", "published", "updated"):
                if not d["published"]:
                    d["published"] = _text(c)
            elif tag in ("description", "summary", "content"):
                if not d["summary"]:
                    d["summary"] = _text(c)[:300]
        items.append(d)
    print(json.dumps({"feed": feed_title, "count": len(items), "items": items}, indent=2, default=str))
    return 0


if __name__ == "__main__":
    sys.exit(main())
