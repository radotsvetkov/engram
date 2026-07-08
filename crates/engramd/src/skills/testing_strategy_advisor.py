#!/usr/bin/env python3
"""testing_strategy_advisor — Engram skill (no network). Recommend a
test-pyramid split (unit/integration/e2e percentages) for a project type,
optionally compare against the caller's current test counts, and always
return a fixed TDD checklist.

Request (stdin): {"project_type": "web_api", "current_test_counts": {"unit": 120, "integration": 15, "e2e": 40}}
Output (stdout): {project_type, assumed_default, recommended_pyramid, actual_ratio, comparison_notes, tdd_checklist}
"""
import json
import sys

_SUPPORTED = ["web_api", "frontend_spa", "cli_tool", "library"]

_PYRAMIDS = {
    "web_api": {"unit": 70, "integration": 20, "e2e": 10},
    "frontend_spa": {"unit": 60, "integration": 30, "e2e": 10},
    "cli_tool": {"unit": 80, "integration": 10, "e2e": 10},
    "library": {"unit": 90, "integration": 10, "e2e": 0},
}

_TDD_CHECKLIST = [
    "Write a failing test first that expresses the behavior you want, before writing implementation code.",
    "Keep the red-green-refactor cycle tight — small steps, run tests after every change.",
    "One assertion-concept per test so failures point at a single, obvious cause.",
    "Avoid testing implementation details or mocking everything — test observable behavior instead.",
    "Delete or update tests when behavior intentionally changes; don't let stale tests linger as false safety nets.",
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
            "example": {"project_type": "web_api", "current_test_counts": {"unit": 100, "integration": 20, "e2e": 5}},
        }))
        return 0

    project_type = q.get("project_type")
    assumed_default = False
    if project_type is None:
        project_type = "web_api"
        assumed_default = True
    if not isinstance(project_type, str) or project_type.strip().lower() not in _SUPPORTED:
        print(json.dumps({
            "error": "'project_type' must be one of: %s" % ", ".join(_SUPPORTED),
            "supported_project_types": _SUPPORTED,
        }))
        return 0
    project_type = project_type.strip().lower()

    counts = q.get("current_test_counts")
    if counts is not None and not isinstance(counts, dict):
        print(json.dumps({"error": "'current_test_counts' must be an object like {'unit': 10, 'integration': 5, 'e2e': 2} if provided"}))
        return 0

    try:
        recommended = dict(_PYRAMIDS[project_type])
        result = {
            "project_type": project_type,
            "assumed_default": assumed_default,
            "recommended_pyramid": {k: "%d%%" % v for k, v in recommended.items()},
            "tdd_checklist": _TDD_CHECKLIST,
        }
        if assumed_default:
            result["note"] = "no 'project_type' given — defaulting to 'web_api' reasoning"

        if counts:
            unit = counts.get("unit") or 0
            integration = counts.get("integration") or 0
            e2e = counts.get("e2e") or 0
            for label, val in (("unit", unit), ("integration", integration), ("e2e", e2e)):
                if not isinstance(val, (int, float)) or val < 0:
                    print(json.dumps({"error": "'current_test_counts.%s' must be a non-negative number" % label}))
                    return 0
            total = unit + integration + e2e
            comparison_notes = []
            if total == 0:
                result["actual_ratio"] = {"unit": "0%", "integration": "0%", "e2e": "0%"}
                comparison_notes.append("no tests counted yet — nothing to compare against the recommendation")
            else:
                actual = {
                    "unit": round(unit / total * 100, 1),
                    "integration": round(integration / total * 100, 1),
                    "e2e": round(e2e / total * 100, 1),
                }
                result["actual_ratio"] = {k: "%s%%" % v for k, v in actual.items()}

                if actual["e2e"] > recommended["e2e"] + 10:
                    comparison_notes.append(
                        "e2e tests are slow/flaky at scale — your e2e share (%.1f%%) is well above the "
                        "recommended %d%%; consider pushing coverage down to integration/unit."
                        % (actual["e2e"], recommended["e2e"])
                    )
                if actual["unit"] < recommended["unit"] - 15:
                    comparison_notes.append(
                        "unit tests look light — your unit share (%.1f%%) is well below the recommended "
                        "%d%%; unit tests are the cheapest to run and maintain, consider adding more."
                        % (actual["unit"], recommended["unit"])
                    )
                if actual["integration"] < recommended["integration"] - 15:
                    comparison_notes.append(
                        "integration coverage (%.1f%%) is well below the recommended %d%% — consider adding "
                        "tests that exercise real component boundaries (DB, HTTP, filesystem)."
                        % (actual["integration"], recommended["integration"])
                    )
                if not comparison_notes:
                    comparison_notes.append("actual ratio is roughly in line with the recommended pyramid")
            result["comparison_notes"] = comparison_notes

        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "testing_strategy_advisor failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
