#!/usr/bin/env python3
"""latency_budget — Engram skill (no network). Split a latency budget across hops.

Two modes. Given measured per-component latencies, it sums them, reports what's
left of the budget, each component's share, whether you're over budget, and the
bottleneck (any hop >40% of the total). Given percentage allocations instead, it
divides the budget across the components. Serial latencies add up. Stdlib only.

Request (stdin): {"total_budget_ms": 200, "components": [{"name":"db","latency_ms":120},
                  {"name":"api","latency_ms":40}]}
                 {"total_budget_ms": 200, "components": [{"name":"db","pct":60}]}
Output (stdout): {mode, total_budget_ms, used_ms/allocated, remaining_ms,
                  over_budget, bottleneck, components, note}
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
            "example": {"total_budget_ms": 200,
                        "components": [{"name": "db", "latency_ms": 120}]},
        }))
        return 0

    budget = q.get("total_budget_ms")
    components = q.get("components")

    if budget is None:
        print(json.dumps({
            "error": "missing required field: total_budget_ms",
            "example": {"total_budget_ms": 200,
                        "components": [{"name": "db", "latency_ms": 120}]},
        }))
        return 0
    if not isinstance(components, list) or len(components) == 0:
        print(json.dumps({
            "error": "'components' must be a non-empty list",
            "example": {"total_budget_ms": 200,
                        "components": [{"name": "db", "latency_ms": 120},
                                       {"name": "api", "latency_ms": 40}]},
        }))
        return 0

    try:
        budget = float(budget)
        if budget <= 0:
            print(json.dumps({
                "error": "'total_budget_ms' must be positive",
                "example": {"total_budget_ms": 200,
                            "components": [{"name": "db", "latency_ms": 120}]},
            }))
            return 0

        has_latency = any("latency_ms" in c for c in components
                          if isinstance(c, dict))
        has_pct = any("pct" in c for c in components if isinstance(c, dict))

        if has_latency:
            mode = "measured"
            out_components = []
            used = 0.0
            for i, c in enumerate(components):
                if not isinstance(c, dict):
                    raise ValueError("each component must be an object")
                name = str(c.get("name") or ("component_%d" % (i + 1)))
                lat = float(c.get("latency_ms", 0) or 0)
                used += lat
                out_components.append({"name": name, "latency_ms": round(lat, 3)})
            # per-component share of the summed latency
            for c in out_components:
                share = (c["latency_ms"] / used * 100.0) if used > 0 else 0.0
                c["pct_of_total"] = round(share, 2)
                c["is_bottleneck"] = share > 40.0
            remaining = budget - used
            bottlenecks = [c["name"] for c in out_components if c["is_bottleneck"]]
            result = {
                "mode": mode,
                "total_budget_ms": round(budget, 3),
                "used_ms": round(used, 3),
                "remaining_ms": round(remaining, 3),
                "over_budget": used > budget,
                "bottleneck": bottlenecks,
                "components": out_components,
                "note": "Serial latencies add up: total = sum of each hop. Any "
                        "hop over 40% of the total is flagged as a bottleneck; "
                        "shave it first.",
            }

        elif has_pct:
            mode = "allocation"
            out_components = []
            total_pct = 0.0
            for i, c in enumerate(components):
                if not isinstance(c, dict):
                    raise ValueError("each component must be an object")
                name = str(c.get("name") or ("component_%d" % (i + 1)))
                pct = float(c.get("pct", 0) or 0)
                total_pct += pct
                allocated = budget * pct / 100.0
                out_components.append({
                    "name": name,
                    "pct": round(pct, 3),
                    "allocated_ms": round(allocated, 3),
                })
            for c in out_components:
                c["is_bottleneck"] = c["pct"] > 40.0
            allocated_sum = budget * total_pct / 100.0
            bottlenecks = [c["name"] for c in out_components if c["is_bottleneck"]]
            result = {
                "mode": mode,
                "total_budget_ms": round(budget, 3),
                "allocated_ms": round(allocated_sum, 3),
                "remaining_ms": round(budget - allocated_sum, 3),
                "total_pct": round(total_pct, 3),
                "over_budget": total_pct > 100.0,
                "bottleneck": bottlenecks,
                "components": out_components,
                "note": "Serial latencies add up: allocations should sum to "
                        "<=100% of the budget. Any hop over 40% is flagged as a "
                        "bottleneck.",
            }
        else:
            print(json.dumps({
                "error": "each component needs either 'latency_ms' (measured "
                         "mode) or 'pct' (allocation mode)",
                "example": {"total_budget_ms": 200,
                            "components": [{"name": "db", "latency_ms": 120}]},
            }))
            return 0

        print(json.dumps(result, indent=2, default=str))
        return 0
    except (TypeError, ValueError) as e:
        print(json.dumps({"error": "invalid input: %s" % e}))
        return 0
    except Exception as e:
        print(json.dumps({"error": "latency_budget failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
