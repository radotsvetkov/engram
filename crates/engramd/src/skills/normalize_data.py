#!/usr/bin/env python3
"""normalize_data — Engram skill (no network). Rescale a numeric series.

Transforms a list of numbers by min-max scaling to [0,1], z-score
standardization (sample stdev), or robust scaling ((x-median)/IQR). Pass
method="all" to return all three. Reports the parameters used for each.

Request (stdin): {"data": [10, 20, 30, 40], "method": "minmax"}
Output (stdout): {method(s) -> {transformed, params, note?}}
"""
import json, sys, statistics


def _minmax(nums):
    lo, hi = min(nums), max(nums)
    if hi == lo:
        return {"transformed": [0.5 for _ in nums],
                "params": {"min": round(lo, 6), "max": round(hi, 6)},
                "note": "max == min; all values mapped to 0.5"}
    rng = hi - lo
    return {"transformed": [round((x - lo) / rng, 6) for x in nums],
            "params": {"min": round(lo, 6), "max": round(hi, 6)}}


def _zscore(nums):
    mean = statistics.fmean(nums)
    sd = statistics.stdev(nums) if len(nums) >= 2 else 0.0
    if sd == 0:
        return {"transformed": [0.0 for _ in nums],
                "params": {"mean": round(mean, 6), "stdev": 0.0},
                "note": "zero stdev; all values mapped to 0.0"}
    return {"transformed": [round((x - mean) / sd, 6) for x in nums],
            "params": {"mean": round(mean, 6), "stdev": round(sd, 6)}}


def _robust(nums):
    median = statistics.median(nums)
    if len(nums) >= 2:
        qs = statistics.quantiles(nums, n=4, method="inclusive")
        iqr = qs[2] - qs[0]
    else:
        iqr = 0.0
    if iqr == 0:
        return {"transformed": [0.0 for _ in nums],
                "params": {"median": round(median, 6), "iqr": 0.0},
                "note": "zero IQR; all values mapped to 0.0"}
    return {"transformed": [round((x - median) / iqr, 6) for x in nums],
            "params": {"median": round(median, 6), "iqr": round(iqr, 6)}}


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e})); return 0

    ex = {"data": [10, 20, 30, 40], "method": "minmax"}
    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object", "example": ex})); return 0

    data = q.get("data")
    if not isinstance(data, list) or not all(isinstance(x, (int, float)) and not isinstance(x, bool) for x in data):
        print(json.dumps({"error": "missing/invalid 'data': expected a list of numbers", "example": ex})); return 0
    if len(data) < 2:
        print(json.dumps({"error": "need at least 2 data points", "example": ex})); return 0

    method = q.get("method", "minmax")
    if method not in ("minmax", "zscore", "robust", "all"):
        print(json.dumps({"error": "'method' must be one of: minmax, zscore, robust, all", "example": ex})); return 0

    try:
        nums = [float(x) for x in data]
        fns = {"minmax": _minmax, "zscore": _zscore, "robust": _robust}
        if method == "all":
            result = {"n": len(nums)}
            for name, fn in fns.items():
                result[name] = fn(nums)
        else:
            result = {"n": len(nums), "method": method}
            result.update(fns[method](nums))
        print(json.dumps(result, indent=2, default=str)); return 0
    except Exception as e:
        print(json.dumps({"error": "normalize_data failed: %s" % e})); return 1


if __name__ == "__main__":
    sys.exit(main())
