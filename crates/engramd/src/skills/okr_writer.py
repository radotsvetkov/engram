#!/usr/bin/env python3
"""okr_writer — Engram skill (no network).

Checks an Objective is qualitative/inspirational (flags numbers — those
belong in the key results) and scores each key result for measurability.
Also flags if you don't have 2-5 key results, the commonly recommended range.

Request (stdin): {"objective": str, "key_results": [str]}
Output (stdout): {objective, objective_warning, key_results: [{text,
  measurable, weak, warning}], key_result_count, count_warning, score_pct}
"""
import json
import re
import sys

TRIGGER_WORDS = {"increase", "decrease", "reduce", "grow", "reach", "achieve", "launch", "complete"}
VAGUE_WORDS = {"improve", "help", "support", "work on", "better"}
SUGGESTION = "quantify this — add a target number, %, or date"


def _has_number(text):
    return bool(re.search(r"\d", text)) or "%" in text


def _is_measurable(text):
    t = text.lower()
    if _has_number(t):
        return True
    for w in TRIGGER_WORDS:
        if re.search(r"\b%s\b\s+\S*\d" % re.escape(w), t):
            return True
    return False


def _is_vague(text):
    t = text.lower()
    if _has_number(t):
        return False
    return any(w in t for w in VAGUE_WORDS)


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0

    example = {"objective": "Delight our customers with a best-in-class onboarding experience",
               "key_results": ["Increase activation rate from 40% to 60%",
                                "Reduce time-to-first-value to under 5 minutes",
                                "Reach an onboarding NPS of 50"]}

    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object", "example": example}))
        return 0

    objective = q.get("objective")
    if not isinstance(objective, str) or not objective.strip():
        print(json.dumps({"error": "missing required field 'objective' (string)", "example": example}))
        return 0
    objective = objective.strip()

    key_results = q.get("key_results")
    if key_results is None or not isinstance(key_results, list):
        print(json.dumps({"error": "missing required field 'key_results' (list of strings)", "example": example}))
        return 0

    try:
        kr_texts = [str(x).strip() for x in key_results if str(x).strip()]

        objective_warning = None
        if _has_number(objective):
            objective_warning = "objectives should be inspirational and qualitative; numbers belong in key results"

        kr_out = []
        measurable_count = 0
        for text in kr_texts:
            measurable = _is_measurable(text)
            weak = _is_vague(text)
            warning = None if (measurable and not weak) else SUGGESTION
            if measurable:
                measurable_count += 1
            kr_out.append({"text": text, "measurable": measurable, "weak": weak, "warning": warning})

        count_warning = None
        n = len(kr_texts)
        if n < 2 or n > 5:
            count_warning = "the common guidance is 2-5 key results per objective; you have %d" % n

        score_pct = round(measurable_count / n * 100, 1) if n else 0.0

        result = {
            "objective": objective,
            "objective_warning": objective_warning,
            "key_results": kr_out,
            "key_result_count": n,
            "count_warning": count_warning,
            "score_pct": score_pct,
        }
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "okr_writer failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
