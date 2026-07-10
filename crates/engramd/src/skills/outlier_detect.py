#!/usr/bin/env python3
"""outlier_detect — Engram skill (no network). Flag anomalous values.

Detects outliers with the IQR fence method (below q1-1.5*IQR or above
q3+1.5*IQR), the z-score method (|z|>3 using the sample stdev), or both.
Reports each outlier's value and original index plus the bounds/threshold used.

Request (stdin): {"data": [10, 12, 11, 13, 12, 99], "method": "both"}
Output (stdout): {"n", "iqr"?: {...}, "zscore"?: {...}}
"""
import json, sys, statistics


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e})); return 0

    ex = {"data": [10, 12, 11, 13, 12, 99], "method": "both"}
    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object", "example": ex})); return 0

    data = q.get("data")
    if not isinstance(data, list) or not all(isinstance(x, (int, float)) and not isinstance(x, bool) for x in data):
        print(json.dumps({"error": "missing/invalid 'data': expected a list of numbers", "example": ex})); return 0
    if len(data) < 4:
        print(json.dumps({"error": "need at least 4 data points", "example": ex})); return 0

    method = q.get("method", "both")
    if method not in ("iqr", "zscore", "both"):
        print(json.dumps({"error": "'method' must be one of: iqr, zscore, both", "example": ex})); return 0

    try:
        nums = [float(x) for x in data]
        result = {"n": len(nums)}

        if method in ("iqr", "both"):
            qs = statistics.quantiles(nums, n=4, method="inclusive")
            q1, q3 = qs[0], qs[2]
            iqr = q3 - q1
            lo = q1 - 1.5 * iqr
            hi = q3 + 1.5 * iqr
            outliers = [{"value": round(v, 6), "index": i}
                        for i, v in enumerate(nums) if v < lo or v > hi]
            result["iqr"] = {
                "q1": round(q1, 6), "q3": round(q3, 6), "iqr": round(iqr, 6),
                "lower_bound": round(lo, 6), "upper_bound": round(hi, 6),
                "outliers": outliers, "outlier_count": len(outliers),
            }

        if method in ("zscore", "both"):
            mean = statistics.fmean(nums)
            sd = statistics.stdev(nums) if len(nums) >= 2 else 0.0
            if sd == 0:
                result["zscore"] = {
                    "mean": round(mean, 6), "stdev": 0.0, "threshold": 3.0,
                    "outliers": [], "outlier_count": 0,
                    "note": "zero variance; no z-score outliers possible",
                }
            else:
                outliers = []
                for i, v in enumerate(nums):
                    z = (v - mean) / sd
                    if abs(z) > 3:
                        outliers.append({"value": round(v, 6), "index": i, "z": round(z, 6)})
                result["zscore"] = {
                    "mean": round(mean, 6), "stdev": round(sd, 6), "threshold": 3.0,
                    "outliers": outliers, "outlier_count": len(outliers),
                }

        print(json.dumps(result, indent=2, default=str)); return 0
    except Exception as e:
        print(json.dumps({"error": "outlier_detect failed: %s" % e})); return 1


if __name__ == "__main__":
    sys.exit(main())
