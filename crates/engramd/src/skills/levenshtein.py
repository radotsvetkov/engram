#!/usr/bin/env python3
"""levenshtein — Engram skill (no network). Levenshtein edit distance between two strings.

Classic dynamic-programming edit distance (insert / delete / substitute, unit cost),
O(len_a*len_b). Inputs are capped at 5000 characters each to bound cost (error if longer).
Reports distance, max_length, and similarity_ratio (1 - distance/max_length).

Request (stdin): {"a": "kitten", "b": "sitting"}
Output (stdout): {distance, max_length, length_a, length_b, similarity_ratio, operations_estimate}
"""
import json, sys

MAX_LEN = 5000


def _levenshtein(a, b):
    m, n = len(a), len(b)
    if m == 0:
        return n
    if n == 0:
        return m
    prev = list(range(n + 1))
    for i in range(1, m + 1):
        cur = [i] + [0] * n
        ai = a[i - 1]
        for j in range(1, n + 1):
            cost = 0 if ai == b[j - 1] else 1
            cur[j] = min(prev[j] + 1, cur[j - 1] + 1, prev[j - 1] + cost)
        prev = cur
    return prev[n]


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e})); return 0

    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"a": "kitten", "b": "sitting"},
        })); return 0

    a = q.get("a")
    b = q.get("b")
    if not isinstance(a, str) or not isinstance(b, str):
        print(json.dumps({
            "error": "missing required fields 'a' and 'b' (strings)",
            "example": {"a": "kitten", "b": "sitting"},
        })); return 0

    try:
        if len(a) > MAX_LEN or len(b) > MAX_LEN:
            print(json.dumps({
                "error": "inputs too long (max %d chars each); got %d and %d" % (MAX_LEN, len(a), len(b)),
            })); return 0

        dist = _levenshtein(a, b)
        max_len = max(len(a), len(b))
        ratio = 1 - dist / max_len if max_len else 1.0
        result = {
            "distance": dist,
            "max_length": max_len,
            "length_a": len(a),
            "length_b": len(b),
            "similarity_ratio": round(ratio, 6),
            "operations_estimate": "%d single-character edit(s): insertions, deletions, or substitutions" % dist,
        }
        print(json.dumps(result, indent=2, default=str)); return 0
    except Exception as e:
        print(json.dumps({"error": "levenshtein failed: %s" % e})); return 1


if __name__ == "__main__":
    sys.exit(main())
