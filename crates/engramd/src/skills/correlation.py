#!/usr/bin/env python3
"""correlation — Engram skill (no network). Pearson & Spearman correlation.

Given two equal-length numeric series, computes the Pearson linear correlation
and the Spearman rank correlation (average ranks for ties), plus a strength
label and direction. Stdlib only.

Request (stdin): {"x": [1, 2, 3, 4, 5], "y": [2, 4, 5, 4, 6]}
Output (stdout): {n, pearson_r, spearman_rho, strength, direction, note?}
"""
import json, sys, math


def _pearson(x, y):
    n = len(x)
    mx = sum(x) / n
    my = sum(y) / n
    sxy = sum((a - mx) * (b - my) for a, b in zip(x, y))
    sxx = sum((a - mx) ** 2 for a in x)
    syy = sum((b - my) ** 2 for b in y)
    if sxx == 0 or syy == 0:
        return None
    return sxy / math.sqrt(sxx * syy)


def _rank(vals):
    # Average ranks for ties (1-based).
    order = sorted(range(len(vals)), key=lambda i: vals[i])
    ranks = [0.0] * len(vals)
    i = 0
    while i < len(order):
        j = i
        while j + 1 < len(order) and vals[order[j + 1]] == vals[order[i]]:
            j += 1
        avg = (i + j) / 2.0 + 1.0  # average of positions i..j, converted to 1-based
        for k in range(i, j + 1):
            ranks[order[k]] = avg
        i = j + 1
    return ranks


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e})); return 0

    ex = {"x": [1, 2, 3, 4, 5], "y": [2, 4, 5, 4, 6]}
    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object", "example": ex})); return 0

    x = q.get("x")
    y = q.get("y")

    def _numlist(v):
        return isinstance(v, list) and all(isinstance(t, (int, float)) and not isinstance(t, bool) for t in v)

    if not _numlist(x) or not _numlist(y):
        print(json.dumps({"error": "'x' and 'y' must both be lists of numbers", "example": ex})); return 0
    if len(x) != len(y):
        print(json.dumps({"error": "'x' and 'y' must have equal length (got %d and %d)" % (len(x), len(y)), "example": ex})); return 0
    if len(x) < 2:
        print(json.dumps({"error": "need at least 2 paired points", "example": ex})); return 0

    try:
        xf = [float(v) for v in x]
        yf = [float(v) for v in y]
        r = _pearson(xf, yf)
        rho = _pearson(_rank(xf), _rank(yf))

        note = None
        if r is None or rho is None:
            note = "one series has zero variance; correlation is undefined"

        if r is None:
            strength, direction = None, None
        else:
            a = abs(r)
            strength = ("strong" if a >= 0.7 else "moderate" if a >= 0.4
                        else "weak" if a >= 0.2 else "negligible")
            direction = "positive" if r > 0 else "negative" if r < 0 else "none"

        result = {
            "n": len(xf),
            "pearson_r": round(r, 6) if r is not None else None,
            "spearman_rho": round(rho, 6) if rho is not None else None,
            "strength": strength,
            "direction": direction,
        }
        if note:
            result["note"] = note
        print(json.dumps(result, indent=2, default=str)); return 0
    except Exception as e:
        print(json.dumps({"error": "correlation failed: %s" % e})); return 1


if __name__ == "__main__":
    sys.exit(main())
