#!/usr/bin/env python3
"""describe_stats — Engram skill (no network). One-shot descriptive statistics.

Given a list of numbers, reports count/min/max/range, central tendency
(mean/median/mode), spread (sample variance/stdev, quartiles + IQR), the
Fisher-Pearson skewness, and a plain-English shape interpretation. Stdlib only.

Request (stdin): {"data": [2, 4, 4, 4, 5, 5, 7, 9]}
Output (stdout): {count, min, max, range, mean, median, mode, variance,
                  stdev, quartiles, iqr, skewness, interpretation}
"""
import json, sys, statistics


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e})); return 0

    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"data": [2, 4, 4, 4, 5, 5, 7, 9]},
        })); return 0

    data = q.get("data")
    if not isinstance(data, list) or not all(isinstance(x, (int, float)) and not isinstance(x, bool) for x in data):
        print(json.dumps({
            "error": "missing/invalid 'data': expected a list of numbers",
            "example": {"data": [2, 4, 4, 4, 5, 5, 7, 9]},
        })); return 0
    if len(data) < 2:
        print(json.dumps({
            "error": "need at least 2 data points",
            "example": {"data": [2, 4, 4, 4, 5, 5, 7, 9]},
        })); return 0

    try:
        nums = [float(x) for x in data]
        n = len(nums)
        mean = statistics.fmean(nums)
        median = statistics.median(nums)

        # Unique mode only; multimodal or all-distinct -> null.
        modes = statistics.multimode(nums)
        mode = round(modes[0], 6) if len(modes) == 1 else None

        var = statistics.variance(nums)      # sample, ddof=1
        stdev = statistics.stdev(nums)       # sample
        pstdev = statistics.pstdev(nums)     # population, for the skew moment

        q1 = q2 = q3 = iqr = None
        if n >= 2:
            qs = statistics.quantiles(nums, n=4, method="inclusive")
            q1, q2, q3 = qs[0], qs[1], qs[2]
            iqr = q3 - q1

        # Fisher-Pearson skewness with population stdev as the scale.
        if pstdev == 0:
            skew = None
            interp = "no spread (all values identical)"
        else:
            m3 = sum((x - mean) ** 3 for x in nums) / n
            skew = m3 / (pstdev ** 3)
            if skew > 0.5:
                interp = "right-skewed (long tail toward larger values)"
            elif skew < -0.5:
                interp = "left-skewed (long tail toward smaller values)"
            else:
                interp = "roughly symmetric"

        result = {
            "count": n,
            "min": round(min(nums), 6),
            "max": round(max(nums), 6),
            "range": round(max(nums) - min(nums), 6),
            "mean": round(mean, 6),
            "median": round(median, 6),
            "mode": mode,
            "variance": round(var, 6),
            "stdev": round(stdev, 6),
            "quartiles": {
                "q1": round(q1, 6) if q1 is not None else None,
                "q2": round(q2, 6) if q2 is not None else None,
                "q3": round(q3, 6) if q3 is not None else None,
            },
            "iqr": round(iqr, 6) if iqr is not None else None,
            "skewness": round(skew, 6) if skew is not None else None,
            "interpretation": interp,
        }
        print(json.dumps(result, indent=2, default=str)); return 0
    except Exception as e:
        print(json.dumps({"error": "describe_stats failed: %s" % e})); return 1


if __name__ == "__main__":
    sys.exit(main())
