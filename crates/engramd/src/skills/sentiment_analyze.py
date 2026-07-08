#!/usr/bin/env python3
"""sentiment_analyze — Engram skill (no network). Lightweight lexicon-based sentiment scoring.

This is a SIMPLE HEURISTIC, not a machine-learned model — it is not
state-of-the-art sentiment analysis. It tokenizes the input text, counts hits
against two built-in word lists (~50 positive, ~50 negative), applies basic
negation handling (a negation word like "not"/"no"/"never" — including
normalized contractions such as "don't", "doesn't", "isn't", "won't", "can't"
— within the 2 words immediately before a sentiment word flips that word's
polarity), and derives a score in [-1, 1].

Request (stdin): {"text": "I don't like this, it's terrible"}
Output (stdout): {sentiment_score, label, positive_hits, negative_hits, word_count}
"""
import json
import re
import sys

POSITIVE_WORDS = {
    "good", "great", "excellent", "amazing", "love", "loved", "loves", "happy", "best", "wonderful",
    "fantastic", "perfect", "awesome", "brilliant", "outstanding", "superb", "delighted", "pleased",
    "satisfied", "recommend", "impressive", "exceptional", "fabulous", "terrific", "delightful",
    "enjoyable", "positive", "remarkable", "incredible", "favorite", "glad", "joy", "joyful", "nice",
    "beautiful", "charming", "elegant", "effective", "efficient", "reliable", "robust", "helpful",
    "friendly", "kind", "grateful", "thrilled", "appreciate", "success", "successful", "superior",
}

NEGATIVE_WORDS = {
    "bad", "terrible", "awful", "hate", "hated", "hates", "worst", "horrible", "disappointing", "poor",
    "frustrating", "annoying", "broken", "useless", "waste", "angry", "upset", "unacceptable", "fail",
    "failed", "failure", "problem", "issue", "disappointed", "sad", "unhappy", "disgusting", "pathetic",
    "inferior", "mediocre", "slow", "buggy", "glitchy", "confusing", "complicated", "rude", "hostile",
    "painful", "difficult", "lousy", "dreadful", "atrocious", "abysmal", "inadequate", "subpar",
    "regret", "concerning", "worried", "worthless", "damaged",
}

# After normalizing contractions ("don't" -> "do not", "doesn't" -> "does not",
# "isn't" -> "is not", "won't" -> "wo not", "can't" -> "can not"), the actual
# negation trigger token is always "not" (plus the standalone words "no"/"never").
NEGATION_WORDS = {"not", "no", "never"}

WINDOW = 2  # look back this many tokens for a negation trigger


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e}))
        return 0

    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"text": "I don't like this, it's terrible"},
        }))
        return 0

    text = q.get("text")
    if not text or not str(text).strip():
        print(json.dumps({
            "error": "missing required field: text",
            "example": {"text": "I don't like this, it's terrible"},
        }))
        return 0
    text = str(text)

    try:
        lowered = text.lower()
        # Normalize contractions ("n't" -> " not") so negation detection is
        # simple substring/token matching rather than a big list of variants.
        normalized = re.sub(r"n't", " not", lowered)
        tokens = re.findall(r"\b\w+\b", normalized)

        positive_hits = 0
        negative_hits = 0
        for i, tok in enumerate(tokens):
            is_pos = tok in POSITIVE_WORDS
            is_neg = tok in NEGATIVE_WORDS
            if not is_pos and not is_neg:
                continue
            window = tokens[max(0, i - WINDOW):i]
            negated = any(w in NEGATION_WORDS for w in window)
            if is_pos:
                if negated:
                    negative_hits += 1
                else:
                    positive_hits += 1
            else:
                if negated:
                    positive_hits += 1
                else:
                    negative_hits += 1

        total = positive_hits + negative_hits
        sentiment_score = (positive_hits - negative_hits) / max(1, total)
        if sentiment_score > 0.2:
            label = "positive"
        elif sentiment_score < -0.2:
            label = "negative"
        else:
            label = "neutral"

        result = {
            "sentiment_score": round(sentiment_score, 3),
            "label": label,
            "positive_hits": positive_hits,
            "negative_hits": negative_hits,
            "word_count": len(tokens),
        }
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "sentiment_analyze failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
