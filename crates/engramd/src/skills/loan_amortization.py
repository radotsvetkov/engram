#!/usr/bin/env python3
"""loan_amortization — Engram skill (no network).

Computes the standard monthly payment for an amortizing loan and simulates
the payoff month by month, optionally applying an extra payment toward
principal each month. Loans over 3 years report a yearly summary (to keep
output small); loans of 3 years or less report the full monthly schedule.

Request (stdin): {"principal": 300000, "annual_rate": 6.5, "years": 30, "extra_payment": 200}
Output (stdout): {monthly_payment, months_to_payoff, total_paid, total_interest, schedule}
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
            "example": {"principal": 300000, "annual_rate": 6.5, "years": 30, "extra_payment": 200},
        }))
        return 0

    principal = q.get("principal")
    annual_rate = q.get("annual_rate")
    years = q.get("years")
    extra_payment = q.get("extra_payment", 0)

    for name, val in (("principal", principal), ("annual_rate", annual_rate), ("years", years)):
        if not isinstance(val, (int, float)) or isinstance(val, bool):
            print(json.dumps({
                "error": "missing or invalid required field '%s' (number)" % name,
                "example": {"principal": 300000, "annual_rate": 6.5, "years": 30, "extra_payment": 200},
            }))
            return 0

    if not isinstance(extra_payment, (int, float)) or isinstance(extra_payment, bool) or extra_payment < 0:
        print(json.dumps({
            "error": "'extra_payment' must be a non-negative number",
            "example": {"principal": 300000, "annual_rate": 6.5, "years": 30, "extra_payment": 200},
        }))
        return 0

    if principal <= 0 or years <= 0:
        print(json.dumps({
            "error": "'principal' and 'years' must be positive",
            "example": {"principal": 300000, "annual_rate": 6.5, "years": 30, "extra_payment": 200},
        }))
        return 0

    try:
        r = annual_rate / 100.0 / 12.0
        n = int(round(years * 12))
        if n <= 0:
            print(json.dumps({"error": "'years' is too small to produce any monthly periods"}))
            return 0

        if r == 0:
            payment = principal / n
        else:
            payment = principal * r / (1 - (1 + r) ** -n)

        balance = float(principal)
        month = 0
        total_paid = 0.0
        total_interest = 0.0
        schedule = []

        while balance > 1e-8 and month < n:
            month += 1
            interest_due = balance * r
            total_due = balance + interest_due
            attempted = payment + extra_payment

            if attempted >= total_due:
                payment_made = total_due
                interest_paid = interest_due
                principal_paid = balance
                balance = 0.0
            else:
                payment_made = attempted
                interest_paid = interest_due
                principal_paid = attempted - interest_due
                balance -= principal_paid

            total_paid += payment_made
            total_interest += interest_paid
            schedule.append({
                "month": month,
                "payment": round(payment_made, 2),
                "principal_paid": round(principal_paid, 2),
                "interest_paid": round(interest_paid, 2),
                "balance": round(max(balance, 0.0), 2),
            })

        if years <= 3:
            schedule_out = schedule
        else:
            schedule_out = []
            for i in range(0, len(schedule), 12):
                chunk = schedule[i:i + 12]
                schedule_out.append({
                    "year": i // 12 + 1,
                    "total_paid": round(sum(x["payment"] for x in chunk), 2),
                    "total_principal": round(sum(x["principal_paid"] for x in chunk), 2),
                    "total_interest": round(sum(x["interest_paid"] for x in chunk), 2),
                    "ending_balance": chunk[-1]["balance"],
                })

        result = {
            "monthly_payment": round(payment, 2),
            "months_to_payoff": month,
            "total_paid": round(total_paid, 2),
            "total_interest": round(total_interest, 2),
            "schedule": schedule_out,
        }
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "loan_amortization failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
