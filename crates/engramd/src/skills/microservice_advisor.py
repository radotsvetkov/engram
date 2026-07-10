#!/usr/bin/env python3
"""microservice_advisor — Engram skill (no network). Monolith vs. microservices.

Scores three architectures — monolith, modular-monolith, and microservices —
against your team size, domain count, expected scale, and deploy-independence
needs using well-worn heuristics. It deliberately leans toward "monolith first":
most teams should start simple and extract services only when a concrete
pressure demands it. Advisory, not prescriptive. Stdlib only.

Request (stdin): {"team_size": 6, "num_domains": 3, "expected_scale": "medium",
                  "deploy_independence_needed": false, "current_pain": ["slow builds"]}
Output (stdout): {recommendation, scores, reasoning, tradeoffs, migration_note}
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
            "example": {"team_size": 6, "num_domains": 3,
                        "expected_scale": "medium"},
        }))
        return 0

    try:
        team_size = q.get("team_size")
        num_domains = q.get("num_domains")
        scale = str(q.get("expected_scale") or "medium").strip().lower()
        deploy_indep = bool(q.get("deploy_independence_needed", False))
        current_pain = q.get("current_pain") or []
        if not isinstance(current_pain, list):
            current_pain = [str(current_pain)]

        if scale not in ("low", "medium", "high"):
            scale = "medium"

        team_size = int(team_size) if team_size is not None else None
        num_domains = int(num_domains) if num_domains is not None else None

        scores = {"monolith": 0, "modular-monolith": 0, "microservices": 0}
        reasoning = []

        # --- team size ---
        if team_size is not None:
            if team_size <= 3:
                scores["monolith"] += 3
                scores["modular-monolith"] += 1
                reasoning.append(
                    "Team of %d is small: a monolith keeps coordination and ops "
                    "overhead lowest." % team_size)
            elif team_size <= 10:
                scores["modular-monolith"] += 3
                scores["monolith"] += 1
                reasoning.append(
                    "Team of %d fits a modular-monolith well — clear internal "
                    "module boundaries without distributed-systems overhead."
                    % team_size)
            else:
                scores["microservices"] += 2
                scores["modular-monolith"] += 1
                reasoning.append(
                    "Team of %d is large enough that independent service "
                    "ownership (a service per team) starts to pay off."
                    % team_size)

        # --- domains ---
        if num_domains is not None:
            if num_domains <= 2:
                scores["monolith"] += 2
                reasoning.append(
                    "Only %d bounded domain(s): little to split apart yet."
                    % num_domains)
            elif num_domains <= 5:
                scores["modular-monolith"] += 2
                reasoning.append(
                    "%d domains map cleanly to modules inside one deployable."
                    % num_domains)
            else:
                scores["microservices"] += 2
                reasoning.append(
                    "%d+ distinct domains suggest genuine service boundaries."
                    % num_domains)

        # --- scale ---
        if scale == "low":
            scores["monolith"] += 2
            reasoning.append("Low expected scale: a single deployable is plenty.")
        elif scale == "medium":
            scores["modular-monolith"] += 2
            reasoning.append(
                "Medium scale: a modular-monolith scales vertically and by "
                "read replicas before you need service-level isolation.")
        else:  # high
            scores["microservices"] += 3
            scores["modular-monolith"] += 1
            reasoning.append(
                "High expected scale: independently scaling hot paths is a "
                "real microservices advantage.")

        # --- deploy independence ---
        if deploy_indep:
            scores["microservices"] += 3
            reasoning.append(
                "Independent deploy cadence per team is the single strongest "
                "reason to adopt microservices.")
        else:
            scores["monolith"] += 1
            scores["modular-monolith"] += 1
            reasoning.append(
                "No hard need for independent deploys: one CI/CD pipeline is "
                "simpler and faster to reason about.")

        # --- current pain signals ---
        pain_text = " ".join(str(p) for p in current_pain).lower()
        if any(k in pain_text for k in ("deploy", "release", "coupl", "blast")):
            scores["microservices"] += 1
            scores["modular-monolith"] += 1
            reasoning.append(
                "Pain around coupling/deploys: enforce module boundaries first "
                "(modular-monolith), extract services only where the seam is "
                "already clean.")
        if any(k in pain_text for k in ("build", "slow", "test", "startup")):
            scores["modular-monolith"] += 1
            reasoning.append(
                "Pain around slow builds/tests: modularizing the codebase often "
                "fixes this without a distributed system.")
        if any(k in pain_text for k in ("scale", "throughput", "latency", "load")):
            scores["microservices"] += 1
            reasoning.append(
                "Pain around scale/throughput: isolate and scale the hot path — "
                "but measure first; it may be one query, not the architecture.")

        # tie-break bias toward the pragmatic middle
        scores["modular-monolith"] += 1

        recommendation = max(scores, key=lambda k: scores[k])

        tradeoffs = {
            "monolith": {
                "pros": ["simplest to build, test, deploy, and debug",
                         "no network/serialization/distributed-txn overhead",
                         "one codebase, easy refactors across boundaries"],
                "cons": ["one deploy pipeline couples all teams",
                         "scales as one unit",
                         "boundaries erode without discipline"],
            },
            "modular-monolith": {
                "pros": ["enforced internal boundaries, single deployable",
                         "easy to extract a module into a service later",
                         "keeps ops simple while teams grow"],
                "cons": ["still one deploy unit and one runtime to scale",
                         "requires discipline to keep modules decoupled"],
            },
            "microservices": {
                "pros": ["independent deploy and scaling per service",
                         "team autonomy and fault isolation",
                         "polyglot / per-service tech choices"],
                "cons": ["distributed-systems complexity (networking, retries, "
                         "eventual consistency, tracing)",
                         "heavy ops/platform investment (CI/CD, observability)",
                         "premature splitting hard-codes wrong boundaries"],
            },
        }

        result = {
            "recommendation": recommendation,
            "scores": scores,
            "reasoning": reasoning or [
                "Too few inputs to differentiate strongly — defaulting to the "
                "pragmatic middle."],
            "tradeoffs": tradeoffs,
            "migration_note": (
                "Prefer 'monolith first' (Martin Fowler): start with a "
                "well-modularized monolith, keep bounded contexts clean, and "
                "extract a microservice only when a concrete, measured pressure "
                "(independent scaling, deploy cadence, team autonomy, fault "
                "isolation) justifies the distributed-systems tax. A "
                "modular-monolith makes that later extraction cheap."),
        }
        print(json.dumps(result, indent=2, default=str))
        return 0
    except (TypeError, ValueError) as e:
        print(json.dumps({
            "error": "invalid input: %s" % e,
            "example": {"team_size": 6, "num_domains": 3,
                        "expected_scale": "medium"},
        }))
        return 0
    except Exception as e:
        print(json.dumps({"error": "microservice_advisor failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
