#!/usr/bin/env python3
"""compound_interest — Engram skill (no network).

Computes future value of a principal with periodic compounding, plus optional
recurring contributions (ordinary annuity, deposited at the end of each
compounding period).

Request (stdin): {"principal": 10000, "rate": 5, "years": 10, "compounds_per_year": 12, "contribution": 100}
Output (stdout): {principal, rate, years, compounds_per_year, contribution, future_value, total_contributions, total_interest}
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
            "example": {"principal": 10000, "rate": 5, "years": 10, "compounds_per_year": 12, "contribution": 100},
        }))
        return 0

    principal = q.get("principal")
    rate = q.get("rate")
    years = q.get("years")
    compounds_per_year = q.get("compounds_per_year", 12)
    contribution = q.get("contribution", 0)

    for name, val in (("principal", principal), ("rate", rate), ("years", years)):
        if not isinstance(val, (int, float)) or isinstance(val, bool):
            print(json.dumps({
                "error": "missing or invalid required field '%s' (number)" % name,
                "example": {"principal": 10000, "rate": 5, "years": 10, "compounds_per_year": 12, "contribution": 100},
            }))
            return 0

    if not isinstance(compounds_per_year, (int, float)) or isinstance(compounds_per_year, bool) or compounds_per_year <= 0:
        print(json.dumps({
            "error": "'compounds_per_year' must be a positive number",
            "example": {"principal": 10000, "rate": 5, "years": 10, "compounds_per_year": 12, "contribution": 100},
        }))
        return 0

    if not isinstance(contribution, (int, float)) or isinstance(contribution, bool):
        print(json.dumps({
            "error": "'contribution' must be a number",
            "example": {"principal": 10000, "rate": 5, "years": 10, "compounds_per_year": 12, "contribution": 100},
        }))
        return 0

    try:
        n = compounds_per_year
        r = rate / 100.0 / n
        periods = years * n

        fv_principal = principal * (1 + r) ** periods
        if r == 0:
            fv_contributions = contribution * periods
        else:
            fv_contributions = contribution * (((1 + r) ** periods - 1) / r)

        future_value = fv_principal + fv_contributions
        total_contributions = contribution * periods
        total_interest = future_value - principal - total_contributions

        result = {
            "principal": principal,
            "rate": rate,
            "years": years,
            "compounds_per_year": n,
            "contribution": contribution,
            "future_value": round(future_value, 2),
            "total_contributions": round(total_contributions, 2),
            "total_interest": round(total_interest, 2),
        }
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "compound_interest failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
