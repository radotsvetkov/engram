#!/usr/bin/env python3
"""email_subject_check — Engram skill (no network). Check an email subject line.

Flags mobile-truncation risk, ALL-CAPS shouting, excess exclamation marks,
spam-trigger phrases, and emoji use, then rolls it into a 0-100 deliverability
score. Stdlib only.

Request (stdin): {"subject": "FREE gift inside!!! Act now"}
Output (stdout): {char_count, word_count, truncation_risk, all_caps_words,
                   exclamation_count, spam_trigger_hits, has_emoji, score, verdict}
"""
import json
import re
import sys

SPAM_TRIGGERS = [
    "free", "guarantee", "act now", "limited time", "click here", "buy now",
    "winner", "cash", "$$$", "urgent", "congratulations", "risk-free",
    "no cost", "act immediately", "apply now", "cancel anytime", "order now",
    "don't miss", "best price", "satisfaction guaranteed", "earn money",
    "work from home", "no obligation", "special promotion", "while supplies last",
]

_EMOJI_PATTERN = re.compile(
    "[\U0001F300-\U0001FAFF\U00002600-\U000027BF\U0001F1E6-\U0001F1FF\U00002B00-\U00002BFF]",
    flags=re.UNICODE,
)


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0
    subject = q.get("subject")
    if not isinstance(subject, str) or not subject.strip():
        print(json.dumps({
            "error": "provide 'subject'",
            "example": {"subject": "Your October newsletter is here"},
        }))
        return 0
    subject = subject.strip()

    try:
        char_count = len(subject)
        words = re.findall(r"[A-Za-z0-9'\-]+", subject)
        word_count = len(words)
        all_caps_words = [w for w in words if len(w) > 1 and w.isupper()]
        exclamation_count = subject.count("!")
        lower = subject.lower()
        spam_trigger_hits = [t for t in SPAM_TRIGGERS if t in lower]
        has_emoji = bool(_EMOJI_PATTERN.search(subject))
        truncation_risk = char_count > 50

        score = 100
        score -= len(spam_trigger_hits) * 10
        score -= len(all_caps_words) * 8
        if truncation_risk:
            score -= 10
        if exclamation_count > 1:
            score -= (exclamation_count - 1) * 5
        score = max(score, 0)

        verdict = "looks clean" if score >= 60 else "may trip spam filters"

        result = {
            "subject": subject,
            "char_count": char_count,
            "word_count": word_count,
            "truncation_risk": truncation_risk,
            "all_caps_words": all_caps_words,
            "exclamation_count": exclamation_count,
            "spam_trigger_hits": spam_trigger_hits,
            "has_emoji": has_emoji,
            "score": score,
            "verdict": verdict,
        }
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "email subject check failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
