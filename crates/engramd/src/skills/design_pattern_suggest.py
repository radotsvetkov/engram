#!/usr/bin/env python3
"""design_pattern_suggest — Engram skill (no network). Suggests GoF design patterns.

Heuristically matches keyword/phrase signals in a free-text problem
description against a built-in table of classic Gang-of-Four object-oriented
design patterns. This is a simple substring-matching heuristic, not a deep
analysis — it's meant to point you toward candidates worth reading up on, not
to be an authoritative recommendation.

Request (stdin): {"problem_description": "I need to notify multiple UI widgets whenever data changes"}
Output (stdout): {matched_patterns: [{pattern, why_it_fits, one_line_description,
caveat}], match_count}
"""
import json
import sys

# (pattern name, signal phrases, one-line description, caveat-or-None)
PATTERN_TABLE = [
    ("Factory Method or Abstract Factory",
     ["create objects", "instantiate", "different types of"],
     "Delegates the creation of related objects to subclasses or a factory "
     "method/object, so calling code doesn't need to know concrete classes.",
     None),
    ("Singleton",
     ["one instance", "single instance", "global access"],
     "Ensures a class has only one instance and provides a global point of "
     "access to it.",
     "Singleton is often considered an anti-pattern in modern code — it "
     "introduces global mutable state and hidden dependencies that make unit "
     "testing harder. Prefer dependency injection where feasible."),
    ("Observer",
     ["notify", "subscribe", "event", "listeners"],
     "Defines a one-to-many dependency so that when one object changes "
     "state, all its dependents are notified automatically.",
     None),
    ("Strategy",
     ["interchangeable algorithms", "switch behavior at runtime", "different algorithms"],
     "Defines a family of interchangeable algorithms and lets the algorithm "
     "vary independently from the clients that use it.",
     None),
    ("Decorator",
     ["add responsibilities dynamically", "wrap", "extend behavior without subclassing"],
     "Attaches additional responsibilities to an object dynamically — a "
     "flexible alternative to subclassing for extending behavior.",
     None),
    ("Adapter",
     ["incompatible interfaces", "adapt", "wrap a legacy"],
     "Converts the interface of a class into another interface clients "
     "expect, letting otherwise-incompatible interfaces work together.",
     None),
    ("Builder",
     ["build complex object step by step", "many optional parameters"],
     "Separates the construction of a complex object from its "
     "representation, so the same construction process can create different "
     "representations.",
     None),
    ("Command",
     ["undo", "redo", "command history", "queue actions"],
     "Encapsulates a request as an object, letting you parameterize clients "
     "with queues, requests, and operations, and support undoable actions.",
     None),
    ("Iterator",
     ["traverse a collection"],
     "Provides a way to access the elements of a collection sequentially "
     "without exposing its underlying representation.",
     None),
    ("State",
     ["state changes behavior", "state machine"],
     "Allows an object to alter its behavior when its internal state "
     "changes, appearing to change its class.",
     None),
]


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e}))
        return 0

    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"problem_description": "I need to notify multiple UI "
                                                 "widgets whenever data changes"},
        }))
        return 0

    problem_description = q.get("problem_description")
    if not problem_description or not str(problem_description).strip():
        print(json.dumps({
            "error": "missing required field: problem_description",
            "example": {"problem_description": "I need to notify multiple UI "
                                                 "widgets whenever data changes"},
        }))
        return 0
    problem_description = str(problem_description).strip()

    try:
        haystack = problem_description.lower()
        matched_patterns = []
        for pattern, signals, description, caveat in PATTERN_TABLE:
            hits = [s for s in signals if s in haystack]
            if not hits:
                continue
            matched_patterns.append({
                "pattern": pattern,
                "why_it_fits": "matched signal phrase(s): " + ", ".join(
                    '"%s"' % h for h in hits),
                "one_line_description": description,
                "caveat": caveat,
            })

        if not matched_patterns:
            result = {
                "matched_patterns": [],
                "match_count": 0,
                "note": "No strong design-pattern signal was detected in the "
                        "problem description. Rather than guessing at random, "
                        "try describing the problem in terms of what varies/"
                        "changes vs. what stays fixed — that's the core "
                        "heuristic for picking an OOP design pattern (e.g. "
                        "'the algorithm varies but the surrounding steps stay "
                        "the same', 'the number of instances must stay at "
                        "one', 'many objects need to react when one object "
                        "changes').",
            }
        else:
            result = {
                "matched_patterns": matched_patterns,
                "match_count": len(matched_patterns),
            }

        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "design_pattern_suggest failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
