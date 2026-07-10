#!/usr/bin/env python3
"""decision_matrix — Engram skill (no network).

Scores options against weighted criteria (a weighted decision / Pugh
matrix). Normalizes the weights to sum to 1.0, computes each option's
weighted total from per-criterion scores (1-10), then ranks options and
names a winner. Flags a close call and any missing scores.

Request (stdin): {"options": [str], "criteria": [{"name": str, "weight": number}],
  "scores": {option: {criterion: number}}}
Output (stdout): {ranking, winner, weights_normalized, warnings, note}
"""
import json
import sys


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0

    example = {
        "options": ["Postgres", "MongoDB"],
        "criteria": [{"name": "cost", "weight": 2}, {"name": "scalability", "weight": 3}],
        "scores": {"Postgres": {"cost": 8, "scalability": 7},
                   "MongoDB": {"cost": 6, "scalability": 9}},
    }

    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object", "example": example}))
        return 0

    options = q.get("options")
    if not isinstance(options, list) or not options or not all(isinstance(o, str) for o in options):
        print(json.dumps({"error": "missing required field 'options' (non-empty list of strings)",
                          "example": example}))
        return 0
    options = [o.strip() for o in options]

    criteria = q.get("criteria")
    if not isinstance(criteria, list) or not criteria:
        print(json.dumps({"error": "missing required field 'criteria' (non-empty list of "
                          "{name, weight})", "example": example}))
        return 0
    parsed_criteria = []
    for c in criteria:
        if not isinstance(c, dict) or "name" not in c or "weight" not in c:
            print(json.dumps({"error": "each criterion must be {\"name\": str, \"weight\": number}",
                              "example": example}))
            return 0
        try:
            w = float(c["weight"])
        except (TypeError, ValueError):
            print(json.dumps({"error": "criterion weight must be a number", "example": example}))
            return 0
        parsed_criteria.append({"name": str(c["name"]).strip(), "weight": w})

    scores = q.get("scores")
    if not isinstance(scores, dict):
        print(json.dumps({"error": "missing required field 'scores' "
                          "({option: {criterion: number}})", "example": example}))
        return 0

    try:
        total_weight = sum(c["weight"] for c in parsed_criteria)
        if total_weight <= 0:
            print(json.dumps({"error": "criteria weights must sum to a positive number",
                              "example": example}))
            return 0
        for c in parsed_criteria:
            c["weight_norm"] = round(c["weight"] / total_weight, 4)

        warnings = []
        ranking = []
        for opt in options:
            opt_scores = scores.get(opt, {})
            if not isinstance(opt_scores, dict):
                opt_scores = {}
                warnings.append("scores for option '%s' missing or malformed; treated as 0" % opt)
            contributions = []
            weighted_total = 0.0
            for c in parsed_criteria:
                raw = opt_scores.get(c["name"], None)
                if raw is None:
                    warnings.append("missing score for option '%s', criterion '%s' (treated as 0)"
                                    % (opt, c["name"]))
                    raw_val = 0.0
                else:
                    try:
                        raw_val = float(raw)
                    except (TypeError, ValueError):
                        warnings.append("non-numeric score for '%s'/'%s' (treated as 0)"
                                        % (opt, c["name"]))
                        raw_val = 0.0
                contrib = c["weight_norm"] * raw_val
                weighted_total += contrib
                contributions.append({
                    "criterion": c["name"],
                    "raw_score": raw_val,
                    "weight_norm": c["weight_norm"],
                    "contribution": round(contrib, 4),
                })
            ranking.append({
                "option": opt,
                "weighted_score": round(weighted_total, 4),
                "contributions": contributions,
            })

        ranking.sort(key=lambda r: r["weighted_score"], reverse=True)
        for i, r in enumerate(ranking, 1):
            r["rank"] = i

        winner = ranking[0]["option"]
        note = "Winner is '%s' by weighted score." % winner
        if len(ranking) >= 2:
            top, second = ranking[0]["weighted_score"], ranking[1]["weighted_score"]
            if top > 0 and (top - second) / top < 0.05:
                note = ("Close call — '%s' and '%s' are within 5%%. Consider a tiebreaker "
                        "(add a criterion, re-check scores, or weigh a must-have)."
                        % (ranking[0]["option"], ranking[1]["option"]))

        result = {
            "options": options,
            "weights_normalized": {c["name"]: c["weight_norm"] for c in parsed_criteria},
            "ranking": ranking,
            "winner": winner,
            "warnings": warnings,
            "note": note,
        }
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "decision_matrix failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
