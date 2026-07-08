#!/usr/bin/env python3
"""youtube_metadata_optimize — Engram skill (no network). Score a video's title/description/tags.

Checks title length (ideal <=60 chars, hard max 100), description length
(first ~125 chars matter most — shown above the fold before "Show more",
hard max 5000 chars), whether a target keyword appears in the title and in
the description's first 125 characters, and whether tags stay within
YouTube's practical ~500-character combined budget. Rolls it into a 0-100
score with suggestions. Stdlib only.

Request (stdin): {"title": "10 SEO Tips for 2026", "description": "Learn SEO...", "tags": ["seo", "marketing"], "target_keyword": "SEO tips"}
Output (stdout): {title_length, title_length_check, description_length,
                   description_length_check, target_keyword_in_title,
                   target_keyword_in_description_first_125_chars,
                   tags_count, tags_char_count, tags_over_budget, score, suggestions}
"""
import json
import sys

DESCRIPTION_ABOVE_FOLD = 125
DESCRIPTION_MAX = 5000
TITLE_IDEAL_MAX = 60
TITLE_HARD_MAX = 100
TAGS_CHAR_BUDGET = 500


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0

    example = {
        "title": "10 SEO Tips for 2026",
        "description": "Learn how to improve your SEO rankings this year...",
        "tags": ["seo", "marketing"],
        "target_keyword": "SEO tips",
    }

    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object", "example": example}))
        return 0

    title = q.get("title")
    if not isinstance(title, str) or not title.strip():
        print(json.dumps({
            "error": "missing required field 'title' (string)",
            "example": example,
        }))
        return 0

    description = q.get("description", "") or ""
    if not isinstance(description, str):
        print(json.dumps({"error": "'description' must be a string", "example": example}))
        return 0

    tags = q.get("tags", []) or []
    if not isinstance(tags, list) or not all(isinstance(t, str) for t in tags):
        print(json.dumps({"error": "'tags' must be a list of strings", "example": example}))
        return 0

    target_keyword = q.get("target_keyword")
    if target_keyword is not None and not isinstance(target_keyword, str):
        print(json.dumps({"error": "'target_keyword' must be a string", "example": example}))
        return 0

    try:
        score = 100
        suggestions = []

        title_length = len(title)
        if title_length <= TITLE_IDEAL_MAX:
            title_length_check = "ok"
        elif title_length <= TITLE_HARD_MAX:
            title_length_check = "long (may truncate)"
            score -= 10
            suggestions.append("shorten the title to %d characters or fewer so it doesn't truncate in search/suggested results" % TITLE_IDEAL_MAX)
        else:
            title_length_check = "exceeds 100 char limit"
            score -= 25
            suggestions.append("title exceeds YouTube's %d-character hard limit — shorten it" % TITLE_HARD_MAX)

        description_length = len(description)
        if not description.strip():
            description_length_check = "ok"
            score -= 15
            suggestions.append("add a description — the first ~%d characters show above the fold and help both viewers and search" % DESCRIPTION_ABOVE_FOLD)
        elif description_length > DESCRIPTION_MAX:
            description_length_check = "exceeds 5000 char limit"
            score -= 10
            suggestions.append("description exceeds YouTube's %d-character limit — trim it" % DESCRIPTION_MAX)
        else:
            description_length_check = "ok"

        target_keyword_in_title = None
        target_keyword_in_description_first_125 = None
        tk = target_keyword.strip().lower() if isinstance(target_keyword, str) else ""
        if tk:
            target_keyword_in_title = tk in title.lower()
            target_keyword_in_description_first_125 = tk in description[:DESCRIPTION_ABOVE_FOLD].lower()
            if target_keyword_in_title is False:
                score -= 20
                suggestions.append("include the target keyword %r in the title" % target_keyword)
            if target_keyword_in_description_first_125 is False:
                score -= 15
                suggestions.append("include the target keyword %r within the first %d characters of the description" % (target_keyword, DESCRIPTION_ABOVE_FOLD))

        tags_char_count = sum(len(t) for t in tags)
        tags_over_budget = tags_char_count > TAGS_CHAR_BUDGET
        if not tags:
            score -= 10
            suggestions.append("add tags — they help YouTube understand and surface the video for related searches")
        elif tags_over_budget:
            score -= 10
            suggestions.append("tags total %d characters, over the ~%d-character practical budget — trim the list" % (tags_char_count, TAGS_CHAR_BUDGET))

        score = max(0, min(100, score))

        result = {
            "title": title,
            "title_length": title_length,
            "title_length_check": title_length_check,
            "description_length": description_length,
            "description_length_check": description_length_check,
            "target_keyword": target_keyword,
            "target_keyword_in_title": target_keyword_in_title,
            "target_keyword_in_description_first_125_chars": target_keyword_in_description_first_125,
            "tags_count": len(tags),
            "tags_char_count": tags_char_count,
            "tags_over_budget": tags_over_budget,
            "score": score,
            "suggestions": suggestions,
        }
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "youtube_metadata_optimize failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
