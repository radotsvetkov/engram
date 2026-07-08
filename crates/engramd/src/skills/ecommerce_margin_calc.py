#!/usr/bin/env python3
"""ecommerce_margin_calc — Engram skill (no network). Net margin and break-even for an e-commerce SKU.

Given a selling price, cost of goods sold, and optional fee/shipping/fixed
costs, computes total fees, net profit, net margin percent, and the
break-even selling price at the same cost structure. Stdlib only, no
external pricing data — pass in your own marketplace/processor fee rates.

Request (stdin): {"selling_price": 49.99, "cogs": 18.5, "marketplace_fee_pct": 15, "payment_processing_fee_pct": 2.9, "shipping_cost": 4.5, "other_fixed_costs": 0.5}
Output (stdout): {selling_price, cogs, total_fees, net_profit, net_margin_pct, break_even_price, verdict}
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
        "selling_price": 49.99,
        "cogs": 18.5,
        "marketplace_fee_pct": 15,
        "payment_processing_fee_pct": 2.9,
        "shipping_cost": 4.5,
        "other_fixed_costs": 0.5,
    }

    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object", "example": example}))
        return 0

    missing = [f for f in ("selling_price", "cogs") if q.get(f) is None]
    if missing:
        print(json.dumps({
            "error": "missing required field(s): %s" % ", ".join(missing),
            "example": example,
        }))
        return 0

    def _num(name, default=0):
        v = q.get(name, default)
        if v is None:
            v = default
        if isinstance(v, bool):
            raise ValueError("'%s' must be a number" % name)
        return float(v)

    try:
        selling_price = _num("selling_price")
        cogs = _num("cogs")
        marketplace_fee_pct = _num("marketplace_fee_pct", 0)
        payment_processing_fee_pct = _num("payment_processing_fee_pct", 0)
        shipping_cost = _num("shipping_cost", 0)
        other_fixed_costs = _num("other_fixed_costs", 0)
    except (TypeError, ValueError) as e:
        print(json.dumps({"error": "all numeric fields must be numbers: %s" % e, "example": example}))
        return 0

    if selling_price <= 0:
        print(json.dumps({"error": "'selling_price' must be greater than 0", "example": example}))
        return 0

    fee_pct_sum = marketplace_fee_pct + payment_processing_fee_pct
    if fee_pct_sum >= 100:
        print(json.dumps({
            "error": "combined 'marketplace_fee_pct' + 'payment_processing_fee_pct' (%.2f%%) must be less than 100%%" % fee_pct_sum,
            "example": example,
        }))
        return 0

    try:
        total_fees = selling_price * fee_pct_sum / 100 + shipping_cost + other_fixed_costs
        net_profit = selling_price - cogs - total_fees
        net_margin_pct = net_profit / selling_price * 100
        break_even_price = (cogs + shipping_cost + other_fixed_costs) / (1 - fee_pct_sum / 100)

        if net_margin_pct >= 30:
            verdict = "healthy"
        elif net_margin_pct >= 10:
            verdict = "thin but workable"
        else:
            verdict = "unsustainable at this price/cost structure"

        result = {
            "selling_price": selling_price,
            "cogs": cogs,
            "marketplace_fee_pct": marketplace_fee_pct,
            "payment_processing_fee_pct": payment_processing_fee_pct,
            "shipping_cost": shipping_cost,
            "other_fixed_costs": other_fixed_costs,
            "total_fees": round(total_fees, 4),
            "net_profit": round(net_profit, 4),
            "net_margin_pct": round(net_margin_pct, 4),
            "break_even_price": round(break_even_price, 4),
            "verdict": verdict,
        }
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "ecommerce_margin_calc failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
