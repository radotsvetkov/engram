#!/usr/bin/env python3
"""growth_projection — Engram skill (no network). Project a value over periods.

Projects an initial value forward across N periods using an exponential,
linear, or logistic (S-curve) growth model. Reports the full series, the final
value, total growth %, and — for logistic — the inflection period nearest 50%
of the carrying capacity. Stdlib only (math).

exponential: value(t) = initial * (1 + rate) ** t
linear:      value(t) = initial + rate * t
logistic:    value(t) = K / (1 + ((K - initial) / initial) * exp(-rate * t))

Request (stdin): {"initial": 100, "periods": 12, "model": "logistic",
                  "rate": 0.5, "carrying_capacity": 10000}
Output (stdout): {model, initial, periods, final_value, total_growth_pct,
                  series: [{period, value}], inflection_period?}
"""
import json
import math
import sys


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e}))
        return 0
    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object"}))
        return 0

    example = {"initial": 100, "periods": 12, "model": "exponential", "rate": 0.1}
    for field in ("initial", "periods", "model"):
        if field not in q:
            print(json.dumps({"error": "missing required field: %s" % field, "example": example}))
            return 0

    try:
        initial = float(q["initial"])
        periods = int(q["periods"])
    except (TypeError, ValueError):
        print(json.dumps({"error": "'initial' must be a number and 'periods' an integer", "example": example}))
        return 0
    if periods < 1:
        print(json.dumps({"error": "'periods' must be >= 1"}))
        return 0
    if periods > 100000:
        print(json.dumps({"error": "'periods' too large (max 100000)"}))
        return 0

    model = str(q["model"]).strip().lower()
    rate = q.get("rate")
    K = q.get("carrying_capacity")

    try:
        if model in ("exponential", "linear"):
            if rate is None:
                raise ValueError("model '%s' requires 'rate'" % model)
            rate = float(rate)
        elif model == "logistic":
            if rate is None:
                raise ValueError("logistic model requires a growth 'rate'")
            if K is None:
                raise ValueError("logistic model requires 'carrying_capacity'")
            rate = float(rate)
            K = float(K)
            if initial <= 0:
                raise ValueError("logistic model requires 'initial' > 0")
            if K <= initial:
                raise ValueError("logistic 'carrying_capacity' must be greater than 'initial'")
        else:
            raise ValueError("unknown model %r (use exponential|linear|logistic)" % model)
    except (TypeError, ValueError) as e:
        print(json.dumps({"error": str(e), "example": example}))
        return 0

    series = []
    inflection_period = None
    prev_gap = None  # for logistic, track distance to 50% of K
    try:
        for t in range(periods + 1):  # include t=0 (the initial point)
            if model == "exponential":
                v = initial * ((1 + rate) ** t)
            elif model == "linear":
                v = initial + rate * t
            else:  # logistic
                v = K / (1 + ((K - initial) / initial) * math.exp(-rate * t))
                gap = abs(v - K / 2.0)
                if prev_gap is None or gap < prev_gap:
                    prev_gap = gap
                    inflection_period = t
            series.append({"period": t, "value": round(v, 4)})
    except (OverflowError, ValueError) as e:
        print(json.dumps({"error": "projection overflowed; reduce rate/periods (%s)" % e}))
        return 0

    final_value = series[-1]["value"]
    if initial != 0:
        total_growth_pct = round((final_value - initial) / abs(initial) * 100.0, 4)
    else:
        total_growth_pct = None

    result = {
        "model": model,
        "initial": round(initial, 4),
        "periods": periods,
        "rate": rate,
        "final_value": final_value,
        "total_growth_pct": total_growth_pct,
        "series": series,
    }
    if model == "logistic":
        result["carrying_capacity"] = round(K, 4)
        result["inflection_period"] = inflection_period
    print(json.dumps(result, indent=2, default=str))
    return 0


if __name__ == "__main__":
    sys.exit(main())
