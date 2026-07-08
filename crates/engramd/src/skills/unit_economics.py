#!/usr/bin/env python3
"""unit_economics — Engram skill (no network).

Computes customer lifetime value (LTV), the LTV:CAC ratio, and CAC payback
period from CAC, average revenue per user, gross margin, and churn rate.

Request (stdin): {"cac": 500, "arpu": 50, "gross_margin_pct": 80, "churn_rate_pct": 5}
Output (stdout): {cac, arpu, gross_margin_pct, churn_rate_pct, contribution_per_period, customer_lifetime_periods, ltv, ltv_cac_ratio, payback_periods, verdict}
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
            "example": {"cac": 500, "arpu": 50, "gross_margin_pct": 80, "churn_rate_pct": 5},
        }))
        return 0

    cac = q.get("cac")
    arpu = q.get("arpu")
    gross_margin_pct = q.get("gross_margin_pct")
    churn_rate_pct = q.get("churn_rate_pct")

    for name, val in (
        ("cac", cac), ("arpu", arpu),
        ("gross_margin_pct", gross_margin_pct), ("churn_rate_pct", churn_rate_pct),
    ):
        if not isinstance(val, (int, float)) or isinstance(val, bool):
            print(json.dumps({
                "error": "missing or invalid required field '%s' (number)" % name,
                "example": {"cac": 500, "arpu": 50, "gross_margin_pct": 80, "churn_rate_pct": 5},
            }))
            return 0

    if churn_rate_pct <= 0:
        print(json.dumps({
            "error": "churn rate must be > 0",
            "example": {"cac": 500, "arpu": 50, "gross_margin_pct": 80, "churn_rate_pct": 5},
        }))
        return 0

    try:
        contribution_per_period = arpu * gross_margin_pct / 100.0
        customer_lifetime_periods = 1.0 / (churn_rate_pct / 100.0)
        ltv = contribution_per_period * customer_lifetime_periods
        ltv_cac_ratio = ltv / cac if cac != 0 else None
        payback_periods = cac / contribution_per_period if contribution_per_period != 0 else None

        if ltv_cac_ratio is None:
            verdict = "cannot evaluate — CAC is 0"
        elif ltv_cac_ratio >= 3:
            verdict = "healthy (3:1 or better)"
        elif ltv_cac_ratio >= 1:
            verdict = "marginal"
        else:
            verdict = "unsustainable — CAC exceeds LTV"

        result = {
            "cac": cac,
            "arpu": arpu,
            "gross_margin_pct": gross_margin_pct,
            "churn_rate_pct": churn_rate_pct,
            "contribution_per_period": round(contribution_per_period, 2),
            "customer_lifetime_periods": round(customer_lifetime_periods, 2),
            "ltv": round(ltv, 2),
            "ltv_cac_ratio": round(ltv_cac_ratio, 2) if ltv_cac_ratio is not None else None,
            "payback_periods": round(payback_periods, 2) if payback_periods is not None else None,
            "verdict": verdict,
        }
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "unit_economics failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
