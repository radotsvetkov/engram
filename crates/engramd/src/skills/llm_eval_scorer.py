#!/usr/bin/env python3
"""llm_eval_scorer — Engram skill (no network). Score LLM outputs against
expected answers using exact-match and token-overlap F1, a lightweight
alternative to a full eval harness.

Request (stdin): {
    "cases": [
        {"prompt": "2+2?", "expected": "4", "actual": "The answer is 4."},
        {"expected": "Paris", "actual": "paris"}
    ]
}
("prompt" is optional per case, purely for context in the output.)
Output (stdout): {
    "results": [
        {"prompt", "expected", "actual", "exact_match": bool, "f1_score": float}, ...
    ],
    "exact_match_rate": float,   # percent, 0-100
    "average_f1": float,          # percent, 0-100
    "case_count": int
}
"""
import json
import re
import sys
from collections import Counter

_EXAMPLE = {
    "cases": [
        {"prompt": "2+2?", "expected": "4", "actual": "The answer is 4."},
        {"expected": "Paris", "actual": "paris"},
    ]
}

_TOKEN_RE = re.compile(r"\b\w+\b")


def _tokenize(text):
    return _TOKEN_RE.findall(text.lower())


def _f1(expected_tokens, actual_tokens):
    """Standard token-multiset overlap F1 (as in SQuAD-style QA eval)."""
    if not expected_tokens and not actual_tokens:
        return 1.0
    if not expected_tokens or not actual_tokens:
        return 0.0
    common = Counter(expected_tokens) & Counter(actual_tokens)
    num_common = sum(common.values())
    if num_common == 0:
        return 0.0
    precision = num_common / len(actual_tokens)
    recall = num_common / len(expected_tokens)
    return 2 * precision * recall / (precision + recall)


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0

    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object", "example": _EXAMPLE}))
        return 0

    cases = q.get("cases")
    if not isinstance(cases, list) or not cases:
        print(json.dumps({
            "error": "provide a non-empty 'cases' list, each with 'expected' and 'actual'",
            "example": _EXAMPLE,
        }))
        return 0

    for i, case in enumerate(cases):
        if not isinstance(case, dict) or not isinstance(case.get("expected"), str) or not isinstance(case.get("actual"), str):
            print(json.dumps({
                "error": "case %d must be an object with string 'expected' and 'actual' fields" % i,
                "example": _EXAMPLE,
            }))
            return 0

    try:
        results = []
        exact_matches = 0
        f1_scores = []

        for case in cases:
            prompt = case.get("prompt")
            expected = case["expected"]
            actual = case["actual"]

            exact_match = expected.strip().lower() == actual.strip().lower()
            if exact_match:
                exact_matches += 1

            expected_tokens = _tokenize(expected)
            actual_tokens = _tokenize(actual)
            f1_score = _f1(expected_tokens, actual_tokens)
            f1_scores.append(f1_score)

            results.append({
                "prompt": prompt,
                "expected": expected,
                "actual": actual,
                "exact_match": exact_match,
                "f1_score": round(f1_score, 4),
            })

        exact_match_rate = round(exact_matches / len(cases) * 100, 2)
        average_f1 = round(sum(f1_scores) / len(f1_scores) * 100, 2)

        result = {
            "results": results,
            "exact_match_rate": exact_match_rate,
            "average_f1": average_f1,
            "case_count": len(cases),
        }
    except Exception as e:
        print(json.dumps({"error": "could not score cases: %s" % e}))
        return 0

    print(json.dumps(result, indent=2, default=str))
    return 0


if __name__ == "__main__":
    sys.exit(main())
