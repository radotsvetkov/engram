#!/usr/bin/env python3
"""sharding_calculator — Engram skill (no network). Estimate shard count and
storage for a horizontally-partitioned dataset.

Works from a row count and a target rows-per-shard, or from a total size and a
target shard size. Reports shard_count (ceil), rows/storage per shard, and total
storage including the replication factor. All estimates are back-of-the-envelope
capacity planning, not a guarantee — real shard sizes drift with data skew.

Request (stdin): {"total_rows": 500000000, "target_rows_per_shard": 10000000,
  "replication_factor": 3, "avg_row_bytes": 1200}
  OR {"total_size_gb": 4000, "target_shard_size_gb": 200, "replication_factor": 3}
Output (stdout): {shard_count, rows_per_shard, total_storage_gb, storage_per_shard_gb,
  hot_shard_note, recommendation, inputs}
"""
import json
import math
import sys


def _pos_num(v):
    return isinstance(v, (int, float)) and not isinstance(v, bool) and v > 0


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e}))
        return 0
    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"total_rows": 500000000, "target_rows_per_shard": 10000000,
                        "replication_factor": 3, "avg_row_bytes": 1200},
        }))
        return 0

    example = {
        "by_rows": {"total_rows": 500000000, "target_rows_per_shard": 10000000,
                    "replication_factor": 3, "avg_row_bytes": 1200},
        "by_size": {"total_size_gb": 4000, "target_shard_size_gb": 200, "replication_factor": 3},
    }

    replication = q.get("replication_factor", 3)
    if not _pos_num(replication):
        print(json.dumps({"error": "replication_factor must be a positive number", "example": example}))
        return 0
    replication = float(replication)

    total_rows = q.get("total_rows")
    total_size_gb = q.get("total_size_gb")

    try:
        gb = float(1024 ** 3)

        if _pos_num(total_size_gb):
            # Size-driven path.
            target_shard_size_gb = q.get("target_shard_size_gb", 200)
            if not _pos_num(target_shard_size_gb):
                print(json.dumps({"error": "target_shard_size_gb must be a positive number", "example": example}))
                return 0
            total_size_gb = float(total_size_gb)
            target_shard_size_gb = float(target_shard_size_gb)
            shard_count = int(math.ceil(total_size_gb / target_shard_size_gb))
            shard_count = max(1, shard_count)
            storage_per_shard_gb = round(total_size_gb / shard_count, 3)
            total_storage_gb = round(total_size_gb * replication, 3)
            rows_per_shard = None
            mode = "by_size"
            inputs = {"total_size_gb": total_size_gb, "target_shard_size_gb": target_shard_size_gb,
                      "replication_factor": replication}

        elif _pos_num(total_rows):
            # Row-driven path.
            target_rows_per_shard = q.get("target_rows_per_shard", 10000000)
            avg_row_bytes = q.get("avg_row_bytes", 1000)
            if not _pos_num(target_rows_per_shard):
                print(json.dumps({"error": "target_rows_per_shard must be a positive number", "example": example}))
                return 0
            if not _pos_num(avg_row_bytes):
                print(json.dumps({"error": "avg_row_bytes must be a positive number", "example": example}))
                return 0
            total_rows = float(total_rows)
            target_rows_per_shard = float(target_rows_per_shard)
            avg_row_bytes = float(avg_row_bytes)

            shard_count = int(math.ceil(total_rows / target_rows_per_shard))
            shard_count = max(1, shard_count)
            rows_per_shard = int(math.ceil(total_rows / shard_count))

            primary_size_gb = (total_rows * avg_row_bytes) / gb
            total_storage_gb = round(primary_size_gb * replication, 3)
            storage_per_shard_gb = round((primary_size_gb * replication) / shard_count, 3)
            mode = "by_rows"
            inputs = {"total_rows": int(total_rows), "target_rows_per_shard": int(target_rows_per_shard),
                      "avg_row_bytes": int(avg_row_bytes), "replication_factor": replication,
                      "primary_data_gb": round(primary_size_gb, 3)}
        else:
            print(json.dumps({
                "error": "provide 'total_rows' (with optional target_rows_per_shard/avg_row_bytes) "
                         "or 'total_size_gb' (with target_shard_size_gb) — all must be positive",
                "example": example,
            }))
            return 0

        hot_shard_note = (
            "Hot-shard risk: if the shard key is skewed (e.g. a monotonic timestamp, "
            "auto-increment id, or a few high-traffic tenants), a handful of shards will "
            "absorb most reads/writes while others sit idle. Prefer a HASH of a "
            "high-cardinality key, or consistent hashing so re-sharding moves ~1/N of the "
            "data instead of everything. Avoid range-sharding on time unless you also "
            "sub-partition or rotate.")

        recommendation = (
            "Provision %d shards (each ~%s), replication factor %g -> ~%s total. "
            "Add ~30%% headroom for growth and compaction, and pick a hash-based shard key "
            "on a high-cardinality, evenly-distributed column to avoid hot shards."
            % (shard_count,
               ("%d rows" % rows_per_shard) if rows_per_shard is not None else ("%.2f GB" % storage_per_shard_gb),
               replication, ("%.2f GB" % total_storage_gb)))

        result = {
            "mode": mode,
            "shard_count": shard_count,
            "rows_per_shard": rows_per_shard,
            "storage_per_shard_gb": storage_per_shard_gb,
            "total_storage_gb": total_storage_gb,
            "replication_factor": replication,
            "hot_shard_note": hot_shard_note,
            "recommendation": recommendation,
            "inputs": inputs,
        }
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "sharding_calculator failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
