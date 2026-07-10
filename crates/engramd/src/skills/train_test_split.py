#!/usr/bin/env python3
"""train_test_split — Engram skill (no network). Deterministic dataset split.

Splits a dataset into train/test (and optional validation) partitions. Provide
either 'n' (number of rows) or 'data' (a list to slice). The split is
deterministic for a given seed: a random.Random(seed) shuffles the index order
when shuffle is true. Ratios must sum to < 1.0 (the remainder is train).

Request (stdin): {"n": 10, "test_ratio": 0.2, "val_ratio": 0.1, "seed": 42, "shuffle": true}
                 {"data": ["a","b","c","d","e"], "test_ratio": 0.4}
Output (stdout): {n, counts, train_indices, test_indices, val_indices?, train?, test?, val?}
"""
import json, sys, random


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e})); return 0

    ex = {"n": 10, "test_ratio": 0.2, "val_ratio": 0.1, "seed": 42, "shuffle": True}
    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object", "example": ex})); return 0

    data = q.get("data", None)
    has_data = data is not None
    if has_data and not isinstance(data, list):
        print(json.dumps({"error": "'data' must be a list", "example": ex})); return 0

    if has_data:
        n = len(data)
    else:
        n = q.get("n")
        if not isinstance(n, int) or isinstance(n, bool) or n < 1:
            print(json.dumps({"error": "provide 'n' (positive integer) or 'data' (a list)", "example": ex})); return 0

    test_ratio = q.get("test_ratio", 0.2)
    val_ratio = q.get("val_ratio", 0.0)
    seed = q.get("seed", 42)
    shuffle = q.get("shuffle", True)

    for name, r in (("test_ratio", test_ratio), ("val_ratio", val_ratio)):
        if not isinstance(r, (int, float)) or isinstance(r, bool) or r < 0 or r >= 1:
            print(json.dumps({"error": "'%s' must be a number in [0, 1)" % name, "example": ex})); return 0
    if test_ratio + val_ratio >= 1.0:
        print(json.dumps({"error": "test_ratio + val_ratio must be < 1.0 (train needs the remainder)", "example": ex})); return 0
    if not isinstance(seed, int) or isinstance(seed, bool):
        print(json.dumps({"error": "'seed' must be an integer", "example": ex})); return 0
    if not isinstance(shuffle, bool):
        print(json.dumps({"error": "'shuffle' must be a boolean", "example": ex})); return 0
    if n < 1:
        print(json.dumps({"error": "need at least 1 item to split", "example": ex})); return 0

    try:
        n_test = int(n * test_ratio)
        n_val = int(n * val_ratio)
        n_train = n - n_test - n_val

        idx = list(range(n))
        if shuffle:
            rng = random.Random(seed)
            rng.shuffle(idx)

        # Order: test, val, train (train takes the remainder).
        test_idx = sorted(idx[:n_test])
        val_idx = sorted(idx[n_test:n_test + n_val])
        train_idx = sorted(idx[n_test + n_val:])

        result = {
            "n": n,
            "shuffle": shuffle,
            "seed": seed,
            "counts": {"train": len(train_idx), "test": len(test_idx), "val": len(val_idx)},
            "train_indices": train_idx,
            "test_indices": test_idx,
        }
        if val_ratio > 0:
            result["val_indices"] = val_idx

        if has_data:
            result["train"] = [data[i] for i in train_idx]
            result["test"] = [data[i] for i in test_idx]
            if val_ratio > 0:
                result["val"] = [data[i] for i in val_idx]

        print(json.dumps(result, indent=2, default=str)); return 0
    except Exception as e:
        print(json.dumps({"error": "train_test_split failed: %s" % e})); return 1


if __name__ == "__main__":
    sys.exit(main())
