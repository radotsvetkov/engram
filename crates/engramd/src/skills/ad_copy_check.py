#!/usr/bin/env python3
"""ad_copy_check — Engram skill (no network). Check ad copy against platform character limits.

Embeds well-documented public character-limit reference data for Google
Search ads, Meta (Facebook/Instagram) ads, and LinkedIn ads. With just a
'platform', returns that platform's full limit table. With a 'headline'
and/or 'description' also given, checks each against the platform's matching
field(s): length, limit, whether it fits, and characters remaining. Stdlib
only.

Request (stdin): {"platform": "google_search", "headline": "Save 20% Today", "description": "Free shipping on orders over $50, ends Sunday."}
Output (stdout): {platform, limits, checks?}
"""
import json
import sys

_PLATFORMS = {
    "google_search": {
        "headline": {"limit": 30, "note": "up to 3 headlines are combined/rotated in the served ad"},
        "description": {"limit": 90, "note": "up to 2 descriptions may be shown"},
    },
    "meta": {
        "primary_text": {"limit": 125, "note": "recommended length before the 'See More' truncation point; the hard limit is much higher (roughly 2200 characters)"},
        "headline": {"limit": 40, "note": "shown below the image/video in Feed-style placements"},
        "description": {"limit": 30, "note": "shown below the headline in some placements; frequently not displayed at all"},
    },
    "linkedin": {
        "headline": {"limit": 70, "note": "the bold text shown under the intro text"},
        "intro_text": {"limit": 150, "note": "recommended length before truncation; used as the 'description' field for this platform"},
    },
}

# which field a generic 'description' input maps to, per platform
_DESCRIPTION_FIELD = {
    "google_search": "description",
    "meta": "description",
    "linkedin": "intro_text",
}


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0

    example = {
        "platform": "google_search",
        "headline": "Save 20% Today",
        "description": "Free shipping on orders over $50, ends Sunday.",
    }

    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object", "example": example}))
        return 0

    platform_raw = q.get("platform")
    platform = platform_raw.strip().lower() if isinstance(platform_raw, str) else None
    if platform not in _PLATFORMS:
        print(json.dumps({
            "error": "'platform' must be one of the supported platforms",
            "supported_platforms": list(_PLATFORMS.keys()),
            "example": example,
        }))
        return 0

    headline = q.get("headline")
    description = q.get("description")
    if headline is not None and not isinstance(headline, str):
        print(json.dumps({"error": "'headline' must be a string", "example": example}))
        return 0
    if description is not None and not isinstance(description, str):
        print(json.dumps({"error": "'description' must be a string", "example": example}))
        return 0

    try:
        limits = _PLATFORMS[platform]
        result = {"platform": platform, "limits": limits}
        checks = {}

        if headline:
            limit = limits["headline"]["limit"]
            length = len(headline)
            checks["headline"] = {
                "length": length,
                "limit": limit,
                "fits": length <= limit,
                "remaining": limit - length,
            }

        if description:
            field = _DESCRIPTION_FIELD[platform]
            limit = limits[field]["limit"]
            length = len(description)
            checks["description"] = {
                "checked_against_field": field,
                "length": length,
                "limit": limit,
                "fits": length <= limit,
                "remaining": limit - length,
            }

        if checks:
            result["checks"] = checks

        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "ad_copy_check failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
