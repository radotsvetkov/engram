#!/usr/bin/env python3
"""load_balance_advisor — Engram skill (no network). Pick a load-balancing algo.

Recommends a load-balancing algorithm from your workload shape: uniform
stateless requests favor round-robin; variable request cost favors
least-connections; sticky sessions favor consistent-hash / ip-hash; heterogeneous
backend capacity favors weighted round-robin. Also emits sane health-check
defaults. Heuristic advice, stdlib only.

Request (stdin): {"backends_stateful": false, "request_cost_uniform": true,
                  "session_affinity_needed": false, "backend_count": 4,
                  "health_check_needed": true}
Output (stdout): {recommended_algorithm, reasoning, health_check_config, notes}
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
            "example": {"request_cost_uniform": True,
                        "session_affinity_needed": False},
        }))
        return 0

    try:
        cost_uniform = q.get("request_cost_uniform", True)
        affinity = bool(q.get("session_affinity_needed", False))
        stateful = bool(q.get("backends_stateful", False))
        backend_count = q.get("backend_count")
        health_needed = q.get("health_check_needed", True)

        if backend_count is not None:
            backend_count = int(backend_count)
            if backend_count < 0:
                raise ValueError("'backend_count' must be non-negative")

        cost_uniform = bool(cost_uniform)
        health_needed = bool(health_needed)

        reasoning = []
        # Precedence: affinity first (correctness), then cost shape.
        if affinity or stateful:
            algorithm = "consistent-hash"
            reasoning.append(
                "Session affinity / stateful backends: hash on a stable key "
                "(session id or client IP) so a client keeps hitting the same "
                "backend. Consistent-hash minimizes reshuffling when backends "
                "are added/removed; ip-hash is a simpler variant.")
            alternatives = ["ip-hash (simpler, but uneven if clients share NAT)"]
        elif not cost_uniform:
            algorithm = "least-connections"
            reasoning.append(
                "Request cost varies (some requests far heavier than others): "
                "least-connections routes to the backend with the fewest "
                "in-flight requests, self-balancing around slow requests better "
                "than blind round-robin.")
            alternatives = ["least-response-time (if your LB supports it)",
                            "weighted-least-connections (if backends differ)"]
        else:
            algorithm = "round-robin"
            reasoning.append(
                "Uniform request cost and stateless backends: plain round-robin "
                "is the simplest fair distribution and is effectively free.")
            alternatives = ["weighted-round-robin if backend capacities differ",
                            "random (near-equivalent at scale, lock-free)"]

        # Heterogeneous capacity note bumps toward a weighted variant.
        if q.get("backends_heterogeneous"):
            reasoning.append(
                "Backends have differing capacity: prefer the *weighted* variant "
                "(weighted-round-robin or weighted-least-connections) and set "
                "weights proportional to each backend's capacity.")

        health_check_config = None
        if health_needed:
            health_check_config = {
                "type": "active",
                "path": "/healthz",
                "interval_seconds": 5,
                "timeout_seconds": 2,
                "healthy_threshold": 2,
                "unhealthy_threshold": 3,
                "note": "Mark a backend unhealthy after 3 consecutive failures; "
                        "return it after 2 consecutive successes. Tune interval "
                        "to detect failure fast without hammering backends.",
            }

        notes = [
            "Always drain connections before removing a backend on deploy "
            "(graceful shutdown + de-register, then wait for in-flight requests "
            "to finish) to avoid dropping live requests.",
            "Pair the LB with health checks and outlier detection so a sick "
            "backend is ejected automatically.",
            "For sticky sessions, prefer externalizing session state (shared "
            "cache/DB) so you can fall back to stateless algorithms and lose "
            "less on a backend failure.",
        ]
        if backend_count is not None and backend_count <= 1:
            notes.append(
                "With <=1 backend there is nothing to balance — the algorithm "
                "only matters once you scale out and add redundancy.")

        result = {
            "recommended_algorithm": algorithm,
            "alternatives": alternatives,
            "reasoning": reasoning,
            "health_check_config": health_check_config,
            "notes": notes,
        }
        print(json.dumps(result, indent=2, default=str))
        return 0
    except (TypeError, ValueError) as e:
        print(json.dumps({
            "error": "invalid input: %s" % e,
            "example": {"request_cost_uniform": True,
                        "session_affinity_needed": False, "backend_count": 4},
        }))
        return 0
    except Exception as e:
        print(json.dumps({"error": "load_balance_advisor failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
