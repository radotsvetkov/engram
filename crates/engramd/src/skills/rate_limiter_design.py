#!/usr/bin/env python3
"""rate_limiter_design — Engram skill (no network). Design a rate limiter.

Turns a rate spec (requests per window) into concrete token-bucket parameters —
refill rate and bucket capacity — and compares the four common algorithms
(token bucket, leaky bucket, sliding window, fixed window) with their burst
behavior. Emits ready-to-adapt pseudocode for the token-bucket check. Stdlib
only.

Request (stdin): {"requests_per_window": 100, "window_seconds": 60,
                  "algorithm": "token_bucket", "burst": 150}
Output (stdout): {algorithm, refill_rate_per_sec, capacity, allows_burst,
                  comparison, pseudocode}
"""
import json
import sys

ALGORITHMS = {
    "token_bucket": {
        "how": "Tokens refill at a fixed rate up to a capacity; each request "
               "spends one token, allowed only if a token is available.",
        "allows_burst": True,
        "burst": "Bursts up to the bucket capacity, then throttles to the "
                 "refill rate. Smooth and the usual default.",
        "cost": "O(1) memory per key; needs last-refill timestamp + token count.",
    },
    "leaky_bucket": {
        "how": "Requests enter a fixed-size queue and drain (leak) at a "
               "constant rate; overflow is rejected.",
        "allows_burst": False,
        "burst": "Enforces a smooth, constant output rate — no bursts pass "
                 "through; excess is queued or dropped.",
        "cost": "O(1) counter (or a bounded queue) per key.",
    },
    "sliding_window": {
        "how": "Counts requests over a rolling time window (log of timestamps "
               "or a weighted count of the current+previous fixed window).",
        "allows_burst": False,
        "burst": "Most accurate; avoids the fixed-window edge burst. Log "
                 "variant is exact but O(N) memory; counter variant approximates.",
        "cost": "O(N) for the log variant; O(1) for the sliding-counter "
                "approximation.",
    },
    "fixed_window": {
        "how": "A counter per fixed calendar window (e.g. per minute) that "
               "resets at the boundary.",
        "allows_burst": True,
        "burst": "Simplest, but allows up to 2x the limit across a window "
                 "boundary (burst at the end of one window + start of the next).",
        "cost": "O(1) counter per key; cheapest and easiest.",
    },
}

PSEUDOCODE = """// Token bucket check (per client key), O(1):
function allow(key, now):
    b = store.get(key) or { tokens: CAPACITY, last: now }
    // refill for elapsed time, capped at CAPACITY
    elapsed = now - b.last
    b.tokens = min(CAPACITY, b.tokens + elapsed * REFILL_RATE_PER_SEC)
    b.last   = now
    if b.tokens >= 1:
        b.tokens -= 1
        store.set(key, b)
        return ALLOW
    else:
        store.set(key, b)
        return DENY   // return 429 + Retry-After = (1 - b.tokens) / REFILL_RATE_PER_SEC
"""


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e}))
        return 0

    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"requests_per_window": 100, "window_seconds": 60,
                        "algorithm": "token_bucket"},
        }))
        return 0

    algorithm = str(q.get("algorithm") or "token_bucket").strip().lower()
    if algorithm not in ALGORITHMS:
        print(json.dumps({
            "error": "'algorithm' must be one of %s" % list(ALGORITHMS.keys()),
            "example": {"requests_per_window": 100, "window_seconds": 60,
                        "algorithm": "token_bucket"},
        }))
        return 0

    try:
        rpw = q.get("requests_per_window")
        window = q.get("window_seconds", 60)
        burst = q.get("burst")

        window = float(window if window is not None else 60)
        if window <= 0:
            print(json.dumps({
                "error": "'window_seconds' must be positive",
                "example": {"requests_per_window": 100, "window_seconds": 60},
            }))
            return 0

        result = {
            "algorithm": algorithm,
            "how_it_works": ALGORITHMS[algorithm]["how"],
            "allows_burst": ALGORITHMS[algorithm]["allows_burst"],
            "window_seconds": window,
        }

        if rpw is not None:
            rpw = float(rpw)
            if rpw <= 0:
                print(json.dumps({
                    "error": "'requests_per_window' must be positive",
                    "example": {"requests_per_window": 100,
                                "window_seconds": 60},
                }))
                return 0

            refill_rate = rpw / window
            if burst is not None:
                burst = float(burst)
                if burst <= 0:
                    print(json.dumps({
                        "error": "'burst' must be positive",
                        "example": {"requests_per_window": 100, "burst": 150},
                    }))
                    return 0
                capacity = burst
            else:
                capacity = rpw

            result["requests_per_window"] = rpw
            result["refill_rate_per_sec"] = round(refill_rate, 6)
            result["capacity"] = round(capacity, 6)
            result["effective_burst_allowance"] = round(capacity, 6)
            result["note"] = (
                "refill_rate = requests_per_window / window_seconds = %g/s. "
                "capacity = burst (%s) — the max tokens/requests allowed in an "
                "instantaneous spike before throttling to the refill rate."
                % (refill_rate,
                   "given" if q.get("burst") is not None else "defaulted to "
                   "requests_per_window"))
        else:
            result["note"] = ("Provide 'requests_per_window' to compute "
                              "refill_rate and capacity.")

        result["comparison"] = {
            name: {
                "allows_burst": info["allows_burst"],
                "burst_behavior": info["burst"],
                "cost": info["cost"],
            }
            for name, info in ALGORITHMS.items()
        }
        result["pseudocode"] = PSEUDOCODE

        print(json.dumps(result, indent=2, default=str))
        return 0
    except (TypeError, ValueError) as e:
        print(json.dumps({
            "error": "invalid numeric input: %s" % e,
            "example": {"requests_per_window": 100, "window_seconds": 60,
                        "algorithm": "token_bucket", "burst": 150},
        }))
        return 0
    except Exception as e:
        print(json.dumps({"error": "rate_limiter_design failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
