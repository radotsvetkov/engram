#!/usr/bin/env python3
"""social_char_limits — Engram skill (no network). Static social-platform character-limit reference.

Embeds well-documented public character limits for common social platforms and
fields. With no 'platform', returns the whole table. With a 'platform' (fuzzy
matched, case-insensitive, common aliases like "twitter"/"x" -> x_post,
"ig" -> instagram_caption), returns that field's limit; if 'text' is also
given, returns how much room is left and whether it fits.

Request (stdin): {"platform": "twitter", "text": "hello world"}
Output (stdout): {platform, limit} or {platform, limit, text_length, remaining, fits} or {limits: {...}}
"""
import difflib
import json
import sys

_LIMITS = {
    "x_post": 280,
    "x_premium_post": 25000,
    "instagram_caption": 2200,
    "instagram_bio": 150,
    "linkedin_post": 3000,
    "linkedin_headline": 220,
    "facebook_post": 63206,
    "facebook_practical_visible": 477,
    "youtube_title": 100,
    "youtube_description": 5000,
    "tiktok_caption": 2200,
    "threads_post": 500,
    "pinterest_description": 500,
}

_ALIASES = {
    "twitter": "x_post", "x": "x_post", "tweet": "x_post",
    "twitter_premium": "x_premium_post", "x_premium": "x_premium_post", "premium": "x_premium_post",
    "instagram": "instagram_caption", "ig": "instagram_caption", "insta": "instagram_caption",
    "instagram_bio": "instagram_bio", "ig_bio": "instagram_bio",
    "linkedin": "linkedin_post", "li": "linkedin_post",
    "linkedin_bio": "linkedin_headline", "li_headline": "linkedin_headline",
    "facebook": "facebook_post", "fb": "facebook_post",
    "facebook_visible": "facebook_practical_visible", "fb_visible": "facebook_practical_visible",
    "youtube": "youtube_description", "yt": "youtube_description",
    "youtube_desc": "youtube_description",
    "tiktok": "tiktok_caption", "tik_tok": "tiktok_caption",
    "threads": "threads_post",
    "pinterest": "pinterest_description", "pin": "pinterest_description",
}


def _resolve(platform_raw):
    """Fuzzy-resolve a user-supplied platform string to a canonical key, or None."""
    key = platform_raw.strip().lower().replace(" ", "_").replace("-", "_")
    if key in _LIMITS:
        return key
    if key in _ALIASES:
        return _ALIASES[key]
    # Fallback: nearest match against canonical keys + aliases.
    pool = list(_LIMITS.keys()) + list(_ALIASES.keys())
    matches = difflib.get_close_matches(key, pool, n=1, cutoff=0.5)
    if matches:
        m = matches[0]
        return _ALIASES.get(m, m)
    return None


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0

    example = {"platform": "twitter", "text": "hello world"}

    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object", "example": example}))
        return 0

    platform_raw = q.get("platform")
    text = q.get("text")

    if text is not None and not isinstance(text, str):
        print(json.dumps({"error": "'text' must be a string", "example": example}))
        return 0

    try:
        if platform_raw is None:
            print(json.dumps({"limits": _LIMITS}, indent=2, default=str))
            return 0

        if not isinstance(platform_raw, str) or not platform_raw.strip():
            print(json.dumps({
                "error": "'platform' must be a non-empty string",
                "valid_platforms": list(_LIMITS.keys()),
                "example": example,
            }))
            return 0

        key = _resolve(platform_raw)
        if key is None:
            print(json.dumps({
                "error": "could not match platform %r to a known field" % platform_raw,
                "valid_platforms": list(_LIMITS.keys()),
                "example": example,
            }))
            return 0

        limit = _LIMITS[key]
        result = {"platform": key, "limit": limit}
        if text is not None:
            text_length = len(text)
            result["text_length"] = text_length
            result["remaining"] = limit - text_length
            result["fits"] = text_length <= limit
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "social_char_limits failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
