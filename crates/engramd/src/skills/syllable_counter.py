#!/usr/bin/env python3
"""syllable_counter — Engram skill (no network). Estimate syllables per line
of text, for songwriters and poets checking meter.

Uses a lightweight heuristic: count vowel-group transitions per word via the
regex [aeiouy]+ against the lowercased word, then subtract 1 if the word
ends in a silent "e" and would otherwise count more than one syllable.
Every non-empty word counts as at least 1 syllable. Results are reported
per line (splitting `text` on newlines; a single-line input still returns
a 1-element array) plus totals across all lines.

Request (stdin): {"text": "Roses are red\\nViolets are blue"}
Output (stdout): {lines: [{line, syllable_count, word_count}, ...], total_syllables, total_words}
"""
import json
import re
import sys

WORD_RE = re.compile(r"[A-Za-z']+")
VOWEL_GROUP_RE = re.compile(r"[aeiouy]+")


def _count_syllables(word):
    w = word.lower()
    groups = VOWEL_GROUP_RE.findall(w)
    count = len(groups)
    if w.endswith("e") and count > 1:
        count -= 1
    return max(count, 1)


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0
    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object", "example": {"text": "Roses are red"}}))
        return 0

    text = q.get("text")
    if not isinstance(text, str) or text.strip() == "":
        print(json.dumps({"error": "provide non-empty 'text'", "example": {"text": "Roses are red\nViolets are blue"}}))
        return 0

    try:
        raw_lines = text.split("\n")
        lines_out = []
        total_syllables = 0
        total_words = 0
        for raw_line in raw_lines:
            words = WORD_RE.findall(raw_line)
            syllable_count = sum(_count_syllables(w) for w in words)
            word_count = len(words)
            lines_out.append({
                "line": raw_line,
                "syllable_count": syllable_count,
                "word_count": word_count,
            })
            total_syllables += syllable_count
            total_words += word_count
    except Exception as e:
        print(json.dumps({"error": "syllable counting failed: %s" % e}))
        return 1

    print(json.dumps({
        "lines": lines_out,
        "total_syllables": total_syllables,
        "total_words": total_words,
    }, indent=2, default=str))
    return 0


if __name__ == "__main__":
    sys.exit(main())
