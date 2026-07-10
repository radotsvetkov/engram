#!/usr/bin/env python3
"""self_consistency_vote — Engram skill (no network).

Aggregates several candidate answers (e.g. from sampling an LLM multiple
times) by majority vote — the self-consistency technique. Optionally
normalizes whitespace/case before comparing, then reports the winner, the
vote tally, agreement percentage and a confidence label.

Request (stdin): {"answers": [str], "normalize"?: bool (default true)}
Output (stdout): {winner, vote_counts, agreement_pct, is_unanimous,
  total_answers, confidence, note}
"""
import json
import re
import sys
from collections import Counter, OrderedDict

_WS = re.compile(r"\s+")


def normalize_answer(s):
    return _WS.sub(" ", s.strip().lower())


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0

    example = {"answers": ["42", "42", "forty-two", "42"], "normalize": True}

    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object", "example": example}))
        return 0

    answers = q.get("answers")
    if not isinstance(answers, list) or not answers:
        print(json.dumps({"error": "missing required field 'answers' (non-empty list of strings)",
                          "example": example}))
        return 0

    normalize = q.get("normalize", True)
    if not isinstance(normalize, bool):
        normalize = bool(normalize)

    try:
        # Preserve a representative original spelling per normalized key (first seen).
        raw = [str(a) for a in answers]
        cleaned = [a.strip() for a in raw if a.strip() != ""]
        if not cleaned:
            print(json.dumps({"error": "all answers were empty after trimming",
                              "example": example}))
            return 0

        display = OrderedDict()  # key -> representative display string
        counts = Counter()
        for a in cleaned:
            key = normalize_answer(a) if normalize else a
            if key not in display:
                display[key] = a
            counts[key] += 1

        total = len(cleaned)
        # Winner: highest count; ties broken by first appearance (stable).
        winner_key = max(display.keys(), key=lambda k: (counts[k], -list(display.keys()).index(k)))
        winner_votes = counts[winner_key]
        agreement = winner_votes / total

        vote_counts = OrderedDict()
        for key in sorted(display.keys(), key=lambda k: (-counts[k], list(display.keys()).index(k))):
            vote_counts[display[key]] = counts[key]

        is_unanimous = len(display) == 1
        if agreement >= 0.8:
            confidence = "high"
        elif agreement >= 0.5:
            confidence = "moderate"
        else:
            confidence = "low — answers diverge, consider more samples or a tiebreak"

        result = {
            "winner": display[winner_key],
            "vote_counts": vote_counts,
            "agreement_pct": round(agreement * 100, 2),
            "is_unanimous": is_unanimous,
            "total_answers": total,
            "distinct_answers": len(display),
            "confidence": confidence,
            "normalized": normalize,
            "note": ("Self-consistency: sample the same question several times and take the "
                     "majority answer. Higher agreement means more reliable; low agreement is a "
                     "signal to sample more or reformulate the question."),
        }
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "self_consistency_vote failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
