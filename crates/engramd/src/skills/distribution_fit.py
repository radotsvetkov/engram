#!/usr/bin/env python3
"""distribution_fit — Engram skill (no network). Heuristic distribution guess.

Builds a histogram and computes mean, stdev, skewness, and excess kurtosis,
then heuristically guesses whether the data looks normal, uniform, or
exponential. This is a rough shape check, NOT a formal goodness-of-fit test.

Request (stdin): {"data": [ ...numbers... ], "bins": 10}
Output (stdout): {n, histogram, moments, likely_distribution, confidence}
"""
import json, sys, statistics


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e})); return 0

    ex = {"data": [1, 2, 2, 3, 3, 3, 4, 4, 5], "bins": 10}
    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object", "example": ex})); return 0

    data = q.get("data")
    if not isinstance(data, list) or not all(isinstance(x, (int, float)) and not isinstance(x, bool) for x in data):
        print(json.dumps({"error": "missing/invalid 'data': expected a list of numbers", "example": ex})); return 0
    if len(data) < 8:
        print(json.dumps({"error": "need at least 8 data points", "example": ex})); return 0

    bins = q.get("bins", 10)
    if not isinstance(bins, int) or isinstance(bins, bool) or bins < 1:
        print(json.dumps({"error": "'bins' must be a positive integer", "example": ex})); return 0

    try:
        nums = [float(x) for x in data]
        n = len(nums)
        lo, hi = min(nums), max(nums)
        mean = statistics.fmean(nums)
        stdev = statistics.stdev(nums)      # sample
        pstdev = statistics.pstdev(nums)    # population, for moments

        # Histogram.
        counts = [0] * bins
        if hi == lo:
            edges = [lo, hi]
            counts = [n]
            bins = 1
        else:
            width = (hi - lo) / bins
            edges = [round(lo + i * width, 6) for i in range(bins + 1)]
            for x in nums:
                idx = int((x - lo) / width)
                if idx >= bins:      # the maximum falls in the last bin
                    idx = bins - 1
                counts[idx] += 1

        # Moments (population scale).
        if pstdev == 0:
            skew = 0.0
            excess_kurt = 0.0
        else:
            m3 = sum((x - mean) ** 3 for x in nums) / n
            m4 = sum((x - mean) ** 4 for x in nums) / n
            skew = m3 / (pstdev ** 3)
            excess_kurt = m4 / (pstdev ** 4) - 3.0

        # Heuristic classification.
        nonzero = [c for c in counts if c > 0]
        flat_ratio = (max(counts) / min(nonzero)) if nonzero else float("inf")
        if abs(skew) < 0.5 and abs(excess_kurt) < 1:
            likely = "normal"
        elif flat_ratio < 1.5 and abs(skew) < 0.3:
            likely = "uniform"
        elif skew > 1 and lo >= 0:
            likely = "exponential"
        else:
            likely = "none of normal/uniform/exponential clearly fits"

        result = {
            "n": n,
            "histogram": {"bin_edges": edges, "counts": counts},
            "moments": {
                "mean": round(mean, 6),
                "stdev": round(stdev, 6),
                "skewness": round(skew, 6),
                "excess_kurtosis": round(excess_kurt, 6),
            },
            "likely_distribution": likely,
            "confidence": "heuristic shape check based on skew/kurtosis and histogram flatness; NOT a formal goodness-of-fit test",
        }
        print(json.dumps(result, indent=2, default=str)); return 0
    except Exception as e:
        print(json.dumps({"error": "distribution_fit failed: %s" % e})); return 1


if __name__ == "__main__":
    sys.exit(main())
