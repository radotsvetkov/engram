#!/usr/bin/env python3
"""mvp_scope_cutter — Engram skill (no network).

Makes a hard binary IN/OUT cut of a feature list against a fixed effort
budget — the classic MVP-scoping problem of maximizing total impact within a
capacity constraint (i.e. 0/1 knapsack). This is different from score-and-rank
tools like rice_score/ice_score: those just order features, this one decides
which ones actually fit.

Uses a greedy heuristic — sort by impact/effort_days ratio descending and fill
the budget in that order. This is a good approximation but NOT an exact
optimal knapsack solver (exact 0/1 knapsack is out of scope for a quick tool;
for a handful of features the greedy cut is generally close to optimal and is
much simpler to reason about).

Request (stdin): {"features": [{"name": str, "impact": 1-10, "effort_days": number}],
  "max_effort_days": number}
Output (stdout): {mvp: [{...feature, cumulative_effort_days}], later: [...],
  total_mvp_effort_days, total_mvp_impact, capacity_utilization_pct}
"""
import json
import sys


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0

    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"features": [
                {"name": "Login", "impact": 8, "effort_days": 3},
                {"name": "Dark mode", "impact": 3, "effort_days": 5},
            ], "max_effort_days": 10},
        }))
        return 0

    features = q.get("features")
    max_effort_days = q.get("max_effort_days")

    if not features or not isinstance(features, list):
        print(json.dumps({
            "error": "'features' (non-empty list) is required",
            "example": {"features": [
                {"name": "Login", "impact": 8, "effort_days": 3},
                {"name": "Dark mode", "impact": 3, "effort_days": 5},
            ], "max_effort_days": 10},
        }))
        return 0

    try:
        max_effort_days = float(max_effort_days)
    except (TypeError, ValueError):
        print(json.dumps({
            "error": "'max_effort_days' (positive number) is required",
            "example": {"features": [{"name": "Login", "impact": 8, "effort_days": 3}],
                        "max_effort_days": 10},
        }))
        return 0

    if max_effort_days <= 0:
        print(json.dumps({"error": "'max_effort_days' must be a positive number"}))
        return 0

    try:
        parsed = []
        for f in features:
            if not isinstance(f, dict) or not f.get("name"):
                raise ValueError("each feature needs a 'name'")
            impact = float(f.get("impact"))
            effort_days = float(f.get("effort_days"))
            if effort_days <= 0:
                raise ValueError("'effort_days' must be positive for feature '%s'" % f.get("name"))
            parsed.append({
                "name": str(f.get("name")),
                "impact": impact,
                "effort_days": effort_days,
                "_ratio": impact / effort_days,
            })
    except (TypeError, ValueError) as e:
        print(json.dumps({
            "error": "invalid feature entry: %s" % e,
            "example": {"features": [{"name": "Login", "impact": 8, "effort_days": 3}],
                        "max_effort_days": 10},
        }))
        return 0

    try:
        parsed.sort(key=lambda f: f["_ratio"], reverse=True)

        mvp = []
        later = []
        cumulative = 0.0
        for f in parsed:
            entry = {"name": f["name"], "impact": f["impact"], "effort_days": f["effort_days"]}
            if cumulative + f["effort_days"] <= max_effort_days:
                cumulative += f["effort_days"]
                entry["cumulative_effort_days"] = cumulative
                mvp.append(entry)
            else:
                later.append(entry)

        total_mvp_effort_days = cumulative
        total_mvp_impact = sum(f["impact"] for f in mvp)
        capacity_utilization_pct = round(total_mvp_effort_days / max_effort_days * 100, 1)

        result = {
            "mvp": mvp,
            "later": later,
            "total_mvp_effort_days": total_mvp_effort_days,
            "total_mvp_impact": total_mvp_impact,
            "capacity_utilization_pct": capacity_utilization_pct,
        }
    except Exception as e:
        print(json.dumps({"error": "could not compute MVP scope cut: %s" % e}))
        return 0

    print(json.dumps(result, indent=2, default=str))
    return 0


if __name__ == "__main__":
    sys.exit(main())
