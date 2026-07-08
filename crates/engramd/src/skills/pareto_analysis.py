#!/usr/bin/env python3
"""pareto_analysis — Engram skill (no network).

Sorts items by value descending, computes running cumulative percentage,
and identifies the "vital few" — the smallest leading subset of items whose
cumulative share of the total first reaches 80% (the classic 80/20 framing).

Request (stdin): {"items": [{"name": str, "value": number}]}
Output (stdout): {"items": [{name, value, cumulative_value, cumulative_pct,
  rank}], "total_value": number, "vital_few": {"count", "pct_of_items",
  "items": [names]}}
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
        {"name": "Bug A", "value": 40}, {"name": "Bug B", "value": 25},
        {"name": "Bug C", "value": 15}, {"name": "Bug D", "value": 10}, {"name": "Bug E", "value": 10},
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
            value = _num(it.get("value"))
            reasons = []
            if not isinstance(name, str) or not name.strip():
                reasons.append("'name' must be a non-empty string")
            if value is None or value < 0:
                reasons.append("'value' must be a non-negative number")
            if reasons:
                invalid.append({"index": idx, "item": it, "reason": "; ".join(reasons)})
                continue
            valid.append({"name": name.strip(), "value": value})

        if not valid:
            print(json.dumps({"error": "no valid items to analyze", "invalid_items": invalid}))
            return 0

        total = sum(v["value"] for v in valid)
        if total <= 0:
            print(json.dumps({"error": "cannot compute Pareto analysis: total value is zero"}))
            return 0

        valid.sort(key=lambda x: x["value"], reverse=True)

        running = 0.0
        out_items = []
        vital_few_count = None
        for rank, v in enumerate(valid, start=1):
            running += v["value"]
            cumulative_pct = round(running / total * 100, 2)
            out_items.append({
                "rank": rank, "name": v["name"], "value": v["value"],
                "cumulative_value": round(running, 4), "cumulative_pct": cumulative_pct,
            })
            if vital_few_count is None and cumulative_pct >= 80:
                vital_few_count = rank

        if vital_few_count is None:
            vital_few_count = len(out_items)

        vital_few_items = [it["name"] for it in out_items[:vital_few_count]]
        result = {
            "items": out_items,
            "total_value": round(total, 4),
            "vital_few": {
                "count": vital_few_count,
                "pct_of_items": round(vital_few_count / len(out_items) * 100, 1),
                "items": vital_few_items,
            },
        }
        if invalid:
            result["invalid_items"] = invalid
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "pareto_analysis failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
