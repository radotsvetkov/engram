#!/usr/bin/env python3
"""hashtag_suggest — Engram skill (no network). Deterministic hashtag ideas from a topic.

Splits the topic into words and builds three kinds of candidates: (a) a single
CamelCase hashtag of the whole topic, (b) one hashtag per significant word
(common stopwords skipped), and (c) a couple of common suffix-template
variants (Tips/101/Ideas) which are clearly templated pattern suggestions, NOT
derived from real trending-hashtag data. Also returns static, well-known
per-platform guidance on how many hashtags to use.

Request (stdin): {"topic": "content marketing", "platform": "instagram"}
Output (stdout): {topic, platform, candidates: [{tag, type}], platform_guidance, note}
"""
import json
import re
import sys

_STOPWORDS = {"a", "an", "the", "for", "of", "in", "on", "to", "and"}

_GUIDANCE = {
    "instagram": "5-15 in caption or first comment",
    "twitter": "1-2 max, more reads as spam",
    "tiktok": "3-8 mixing broad + niche",
    "linkedin": "3-5 professional/industry tags",
}

_ALIASES = {
    "instagram": "instagram", "ig": "instagram", "insta": "instagram",
    "twitter": "twitter", "x": "twitter",
    "tiktok": "tiktok", "tik-tok": "tiktok",
    "linkedin": "linkedin", "li": "linkedin",
}

_MAX_CANDIDATES = 15


def _camel(words):
    return "".join(w[:1].upper() + w[1:].lower() for w in words if w)


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0

    example = {"topic": "content marketing", "platform": "instagram"}

    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object", "example": example}))
        return 0

    topic = q.get("topic")
    if not isinstance(topic, str) or not topic.strip():
        print(json.dumps({
            "error": "missing required field 'topic' (string)",
            "example": example,
        }))
        return 0

    platform_raw = q.get("platform", "instagram")
    if not isinstance(platform_raw, str):
        print(json.dumps({"error": "'platform' must be a string", "example": example}))
        return 0

    note = None
    platform_key = _ALIASES.get(platform_raw.strip().lower())
    if platform_key is None:
        note = "unrecognized platform %r — defaulted to 'instagram'" % platform_raw
        platform_key = "instagram"

    try:
        words = re.findall(r"[A-Za-z0-9]+", topic)
        if not words:
            print(json.dumps({
                "error": "topic contains no usable words (letters/numbers)",
                "example": example,
            }))
            return 0

        significant = [w for w in words if w.lower() not in _STOPWORDS]

        candidates = []
        seen = set()

        def add(tag, kind):
            if tag not in seen:
                seen.add(tag)
                candidates.append({"tag": tag, "type": kind})

        # (a) whole-topic CamelCase hashtag.
        whole = _camel(words)
        if whole:
            add("#" + whole, "full_topic")

        # (b) per-significant-word hashtags.
        for w in significant:
            tag = "#" + (w[:1].upper() + w[1:].lower())
            add(tag, "keyword")

        # (c) templated suffix variants (pattern-based, not real trending data).
        base = _camel(significant) or whole
        if base:
            for suffix in ("Tips", "101", "Ideas"):
                add("#" + base + suffix, "templated_suggestion")

        candidates = candidates[:_MAX_CANDIDATES]

        result = {
            "topic": topic,
            "platform": platform_key,
            "candidates": candidates,
            "platform_guidance": {
                "platform": platform_key,
                "recommended_count": _GUIDANCE[platform_key],
            },
            "disclaimer": (
                "Candidates are generated deterministically from the topic text "
                "(CamelCase join, per-keyword, and Tips/101/Ideas templates) — "
                "they are pattern-based suggestions, not real trending-hashtag data."
            ),
        }
        if note:
            result["note"] = note
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "hashtag_suggest failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
