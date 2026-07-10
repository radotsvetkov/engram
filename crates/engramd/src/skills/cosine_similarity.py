#!/usr/bin/env python3
"""cosine_similarity — Engram skill (no network). Cosine similarity of two vectors.

cosine = dot / (||a|| * ||b||). Also reports dot_product and euclidean_distance, plus a
qualitative label. Guards unequal length and zero vectors (cosine null with a note).

Request (stdin): {"a": [1, 2, 3], "b": [2, 3, 4]}
Output (stdout): {cosine_similarity, similarity, dot_product, euclidean_distance, magnitude_a, magnitude_b, dimensions}
"""
import json, sys, math


def _label(c):
    if c is None:
        return "undefined"
    if c >= 0.9:
        return "near-identical"
    if c >= 0.7:
        return "high"
    if c >= 0.4:
        return "moderate"
    return "low"


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e})); return 0

    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"a": [1, 2, 3], "b": [2, 3, 4]},
        })); return 0

    a = q.get("a")
    b = q.get("b")
    if not isinstance(a, list) or not isinstance(b, list):
        print(json.dumps({
            "error": "missing required fields 'a' and 'b' (lists of numbers)",
            "example": {"a": [1, 2, 3], "b": [2, 3, 4]},
        })); return 0

    try:
        if not a or not b:
            print(json.dumps({"error": "vectors must be non-empty"})); return 0
        for vec, name in ((a, "a"), (b, "b")):
            for x in vec:
                if isinstance(x, bool) or not isinstance(x, (int, float)):
                    print(json.dumps({"error": "vector '%s' must contain only numbers" % name})); return 0
        if len(a) != len(b):
            print(json.dumps({
                "error": "vectors must be equal length (got %d and %d)" % (len(a), len(b)),
            })); return 0

        dot = sum(x * y for x, y in zip(a, b))
        mag_a = math.sqrt(sum(x * x for x in a))
        mag_b = math.sqrt(sum(y * y for y in b))
        euclid = math.sqrt(sum((x - y) ** 2 for x, y in zip(a, b)))

        note = None
        if mag_a == 0 or mag_b == 0:
            cosine = None
            note = "cosine undefined: at least one vector is all zeros"
        else:
            cosine = dot / (mag_a * mag_b)

        result = {
            "cosine_similarity": round(cosine, 6) if cosine is not None else None,
            "similarity": _label(cosine),
            "dot_product": round(dot, 6),
            "euclidean_distance": round(euclid, 6),
            "magnitude_a": round(mag_a, 6),
            "magnitude_b": round(mag_b, 6),
            "dimensions": len(a),
        }
        if note:
            result["note"] = note
        print(json.dumps(result, indent=2, default=str)); return 0
    except Exception as e:
        print(json.dumps({"error": "cosine_similarity failed: %s" % e})); return 1


if __name__ == "__main__":
    sys.exit(main())
