#!/usr/bin/env python3
"""ngram_analyze — Engram skill (no network). Frequency analysis of word n-grams.

Tokenizes to lowercase words, generates all n-grams (n in 1..5), counts frequencies, and
returns the most common. Also reports the single most common word for convenience.

Request (stdin): {"text": "the cat the dog the cat", "n": 2, "top": 15}
Output (stdout): {n, total_ngrams, unique_ngrams, most_common_word, top_ngrams:[{ngram, count}]}
"""
import json, sys, re
from collections import Counter

_WORD_RE = re.compile(r"[a-z0-9']+")


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e})); return 0

    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"text": "the cat the dog the cat", "n": 2, "top": 15},
        })); return 0

    text = q.get("text")
    if text is None or not isinstance(text, str):
        print(json.dumps({
            "error": "missing required field 'text' (string)",
            "example": {"text": "the cat the dog the cat", "n": 2, "top": 15},
        })); return 0

    n = q.get("n", 2)
    if not isinstance(n, int) or isinstance(n, bool) or n < 1 or n > 5:
        print(json.dumps({"error": "'n' must be an integer in 1..5"})); return 0

    top = q.get("top", 15)
    if not isinstance(top, int) or isinstance(top, bool) or top < 1:
        print(json.dumps({"error": "'top' must be a positive integer"})); return 0

    try:
        tokens = _WORD_RE.findall(text.lower())
        word_counts = Counter(tokens)
        most_common_word = word_counts.most_common(1)[0][0] if tokens else None

        if len(tokens) < n:
            result = {
                "n": n,
                "total_ngrams": 0,
                "unique_ngrams": 0,
                "most_common_word": most_common_word,
                "top_ngrams": [],
                "note": "text has fewer than n (%d) tokens" % n,
            }
            print(json.dumps(result, indent=2, default=str)); return 0

        ngrams = [" ".join(tokens[i:i + n]) for i in range(len(tokens) - n + 1)]
        counts = Counter(ngrams)
        result = {
            "n": n,
            "total_ngrams": len(ngrams),
            "unique_ngrams": len(counts),
            "most_common_word": most_common_word,
            "top_ngrams": [{"ngram": g, "count": c} for g, c in counts.most_common(top)],
        }
        print(json.dumps(result, indent=2, default=str)); return 0
    except Exception as e:
        print(json.dumps({"error": "ngram_analyze failed: %s" % e})); return 1


if __name__ == "__main__":
    sys.exit(main())
