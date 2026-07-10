#!/usr/bin/env python3
"""solid_principles_advisor — Engram skill (no network). Produce a SOLID design
checklist for a described class/module, with light heuristic flags.

For each of the five SOLID principles it returns a guiding question, an
optional heuristic_flag (raised from simple signals — many methods or
"and"/"also" in the description for SRP, a type switch for OCP, many
dependencies or mentions of concrete classes for DIP, etc.), and a
suggestion. This is a heuristic checklist, not a static analysis.

Request (stdin): {"description": "Handles user auth and also sends emails", "num_methods": 18, "num_dependencies": 6, "has_type_switch": true}
Output (stdout): {principles: [{key, name, question, heuristic_flag, suggestion}], flags_raised, caveat}
"""
import json
import re
import sys

_EXAMPLE = {
    "description": "Handles user auth and also sends emails and logs to a MySQL database",
    "num_methods": 18,
    "num_dependencies": 6,
    "has_type_switch": True,
}


def _as_int(v):
    try:
        return int(v)
    except (TypeError, ValueError):
        return None


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e}))
        return 0
    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object", "example": _EXAMPLE}))
        return 0

    description = q.get("description")
    if not isinstance(description, str) or not description.strip():
        print(json.dumps({
            "error": "missing required field 'description' (non-empty string describing the class/module)",
            "example": _EXAMPLE,
        }))
        return 0

    num_methods = _as_int(q.get("num_methods"))
    num_dependencies = _as_int(q.get("num_dependencies"))
    has_type_switch = bool(q.get("has_type_switch", False))

    try:
        desc = description.lower()
        multi_resp = bool(re.search(r"\b(and also|also|as well as|in addition)\b", desc)) or desc.count(" and ") >= 2
        mentions_concrete = bool(re.search(
            r"\b(mysql|postgres|sqlite|redis|mongo|new [A-Z]|concrete|specific class|filesystem|http client|smtp)\b",
            desc))
        mentions_bigiface = bool(re.search(r"\b(one big|fat interface|god|many methods|do everything)\b", desc))

        principles = []

        # S — Single Responsibility
        srp_flag = None
        if (num_methods is not None and num_methods > 12) or multi_resp:
            reasons = []
            if num_methods is not None and num_methods > 12:
                reasons.append("%d methods" % num_methods)
            if multi_resp:
                reasons.append("description lists multiple responsibilities")
            srp_flag = "Possible SRP violation: " + "; ".join(reasons) + "."
        principles.append({
            "key": "S",
            "name": "Single Responsibility Principle",
            "question": "Does this class have exactly one reason to change (one responsibility)?",
            "heuristic_flag": srp_flag,
            "suggestion": "Split unrelated responsibilities into separate classes; a class should "
                          "answer to a single actor/stakeholder.",
        })

        # O — Open/Closed
        ocp_flag = None
        if has_type_switch:
            ocp_flag = "A type switch was reported: adding a new case means editing this class."
        principles.append({
            "key": "O",
            "name": "Open/Closed Principle",
            "question": "Can you add new behavior without modifying existing, tested code?",
            "heuristic_flag": ocp_flag,
            "suggestion": "Replace type-switching with polymorphism (a common interface + one class "
                          "per variant) so new cases are added by extension, not modification.",
        })

        # L — Liskov Substitution
        principles.append({
            "key": "L",
            "name": "Liskov Substitution Principle",
            "question": "Can any subtype replace its base type without breaking callers' expectations?",
            "heuristic_flag": None,
            "suggestion": "Avoid overrides that weaken postconditions, strengthen preconditions, or "
                          "throw 'not supported' — that signals a broken inheritance hierarchy.",
        })

        # I — Interface Segregation
        isp_flag = None
        if (num_methods is not None and num_methods > 15) or mentions_bigiface:
            isp_flag = "Interface may be too broad; clients could be forced to depend on methods they don't use."
        principles.append({
            "key": "I",
            "name": "Interface Segregation Principle",
            "question": "Are clients forced to depend on methods they never call?",
            "heuristic_flag": isp_flag,
            "suggestion": "Break wide interfaces into smaller, role-specific ones so each client "
                          "depends only on what it uses.",
        })

        # D — Dependency Inversion
        dip_flag = None
        if (num_dependencies is not None and num_dependencies > 4) or mentions_concrete:
            reasons = []
            if num_dependencies is not None and num_dependencies > 4:
                reasons.append("%d dependencies" % num_dependencies)
            if mentions_concrete:
                reasons.append("mentions concrete/low-level collaborators")
            dip_flag = "High-level module may depend on concretions: " + "; ".join(reasons) + "."
        principles.append({
            "key": "D",
            "name": "Dependency Inversion Principle",
            "question": "Do high-level modules depend on abstractions rather than concrete implementations?",
            "heuristic_flag": dip_flag,
            "suggestion": "Depend on interfaces/abstractions and inject concrete implementations "
                          "(constructor injection) instead of instantiating them inside the class.",
        })

        flags_raised = sum(1 for p in principles if p["heuristic_flag"])
        result = {
            "principles": principles,
            "flags_raised": flags_raised,
            "caveat": "These flags are lightweight heuristics from the description and counts you "
                      "provided, not a static analysis of real code. Use them as prompts for review, "
                      "not as verdicts.",
        }
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "solid_principles_advisor failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
