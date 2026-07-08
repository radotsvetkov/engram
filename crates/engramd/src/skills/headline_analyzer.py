#!/usr/bin/env python3
"""headline_analyzer — Engram skill (no network). Score a headline for copywriting strength.

Checks word/char count, numbers, question framing, "how to" framing, and
power-word usage, then rolls it into a 0-100 score with actionable
suggestions. Stdlib only.

Request (stdin): {"headline": "7 Proven Ways to Boost Your SEO Rankings"}
Output (stdout): {word_count, char_count, has_number, starts_with_number,
                   is_question, has_how_to, power_word_hits, length_assessment,
                   score, suggestions}
"""
import json
import re
import sys

POWER_WORDS = [
    "free", "proven", "secret", "instantly", "ultimate", "essential", "guide",
    "best", "new", "exclusive", "powerful", "easy", "amazing", "surprising",
    "effortless", "guaranteed", "incredible", "limited", "boost", "transform",
    "discover", "unlock", "insider", "effective", "simple", "quick",
    "remarkable", "stunning", "shocking", "revolutionary",
]


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0
    headline = q.get("headline")
    if not isinstance(headline, str) or not headline.strip():
        print(json.dumps({
            "error": "provide 'headline'",
            "example": {"headline": "7 Proven Ways to Boost Your SEO Rankings"},
        }))
        return 0
    headline = headline.strip()

    try:
        words = re.findall(r"[A-Za-z0-9']+", headline)
        word_count = len(words)
        char_count = len(headline)
        has_number = bool(re.search(r"\d", headline))
        starts_with_number = bool(re.match(r"^\s*\d", headline))
        is_question = headline.endswith("?")
        has_how_to = "how to" in headline.lower()

        lower = headline.lower()
        power_word_hits = [w for w in POWER_WORDS if re.search(r"\b%s\b" % re.escape(w), lower)]

        if word_count < 6:
            length_assessment = "short"
        elif word_count > 12:
            length_assessment = "long"
        else:
            length_assessment = "ideal"

        score = 50
        if length_assessment == "ideal":
            score += 15
        score += min(len(power_word_hits) * 5, 20)
        if has_number:
            score += 10
        if is_question or has_how_to:
            score += 10
        score = min(score, 100)

        suggestions = []
        if score < 80:
            if length_assessment == "short":
                suggestions.append("expand toward 6-12 words for more context")
            elif length_assessment == "long":
                suggestions.append("tighten to under 12 words")
            if not has_number:
                suggestions.append("add a number (e.g. a count or year)")
            if not power_word_hits:
                suggestions.append("consider a power word like 'proven' or 'essential'")
            if not (is_question or has_how_to):
                suggestions.append("try a question or a 'how to' framing")
        suggestions = suggestions[:3]

        result = {
            "headline": headline,
            "word_count": word_count,
            "char_count": char_count,
            "has_number": has_number,
            "starts_with_number": starts_with_number,
            "is_question": is_question,
            "has_how_to": has_how_to,
            "power_word_hits": power_word_hits,
            "length_assessment": length_assessment,
            "score": score,
            "suggestions": suggestions,
        }
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "headline analysis failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
