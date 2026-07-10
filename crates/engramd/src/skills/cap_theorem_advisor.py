#!/usr/bin/env python3
"""cap_theorem_advisor — Engram skill (no network). Advise CP vs AP under the
CAP theorem for a distributed data store.

Network partitions are unavoidable in any distributed system, so the real choice
during a partition is Consistency vs Availability. Given your consistency /
availability needs this recommends CP or AP (or "tunable"), names representative
databases, and explains the trade-off. Guidance only — many engines let you tune
consistency per operation.

Request (stdin): {"needs_strong_consistency": true, "needs_high_availability": false,
  "tolerate_partition": true, "workload": "financial ledger"}
Output (stdout): {recommendation, example_databases, tradeoff, partition_note, notes, inputs}
"""
import json
import sys


def _as_bool(v, default=None):
    if isinstance(v, bool):
        return v
    if v is None:
        return default
    if isinstance(v, str):
        s = v.strip().lower()
        if s in ("true", "yes", "y", "1"):
            return True
        if s in ("false", "no", "n", "0"):
            return False
    if isinstance(v, (int, float)):
        return bool(v)
    return default


_CP_DBS = ["PostgreSQL (synchronous replication)", "MongoDB (majority write/read concern)",
           "etcd", "ZooKeeper", "HBase", "Google Spanner / CockroachDB (externally consistent)"]
_AP_DBS = ["Apache Cassandra", "Amazon DynamoDB (eventually consistent reads)", "Riak",
           "Couchbase", "Amazon Aurora read replicas (stale reads)", "Voldemort"]
_TUNABLE_DBS = ["Cassandra (per-query consistency level)", "DynamoDB (strong vs eventual reads)",
                "MongoDB (write/read concern)", "CockroachDB (follower reads)"]


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e}))
        return 0
    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"needs_strong_consistency": True, "needs_high_availability": False,
                        "workload": "financial ledger"},
        }))
        return 0

    needs_consistency = _as_bool(q.get("needs_strong_consistency"), None)
    needs_availability = _as_bool(q.get("needs_high_availability"), None)
    tolerate_partition = _as_bool(q.get("tolerate_partition"), True)
    workload = q.get("workload")
    if not isinstance(workload, str):
        workload = None

    try:
        partition_note = (
            "The CAP theorem says that when a network partition (P) occurs, a distributed "
            "system must sacrifice either Consistency (C) or Availability (A) — you cannot "
            "keep both. Partitions WILL happen (dropped packets, network splits, node "
            "failures), so 'tolerating partitions' is not optional; the choice is what to "
            "give up WHEN one happens.")

        notes = [
            "Many modern databases offer TUNABLE consistency per operation (e.g. Cassandra "
            "consistency levels, DynamoDB strong vs eventual reads, MongoDB write/read "
            "concern) — so 'CP vs AP' is often a per-query decision, not a whole-system one.",
            "When there is NO partition, a well-designed system delivers both consistency "
            "and availability; CAP only forces the trade-off during a partition (see also "
            "PACELC, which adds the latency-vs-consistency trade-off in the normal case).",
        ]

        if tolerate_partition is False:
            notes.insert(0, "You asked to NOT tolerate partitions, but partitions are "
                            "unavoidable in a genuinely distributed system. If you truly "
                            "cannot tolerate partitions, consider a single-node (non-"
                            "distributed) database — then CAP does not force a trade-off.")

        # Decide.
        if needs_consistency and needs_availability:
            recommendation = "tunable"
            example_databases = _TUNABLE_DBS
            tradeoff = (
                "You want BOTH strong consistency and high availability — CAP says you can't "
                "have both during a partition. Pick a database with tunable consistency and "
                "choose per-operation: strong/quorum reads+writes for the paths that must be "
                "correct (money, inventory), relaxed/eventual for the paths that must stay up "
                "(feeds, view counts). Or lean CP and add redundancy to shrink partition risk.")
        elif needs_consistency:
            recommendation = "CP"
            example_databases = _CP_DBS
            tradeoff = (
                "Prioritising Consistency: during a partition the system will REJECT or block "
                "requests it can't serve consistently (reduced availability) rather than return "
                "stale/conflicting data. Right for ledgers, inventory, bookings, config/"
                "coordination — anywhere a wrong answer is worse than no answer.")
        elif needs_availability:
            recommendation = "AP"
            example_databases = _AP_DBS
            tradeoff = (
                "Prioritising Availability: during a partition every node keeps serving reads/"
                "writes and reconciles later (eventual consistency, possible conflicts to "
                "merge). Right for shopping carts, feeds, telemetry, sessions, caches — where "
                "staying up matters more than every replica agreeing instantly.")
        else:
            recommendation = "tunable"
            example_databases = _TUNABLE_DBS
            tradeoff = (
                "No hard consistency or availability requirement was specified. Default to a "
                "database with tunable consistency so you can decide per-operation. As a rule "
                "of thumb: choose CP when a wrong answer is worse than an error (money, "
                "inventory), AP when downtime is worse than staleness (feeds, carts, caches).")
            notes.insert(0, "Neither needs_strong_consistency nor needs_high_availability was "
                            "set — describe the workload's tolerance for stale data vs downtime "
                            "for a sharper recommendation.")

        result = {
            "recommendation": recommendation,
            "example_databases": example_databases,
            "tradeoff": tradeoff,
            "partition_note": partition_note,
            "notes": notes,
            "inputs": {
                "needs_strong_consistency": needs_consistency,
                "needs_high_availability": needs_availability,
                "tolerate_partition": tolerate_partition,
                "workload": workload,
            },
        }
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "cap_theorem_advisor failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
