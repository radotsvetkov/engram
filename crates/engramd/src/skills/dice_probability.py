#!/usr/bin/env python3
"""dice_probability — Engram skill (no network). Exact dice-roll distribution.

Computes the EXACT probability distribution of the sum of a set of dice (via
convolution — enumerated, not simulated), plus an optional flat modifier.
Accepts either a list of die sizes or a standard "NdM+K" expression. Reports
the expected value, variance, most likely total, and the probability of each
possible total. Stdlib only (no network).

Request (stdin): {"dice": [6, 6], "modifier": 3}   OR   {"expression": "2d6+3"}
Output (stdout): {dice, modifier, min, max, expected_value, variance, stdev,
                  most_likely, distribution: {total: prob},
                  prob_at_least: {total: prob}}
"""
import json
import re
import sys

MAX_FACES = 100000  # cap total enumerated states to stay fast/bounded


def _parse_expression(expr):
    # e.g. "2d6+3", "d20", "3d8-1", "1d6 + 2d4" (sum of terms), optional +K/-K
    s = expr.replace(" ", "").lower()
    if not s:
        raise ValueError("empty expression")
    dice = []
    modifier = 0
    # Split on +/- but keep signs.
    tokens = re.findall(r"[+-]?[^+-]+", s)
    for tok in tokens:
        sign = -1 if tok.startswith("-") else 1
        body = tok.lstrip("+-")
        m = re.fullmatch(r"(\d*)d(\d+)", body)
        if m:
            count = int(m.group(1)) if m.group(1) else 1
            sides = int(m.group(2))
            if sign < 0:
                raise ValueError("cannot subtract dice; only a flat modifier may be negative")
            for _ in range(count):
                dice.append(sides)
        elif re.fullmatch(r"\d+", body):
            modifier += sign * int(body)
        else:
            raise ValueError("could not parse term %r (use NdM+K, e.g. 2d6+3)" % tok)
    if not dice:
        raise ValueError("expression has no dice (need at least one NdM term)")
    return dice, modifier


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e}))
        return 0
    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object"}))
        return 0

    dice = None
    modifier = 0
    try:
        if q.get("expression"):
            dice, modifier = _parse_expression(str(q["expression"]))
        elif q.get("dice") is not None:
            raw = q.get("dice")
            if not isinstance(raw, list) or not raw:
                raise ValueError("'dice' must be a non-empty list of die sizes, e.g. [6, 6]")
            dice = [int(x) for x in raw]
            modifier = int(q.get("modifier", 0) or 0)
        else:
            print(json.dumps({
                "error": "provide 'dice' (list of die sizes) or an 'expression'",
                "example": {"dice": [6, 6], "modifier": 3},
                "example2": {"expression": "2d6+3"},
            }))
            return 0
        for d in dice:
            if d < 1:
                raise ValueError("each die must have at least 1 side")
        states = 1
        for d in dice:
            states *= d
        if states > MAX_FACES:
            raise ValueError("too many combinations (%d); reduce dice count/size (cap %d)" % (states, MAX_FACES))
    except ValueError as e:
        print(json.dumps({"error": str(e),
                          "example": {"dice": [6, 6], "modifier": 3}}))
        return 0
    except (TypeError, OverflowError) as e:
        print(json.dumps({"error": "invalid dice input: %s" % e}))
        return 0

    # Convolve distributions. Track counts (integers) then normalize.
    counts = {0: 1}
    for sides in dice:
        nxt = {}
        for total, c in counts.items():
            for face in range(1, sides + 1):
                nxt[total + face] = nxt.get(total + face, 0) + c
        counts = nxt

    total_combos = 1
    for d in dice:
        total_combos *= d

    # Apply modifier, build probability distribution.
    dist = {}
    for total, c in counts.items():
        dist[total + modifier] = c / total_combos

    totals_sorted = sorted(dist)
    expected = sum(t * p for t, p in dist.items())
    variance = sum(((t - expected) ** 2) * p for t, p in dist.items())
    stdev = variance ** 0.5
    max_prob = max(dist.values())
    most_likely = [t for t in totals_sorted if abs(dist[t] - max_prob) < 1e-12]

    # cumulative P(total >= t)
    prob_at_least = {}
    running = 0.0
    for t in reversed(totals_sorted):
        running += dist[t]
        prob_at_least[t] = min(running, 1.0)
    prob_at_least = {t: prob_at_least[t] for t in totals_sorted}

    result = {
        "dice": dice,
        "modifier": modifier,
        "min": totals_sorted[0],
        "max": totals_sorted[-1],
        "expected_value": round(expected, 6),
        "variance": round(variance, 6),
        "stdev": round(stdev, 6),
        "most_likely": most_likely if len(most_likely) > 1 else most_likely[0],
        "distribution": {str(t): round(dist[t], 8) for t in totals_sorted},
        "prob_at_least": {str(t): round(prob_at_least[t], 8) for t in totals_sorted},
    }
    print(json.dumps(result, indent=2, default=str))
    return 0


if __name__ == "__main__":
    sys.exit(main())
