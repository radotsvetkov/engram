#!/usr/bin/env python3
"""npv_irr — Engram skill (no network).

Computes Net Present Value at a given discount rate, the Internal Rate of
Return (via bisection over -99% to 1000%, no numpy/scipy needed), and the
simple undiscounted payback period for a series of periodic cashflows.

Request (stdin): {"rate": 10, "cashflows": [-10000, 3000, 4000, 5000, 3000]}
Output (stdout): {rate, cashflows, npv, irr_pct, payback_period}
"""
import json
import sys


def _npv(rate_decimal, cashflows):
    return sum(cf / (1 + rate_decimal) ** i for i, cf in enumerate(cashflows))


def _find_irr(cashflows):
    lo, hi = -0.99, 10.0
    steps = 500
    prev_r = lo
    prev_val = _npv(prev_r, cashflows)
    interval = None
    if prev_val == 0:
        return prev_r
    for i in range(1, steps + 1):
        r = lo + (hi - lo) * i / steps
        val = _npv(r, cashflows)
        if val == 0:
            return r
        if (prev_val < 0) != (val < 0):
            interval = (prev_r, r)
            break
        prev_r, prev_val = r, val
    if interval is None:
        return None
    a, b = interval
    fa = _npv(a, cashflows)
    for _ in range(100):
        m = (a + b) / 2.0
        fm = _npv(m, cashflows)
        if fm == 0:
            return m
        if (fa < 0) != (fm < 0):
            b = m
        else:
            a, fa = m, fm
    return (a + b) / 2.0


def _payback_period(cashflows):
    running = 0.0
    cum = []
    for cf in cashflows:
        running += cf
        cum.append(running)
    for i, c in enumerate(cum):
        if c >= 0:
            if i == 0:
                return 0.0
            prev = cum[i - 1]
            cf_i = cashflows[i]
            frac = (-prev / cf_i) if cf_i != 0 else 0.0
            return (i - 1) + frac
    return None


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0

    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"rate": 10, "cashflows": [-10000, 3000, 4000, 5000, 3000]},
        }))
        return 0

    rate = q.get("rate")
    cashflows = q.get("cashflows")

    if not isinstance(rate, (int, float)) or isinstance(rate, bool):
        print(json.dumps({
            "error": "missing or invalid required field 'rate' (annual discount rate as a percent, e.g. 10)",
            "example": {"rate": 10, "cashflows": [-10000, 3000, 4000, 5000, 3000]},
        }))
        return 0

    if not isinstance(cashflows, list) or len(cashflows) < 1 or not all(
        isinstance(x, (int, float)) and not isinstance(x, bool) for x in cashflows
    ):
        print(json.dumps({
            "error": "missing or invalid required field 'cashflows' (non-empty list of numbers, one per period)",
            "example": {"rate": 10, "cashflows": [-10000, 3000, 4000, 5000, 3000]},
        }))
        return 0

    try:
        npv_value = _npv(rate / 100.0, cashflows)
        irr = _find_irr(cashflows)
        payback = _payback_period(cashflows)

        result = {
            "rate": rate,
            "cashflows": cashflows,
            "npv": round(npv_value, 2),
            "irr_pct": round(irr * 100, 4) if irr is not None else None,
            "payback_period": round(payback, 4) if payback is not None else None,
        }
        if irr is None:
            result["irr_note"] = "no real IRR root found between -99% and 1000% for these cashflows"
        if payback is None:
            result["payback_note"] = "cumulative cashflows never turn non-negative; no payback within the given periods"

        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "npv_irr failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
