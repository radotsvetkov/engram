#!/usr/bin/env python3
"""break_even — Engram skill (no network).

Computes the break-even point in units and revenue from fixed costs, price
per unit, and variable cost per unit.

Request (stdin): {"fixed_costs": 50000, "price_per_unit": 25, "variable_cost_per_unit": 10}
Output (stdout): {contribution_margin, contribution_margin_ratio, break_even_units, break_even_revenue}
"""
import json
import sys
import math


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0

    example = {"fixed_costs": 50000, "price_per_unit": 25, "variable_cost_per_unit": 10}

    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object", "example": example}))
        return 0

    fixed_costs = q.get("fixed_costs")
    price_per_unit = q.get("price_per_unit")
    variable_cost_per_unit = q.get("variable_cost_per_unit")

    for name, val in (
        ("fixed_costs", fixed_costs),
        ("price_per_unit", price_per_unit),
        ("variable_cost_per_unit", variable_cost_per_unit),
    ):
        if not isinstance(val, (int, float)) or isinstance(val, bool):
            print(json.dumps({
                "error": "missing or invalid required field '%s' (number)" % name,
                "example": example,
            }))
            return 0

    if price_per_unit <= variable_cost_per_unit:
        print(json.dumps({
            "error": "price must exceed variable cost per unit, or you can never break even",
            "example": example,
        }))
        return 0

    try:
        contribution_margin = price_per_unit - variable_cost_per_unit
        contribution_margin_ratio = contribution_margin / price_per_unit * 100.0
        break_even_units = math.ceil(fixed_costs / contribution_margin)
        break_even_revenue = break_even_units * price_per_unit

        result = {
            "fixed_costs": fixed_costs,
            "price_per_unit": price_per_unit,
            "variable_cost_per_unit": variable_cost_per_unit,
            "contribution_margin": round(contribution_margin, 2),
            "contribution_margin_ratio": round(contribution_margin_ratio, 2),
            "break_even_units": break_even_units,
            "break_even_revenue": round(break_even_revenue, 2),
        }
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "break_even failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
