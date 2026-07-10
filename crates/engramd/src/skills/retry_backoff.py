#!/usr/bin/env python3
"""retry_backoff — Engram skill (no network).

Computes a deterministic retry/backoff schedule for flaky operations.
Supports exponential, linear or fibonacci growth, a max-delay cap, and
full/equal jitter (shown as delay RANGES, no randomness needed). Use it to
size timeouts and reason about worst-case total wait before you code it.

Request (stdin): {"max_retries"?: int=5, "base_delay"?: float=1.0,
  "factor"?: float=2.0, "max_delay"?: float=60.0,
  "jitter"?: "none"|"full"|"equal", "strategy"?: "exponential"|"linear"|"fibonacci"}
Output (stdout): {strategy, jitter, params, schedule, total_max_wait_seconds, note}
"""
import json
import sys

STRATEGIES = ("exponential", "linear", "fibonacci")
JITTERS = ("none", "full", "equal")


def base_delay_for(strategy, i, base, factor, max_delay):
    """Un-jittered delay for attempt index i (0-based)."""
    if strategy == "exponential":
        d = base * (factor ** i)
    elif strategy == "linear":
        d = base * (i + 1)
    else:  # fibonacci: 1,1,2,3,5,8,... scaled by base
        a, b = 1, 1
        for _ in range(i):
            a, b = b, a + b
        d = base * a
    return min(d, max_delay)


def _num(q, key, default, example):
    v = q.get(key, default)
    try:
        return float(v), None
    except (TypeError, ValueError):
        return None, json.dumps({"error": "'%s' must be a number" % key, "example": example})


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0

    example = {"max_retries": 5, "base_delay": 1.0, "factor": 2.0,
               "max_delay": 60.0, "jitter": "full", "strategy": "exponential"}

    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object", "example": example}))
        return 0

    max_retries = q.get("max_retries", 5)
    try:
        max_retries = int(max_retries)
    except (TypeError, ValueError):
        print(json.dumps({"error": "'max_retries' must be an integer", "example": example}))
        return 0
    if max_retries < 1:
        max_retries = 1
    if max_retries > 100:
        max_retries = 100

    base_delay, err = _num(q, "base_delay", 1.0, example)
    if err:
        print(err)
        return 0
    factor, err = _num(q, "factor", 2.0, example)
    if err:
        print(err)
        return 0
    max_delay, err = _num(q, "max_delay", 60.0, example)
    if err:
        print(err)
        return 0

    strategy = str(q.get("strategy", "exponential")).strip().lower()
    if strategy not in STRATEGIES:
        print(json.dumps({"error": "'strategy' must be one of %s" % list(STRATEGIES),
                          "example": example}))
        return 0
    jitter = str(q.get("jitter", "none")).strip().lower()
    if jitter not in JITTERS:
        print(json.dumps({"error": "'jitter' must be one of %s" % list(JITTERS),
                          "example": example}))
        return 0

    if base_delay < 0 or factor < 0 or max_delay < 0:
        print(json.dumps({"error": "base_delay, factor and max_delay must be non-negative",
                          "example": example}))
        return 0

    try:
        schedule = []
        total_max = 0.0
        for i in range(max_retries):
            d = base_delay_for(strategy, i, base_delay, factor, max_delay)
            d = round(d, 4)
            entry = {"attempt": i + 1, "delay_seconds": d}
            if jitter == "full":
                # random_between(0, d): worst case is d
                entry["delay_range"] = [0.0, d]
                total_max += d
            elif jitter == "equal":
                # d/2 + random_between(0, d/2): range [d/2, d]
                entry["delay_range"] = [round(d / 2.0, 4), d]
                total_max += d
            else:
                total_max += d
            schedule.append(entry)

        result = {
            "strategy": strategy,
            "jitter": jitter,
            "params": {
                "max_retries": max_retries,
                "base_delay": base_delay,
                "factor": factor,
                "max_delay": max_delay,
            },
            "schedule": schedule,
            "total_max_wait_seconds": round(total_max, 4),
            "note": ("delay_seconds is the un-jittered target; with jitter the actual wait is "
                     "drawn from delay_range. Use jitter (full or equal) to spread retries and "
                     "avoid a thundering herd where many clients retry in lockstep. "
                     "total_max_wait_seconds is the worst-case sum, not the expected wait."),
        }
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "retry_backoff failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
