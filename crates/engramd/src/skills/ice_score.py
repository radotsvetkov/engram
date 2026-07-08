#!/usr/bin/env python3
"""ice_score — Engram skill (no network).

Ranks initiatives by the ICE score: (Impact + Confidence + Ease) / 3. All
three inputs are typically rated on the same 1-10 scale — the simpler
cousin of RICE (no reach/effort dimension).

Request (stdin): {"items": [{"name": str, "impact": number, "confidence":
  number, "ease": number}]}
Output (stdout): {"items": [{...input fields, score, rank}], "invalid_items": [...]}
"""
import json
import sys


def _num(v):
    if isinstance(v, bool):
        return None
    if isinstance(v, (int, float)):
        return float(v)
    return None


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0

    example = {"items": [
        {"name": "Add SSO login", "impact": 8, "confidence": 7, "ease": 5},
        {"name": "Redesign onboarding", "impact": 9, "confidence": 6, "ease": 3},
    ]}

    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object", "example": example}))
        return 0

    items = q.get("items")
    if not isinstance(items, list) or not items:
        print(json.dumps({"error": "missing required field 'items' (non-empty list)", "example": example}))
        return 0

    try:
        valid = []
        invalid = []
        for idx, it in enumerate(items):
            if not isinstance(it, dict):
                invalid.append({"index": idx, "item": it, "reason": "item must be an object"})
                continue
            name = it.get("name")
            impact = _num(it.get("impact"))
            confidence = _num(it.get("confidence"))
            ease = _num(it.get("ease"))

            reasons = []
            if not isinstance(name, str) or not name.strip():
                reasons.append("'name' must be a non-empty string")
            if impact is None:
                reasons.append("'impact' must be a number")
            if confidence is None:
                reasons.append("'confidence' must be a number")
            if ease is None:
                reasons.append("'ease' must be a number")
            if reasons:
                invalid.append({"index": idx, "item": it, "reason": "; ".join(reasons)})
                continue

            score = round((impact + confidence + ease) / 3.0, 2)
            valid.append({
                "name": name.strip(), "impact": impact, "confidence": confidence,
                "ease": ease, "score": score,
            })

        valid.sort(key=lambda x: x["score"], reverse=True)
        for rank, it in enumerate(valid, start=1):
            it["rank"] = rank

        result = {"items": valid, "count": len(valid)}
        if invalid:
            result["invalid_items"] = invalid
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "ice_score failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
