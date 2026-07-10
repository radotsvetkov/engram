#!/usr/bin/env python3
"""monte_carlo_sim — Engram skill (no network). Monte Carlo risk simulation.

Draws `trials` random samples from a chosen distribution (uniform, normal, or
triangular) with a fixed seed for reproducibility, then reports the summary
statistics and percentiles of the trial outcomes. Optionally sum one draw from
each of several `variables` per trial (e.g. total project cost = sum of tasks).
Stdlib only (random, math) — deterministic given the seed.

Request (stdin): {"trials": 10000, "seed": 42,
                  "distribution": "triangular",
                  "params": {"low": 1, "mode": 3, "high": 10},
                  "aggregate": "sum",
                  "variables": [{"distribution":"normal","params":{"mean":5,"stdev":1}}, ...]}
Output (stdout): {trials, distribution, mean, stdev, min, max,
                  percentiles: {p5,p25,p50,p75,p95}}
"""
import json
import math
import random
import sys

MAX_TRIALS = 1_000_000


def _draw(rng, distribution, params):
    d = (distribution or "").strip().lower()
    if d == "uniform":
        low = float(params["low"])
        high = float(params["high"])
        if high < low:
            raise ValueError("uniform: high must be >= low")
        return rng.uniform(low, high)
    if d == "normal":
        mean = float(params["mean"])
        stdev = float(params["stdev"])
        if stdev < 0:
            raise ValueError("normal: stdev must be >= 0")
        return rng.gauss(mean, stdev)
    if d == "triangular":
        low = float(params["low"])
        mode = float(params["mode"])
        high = float(params["high"])
        if not (low <= mode <= high):
            raise ValueError("triangular: require low <= mode <= high")
        return rng.triangular(low, high, mode)
    raise ValueError("unknown distribution %r (use uniform|normal|triangular)" % distribution)


def _percentile(sorted_vals, pct):
    # Linear interpolation between closest ranks.
    if not sorted_vals:
        return None
    if len(sorted_vals) == 1:
        return sorted_vals[0]
    rank = (pct / 100.0) * (len(sorted_vals) - 1)
    lo = math.floor(rank)
    hi = math.ceil(rank)
    if lo == hi:
        return sorted_vals[int(rank)]
    frac = rank - lo
    return sorted_vals[lo] * (1 - frac) + sorted_vals[hi] * frac


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e}))
        return 0
    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object"}))
        return 0

    variables = q.get("variables")
    single_dist = q.get("distribution")
    if not variables and not single_dist:
        print(json.dumps({
            "error": "provide 'distribution' + 'params', or a 'variables' list",
            "example": {
                "trials": 10000, "seed": 42, "distribution": "triangular",
                "params": {"low": 1, "mode": 3, "high": 10},
            },
        }))
        return 0

    try:
        trials = int(q.get("trials", 10000) or 10000)
    except (TypeError, ValueError):
        print(json.dumps({"error": "'trials' must be an integer"}))
        return 0
    if trials < 1:
        print(json.dumps({"error": "'trials' must be >= 1"}))
        return 0
    trials = min(trials, MAX_TRIALS)

    try:
        seed = int(q.get("seed", 42) or 42)
    except (TypeError, ValueError):
        print(json.dumps({"error": "'seed' must be an integer"}))
        return 0

    rng = random.Random(seed)

    try:
        if variables:
            if not isinstance(variables, list) or not variables:
                raise ValueError("'variables' must be a non-empty list")
            specs = []
            for v in variables:
                if not isinstance(v, dict):
                    raise ValueError("each variable must be an object with 'distribution' and 'params'")
                specs.append((v.get("distribution"), v.get("params") or {}))
            # sanity check specs with one draw each (raises if params bad)
            for dist, params in specs:
                _draw(random.Random(0), dist, params)
            outcomes = []
            for _ in range(trials):
                total = 0.0
                for dist, params in specs:
                    total += _draw(rng, dist, params)
                outcomes.append(total)
            desc = "sum of %d variables" % len(specs)
        else:
            params = q.get("params") or {}
            if not isinstance(params, dict):
                raise ValueError("'params' must be an object")
            # sanity check
            _draw(random.Random(0), single_dist, params)
            outcomes = [_draw(rng, single_dist, params) for _ in range(trials)]
            desc = single_dist
    except KeyError as e:
        print(json.dumps({"error": "missing distribution parameter: %s" % e,
                          "example": {"distribution": "uniform", "params": {"low": 0, "high": 1}}}))
        return 0
    except ValueError as e:
        print(json.dumps({"error": str(e)}))
        return 0

    # aggregate is metadata describing the per-trial reduction; we already
    # produce one outcome per trial (a single draw, or the sum of variables).
    aggregate = (q.get("aggregate") or ("sum" if variables else "value"))

    outcomes.sort()
    n = len(outcomes)
    mean = sum(outcomes) / n
    if n > 1:
        var = sum((x - mean) ** 2 for x in outcomes) / (n - 1)
    else:
        var = 0.0
    stdev = math.sqrt(var)

    result = {
        "trials": n,
        "distribution": desc,
        "aggregate": aggregate,
        "mean": round(mean, 6),
        "stdev": round(stdev, 6),
        "min": round(outcomes[0], 6),
        "max": round(outcomes[-1], 6),
        "percentiles": {
            "p5": round(_percentile(outcomes, 5), 6),
            "p25": round(_percentile(outcomes, 25), 6),
            "p50": round(_percentile(outcomes, 50), 6),
            "p75": round(_percentile(outcomes, 75), 6),
            "p95": round(_percentile(outcomes, 95), 6),
        },
    }
    print(json.dumps(result, indent=2, default=str))
    return 0


if __name__ == "__main__":
    sys.exit(main())
