#!/usr/bin/env python3
"""linear_forecast — Engram skill (no network). Ordinary least-squares linear
regression and a simple forward forecast, implemented from the standard
formulas (stdlib only, no numpy) so it works the same on any Python 3.

Request (stdin): {"series": [1, 2, 3, 4, 5]} OR {"x": [...], "y": [...]},
                   optional "periods" (int, default 5) for how many future
                   points to forecast.
Output (stdout): {slope, intercept, r_squared, forecast: [...]}
"""
import json
import sys


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e})); return 0
    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"series": [1, 2, 3, 4, 5]},
        })); return 0

    series = q.get("series")
    x = q.get("x")
    y = q.get("y")

    if series is not None:
        if not isinstance(series, list) or len(series) < 2:
            print(json.dumps({
                "error": "'series' must be a list of at least 2 numbers",
                "example": {"series": [1, 2, 3, 4, 5]},
            })); return 0
        try:
            y_vals = [float(v) for v in series]
        except (TypeError, ValueError):
            print(json.dumps({"error": "'series' must contain only numbers"})); return 0
        x_vals = list(range(len(y_vals)))
    elif x is not None and y is not None:
        if not isinstance(x, list) or not isinstance(y, list) or len(x) != len(y) or len(x) < 2:
            print(json.dumps({
                "error": "'x' and 'y' must be lists of equal length with at least 2 numbers",
                "example": {"x": [0, 1, 2], "y": [1.0, 2.1, 3.2]},
            })); return 0
        try:
            x_vals = [float(v) for v in x]
            y_vals = [float(v) for v in y]
        except (TypeError, ValueError):
            print(json.dumps({"error": "'x' and 'y' must contain only numbers"})); return 0
    else:
        print(json.dumps({
            "error": "provide 'series' (list of numbers) or both 'x' and 'y' (lists of numbers)",
            "example": {"series": [1, 2, 3, 4, 5]},
        })); return 0

    periods = q.get("periods", 5)
    try:
        periods = int(periods)
        if periods < 1:
            raise ValueError
    except (TypeError, ValueError):
        print(json.dumps({"error": "'periods' must be a positive integer", "example": {"periods": 5}})); return 0

    try:
        n = len(x_vals)
        mean_x = sum(x_vals) / n
        mean_y = sum(y_vals) / n
        denom = sum((xi - mean_x) ** 2 for xi in x_vals)
        if denom == 0:
            print(json.dumps({"error": "cannot fit a line: all x values are identical"})); return 0
        slope = sum((xi - mean_x) * (yi - mean_y) for xi, yi in zip(x_vals, y_vals)) / denom
        intercept = mean_y - slope * mean_x

        ss_res = sum((yi - (slope * xi + intercept)) ** 2 for xi, yi in zip(x_vals, y_vals))
        ss_tot = sum((yi - mean_y) ** 2 for yi in y_vals)
        if ss_tot == 0:
            r_squared = 1.0 if ss_res == 0 else 0.0
        else:
            r_squared = 1 - ss_res / ss_tot

        forecast = [slope * (n - 1 + i) + intercept for i in range(1, periods + 1)]

        result = {
            "slope": round(slope, 6),
            "intercept": round(intercept, 6),
            "r_squared": round(r_squared, 4),
            "forecast": [round(v, 6) for v in forecast],
        }
        print(json.dumps(result, indent=2, default=str)); return 0
    except Exception as e:
        print(json.dumps({"error": "linear_forecast failed: %s" % e})); return 1


if __name__ == "__main__":
    sys.exit(main())
