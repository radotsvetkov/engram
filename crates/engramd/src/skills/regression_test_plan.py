#!/usr/bin/env python3
"""regression_test_plan — Engram skill (no network).

Builds a structured regression test plan from a change description and the
list of feature/module areas it touches. Each area gets a risk level (keyword
heuristic against critical terms like auth/payment/checkout/billing/security),
a templated test focus, and a suggested subset of test types. When more than
one area is affected, a cross-area risk note flags the need for an
integration-focused pass, since interactions between touched areas are the
real regression risk (not just each area in isolation).

Request (stdin): {"change_description": str, "affected_areas": [str]}
Output (stdout): {change_description: str,
  areas: [{area, risk_level, test_focus, suggested_test_types: [str]}],
  cross_area_risk_note: str|null}
"""
import json
import sys

HIGH_RISK_KEYWORDS = (
    "auth", "login", "password", "payment", "checkout", "billing",
    "security", "permission", "credential", "encrypt", "token", "session",
)

ALL_TEST_TYPES = [
    "smoke test", "functional regression", "integration test",
    "cross-browser check", "load test",
]


def _risk_level(area):
    # Match against the area's own name only — matching the shared,
    # whole-change description here would flag every area "high" just
    # because *some* area in the change is auth/payment-related.
    haystack = str(area).lower()
    if any(kw in haystack for kw in HIGH_RISK_KEYWORDS):
        return "high"
    return "medium"


def _test_focus(area, change_description):
    return (
        "Re-verify that existing '%s' behavior still works as expected after "
        "the change (\"%s\"), with particular attention to flows that "
        "previously depended on the code paths this change touches."
    ) % (area, change_description)


def _suggested_test_types(risk_level, multi_area):
    types = ["smoke test", "functional regression"]
    if multi_area:
        types.append("integration test")
    if risk_level == "high":
        types.append("load test")
    return types


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0

    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"change_description": "Reworked checkout payment flow",
                        "affected_areas": ["checkout", "billing"]},
        }))
        return 0

    change_description = q.get("change_description")
    affected_areas = q.get("affected_areas")

    if not change_description or not isinstance(change_description, str):
        print(json.dumps({
            "error": "'change_description' (string) is required",
            "example": {"change_description": "Reworked checkout payment flow",
                        "affected_areas": ["checkout", "billing"]},
        }))
        return 0

    if not affected_areas or not isinstance(affected_areas, list):
        print(json.dumps({
            "error": "'affected_areas' (non-empty list of strings) is required",
            "example": {"change_description": "Reworked checkout payment flow",
                        "affected_areas": ["checkout", "billing"]},
        }))
        return 0

    affected_areas = [str(a).strip() for a in affected_areas if str(a).strip()]
    if not affected_areas:
        print(json.dumps({"error": "'affected_areas' must contain at least one non-empty string"}))
        return 0

    try:
        multi_area = len(affected_areas) > 1
        areas = []
        for area in affected_areas:
            risk_level = _risk_level(area)
            areas.append({
                "area": area,
                "risk_level": risk_level,
                "test_focus": _test_focus(area, change_description),
                "suggested_test_types": _suggested_test_types(risk_level, multi_area),
            })

        cross_area_risk_note = None
        if multi_area:
            cross_area_risk_note = (
                "This change touches %d areas (%s). Changes spanning multiple "
                "areas carry interaction risk that per-area testing alone won't "
                "catch — run an integration-focused regression pass covering "
                "the boundaries between these areas, not just each area in "
                "isolation." % (len(affected_areas), ", ".join(affected_areas))
            )

        result = {
            "change_description": change_description,
            "areas": areas,
            "cross_area_risk_note": cross_area_risk_note,
        }
    except Exception as e:
        print(json.dumps({"error": "could not build regression test plan: %s" % e}))
        return 0

    print(json.dumps(result, indent=2, default=str))
    return 0


if __name__ == "__main__":
    sys.exit(main())
