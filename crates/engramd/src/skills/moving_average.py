#!/usr/bin/env python3
"""moving_average — Engram skill (no network). Smooth a time series.

Smooths a numeric series with a simple moving average, a linearly weighted
moving average (weights 1..window), or an exponential moving average (EMA with
alpha, default 2/(window+1)). SMA/WMA leave the first window-1 points null; EMA
is full length seeded at the first value. Also reports a coarse trend label.

Request (stdin): {"data": [1,2,3,4,5], "window": 3, "method": "simple"}
Output (stdout): {method, window, smoothed, trend, alpha?}
"""
import json, sys


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e})); return 0

    ex = {"data": [1, 2, 3, 4, 5], "window": 3, "method": "simple"}
    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object", "example": ex})); return 0

    data = q.get("data")
    if not isinstance(data, list) or not all(isinstance(x, (int, float)) and not isinstance(x, bool) for x in data):
        print(json.dumps({"error": "missing/invalid 'data': expected a list of numbers", "example": ex})); return 0
    if len(data) == 0:
        print(json.dumps({"error": "'data' is empty", "example": ex})); return 0

    window = q.get("window", 3)
    if not isinstance(window, int) or isinstance(window, bool):
        print(json.dumps({"error": "'window' must be an integer", "example": ex})); return 0
    if window < 1:
        print(json.dumps({"error": "'window' must be >= 1", "example": ex})); return 0
    if window > len(data):
        print(json.dumps({"error": "'window' (%d) must be <= number of points (%d)" % (window, len(data)), "example": ex})); return 0

    method = q.get("method", "simple")
    if method not in ("simple", "weighted", "exponential"):
        print(json.dumps({"error": "'method' must be one of: simple, weighted, exponential", "example": ex})); return 0

    try:
        nums = [float(x) for x in data]
        n = len(nums)
        result = {"method": method, "window": window}

        if method == "simple":
            smoothed = [None] * (window - 1)
            for i in range(window - 1, n):
                win = nums[i - window + 1:i + 1]
                smoothed.append(round(sum(win) / window, 6))

        elif method == "weighted":
            weights = list(range(1, window + 1))
            wsum = sum(weights)
            smoothed = [None] * (window - 1)
            for i in range(window - 1, n):
                win = nums[i - window + 1:i + 1]
                val = sum(w * v for w, v in zip(weights, win)) / wsum
                smoothed.append(round(val, 6))

        else:  # exponential
            alpha = q.get("alpha")
            if alpha is None:
                alpha = 2.0 / (window + 1)
            if not isinstance(alpha, (int, float)) or isinstance(alpha, bool) or not (0 < alpha <= 1):
                print(json.dumps({"error": "'alpha' must be a number in (0, 1]", "example": {"data": [1, 2, 3], "method": "exponential", "alpha": 0.5}})); return 0
            alpha = float(alpha)
            smoothed = [round(nums[0], 6)]
            prev = nums[0]
            for i in range(1, n):
                prev = alpha * nums[i] + (1 - alpha) * prev
                smoothed.append(round(prev, 6))
            result["alpha"] = round(alpha, 6)

        # Trend: last smoothed vs first non-null smoothed.
        non_null = [v for v in smoothed if v is not None]
        if len(non_null) >= 2:
            delta = non_null[-1] - non_null[0]
            eps = 1e-9
            trend = "rising" if delta > eps else "falling" if delta < -eps else "flat"
        else:
            trend = "insufficient data"

        result["smoothed"] = smoothed
        result["trend"] = trend
        print(json.dumps(result, indent=2, default=str)); return 0
    except Exception as e:
        print(json.dumps({"error": "moving_average failed: %s" % e})); return 1


if __name__ == "__main__":
    sys.exit(main())
