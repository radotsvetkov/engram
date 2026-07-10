#!/usr/bin/env python3
"""queue_sizing — Engram skill (no network). Little's Law + utilization.

Applies Little's Law (L = lambda * W) to a queue/service: given any solvable
subset of arrival rate, average time-in-system, and number-in-system, it derives
the rest, then computes server utilization rho = lambda * service_time / servers
and warns when the system is saturated (rho >= 1, unstable) or hot (rho > 0.8).
Pure arithmetic, stdlib only.

Request (stdin): {"arrival_rate_per_sec": 10, "avg_service_time_sec": 0.5,
                  "servers": 8, "avg_wait_time_sec": 0.5}
Output (stdout): {computed: {...}, utilization, stable, warnings, note}
"""
import json
import sys


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e}))
        return 0

    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"arrival_rate_per_sec": 10, "avg_service_time_sec": 0.5,
                        "servers": 8},
        }))
        return 0

    try:
        def num(key):
            v = q.get(key)
            return None if v is None else float(v)

        lam = num("arrival_rate_per_sec")     # lambda
        svc = num("avg_service_time_sec")      # service time per request
        servers = num("servers")
        wait = num("avg_wait_time_sec")        # W (time in system)
        length = num("queue_length")           # L (number in system)

        for name, v in (("arrival_rate_per_sec", lam),
                        ("avg_service_time_sec", svc),
                        ("servers", servers),
                        ("avg_wait_time_sec", wait),
                        ("queue_length", length)):
            if v is not None and v < 0:
                print(json.dumps({
                    "error": "'%s' must be non-negative" % name,
                    "example": {"arrival_rate_per_sec": 10,
                                "avg_service_time_sec": 0.5, "servers": 8},
                }))
                return 0

        derivations = []

        # Little's Law: L = lambda * W. Solve for the missing one.
        if length is None and lam is not None and wait is not None:
            length = lam * wait
            derivations.append("queue_length = arrival_rate * avg_wait_time "
                               "(Little's Law)")
        elif lam is None and length is not None and wait not in (None, 0):
            lam = length / wait
            derivations.append("arrival_rate = queue_length / avg_wait_time "
                               "(Little's Law)")
        elif wait is None and length is not None and lam not in (None, 0):
            wait = length / lam
            derivations.append("avg_wait_time = queue_length / arrival_rate "
                               "(Little's Law)")

        computed = {}
        if lam is not None:
            computed["arrival_rate_per_sec"] = round(lam, 6)
        if svc is not None:
            computed["avg_service_time_sec"] = round(svc, 6)
        if servers is not None:
            computed["servers"] = round(servers, 6)
        if wait is not None:
            computed["avg_wait_time_sec"] = round(wait, 6)
        if length is not None:
            computed["queue_length"] = round(length, 6)

        # throughput / service rate per server = 1/service_time
        if svc not in (None, 0):
            computed["service_rate_per_server_per_sec"] = round(1.0 / svc, 6)

        warnings = []
        utilization = None
        stable = None
        if lam is not None and svc is not None:
            n = servers if (servers is not None and servers > 0) else 1.0
            utilization = lam * svc / n
            computed["utilization"] = round(utilization, 6)
            computed["utilization_pct"] = round(utilization * 100.0, 4)
            stable = utilization < 1.0
            if utilization >= 1.0:
                warnings.append(
                    "UNSTABLE: utilization (rho) = %.3f >= 1. Arrivals exceed "
                    "service capacity — the queue grows without bound. Add "
                    "servers, cut service time, or shed load." % utilization)
            elif utilization > 0.8:
                warnings.append(
                    "HIGH: utilization (rho) = %.3f > 0.8. Queue wait grows "
                    "sharply as rho approaches 1 (M/M/1: Wq ~ rho/(1-rho)). "
                    "Leave headroom." % utilization)

        if not derivations and len(computed) <= 1:
            print(json.dumps({
                "error": "provide enough inputs to derive something — e.g. "
                         "arrival_rate + avg_wait_time (for queue_length), or "
                         "arrival_rate + avg_service_time + servers (for "
                         "utilization)",
                "example": {"arrival_rate_per_sec": 10,
                            "avg_service_time_sec": 0.5, "servers": 8},
            }))
            return 0

        result = {
            "computed": computed,
            "utilization": round(utilization, 6) if utilization is not None
            else None,
            "stable": stable,
            "derivations": derivations,
            "warnings": warnings,
            "note": "Little's Law L = lambda * W relates number-in-system, "
                    "arrival rate, and time-in-system for any stable queue. "
                    "utilization rho = lambda * service_time / servers must stay "
                    "below 1; keep it under ~0.7-0.8 for sane tail latency.",
        }
        print(json.dumps(result, indent=2, default=str))
        return 0
    except (TypeError, ValueError) as e:
        print(json.dumps({
            "error": "invalid numeric input: %s" % e,
            "example": {"arrival_rate_per_sec": 10, "avg_service_time_sec": 0.5,
                        "servers": 8},
        }))
        return 0
    except Exception as e:
        print(json.dumps({"error": "queue_sizing failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
