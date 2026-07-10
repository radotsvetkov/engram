#!/usr/bin/env python3
"""amdahl_speedup — Engram skill (no network). Parallel speedup calculator.

Given the parallelizable fraction of a workload and a processor count (or a
list of counts), computes Amdahl's-law speedup (fixed problem size),
Gustafson's-law scaled speedup (problem grows with cores), parallel efficiency,
and the theoretical maximum speedup as cores -> infinity. Stdlib only.

Amdahl:    speedup = 1 / ((1 - p) + p / n)
Gustafson: scaled_speedup = (1 - p) + p * n
Max:       1 / (1 - p)

Request (stdin): {"parallel_fraction": 0.9, "num_processors": 10}
             OR  {"parallel_fraction": 0.9, "num_processors": [1, 2, 4, 8, 16]}
Output (stdout): {parallel_fraction, max_theoretical_speedup,
                  results: [{n, amdahl_speedup, gustafson_speedup, efficiency}],
                  note}
"""
import json
import math
import sys


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e}))
        return 0
    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object"}))
        return 0

    if "parallel_fraction" not in q:
        print(json.dumps({
            "error": "missing required field: parallel_fraction (0..1)",
            "example": {"parallel_fraction": 0.9, "num_processors": 10},
        }))
        return 0

    try:
        p = float(q["parallel_fraction"])
    except (TypeError, ValueError):
        print(json.dumps({"error": "parallel_fraction must be a number between 0 and 1"}))
        return 0
    if not (0.0 <= p <= 1.0):
        print(json.dumps({"error": "parallel_fraction must be between 0 and 1 (got %s)" % p}))
        return 0

    raw_n = q.get("num_processors", q.get("processors"))
    if raw_n is None:
        print(json.dumps({
            "error": "missing required field: num_processors (int or list of ints)",
            "example": {"parallel_fraction": 0.9, "num_processors": [1, 2, 4, 8]},
        }))
        return 0
    n_list = raw_n if isinstance(raw_n, list) else [raw_n]
    try:
        n_list = [int(x) for x in n_list]
    except (TypeError, ValueError):
        print(json.dumps({"error": "num_processors must be an integer or a list of integers"}))
        return 0
    if not n_list or any(n < 1 for n in n_list):
        print(json.dumps({"error": "each processor count must be an integer >= 1"}))
        return 0

    serial = 1.0 - p
    max_speedup = math.inf if serial == 0 else 1.0 / serial

    results = []
    for n in n_list:
        amdahl = 1.0 / (serial + p / n)
        gustafson = serial + p * n
        results.append({
            "n": n,
            "amdahl_speedup": round(amdahl, 4),
            "gustafson_speedup": round(gustafson, 4),
            "efficiency": round(amdahl / n, 4),
        })

    result = {
        "parallel_fraction": p,
        "serial_fraction": round(serial, 6),
        "max_theoretical_speedup": ("infinite" if math.isinf(max_speedup) else round(max_speedup, 4)),
        "results": results,
        "note": ("Amdahl fixes the problem size (speedup is capped by the serial "
                 "fraction, approaching %s as n grows); Gustafson grows the problem "
                 "with the core count (speedup scales roughly linearly)."
                 % ("infinity" if math.isinf(max_speedup) else round(max_speedup, 2))),
    }
    print(json.dumps(result, indent=2, default=str))
    return 0


if __name__ == "__main__":
    sys.exit(main())
