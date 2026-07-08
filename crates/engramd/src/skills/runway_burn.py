#!/usr/bin/env python3
"""runway_burn — Engram skill (no network).

Computes cash runway (months until cash runs out) from a cash balance and
either a direct monthly burn figure, or monthly revenue and expenses. Uses
the real current date to project an estimated runway end date.

Request (stdin): {"cash_balance": 500000, "monthly_burn": 40000}
              or: {"cash_balance": 500000, "monthly_revenue": 20000, "monthly_expenses": 60000}
Output (stdout): {cash_balance, net_burn, runway_months, runway_end_date}
"""
import json
import sys
import datetime


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0

    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"cash_balance": 500000, "monthly_burn": 40000},
        }))
        return 0

    cash_balance = q.get("cash_balance")
    if not isinstance(cash_balance, (int, float)) or isinstance(cash_balance, bool):
        print(json.dumps({
            "error": "missing or invalid required field 'cash_balance' (number)",
            "example": {"cash_balance": 500000, "monthly_burn": 40000},
        }))
        return 0

    monthly_burn = q.get("monthly_burn")
    monthly_revenue = q.get("monthly_revenue")
    monthly_expenses = q.get("monthly_expenses")

    if isinstance(monthly_burn, (int, float)) and not isinstance(monthly_burn, bool):
        net_burn = monthly_burn
    elif (
        isinstance(monthly_revenue, (int, float)) and not isinstance(monthly_revenue, bool)
        and isinstance(monthly_expenses, (int, float)) and not isinstance(monthly_expenses, bool)
    ):
        net_burn = monthly_expenses - monthly_revenue
    else:
        print(json.dumps({
            "error": "provide either 'monthly_burn', or both 'monthly_revenue' and 'monthly_expenses'",
            "example_1": {"cash_balance": 500000, "monthly_burn": 40000},
            "example_2": {"cash_balance": 500000, "monthly_revenue": 20000, "monthly_expenses": 60000},
        }))
        return 0

    try:
        result = {
            "cash_balance": cash_balance,
            "net_burn": round(net_burn, 2),
        }
        if net_burn <= 0:
            result["runway_months"] = None
            result["note"] = "profitable / break-even — no burn"
        else:
            runway_months = cash_balance / net_burn
            end_date = datetime.date.today() + datetime.timedelta(days=runway_months * 30.44)
            result["runway_months"] = round(runway_months, 1)
            result["runway_end_date"] = end_date.isoformat()

        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "runway_burn failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
