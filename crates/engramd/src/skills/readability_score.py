#!/usr/bin/env python3
"""readability_score — Engram skill (no network). Flesch readability scoring.

Computes Flesch Reading Ease and Flesch-Kincaid Grade Level for a block of
text using a heuristic syllable counter (vowel-group transitions). Stdlib only.

Request (stdin): {"text": "Your article or paragraph goes here."}
Output (stdout): {sentences, words, syllables, flesch_reading_ease,
                   flesch_kincaid_grade, label}
"""
import json
import re
import sys


def _count_syllables(word):
    w = word.lower()
    groups = re.findall(r"[aeiouy]+", w)
    count = len(groups)
    if w.endswith("e") and count > 1:
        count -= 1
    return max(count, 1)


def _label(score):
    s = max(0, min(100, score))
    if s >= 90:
        return "very easy"
    if s >= 80:
        return "easy"
    if s >= 70:
        return "fairly easy"
    if s >= 60:
        return "standard"
    if s >= 50:
        return "fairly difficult"
    if s >= 30:
        return "difficult"
    return "very confusing"


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0
    text = q.get("text")
    if not isinstance(text, str) or not text.strip():
        print(json.dumps({
            "error": "provide 'text'",
            "example": {"text": "Short sentences are easy to read. Long, complex sentences with many clauses are harder."},
        }))
        return 0

    try:
        parts = re.split(r"[.!?]+", text)
        sentences = len([p for p in parts if p.strip()])
        sentences = max(sentences, 1)

        word_list = re.findall(r"\b\w+\b", text)
        words = len(word_list)
        if words == 0:
            print(json.dumps({"error": "no words found in 'text'"}))
            return 0

        syllables = sum(_count_syllables(w) for w in word_list)

        flesch_reading_ease = 206.835 - 1.015 * (words / sentences) - 84.6 * (syllables / words)
        flesch_kincaid_grade = 0.39 * (words / sentences) + 11.8 * (syllables / words) - 15.59

        result = {
            "sentences": sentences,
            "words": words,
            "syllables": syllables,
            "flesch_reading_ease": round(flesch_reading_ease, 1),
            "flesch_kincaid_grade": round(flesch_kincaid_grade, 1),
            "label": _label(flesch_reading_ease),
        }
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "readability scoring failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
