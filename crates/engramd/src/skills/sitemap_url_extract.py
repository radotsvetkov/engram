#!/usr/bin/env python3
"""sitemap_url_extract — Engram skill (NETWORK-capable). Parse a sitemap.xml.

Parses a sitemap with the stdlib xml.etree.ElementTree, handling the standard
sitemaps.org namespace. For a <urlset> it extracts each page <loc> (+ lastmod
when present); for a <sitemapindex> it extracts the child sitemap <loc> URLs
and marks type="sitemapindex". Pass raw `xml`, OR a `url` to fetch (bounded
read, 20s timeout) — network only used when `url` is given. Caps at 1000 URLs.

Request (stdin): {"xml": "<urlset>...</urlset>"}   OR   {"url": "https://site/sitemap.xml"}
Output (stdout): {source, type, url_count, truncated, urls: [{loc, lastmod?}]}
"""
import json
import re
import sys
import urllib.error
import urllib.request
import xml.etree.ElementTree as ET

TIMEOUT = 20
MAX_BYTES = 5_000_000
MAX_URLS = 1000


def _local(tag):
    return tag.split("}")[-1] if "}" in tag else tag


def _find_child_text(el, name):
    for c in el:
        if _local(c.tag) == name:
            return (c.text or "").strip()
    return None


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e}))
        return 0
    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object"}))
        return 0

    xml_text = q.get("xml")
    url = (q.get("url") or "").strip()
    source = "inline"

    if not xml_text and not url:
        print(json.dumps({
            "error": "provide 'xml' (a string) or a 'url' to fetch",
            "example": {"url": "https://www.example.com/sitemap.xml"},
        }))
        return 0

    if not xml_text and url:
        if not re.match(r"^https?://", url, re.IGNORECASE):
            print(json.dumps({"error": "url must start with http:// or https://"}))
            return 0
        try:
            req = urllib.request.Request(url, headers={
                "User-Agent": "engram-sitemap_url_extract/1",
                "Accept": "application/xml, text/xml, */*;q=0.8",
            })
            with urllib.request.urlopen(req, timeout=TIMEOUT) as resp:
                raw = resp.read(MAX_BYTES)
            xml_text = raw.decode("utf-8", "replace")
            source = url
        except urllib.error.HTTPError as e:
            print(json.dumps({"error": "fetch failed: HTTP %s for %s" % (e.code, url)}))
            return 0
        except urllib.error.URLError as e:
            print(json.dumps({"error": "fetch failed: could not reach %s (%s)" % (url, e.reason)}))
            return 0
        except Exception as e:
            print(json.dumps({"error": "fetch failed: %s" % e}))
            return 1

    if not isinstance(xml_text, str):
        print(json.dumps({"error": "'xml' must be a string"}))
        return 0

    try:
        root = ET.fromstring(xml_text.strip())
    except ET.ParseError as e:
        print(json.dumps({"error": "malformed XML: %s" % e}))
        return 0
    except Exception as e:
        print(json.dumps({"error": "could not parse XML: %s" % e}))
        return 0

    root_tag = _local(root.tag)
    if root_tag == "sitemapindex":
        smtype = "sitemapindex"
        entry_name = "sitemap"
    elif root_tag == "urlset":
        smtype = "urlset"
        entry_name = "url"
    else:
        # Be lenient: infer from children.
        child_tags = {_local(c.tag) for c in root}
        if "sitemap" in child_tags:
            smtype = "sitemapindex"
            entry_name = "sitemap"
        elif "url" in child_tags:
            smtype = "urlset"
            entry_name = "url"
        else:
            print(json.dumps({"error": "not a sitemap: root <%s> has no <url> or <sitemap> entries" % root_tag}))
            return 0

    urls = []
    truncated = False
    for el in root:
        if _local(el.tag) != entry_name:
            continue
        loc = _find_child_text(el, "loc")
        if not loc:
            continue
        rec = {"loc": loc}
        lastmod = _find_child_text(el, "lastmod")
        if lastmod:
            rec["lastmod"] = lastmod
        if smtype == "sitemapindex":
            rec["type"] = "index"
        urls.append(rec)
        if len(urls) >= MAX_URLS:
            truncated = True
            break

    result = {
        "source": source,
        "type": smtype,
        "url_count": len(urls),
        "truncated": truncated,
        "urls": urls,
    }
    print(json.dumps(result, indent=2, default=str))
    return 0


if __name__ == "__main__":
    sys.exit(main())
