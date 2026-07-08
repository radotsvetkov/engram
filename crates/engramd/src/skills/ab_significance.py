#!/usr/bin/env python3
"""ab_significance — Engram skill (no network). Two-proportion z-test for A/B tests.

Given conversion counts/totals for a control and a variant, runs a standard
two-proportion z-test (pooled variance, normal approximation via math.erf) and
reports the z-score, two-tailed p-value, whether the result is significant at
the requested confidence level, and the relative uplift.

Request (stdin): {"control_conversions": 120, "control_total": 2000, "variant_conversions": 150, "variant_total": 2000, "confidence": 0.95}
Output (stdout): {control_rate_pct, variant_rate_pct, z_score, p_value, significant, relative_uplift_pct, verdict}
"""
import json
import math
import sys


def _norm_cdf(x):
    return 0.5 * (1 + math.erf(x / math.sqrt(2)))


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0

    example = {
        "control_conversions": 120, "control_total": 2000,
        "variant_conversions": 150, "variant_total": 2000,
        "confidence": 0.95,
    }

    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object", "example": example}))
        return 0

    fields = ("control_conversions", "control_total", "variant_conversions", "variant_total")
    values = {}
    for name in fields:
        val = q.get(name)
        if not isinstance(val, (int, float)) or isinstance(val, bool) or val < 0:
            print(json.dumps({
                "error": "missing or invalid required field '%s' (non-negative number)" % name,
                "example": example,
            }))
            return 0
        values[name] = val

    if values["control_total"] == 0 or values["variant_total"] == 0:
        print(json.dumps({
            "error": "'control_total' and 'variant_total' must both be greater than 0",
            "example": example,
        }))
        return 0
    if values["control_conversions"] > values["control_total"]:
        print(json.dumps({"error": "'control_conversions' cannot exceed 'control_total'", "example": example}))
        return 0
    if values["variant_conversions"] > values["variant_total"]:
        print(json.dumps({"error": "'variant_conversions' cannot exceed 'variant_total'", "example": example}))
        return 0

    confidence = q.get("confidence", 0.95)
    if not isinstance(confidence, (int, float)) or isinstance(confidence, bool) or not (0 < confidence < 1):
        print(json.dumps({
            "error": "'confidence' must be a number strictly between 0 and 1 (e.g. 0.95)",
            "example": example,
        }))
        return 0

    try:
        cc, ct = values["control_conversions"], values["control_total"]
        vc, vt = values["variant_conversions"], values["variant_total"]

        p1 = cc / ct
        p2 = vc / vt
        pooled_p = (cc + vc) / (ct + vt)
        variance = pooled_p * (1 - pooled_p) * (1.0 / ct + 1.0 / vt)
        se = math.sqrt(variance) if variance > 0 else 0.0

        if se == 0:
            z = 0.0
        else:
            z = (p2 - p1) / se

        p_value = 2 * (1 - _norm_cdf(abs(z)))
        alpha = 1 - confidence
        significant = p_value < alpha

        if p1 == 0:
            relative_uplift_pct = None
        else:
            relative_uplift_pct = (p2 - p1) / p1 * 100.0

        if significant and p2 > p1:
            verdict = (
                "Variant significantly outperforms control (p=%.4f < alpha=%.4f)." % (p_value, alpha)
            )
        elif significant and p2 < p1:
            verdict = (
                "Variant significantly underperforms control (p=%.4f < alpha=%.4f)." % (p_value, alpha)
            )
        else:
            verdict = (
                "No statistically significant difference detected at the %.1f%% confidence level (p=%.4f)."
                % (confidence * 100.0, p_value)
            )

        result = {
            "control_rate_pct": round(p1 * 100.0, 4),
            "variant_rate_pct": round(p2 * 100.0, 4),
            "z_score": round(z, 4),
            "p_value": round(p_value, 6),
            "significant": significant,
            "relative_uplift_pct": round(relative_uplift_pct, 2) if relative_uplift_pct is not None else None,
            "verdict": verdict,
        }
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "ab_significance failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
