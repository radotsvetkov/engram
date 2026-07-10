#!/usr/bin/env python3
"""link_extract — Engram skill (NETWORK-capable). Extract & classify links.

Parses <a href> (and optionally <img src>) from HTML with the stdlib
html.parser (NO bs4), resolves relative URLs against a base, and classifies
each link as internal / external / anchor / mailto / tel relative to the base
host. Pass raw `html` (+ optional `base_url`), OR a `url` to fetch and use as
the base (bounded read, 20s timeout) — network only used when `url` is given.

Request (stdin): {"html": "<a href=...>", "base_url": "https://site.com/x"}
             OR  {"url": "https://site.com/x", "images": true}
Output (stdout): {source, base, links: [{href, resolved, type, text}],
                  counts: {internal, external, anchor, mailto, tel, other},
                  unique_domains: [...]}
"""
import json
import re
import sys
import urllib.error
import urllib.parse
import urllib.request
from html.parser import HTMLParser

TIMEOUT = 20
MAX_BYTES = 2_000_000


class _LinkParser(HTMLParser):
    def __init__(self, include_images):
        super().__init__(convert_charrefs=True)
        self.include_images = include_images
        self.links = []          # {"href","text","kind"}
        self._cur_a = None       # current <a> href being read
        self._cur_text = None    # accumulator for anchor text

    def handle_starttag(self, tag, attrs):
        d = dict(attrs)
        if tag == "a" and "href" in d:
            self._cur_a = d["href"]
            self._cur_text = []
        elif tag == "img" and self.include_images and "src" in d:
            self.links.append({"href": d["src"], "text": (d.get("alt") or "").strip(), "kind": "img"})

    def handle_data(self, data):
        if self._cur_text is not None:
            self._cur_text.append(data)

    def handle_endtag(self, tag):
        if tag == "a" and self._cur_a is not None:
            text = re.sub(r"\s+", " ", "".join(self._cur_text or [])).strip()
            self.links.append({"href": self._cur_a, "text": text, "kind": "a"})
            self._cur_a = None
            self._cur_text = None


def _classify(href, resolved, base_host):
    low = href.strip().lower()
    if low.startswith("mailto:"):
        return "mailto"
    if low.startswith("tel:"):
        return "tel"
    if low.startswith("#") or (resolved and urllib.parse.urlparse(resolved).fragment and not urllib.parse.urlparse(resolved).netloc):
        return "anchor"
    if low.startswith("javascript:") or low.startswith("data:"):
        return "other"
    parsed = urllib.parse.urlparse(resolved) if resolved else urllib.parse.urlparse(href)
    host = parsed.netloc.lower()
    if not host:
        # relative with no host resolved -> internal (or pure anchor already handled)
        if low.startswith("#"):
            return "anchor"
        return "internal"
    if base_host and host == base_host:
        return "internal"
    if not base_host:
        return "external"
    return "external"


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e}))
        return 0
    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object"}))
        return 0

    html = q.get("html")
    url = (q.get("url") or "").strip()
    base_url = (q.get("base_url") or "").strip()
    include_images = bool(q.get("images"))
    source = "inline"

    if not html and not url:
        print(json.dumps({
            "error": "provide 'html' (+ optional 'base_url') or a 'url' to fetch",
            "example": {"html": "<a href='/about'>About</a>", "base_url": "https://site.com"},
        }))
        return 0

    if not html and url:
        if not re.match(r"^https?://", url, re.IGNORECASE):
            print(json.dumps({"error": "url must start with http:// or https://"}))
            return 0
        try:
            req = urllib.request.Request(url, headers={
                "User-Agent": "engram-link_extract/1",
                "Accept": "text/html,application/xhtml+xml,*/*;q=0.8",
            })
            with urllib.request.urlopen(req, timeout=TIMEOUT) as resp:
                raw = resp.read(MAX_BYTES)
                final_url = resp.geturl() or url
            html = raw.decode("utf-8", "replace")
            source = url
            if not base_url:
                base_url = final_url
        except urllib.error.HTTPError as e:
            print(json.dumps({"error": "fetch failed: HTTP %s for %s" % (e.code, url)}))
            return 0
        except urllib.error.URLError as e:
            print(json.dumps({"error": "fetch failed: could not reach %s (%s)" % (url, e.reason)}))
            return 0
        except Exception as e:
            print(json.dumps({"error": "fetch failed: %s" % e}))
            return 1

    if not isinstance(html, str):
        print(json.dumps({"error": "'html' must be a string"}))
        return 0

    base_host = urllib.parse.urlparse(base_url).netloc.lower() if base_url else ""

    try:
        parser = _LinkParser(include_images)
        parser.feed(html)
        parser.close()
    except Exception as e:
        print(json.dumps({"error": "could not parse HTML: %s" % e}))
        return 0

    counts = {"internal": 0, "external": 0, "anchor": 0, "mailto": 0, "tel": 0, "other": 0}
    domains = set()
    out_links = []
    for item in parser.links:
        href = item["href"]
        resolved = None
        if base_url and not re.match(r"^[a-zA-Z][a-zA-Z0-9+.-]*:", href) and not href.startswith("#"):
            resolved = urllib.parse.urljoin(base_url, href)
        elif base_url and href.startswith("#"):
            resolved = urllib.parse.urljoin(base_url, href)
        elif re.match(r"^https?://", href, re.IGNORECASE):
            resolved = href
        ltype = _classify(href, resolved, base_host)
        counts[ltype] = counts.get(ltype, 0) + 1
        rec = {"href": href, "resolved": resolved, "type": ltype, "text": item["text"]}
        if include_images and item["kind"] == "img":
            rec["is_image"] = True
        out_links.append(rec)
        target = resolved or href
        if re.match(r"^https?://", target, re.IGNORECASE):
            host = urllib.parse.urlparse(target).netloc.lower()
            if host:
                domains.add(host)

    result = {
        "source": source,
        "base": base_url or None,
        "link_count": len(out_links),
        "counts": counts,
        "unique_domains": sorted(domains),
        "links": out_links,
    }
    print(json.dumps(result, indent=2, default=str))
    return 0


if __name__ == "__main__":
    sys.exit(main())
