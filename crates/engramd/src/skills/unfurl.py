#!/usr/bin/env python3
"""unfurl — Engram skill (keyless). Unfurl a URL into its title + preview.

Fetches the page (User-Agent + 20s timeout, reads up to ~200KB) and extracts
title/description/image/site from <title>, og:* and meta description tags via
regex. Request: {"url": "https://..."}. Output: {url, title, description,
image, site}. Prefers og:* tags over plain ones; all values whitespace-stripped.
"""
import json, sys, re, urllib.request, urllib.error

MAX_BYTES = 200000


def _strip(s):
    if s is None:
        return None
    s = re.sub(r"\s+", " ", s).strip()
    return s or None


def _attr(html, prop_name, prop_val):
    # Match a <meta ...> tag where attribute `prop_name` == `prop_val`, in
    # either attribute order, and capture its content="..." value.
    quoted = re.escape(prop_val)
    patterns = [
        # prop before content
        r'<meta[^>]*?\b%s\s*=\s*["\']%s["\'][^>]*?\bcontent\s*=\s*["\']([^"\']*)["\']'
        % (re.escape(prop_name), quoted),
        # content before prop
        r'<meta[^>]*?\bcontent\s*=\s*["\']([^"\']*)["\'][^>]*?\b%s\s*=\s*["\']%s["\']'
        % (re.escape(prop_name), quoted),
    ]
    for p in patterns:
        m = re.search(p, html, re.IGNORECASE | re.DOTALL)
        if m:
            v = _strip(m.group(1))
            if v:
                return v
    return None


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e}))
        return 0

    url = (q.get("url") or "").strip() if isinstance(q, dict) else ""
    if not url:
        print(json.dumps({
            "error": "missing required field: url",
            "example": {"url": "https://example.com"},
        }))
        return 0
    if not re.match(r"^https?://", url, re.IGNORECASE):
        print(json.dumps({
            "error": "url must start with http:// or https://",
            "example": {"url": "https://example.com"},
        }))
        return 0

    try:
        req = urllib.request.Request(url, headers={
            "User-Agent": "engram-unfurl/1",
            "Accept": "text/html,application/xhtml+xml,*/*;q=0.8",
        })
        with urllib.request.urlopen(req, timeout=20) as resp:
            raw = resp.read(MAX_BYTES)
            final_url = resp.geturl() or url
        html = raw.decode("utf-8", "replace")

        og_title = _attr(html, "property", "og:title") or _attr(html, "name", "og:title")
        og_desc = _attr(html, "property", "og:description") or _attr(html, "name", "og:description")
        og_image = _attr(html, "property", "og:image") or _attr(html, "name", "og:image")
        og_site = _attr(html, "property", "og:site_name") or _attr(html, "name", "og:site_name")
        meta_desc = _attr(html, "name", "description")

        plain_title = None
        m = re.search(r"<title[^>]*>(.*?)</title>", html, re.IGNORECASE | re.DOTALL)
        if m:
            plain_title = _strip(m.group(1))

        result = {
            "url": final_url,
            "title": og_title or plain_title,
            "description": og_desc or meta_desc,
            "image": og_image,
            "site": og_site,
        }
        print(json.dumps(result, indent=2, default=str))
        return 0
    except urllib.error.HTTPError as e:
        print(json.dumps({"error": "unfurl failed: HTTP %s for %s" % (e.code, url)}))
        return 0
    except urllib.error.URLError as e:
        print(json.dumps({"error": "unfurl failed: could not reach %s (%s)" % (url, e.reason)}))
        return 0
    except Exception as e:
        print(json.dumps({"error": "unfurl failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
