#!/usr/bin/env python3
"""regression_metrics — Engram skill (no network). Score regression predictions.

Given actual and predicted numeric series of equal length, computes MAE, MSE,
RMSE, MAPE (percent, skipping terms where actual==0 and reporting how many were
skipped), and the coefficient of determination R^2. Stdlib only.

Request (stdin): {"actual": [3, -0.5, 2, 7], "predicted": [2.5, 0.0, 2, 8]}
Output (stdout): {n, mae, mse, rmse, mape, mape_skipped, r2}
"""
import json, sys, math


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e})); return 0

    ex = {"actual": [3, -0.5, 2, 7], "predicted": [2.5, 0.0, 2, 8]}
    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object", "example": ex})); return 0

    def _numlist(v):
        return isinstance(v, list) and all(isinstance(t, (int, float)) and not isinstance(t, bool) for t in v)

    actual = q.get("actual")
    predicted = q.get("predicted")
    if not _numlist(actual) or not _numlist(predicted):
        print(json.dumps({"error": "'actual' and 'predicted' must be lists of numbers", "example": ex})); return 0
    if len(actual) != len(predicted):
        print(json.dumps({"error": "'actual' and 'predicted' must have equal length (got %d and %d)" % (len(actual), len(predicted)), "example": ex})); return 0
    if len(actual) == 0:
        print(json.dumps({"error": "series are empty", "example": ex})); return 0

    try:
        a = [float(v) for v in actual]
        p = [float(v) for v in predicted]
        n = len(a)
        errs = [ai - pi for ai, pi in zip(a, p)]
        mae = sum(abs(e) for e in errs) / n
        mse = sum(e * e for e in errs) / n
        rmse = math.sqrt(mse)

        # MAPE, skipping actual == 0.
        pct_terms = [abs(e / ai) for ai, e in zip(a, errs) if ai != 0]
        skipped = n - len(pct_terms)
        mape = (sum(pct_terms) / len(pct_terms) * 100.0) if pct_terms else None

        # R^2.
        mean_a = sum(a) / n
        ss_tot = sum((ai - mean_a) ** 2 for ai in a)
        ss_res = sum(e * e for e in errs)
        r2 = None if ss_tot == 0 else 1.0 - ss_res / ss_tot

        result = {
            "n": n,
            "mae": round(mae, 6),
            "mse": round(mse, 6),
            "rmse": round(rmse, 6),
            "mape": round(mape, 6) if mape is not None else None,
            "mape_skipped": skipped,
            "r2": round(r2, 6) if r2 is not None else None,
        }
        if mape is None:
            result["mape_note"] = "all actual values are 0; MAPE undefined"
        if r2 is None:
            result["r2_note"] = "actual has zero variance; R^2 undefined"
        print(json.dumps(result, indent=2, default=str)); return 0
    except Exception as e:
        print(json.dumps({"error": "regression_metrics failed: %s" % e})); return 1


if __name__ == "__main__":
    sys.exit(main())
