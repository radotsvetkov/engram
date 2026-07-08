#!/usr/bin/env python3
"""ppc_bid_calculator — Engram skill (no network). Compute a max CPC bid from a
target cost-per-acquisition and expected conversion rate, with optional daily
budget projections.

Request (stdin): {
    "target_cpa": 50,              # target cost per acquisition/conversion, currency units
    "conversion_rate_pct": 3,      # expected click-to-conversion rate as a percent (e.g. 3 for 3%)
    "daily_budget": 200,            # optional: daily ad spend budget, currency units
    "clicks_budget": 100             # optional: a target number of clicks to project spend/conversions for
}
Output (stdout): {
    "max_cpc": float,
    "target_cpa": float,
    "conversion_rate_pct": float,
    "estimated_daily_clicks": float,          # only if daily_budget given
    "estimated_daily_conversions": float,      # only if daily_budget given
    "estimated_spend_for_clicks_budget": float,        # only if clicks_budget given
    "estimated_conversions_for_clicks_budget": float    # only if clicks_budget given
}
"""
import json
import sys

_EXAMPLE = {"target_cpa": 50, "conversion_rate_pct": 3, "daily_budget": 200}


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0

    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object", "example": _EXAMPLE}))
        return 0

    target_cpa = q.get("target_cpa")
    conversion_rate_pct = q.get("conversion_rate_pct")

    if not isinstance(target_cpa, (int, float)) or isinstance(target_cpa, bool):
        print(json.dumps({
            "error": "provide numeric 'target_cpa' (target cost per acquisition)",
            "example": _EXAMPLE,
        }))
        return 0
    if not isinstance(conversion_rate_pct, (int, float)) or isinstance(conversion_rate_pct, bool):
        print(json.dumps({
            "error": "provide numeric 'conversion_rate_pct' (e.g. 3 for 3%)",
            "example": _EXAMPLE,
        }))
        return 0
    if conversion_rate_pct <= 0:
        print(json.dumps({
            "error": "'conversion_rate_pct' must be greater than 0",
            "example": _EXAMPLE,
        }))
        return 0
    if target_cpa < 0:
        print(json.dumps({"error": "'target_cpa' must be non-negative", "example": _EXAMPLE}))
        return 0

    daily_budget = q.get("daily_budget")
    if daily_budget is not None and (not isinstance(daily_budget, (int, float)) or isinstance(daily_budget, bool) or daily_budget < 0):
        print(json.dumps({"error": "'daily_budget' must be a non-negative number if provided", "example": _EXAMPLE}))
        return 0

    clicks_budget = q.get("clicks_budget")
    if clicks_budget is not None and (not isinstance(clicks_budget, (int, float)) or isinstance(clicks_budget, bool) or clicks_budget < 0):
        print(json.dumps({"error": "'clicks_budget' must be a non-negative number if provided", "example": _EXAMPLE}))
        return 0

    try:
        max_cpc = target_cpa * (conversion_rate_pct / 100.0)

        result = {
            "max_cpc": round(max_cpc, 4),
            "target_cpa": target_cpa,
            "conversion_rate_pct": conversion_rate_pct,
        }

        if daily_budget is not None:
            if max_cpc <= 0:
                result["estimated_daily_clicks"] = None
                result["estimated_daily_conversions"] = None
                result["warning"] = "max_cpc is 0, cannot estimate clicks/conversions from daily_budget"
            else:
                estimated_daily_clicks = daily_budget / max_cpc
                estimated_daily_conversions = estimated_daily_clicks * (conversion_rate_pct / 100.0)
                result["estimated_daily_clicks"] = round(estimated_daily_clicks, 2)
                result["estimated_daily_conversions"] = round(estimated_daily_conversions, 2)

        if clicks_budget is not None:
            result["estimated_spend_for_clicks_budget"] = round(clicks_budget * max_cpc, 4)
            result["estimated_conversions_for_clicks_budget"] = round(clicks_budget * (conversion_rate_pct / 100.0), 2)
    except Exception as e:
        print(json.dumps({"error": "could not compute bid: %s" % e}))
        return 0

    print(json.dumps(result, indent=2, default=str))
    return 0


if __name__ == "__main__":
    sys.exit(main())
