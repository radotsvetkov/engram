#!/usr/bin/env python3
"""robots_sitemap_check — Engram skill (network). Check robots.txt and sitemaps.

Fetches a site's /robots.txt, lists its Disallow/Allow rules and declared
sitemaps, then best-effort fetches and parses the first declared sitemap (as
a urlset or sitemapindex) to count its entries. Stdlib only (xml.etree).

Request (stdin): {"url": "https://example.com/some/page"}
  - scheme+host are derived from the URL; only /robots.txt is fetched from it.
Output (stdout): {robots_found, disallow_rules, allow_rules, sitemaps_declared,
                   first_sitemap_entry_count, first_sitemap_type}
"""
import json
import sys
import urllib.error
import urllib.request
import xml.etree.ElementTree as ET
from urllib.parse import urlsplit

TIMEOUT = 20


def _tag(el):
    return el.tag.split("}")[-1].lower() if el is not None else ""


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
    if "://" not in url:
        url = "https://" + url

    try:
        parts = urlsplit(url)
        if not parts.netloc:
            print(json.dumps({"error": "could not parse host from 'url': %s" % url}))
            return 0
        scheme = parts.scheme or "https"
        robots_url = "%s://%s/robots.txt" % (scheme, parts.netloc)
    except Exception as e:
        print(json.dumps({"error": "could not parse 'url': %s" % e}))
        return 0

    robots_found = True
    robots_text = ""
    try:
        req = urllib.request.Request(robots_url, headers={"User-Agent": "engram-robots_sitemap_check/1"})
        with urllib.request.urlopen(req, timeout=TIMEOUT) as r:
            robots_text = r.read(200000).decode("utf-8", "replace")
    except urllib.error.HTTPError as e:
        if e.code == 404:
            robots_found = False
        else:
            print(json.dumps({"error": "HTTP error %s fetching %s" % (e.code, robots_url)}))
            return 0
    except urllib.error.URLError as e:
        print(json.dumps({"error": "could not reach %s: %s" % (robots_url, e.reason)}))
        return 0
    except Exception as e:
        print(json.dumps({"error": "fetch failed: %s" % e}))
        return 1

    try:
        disallow_rules = []
        allow_rules = []
        sitemaps_declared = []
        for line in robots_text.splitlines():
            line = line.strip()
            if not line or line.startswith("#") or ":" not in line:
                continue
            key, _, val = line.partition(":")
            key = key.strip().lower()
            val = val.strip()
            if key == "disallow":
                disallow_rules.append(val)
            elif key == "allow":
                allow_rules.append(val)
            elif key == "sitemap":
                sitemaps_declared.append(val)

        first_sitemap_entry_count = None
        first_sitemap_type = None
        if sitemaps_declared:
            try:
                req2 = urllib.request.Request(
                    sitemaps_declared[0], headers={"User-Agent": "engram-robots_sitemap_check/1"})
                with urllib.request.urlopen(req2, timeout=TIMEOUT) as r2:
                    # Stream-parse rather than reading a bounded byte cap first — sitemap
                    # files can legitimately exceed a few MB and truncating mid-XML would
                    # break the parse.
                    root = ET.parse(r2).getroot()
                tag = _tag(root)
                if tag == "urlset":
                    first_sitemap_type = "urlset"
                    first_sitemap_entry_count = sum(1 for c in root if _tag(c) == "url")
                elif tag == "sitemapindex":
                    first_sitemap_type = "sitemapindex"
                    first_sitemap_entry_count = sum(1 for c in root if _tag(c) == "sitemap")
                else:
                    first_sitemap_type = tag or None
            except Exception:
                # Best-effort: sitemap fetch/parse failures don't fail the whole check.
                pass

        result = {
            "robots_url": robots_url,
            "robots_found": robots_found,
            "disallow_rules": disallow_rules,
            "allow_rules": allow_rules,
            "sitemaps_declared": sitemaps_declared,
            "first_sitemap_entry_count": first_sitemap_entry_count,
            "first_sitemap_type": first_sitemap_type,
        }
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "robots/sitemap check failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
