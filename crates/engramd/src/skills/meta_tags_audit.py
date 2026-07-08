#!/usr/bin/env python3
"""meta_tags_audit — Engram skill (network). Audit a page's SEO meta tags.

Fetches a URL's HTML (bounded to 500KB) and extracts title, meta description,
Open Graph tags, canonical link, and H1 count via regex (no bs4 available),
flagging common length/count issues. Stdlib only.

Request (stdin): {"url": "https://example.com"}
Output (stdout): {title, title_length, title_check, meta_description,
                   meta_description_length, meta_description_check,
                   canonical, og_title, og_description, og_image,
                   h1_count, h1_check}
"""
import html
import json
import re
import sys
import urllib.error
import urllib.request

TIMEOUT = 20
MAX_BYTES = 500000


def _first(html_text, patterns):
    for p in patterns:
        m = re.search(p, html_text, re.I | re.S)
        if m:
            return html.unescape(m.group(1)).strip()
    return None


def _meta_content(html_text, attr, name):
    esc = re.escape(name)
    return _first(html_text, [
        r'<meta[^>]+%s=["\']%s["\'][^>]*content=["\']([^"\']*)["\']' % (attr, esc),
        r'<meta[^>]+content=["\']([^"\']*)["\'][^>]*%s=["\']%s["\']' % (attr, esc),
    ])


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0
    url = (q.get("url") or "").strip()
    if not url:
        print(json.dumps({"error": "provide 'url'", "example": {"url": "https://example.com"}}))
        return 0
    if not re.match(r"^https?://", url, re.I):
        url = "https://" + url

    try:
        req = urllib.request.Request(url, headers={"User-Agent": "engram-meta_tags_audit/1"})
        with urllib.request.urlopen(req, timeout=TIMEOUT) as r:
            raw = r.read(MAX_BYTES)
        page = raw.decode("utf-8", "replace")
    except urllib.error.HTTPError as e:
        print(json.dumps({"error": "HTTP error %s fetching %s" % (e.code, url)}))
        return 0
    except urllib.error.URLError as e:
        print(json.dumps({"error": "could not reach %s: %s" % (url, e.reason)}))
        return 0
    except Exception as e:
        print(json.dumps({"error": "fetch failed: %s" % e}))
        return 1

    try:
        title = _first(page, [r"<title[^>]*>(.*?)</title>"]) or ""
        title = re.sub(r"\s+", " ", title).strip()
        title_length = len(title)
        if not title:
            title_check = "warn: missing"
        elif title_length < 30:
            title_check = "too_short"
        elif title_length > 60:
            title_check = "too_long"
        else:
            title_check = "ok"

        meta_description = _meta_content(page, "name", "description") or ""
        meta_description_length = len(meta_description)
        if not meta_description:
            meta_description_check = "warn: missing"
        elif meta_description_length < 70:
            meta_description_check = "too_short"
        elif meta_description_length > 160:
            meta_description_check = "too_long"
        else:
            meta_description_check = "ok"

        canonical = _first(page, [
            r'<link[^>]+rel=["\']canonical["\'][^>]*href=["\']([^"\']*)["\']',
            r'<link[^>]+href=["\']([^"\']*)["\'][^>]*rel=["\']canonical["\']',
        ])
        og_title = _meta_content(page, "property", "og:title")
        og_description = _meta_content(page, "property", "og:description")
        og_image = _meta_content(page, "property", "og:image")

        h1_count = len(re.findall(r"<h1[\s>]", page, re.I))
        if h1_count == 0:
            h1_check = "warn: none found"
        elif h1_count == 1:
            h1_check = "ok"
        else:
            h1_check = "warn: multiple H1s"

        result = {
            "url": url,
            "title": title,
            "title_length": title_length,
            "title_check": title_check,
            "meta_description": meta_description,
            "meta_description_length": meta_description_length,
            "meta_description_check": meta_description_check,
            "canonical": canonical,
            "og_title": og_title,
            "og_description": og_description,
            "og_image": og_image,
            "h1_count": h1_count,
            "h1_check": h1_check,
        }
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "meta tag audit failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
