#!/usr/bin/env python3
"""sample_size_calc — Engram skill (no network). A/B test sample size per variant.

Standard two-proportion sample-size formula, using a lookup table of inverse-
normal z-scores for common power/significance combos (stdlib has no probit
function). Falls back to the nearest supported (power, significance) pair if
the exact one isn't tabulated.

Request (stdin): {"baseline_rate_pct": 5, "minimum_detectable_effect_pct": 1, "power": 80, "significance": 5}
Output (stdout): {sample_size_per_variant, total_sample_size, p1_pct, p2_pct, z_alpha, z_beta, power_used, significance_used, approximated_from?}
"""
import json
import math
import sys

# (power_pct, significance_pct) -> (z_alpha two-tailed, z_beta)
_Z_TABLE = {
    (80, 5): (1.96, 0.84),
    (90, 5): (1.96, 1.28),
    (95, 5): (1.96, 1.645),
    (80, 10): (1.645, 0.84),
    (90, 10): (1.645, 1.28),
}


def _nearest_key(power, significance):
    return min(
        _Z_TABLE.keys(),
        key=lambda k: abs(k[0] - power) + abs(k[1] - significance),
    )


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0

    example = {
        "baseline_rate_pct": 5, "minimum_detectable_effect_pct": 1,
        "power": 80, "significance": 5,
    }

    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object", "example": example}))
        return 0

    baseline_rate_pct = q.get("baseline_rate_pct")
    mde_pct = q.get("minimum_detectable_effect_pct")

    if not isinstance(baseline_rate_pct, (int, float)) or isinstance(baseline_rate_pct, bool) or not (0 < baseline_rate_pct < 100):
        print(json.dumps({
            "error": "missing or invalid required field 'baseline_rate_pct' (number strictly between 0 and 100)",
            "example": example,
        }))
        return 0
    if not isinstance(mde_pct, (int, float)) or isinstance(mde_pct, bool) or mde_pct == 0:
        print(json.dumps({
            "error": "missing or invalid required field 'minimum_detectable_effect_pct' (non-zero number)",
            "example": example,
        }))
        return 0

    power = q.get("power", 80)
    significance = q.get("significance", 5)
    if not isinstance(power, (int, float)) or isinstance(power, bool) or not (0 < power < 100):
        print(json.dumps({"error": "'power' must be a number between 0 and 100 (percent)", "example": example}))
        return 0
    if not isinstance(significance, (int, float)) or isinstance(significance, bool) or not (0 < significance < 100):
        print(json.dumps({"error": "'significance' must be a number between 0 and 100 (percent)", "example": example}))
        return 0

    try:
        key = (power, significance)
        approximated = key not in _Z_TABLE
        if approximated:
            key = _nearest_key(power, significance)
        z_alpha, z_beta = _Z_TABLE[key]
        power_used, significance_used = key

        p1 = baseline_rate_pct / 100.0
        p2 = p1 + mde_pct / 100.0
        p2 = min(max(p2, 0.0001), 0.9999)

        if p2 == p1:
            print(json.dumps({
                "error": "effective effect size is 0 after clamping — 'minimum_detectable_effect_pct' is too small or pushes past the valid [0.01%, 99.99%] range",
                "example": example,
            }))
            return 0

        p_bar = (p1 + p2) / 2.0
        numerator = (
            z_alpha * math.sqrt(2 * p_bar * (1 - p_bar))
            + z_beta * math.sqrt(p1 * (1 - p1) + p2 * (1 - p2))
        ) ** 2
        denominator = (p2 - p1) ** 2
        n = math.ceil(numerator / denominator)

        result = {
            "sample_size_per_variant": n,
            "total_sample_size": n * 2,
            "p1_pct": round(p1 * 100.0, 4),
            "p2_pct": round(p2 * 100.0, 4),
            "z_alpha": z_alpha,
            "z_beta": z_beta,
            "power_used": power_used,
            "significance_used": significance_used,
        }
        if approximated:
            result["approximated_from"] = [power_used, significance_used]
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "sample_size_calc failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
