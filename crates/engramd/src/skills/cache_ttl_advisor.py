#!/usr/bin/env python3
"""cache_ttl_advisor — Engram skill (no network). Recommend a cache TTL and
caching strategy from read/write rates, staleness tolerance, and volatility.

TTL is driven by staleness_tolerance_seconds when given, otherwise mapped from
data_volatility (low=3600s, medium=300s, high=30s). The read/write ratio picks
a strategy (cache-aside vs write-through vs write-behind) and warns when a
write-heavy workload will thrash the cache (low hit rate). Heuristics for
capacity planning, not a guarantee — validate against real hit-rate metrics.

Request (stdin): {"reads_per_min": 5000, "writes_per_min": 50,
  "staleness_tolerance_seconds": 60, "data_volatility": "medium"}
Output (stdout): {recommended_ttl_seconds, strategy, estimated_hit_rate_note, reasoning, inputs}
"""
import json
import sys

_VOLATILITY_TTL = {"low": 3600, "medium": 300, "high": 30}


def _num(v):
    return isinstance(v, (int, float)) and not isinstance(v, bool)


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e}))
        return 0
    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"reads_per_min": 5000, "writes_per_min": 50,
                        "staleness_tolerance_seconds": 60, "data_volatility": "medium"},
        }))
        return 0

    example = {"reads_per_min": 5000, "writes_per_min": 50,
               "staleness_tolerance_seconds": 60, "data_volatility": "medium"}

    reads = q.get("reads_per_min")
    writes = q.get("writes_per_min")
    staleness = q.get("staleness_tolerance_seconds")
    volatility = q.get("data_volatility")
    if isinstance(volatility, str):
        volatility = volatility.strip().lower()
        if volatility not in _VOLATILITY_TTL:
            volatility = None

    # Validate numeric inputs if present (must be non-negative).
    for name, val in (("reads_per_min", reads), ("writes_per_min", writes),
                      ("staleness_tolerance_seconds", staleness)):
        if val is not None and (not _num(val) or val < 0):
            print(json.dumps({"error": "%s must be a non-negative number" % name, "example": example}))
            return 0

    try:
        reasoning = []

        # --- TTL ---
        if _num(staleness):
            recommended_ttl = int(staleness)
            reasoning.append("TTL set to the staleness tolerance (%ds): entries expire within "
                             "the window during which a stale read is acceptable." % int(staleness))
        elif volatility:
            recommended_ttl = _VOLATILITY_TTL[volatility]
            reasoning.append("no staleness tolerance given; TTL mapped from data_volatility=%s "
                             "-> %ds (low=3600, medium=300, high=30)." % (volatility, recommended_ttl))
        else:
            recommended_ttl = _VOLATILITY_TTL["medium"]
            reasoning.append("neither staleness_tolerance_seconds nor data_volatility given; "
                             "defaulting to a medium TTL of 300s. Supply either for a tighter fit.")

        # --- read/write ratio & strategy ---
        ratio = None
        if _num(reads) and _num(writes):
            if writes > 0:
                ratio = round(reads / writes, 2)
            else:
                ratio = None  # infinite / read-only
        write_heavy = False

        if _num(reads) and _num(writes):
            if writes == 0:
                strategy = "cache-aside"
                reasoning.append("read-only workload (0 writes): cache-aside with a TTL is "
                                 "ideal — populate on miss, no invalidation needed.")
            elif ratio is not None and ratio >= 10:
                strategy = "cache-aside"
                reasoning.append("read-heavy (read:write ~%.1f:1): cache-aside (lazy-load on "
                                 "miss) gives a high hit rate with minimal write overhead." % ratio)
            elif ratio is not None and ratio >= 2:
                strategy = "write-through"
                reasoning.append("moderately read-heavy (read:write ~%.1f:1): write-through keeps "
                                 "the cache warm and consistent on every write, at the cost of a "
                                 "little extra write latency." % ratio)
            else:
                strategy = "write-behind"
                write_heavy = True
                reasoning.append("write-heavy (read:write ~%s): consider write-behind (write-back) "
                                 "to batch writes to the store, or reconsider caching at all — a "
                                 "write-dominated key thrashes the cache." % (
                                     ("%.2f:1" % ratio) if ratio is not None else "high"))
        else:
            strategy = "cache-aside"
            reasoning.append("read/write rates not both given; defaulting to cache-aside (the most "
                             "common, simplest strategy: app reads cache, loads from DB on miss, "
                             "writes invalidate/update the key).")

        # --- hit-rate note ---
        if write_heavy:
            hit_rate_note = ("WARNING: writes are a large share of traffic, so cached entries are "
                             "frequently invalidated/overwritten before they're reused — expect a "
                             "LOW hit rate and cache thrash. Caching may add overhead without much "
                             "benefit here; cache only the read-mostly sub-keys, or shorten TTL and "
                             "measure the hit rate before committing.")
        elif ratio is not None and ratio >= 10:
            hit_rate_note = ("read:write ~%.1f:1 with a %ds TTL should yield a HIGH hit rate for "
                             "hot keys. Measure real hit rate in production and tune TTL up if "
                             "staleness allows." % (ratio, recommended_ttl))
        else:
            hit_rate_note = ("hit rate depends on key reuse within the %ds TTL window and how often "
                             "writes invalidate entries. Instrument cache hits/misses and tune TTL "
                             "to the highest value your staleness tolerance permits." % recommended_ttl)

        result = {
            "recommended_ttl_seconds": recommended_ttl,
            "strategy": strategy,
            "read_write_ratio": ratio,
            "estimated_hit_rate_note": hit_rate_note,
            "reasoning": reasoning,
            "inputs": {
                "reads_per_min": reads,
                "writes_per_min": writes,
                "staleness_tolerance_seconds": staleness,
                "data_volatility": volatility,
            },
        }
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "cache_ttl_advisor failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
