#!/usr/bin/env python3
"""utm_builder — Engram skill (no network). Build a UTM-tagged campaign URL.

Merges utm_source/medium/campaign/term/content into a base URL's existing
query string without clobbering any params already there. Stdlib only
(urllib.parse).

Request (stdin): {"base_url": "https://example.com/page?ref=1", "source": "newsletter",
                   "medium": "email", "campaign": "spring_sale", "term": "shoes", "content": "cta"}
Output (stdout): {url, params}
"""
import json
import sys
from urllib.parse import parse_qsl, urlencode, urlsplit, urlunsplit


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0

    base_url = (q.get("base_url") or "").strip()
    source = q.get("source")
    medium = q.get("medium")
    campaign = q.get("campaign")
    missing = [k for k, v in (
        ("base_url", base_url), ("source", source), ("medium", medium), ("campaign", campaign)
    ) if not v]
    if missing:
        print(json.dumps({
            "error": "missing required field(s): %s" % ", ".join(missing),
            "example": {
                "base_url": "https://example.com/page",
                "source": "newsletter", "medium": "email", "campaign": "spring_sale",
                "term": "running shoes", "content": "cta_button",
            },
        }))
        return 0

    try:
        parts = urlsplit(base_url)
        if not parts.scheme or not parts.netloc:
            print(json.dumps({"error": "'base_url' must be an absolute URL, e.g. https://example.com/page"}))
            return 0

        utm_params = {"utm_source": source, "utm_medium": medium, "utm_campaign": campaign}
        if q.get("term"):
            utm_params["utm_term"] = q["term"]
        if q.get("content"):
            utm_params["utm_content"] = q["content"]

        existing = [(k, v) for k, v in parse_qsl(parts.query, keep_blank_values=True) if k not in utm_params]
        new_query = urlencode(existing + list(utm_params.items()))
        final_url = urlunsplit((parts.scheme, parts.netloc, parts.path, new_query, parts.fragment))

        print(json.dumps({"url": final_url, "params": utm_params}, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "failed to build UTM url: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
