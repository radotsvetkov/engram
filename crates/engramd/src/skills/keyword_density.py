#!/usr/bin/env python3
"""keyword_density — Engram skill (no network). SEO keyword density analysis.

Tokenizes text into lowercase words and reports how often given keywords (or,
if none given, the top 10 most frequent non-stopword words) appear — including
density percentage and how early each first appears (lower = earlier = better
for SEO). Stdlib only.

Request (stdin): {"text": "...", "keywords": ["seo", "content marketing"]}
  - keywords is optional; omit it to auto-derive the top 10 frequent words.
Output (stdout): {total_words, unique_words, keywords: [...]} or
                 {total_words, unique_words, top_words: [...]}
"""
import json
import re
import sys
from collections import Counter

STOPWORDS = {
    "a", "an", "the", "is", "are", "was", "were", "be", "been", "being",
    "of", "to", "in", "on", "at", "for", "with", "by", "from", "as",
    "and", "or", "but", "not", "no", "nor",
    "this", "that", "these", "those",
    "it", "its", "it's", "he", "she", "they", "we", "you", "i",
    "him", "her", "them", "us", "me", "my", "his", "their", "our", "your",
    "if", "then", "so", "than", "too", "very",
    "can", "will", "just", "do", "does", "did", "done",
    "have", "has", "had", "having",
    "am", "up", "down", "out", "over", "under", "again",
    "there", "here", "when", "where", "who", "whom", "which", "what", "why", "how",
    "all", "any", "both", "each", "few", "more", "most", "other", "some", "such",
    "only", "own", "same", "s", "t", "don", "now", "into", "about", "above", "below",
    "between", "through", "during", "before", "after", "further", "once",
}


def _tokenize(s):
    return re.findall(r"\b[a-z0-9']+\b", s.lower())


def _keyword_stats(words, total_words, keyword):
    kw_tokens = _tokenize(keyword)
    if not kw_tokens:
        return {"keyword": keyword, "count": 0, "density_pct": 0.0, "first_position_pct": None}
    n = len(kw_tokens)
    count = 0
    first_idx = None
    i = 0
    limit = len(words) - n
    while i <= limit:
        if words[i:i + n] == kw_tokens:
            count += 1
            if first_idx is None:
                first_idx = i
            i += n  # non-overlapping matches
        else:
            i += 1
    density_pct = round(count / total_words * 100, 2) if total_words else 0.0
    first_position_pct = (
        round(first_idx / total_words * 100, 2) if first_idx is not None and total_words else None
    )
    return {
        "keyword": keyword,
        "count": count,
        "density_pct": density_pct,
        "first_position_pct": first_position_pct,
    }


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
            "example": {"text": "SEO is great. SEO helps content marketing.", "keywords": ["seo"]},
        }))
        return 0
    keywords = q.get("keywords")
    if keywords is not None and not isinstance(keywords, list):
        print(json.dumps({"error": "'keywords' must be a list of strings if provided"}))
        return 0

    try:
        words = _tokenize(text)
        total_words = len(words)
        unique_words = len(set(words))
        result = {"total_words": total_words, "unique_words": unique_words}
        if keywords:
            result["keywords"] = [
                _keyword_stats(words, total_words, str(k)) for k in keywords if str(k).strip()
            ]
        else:
            counts = Counter(w for w in words if w not in STOPWORDS)
            top = counts.most_common(10)
            result["top_words"] = [
                {
                    "word": w,
                    "count": c,
                    "density_pct": round(c / total_words * 100, 2) if total_words else 0.0,
                }
                for w, c in top
            ]
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "keyword density analysis failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
