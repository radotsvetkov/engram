#!/usr/bin/env python3
"""valuation_vc — Engram skill (no network).

Applies the classic "VC Method" of startup valuation: works backward from a
target exit value and the investors' required return multiple to derive
post-money and pre-money valuation and the investor's resulting ownership.

Request (stdin): {"target_exit_value": 100000000, "required_return_multiple": 10, "investment_amount": 2000000}
Output (stdout): {post_money_valuation, pre_money_valuation, investor_ownership_pct}
"""
import json
import sys


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0

    example = {"target_exit_value": 100000000, "required_return_multiple": 10, "investment_amount": 2000000}

    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object", "example": example}))
        return 0

    target_exit_value = q.get("target_exit_value")
    required_return_multiple = q.get("required_return_multiple")
    investment_amount = q.get("investment_amount")

    for name, val in (
        ("target_exit_value", target_exit_value),
        ("required_return_multiple", required_return_multiple),
        ("investment_amount", investment_amount),
    ):
        if not isinstance(val, (int, float)) or isinstance(val, bool) or val <= 0:
            print(json.dumps({
                "error": "missing or invalid required field '%s' (positive number)" % name,
                "example": example,
            }))
            return 0

    try:
        post_money_valuation = target_exit_value / required_return_multiple
        pre_money_valuation = post_money_valuation - investment_amount

        if pre_money_valuation < 0:
            print(json.dumps({
                "error": (
                    "deal is not fundable at these terms: the investment amount (%.2f) exceeds the "
                    "discounted post-money valuation (%.2f) implied by the target exit value and required "
                    "return multiple" % (investment_amount, post_money_valuation)
                ),
                "post_money_valuation": round(post_money_valuation, 2),
            }))
            return 0

        investor_ownership_pct = investment_amount / post_money_valuation * 100.0

        result = {
            "target_exit_value": target_exit_value,
            "required_return_multiple": required_return_multiple,
            "investment_amount": investment_amount,
            "post_money_valuation": round(post_money_valuation, 2),
            "pre_money_valuation": round(pre_money_valuation, 2),
            "investor_ownership_pct": round(investor_ownership_pct, 2),
        }
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "valuation_vc failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
