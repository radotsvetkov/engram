#!/usr/bin/env python3
"""growth_funnel — Engram skill (no network). AARRR funnel conversion analysis.

Takes raw user COUNTS at each AARRR stage (Acquisition, Activation, Retention,
Referral, Revenue — each stage should be <= the previous one), computes
stage-to-stage conversion rates and overall conversion, and flags the
highest-leverage stage to fix (the transition with the lowest conversion
rate).

Request (stdin): {"acquisition": 10000, "activation": 4000, "retention": 2000, "referral": 300, "revenue": 150}
Output (stdout): {..., activation_rate_pct, retention_rate_pct, referral_rate_pct, revenue_rate_pct, overall_conversion_pct, biggest_dropoff_stage, recommendation, warnings}
"""
import json
import sys

_STAGES = ("acquisition", "activation", "retention", "referral", "revenue")

_TRANSITIONS = [
    ("acquisition_to_activation", "acquisition", "activation", "activation_rate_pct"),
    ("activation_to_retention", "activation", "retention", "retention_rate_pct"),
    ("retention_to_referral", "retention", "referral", "referral_rate_pct"),
    ("referral_to_revenue", "referral", "revenue", "revenue_rate_pct"),
]


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0

    example = {"acquisition": 10000, "activation": 4000, "retention": 2000, "referral": 300, "revenue": 150}

    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object", "example": example}))
        return 0

    values = {}
    for name in _STAGES:
        val = q.get(name)
        if not isinstance(val, (int, float)) or isinstance(val, bool) or val < 0:
            print(json.dumps({
                "error": "missing or invalid required field '%s' (non-negative number, a raw count)" % name,
                "example": example,
            }))
            return 0
        values[name] = val

    try:
        warnings = []
        for prev, cur in zip(_STAGES, _STAGES[1:]):
            if values[cur] > values[prev]:
                warnings.append(
                    "'%s' (%s) is greater than '%s' (%s) — funnel stages are usually non-increasing"
                    % (cur, values[cur], prev, values[prev])
                )

        result = dict(values)
        rates = {}
        for key, from_stage, to_stage, rate_field in _TRANSITIONS:
            denom = values[from_stage]
            if denom == 0:
                result[rate_field] = None
                warnings.append("cannot compute '%s': '%s' is 0" % (rate_field, from_stage))
            else:
                rate = values[to_stage] / denom * 100.0
                result[rate_field] = round(rate, 2)
                rates[key] = result[rate_field]

        if values["acquisition"] == 0:
            result["overall_conversion_pct"] = None
            warnings.append("cannot compute 'overall_conversion_pct': 'acquisition' is 0")
        else:
            result["overall_conversion_pct"] = round(values["revenue"] / values["acquisition"] * 100.0, 2)

        if rates:
            biggest_key = min(rates, key=rates.get)
            result["biggest_dropoff_stage"] = biggest_key
            label = biggest_key.replace("_to_", " → ")
            result["recommendation"] = (
                "The biggest drop-off is %s (%.2f%% conversion) — this is the highest-leverage "
                "stage to optimize first." % (label, rates[biggest_key])
            )
        else:
            result["biggest_dropoff_stage"] = None
            result["recommendation"] = "Not enough data to identify the biggest drop-off stage."

        result["warnings"] = warnings
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "growth_funnel failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
