#!/usr/bin/env python3
"""content_brief — Engram skill (no network). Structured content-writing brief from a topic.

Generates templated title options, a generic-but-useful outline (adapted to a
target keyword where given), a word-count target heuristic by goal, a filled
meta-description template, and FAQ question prompts. Purely templated —
does not check real search volume, rankings, or competitor content.

Request (stdin): {"topic": "email marketing", "target_keyword": "email marketing automation", "audience": "small business owners", "goal": "seo"}
Output (stdout): {topic, target_keyword, audience, goal, title_options, recommended_outline, word_count_target, meta_description_template, faq_prompts}
"""
import datetime
import json
import sys

_WORD_COUNT_BY_GOAL = {
    "seo": "1500-2500 words (comprehensive, ranks better for competitive terms)",
    "conversion": "600-1200 words (focused, scannable, single CTA)",
    "awareness": "800-1500 words",
}
_DEFAULT_WORD_COUNT = "1000-1800 words"
_LISTICLE_N = 10


def _truncate(text, limit):
    if len(text) <= limit:
        return text
    cut = text[:limit - 3].rstrip()
    return cut + "..."


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0

    example = {
        "topic": "email marketing",
        "target_keyword": "email marketing automation",
        "audience": "small business owners",
        "goal": "seo",
    }

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
    topic = topic.strip()

    target_keyword = q.get("target_keyword")
    if target_keyword is not None and (not isinstance(target_keyword, str) or not target_keyword.strip()):
        print(json.dumps({"error": "'target_keyword' must be a non-empty string if provided", "example": example}))
        return 0
    target_keyword = target_keyword.strip() if isinstance(target_keyword, str) else None

    audience = q.get("audience")
    if audience is not None and not isinstance(audience, str):
        print(json.dumps({"error": "'audience' must be a string if provided", "example": example}))
        return 0

    goal = q.get("goal")
    if goal is not None and not isinstance(goal, str):
        print(json.dumps({"error": "'goal' must be a string if provided", "example": example}))
        return 0
    goal_key = goal.strip().lower() if isinstance(goal, str) else None

    try:
        current_year = datetime.date.today().year
        keyword_term = target_keyword or topic

        title_options = [
            "The Complete Guide to %s" % topic,
            "%s: Everything You Need to Know in %d" % (topic, current_year),
            "How to %s (Step-by-Step)" % topic,
            "%d %s Tips That Actually Work" % (_LISTICLE_N, topic),
        ]

        recommended_outline = [
            "Introduction / hook",
            "What is %s?" % keyword_term,
            "Why %s matters" % keyword_term,
            "How to get started with %s: step-by-step" % topic,
            "Common mistakes to avoid",
            "Examples and case studies",
            "FAQ",
            "Conclusion and call to action",
        ]

        word_count_target = _WORD_COUNT_BY_GOAL.get(goal_key, _DEFAULT_WORD_COUNT)

        meta_description_template = _truncate(
            "Learn everything about %s: key concepts, practical tips, and real "
            "examples to help you get started today." % keyword_term,
            160,
        )

        faq_prompts = [
            "What is %s?" % topic,
            "How does %s work?" % topic,
            "Why is %s important?" % topic,
            "What are the best %s tools/practices?" % topic,
            "How much does %s cost?" % topic,
        ]

        result = {
            "topic": topic,
            "target_keyword": target_keyword,
            "audience": audience,
            "goal": goal,
            "title_options": title_options,
            "recommended_outline": recommended_outline,
            "word_count_target": word_count_target,
            "meta_description_template": meta_description_template,
            "faq_prompts": faq_prompts,
        }
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "content_brief failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
