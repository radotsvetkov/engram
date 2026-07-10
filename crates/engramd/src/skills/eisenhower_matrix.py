#!/usr/bin/env python3
"""eisenhower_matrix — Engram skill (no network).

Sorts tasks into the Eisenhower urgent/important matrix. Each task carries
urgent + important as booleans, or as 1-10 numbers (>=6 counts as true).
Returns the four quadrants (do now / schedule / delegate / eliminate) with
a recommended action verb and counts.

Request (stdin): {"tasks": [{"name": str, "urgent": bool|number, "important": bool|number}]}
Output (stdout): {quadrants, counts, total, note}
"""
import json
import sys

THRESHOLD = 6  # for numeric urgency/importance, >= 6 counts as true

QUADRANT_META = {
    "Q1": {"label": "Do now", "urgent": True, "important": True, "action": "DO"},
    "Q2": {"label": "Schedule", "urgent": False, "important": True, "action": "SCHEDULE"},
    "Q3": {"label": "Delegate", "urgent": True, "important": False, "action": "DELEGATE"},
    "Q4": {"label": "Eliminate", "urgent": False, "important": False, "action": "ELIMINATE"},
}


def to_bool(v):
    """Interpret a bool or a 1-10 number as urgent/important true/false."""
    if isinstance(v, bool):
        return v
    if isinstance(v, (int, float)):
        return v >= THRESHOLD
    if isinstance(v, str):
        s = v.strip().lower()
        if s in ("true", "yes", "y", "high", "1"):
            return True
        if s in ("false", "no", "n", "low", "0"):
            return False
        try:
            return float(s) >= THRESHOLD
        except ValueError:
            return False
    return False


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0

    example = {"tasks": [
        {"name": "Fix prod outage", "urgent": True, "important": True},
        {"name": "Plan Q3 roadmap", "urgent": 3, "important": 9},
    ]}

    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object", "example": example}))
        return 0

    tasks = q.get("tasks")
    if not isinstance(tasks, list) or not tasks:
        print(json.dumps({"error": "missing required field 'tasks' (non-empty list of "
                          "{name, urgent, important})", "example": example}))
        return 0

    try:
        buckets = {k: [] for k in QUADRANT_META}
        for t in tasks:
            if not isinstance(t, dict) or "name" not in t:
                print(json.dumps({"error": "each task must be an object with a 'name'",
                                  "example": example}))
                return 0
            name = str(t.get("name")).strip()
            urgent = to_bool(t.get("urgent", False))
            important = to_bool(t.get("important", False))
            if urgent and important:
                key = "Q1"
            elif important and not urgent:
                key = "Q2"
            elif urgent and not important:
                key = "Q3"
            else:
                key = "Q4"
            buckets[key].append({"name": name, "urgent": urgent, "important": important})

        quadrants = {}
        counts = {}
        for key, meta in QUADRANT_META.items():
            quadrants[key] = {
                "label": meta["label"],
                "criteria": "urgent=%s, important=%s" % (meta["urgent"], meta["important"]),
                "action": meta["action"],
                "tasks": buckets[key],
            }
            counts[key] = len(buckets[key])

        result = {
            "quadrants": quadrants,
            "counts": counts,
            "total": len(tasks),
            "note": ("Q2 (Schedule — important, not urgent) is where high performers spend most "
                     "of their time; protect it. Q1 is firefighting, Q3 is interruptions to "
                     "delegate, Q4 is noise to eliminate. Numeric urgency/importance use a "
                     ">=%d threshold." % THRESHOLD),
        }
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "eisenhower_matrix failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
