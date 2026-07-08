#!/usr/bin/env python3
"""cohort_retention — Engram skill (no network). Cohort retention/churn curve analysis.

Takes a series of active-user counts for one cohort at month 0, 1, 2, ... and
computes retention %, churn %, and month-over-month change per period, plus a
heuristic signal for whether the retention curve is "flattening out" (recent
drop-off much smaller than early drop-off — a sign of a stable core-user
base).

Request (stdin): {"cohort_counts": [1000, 600, 420, 340, 300, 280, 270, 265]}
Output (stdout): {periods: [{month, count, retention_pct, churn_pct, pct_change}], retention_curve_flattening, note}
"""
import json
import sys


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0

    example = {"cohort_counts": [1000, 600, 420, 340, 300, 280, 270, 265]}

    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object", "example": example}))
        return 0

    counts = q.get("cohort_counts")
    if not isinstance(counts, list) or not counts:
        print(json.dumps({
            "error": "missing or invalid required field 'cohort_counts' (non-empty list of numbers)",
            "example": example,
        }))
        return 0
    for v in counts:
        if not isinstance(v, (int, float)) or isinstance(v, bool) or v < 0:
            print(json.dumps({
                "error": "'cohort_counts' must contain only non-negative numbers",
                "example": example,
            }))
            return 0
    if counts[0] == 0:
        print(json.dumps({
            "error": "'cohort_counts[0]' (the starting cohort size) must be greater than 0",
            "example": example,
        }))
        return 0

    try:
        base = counts[0]
        periods = []
        pct_changes = []  # index i corresponds to month i (i >= 1)
        for i, count in enumerate(counts):
            retention_pct = count / base * 100.0
            churn_pct = 100.0 - retention_pct
            pct_change = None
            if i > 0:
                prev = counts[i - 1]
                if prev == 0:
                    pct_change = None
                else:
                    pct_change = (count - prev) / prev * 100.0
                    pct_changes.append(pct_change)
            periods.append({
                "month": i,
                "count": count,
                "retention_pct": round(retention_pct, 2),
                "churn_pct": round(churn_pct, 2),
                "pct_change": round(pct_change, 2) if pct_change is not None else None,
            })

        flattening = None
        note = None
        if len(pct_changes) >= 4:
            early = pct_changes[:2]
            recent = pct_changes[-2:]
            early_avg_abs = sum(abs(x) for x in early) / len(early)
            recent_avg_abs = sum(abs(x) for x in recent) / len(recent)
            if early_avg_abs == 0:
                flattening = False
                note = "early-period change was already ~0, nothing to flatten from"
            else:
                ratio = recent_avg_abs / early_avg_abs
                flattening = ratio < 0.25
        else:
            note = "insufficient periods to assess flattening (need at least 5 months of data)"

        result = {
            "periods": periods,
            "retention_curve_flattening": flattening,
        }
        if note:
            result["note"] = note
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "cohort_retention failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
