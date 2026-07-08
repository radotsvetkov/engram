#!/usr/bin/env python3
"""rice_score — Engram skill (no network).

Ranks initiatives by the RICE score: (Reach * Impact * Confidence) / Effort.
Impact accepts either a raw number or one of the RICE convention labels
(massive=3, high=2, medium=1, low=0.5, minimal=0.25). Confidence is a percent
(e.g. 80 for 80%). Effort must be a positive number of person-months.

Request (stdin): {"items": [{"name": str, "reach": number, "impact":
  number|"massive"|"high"|"medium"|"low"|"minimal", "confidence": number,
  "effort": number}]}
Output (stdout): {"items": [{...input fields, score, rank}], "invalid_items": [...]}
"""
import json
import sys

IMPACT_LABELS = {"massive": 3.0, "high": 2.0, "medium": 1.0, "low": 0.5, "minimal": 0.25}


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
        {"name": "Add SSO login", "reach": 500, "impact": "high", "confidence": 80, "effort": 2},
        {"name": "Redesign onboarding", "reach": 2000, "impact": 3, "confidence": 50, "effort": 4},
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
            reach = _num(it.get("reach"))
            raw_impact = it.get("impact")
            if isinstance(raw_impact, str):
                impact = IMPACT_LABELS.get(raw_impact.strip().lower())
                if impact is None:
                    invalid.append({"index": idx, "item": it,
                                     "reason": "impact label must be one of %s" % sorted(IMPACT_LABELS)})
                    continue
            else:
                impact = _num(raw_impact)
            confidence = _num(it.get("confidence"))
            effort = _num(it.get("effort"))

            reasons = []
            if not isinstance(name, str) or not name.strip():
                reasons.append("'name' must be a non-empty string")
            if reach is None:
                reasons.append("'reach' must be a number")
            if impact is None:
                reasons.append("'impact' must be a number or one of %s" % sorted(IMPACT_LABELS))
            if confidence is None:
                reasons.append("'confidence' must be a number (percent, e.g. 80)")
            if effort is None or effort <= 0:
                reasons.append("'effort' must be a positive number")
            if reasons:
                invalid.append({"index": idx, "item": it, "reason": "; ".join(reasons)})
                continue

            score = round(reach * impact * (confidence / 100.0) / effort, 2)
            valid.append({
                "name": name.strip(), "reach": reach, "impact": impact,
                "confidence": confidence, "effort": effort, "score": score,
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
        print(json.dumps({"error": "rice_score failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
