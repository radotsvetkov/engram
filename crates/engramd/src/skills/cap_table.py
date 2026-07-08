#!/usr/bin/env python3
"""cap_table — Engram skill (no network).

Models a new financing round: computes price per share from a pre-money
valuation, the new shares issued to an incoming investor, post-money
valuation, and the resulting ownership/dilution for each existing
shareholder.

Request (stdin): {"existing_shareholders": [{"name": "Founder A", "shares": 6000000}, {"name": "Founder B", "shares": 4000000}], "new_investment": 2000000, "pre_money_valuation": 8000000}
Output (stdout): {price_per_share, new_shares_issued, post_money_valuation, cap_table_after}
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
        "existing_shareholders": [
            {"name": "Founder A", "shares": 6000000},
            {"name": "Founder B", "shares": 4000000},
        ],
        "new_investment": 2000000,
        "pre_money_valuation": 8000000,
    }

    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object", "example": example}))
        return 0

    existing = q.get("existing_shareholders")
    new_investment = q.get("new_investment")
    pre_money_valuation = q.get("pre_money_valuation")

    if not isinstance(existing, list) or len(existing) < 1 or not all(
        isinstance(h, dict) and isinstance(h.get("name"), str)
        and isinstance(h.get("shares"), (int, float)) and not isinstance(h.get("shares"), bool)
        and h.get("shares") > 0
        for h in existing
    ):
        print(json.dumps({
            "error": "missing or invalid 'existing_shareholders' (non-empty list of {name: string, shares: positive number})",
            "example": example,
        }))
        return 0

    if not isinstance(new_investment, (int, float)) or isinstance(new_investment, bool) or new_investment <= 0:
        print(json.dumps({
            "error": "missing or invalid required field 'new_investment' (positive number)",
            "example": example,
        }))
        return 0

    if not isinstance(pre_money_valuation, (int, float)) or isinstance(pre_money_valuation, bool) or pre_money_valuation <= 0:
        print(json.dumps({
            "error": "missing or invalid required field 'pre_money_valuation' (positive number)",
            "example": example,
        }))
        return 0

    try:
        existing_total_shares = sum(h["shares"] for h in existing)
        price_per_share = pre_money_valuation / existing_total_shares
        new_shares_issued = new_investment / price_per_share
        post_money_valuation = pre_money_valuation + new_investment
        total_shares_after = existing_total_shares + new_shares_issued

        cap_table_after = []
        for h in existing:
            old_pct = h["shares"] / existing_total_shares * 100.0
            new_pct = h["shares"] / total_shares_after * 100.0
            cap_table_after.append({
                "name": h["name"],
                "shares": h["shares"],
                "ownership_pct": round(new_pct, 2),
                "dilution_pct": round(old_pct - new_pct, 2),
            })

        new_investor_pct = new_shares_issued / total_shares_after * 100.0
        cap_table_after.append({
            "name": "New Investor",
            "shares": round(new_shares_issued, 2),
            "ownership_pct": round(new_investor_pct, 2),
        })

        result = {
            "existing_total_shares": existing_total_shares,
            "price_per_share": round(price_per_share, 4),
            "new_shares_issued": round(new_shares_issued, 2),
            "post_money_valuation": round(post_money_valuation, 2),
            "total_shares_after": round(total_shares_after, 2),
            "cap_table_after": cap_table_after,
        }
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "cap_table failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
