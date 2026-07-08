#!/usr/bin/env python3
"""security_checklist — Engram skill (no network). OWASP-Top-10-inspired self-assessment.

This is NOT a scanner — it does not inspect any code or system. It simply takes
a list of control ids the caller says they've already implemented and echoes
back a checklist against a fixed, simplified set of 10 baseline security
controls, flagging which of the highest-impact ones are still missing.

Request (stdin): {"implemented_controls": ["auth_mfa", "encryption_in_transit"]}
    ("implemented_controls" is optional; omit or send [] to see the full checklist
    with nothing marked implemented.)
Output (stdout): {
    "checklist": [{"id", "label", "why_it_matters", "implemented"}, ...] (10 items),
    "completeness_pct": float,
    "implemented_count": int,
    "total_count": 10,
    "missing_high_priority": [ids from the top-3 priority list not yet implemented]
}
"""
import json
import sys

CONTROLS = [
    {
        "id": "input_validation",
        "label": "Validate and sanitize all user input",
        "why_it_matters": "prevents injection attacks",
    },
    {
        "id": "parameterized_queries",
        "label": "Use parameterized queries/ORM, never string-concatenated SQL",
        "why_it_matters": "prevents SQL injection",
    },
    {
        "id": "auth_mfa",
        "label": "Enforce strong auth, ideally with MFA",
        "why_it_matters": "limits blast radius of credential theft",
    },
    {
        "id": "least_privilege",
        "label": "Grant least-privilege access by default",
        "why_it_matters": "limits damage from a compromised account or service",
    },
    {
        "id": "dependency_scanning",
        "label": "Scan dependencies for known CVEs",
        "why_it_matters": "catches known-vulnerable libraries before they ship",
    },
    {
        "id": "secrets_management",
        "label": "Never hardcode secrets — use a secrets manager or env vars",
        "why_it_matters": "prevents leaked credentials in source control",
    },
    {
        "id": "encryption_in_transit",
        "label": "Enforce TLS/HTTPS everywhere",
        "why_it_matters": "prevents eavesdropping and MITM",
    },
    {
        "id": "encryption_at_rest",
        "label": "Encrypt sensitive data at rest",
        "why_it_matters": "limits damage from storage/backup compromise",
    },
    {
        "id": "logging_monitoring",
        "label": "Log security-relevant events and monitor/alert on anomalies",
        "why_it_matters": "enables detection and incident response",
    },
    {
        "id": "security_headers",
        "label": "Set standard HTTP security headers (CSP, HSTS, etc.)",
        "why_it_matters": "defense-in-depth against XSS/clickjacking",
    },
]
CONTROL_IDS = {c["id"] for c in CONTROLS}
PRIORITY_ORDER = ["parameterized_queries", "secrets_management", "auth_mfa"]

_EXAMPLE = {"implemented_controls": ["auth_mfa", "encryption_in_transit"]}


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0

    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object", "example": _EXAMPLE}))
        return 0

    raw = q.get("implemented_controls", [])
    if raw is None:
        raw = []
    if not isinstance(raw, list) or not all(isinstance(x, str) for x in raw):
        print(json.dumps({
            "error": "'implemented_controls' must be a list of strings (control ids)",
            "valid_ids": sorted(CONTROL_IDS),
            "example": _EXAMPLE,
        }))
        return 0

    try:
        implemented = {c.strip() for c in raw if c.strip()}
        unknown = sorted(implemented - CONTROL_IDS)

        checklist = [
            {
                "id": c["id"],
                "label": c["label"],
                "why_it_matters": c["why_it_matters"],
                "implemented": c["id"] in implemented,
            }
            for c in CONTROLS
        ]
        implemented_count = sum(1 for c in checklist if c["implemented"])
        completeness_pct = round(implemented_count / len(CONTROLS) * 100, 1)
        missing_high_priority = [p for p in PRIORITY_ORDER if p not in implemented]

        result = {
            "checklist": checklist,
            "completeness_pct": completeness_pct,
            "implemented_count": implemented_count,
            "total_count": len(CONTROLS),
            "missing_high_priority": missing_high_priority,
        }
        if unknown:
            result["warning"] = "unrecognized control ids ignored: %s" % ", ".join(unknown)
    except Exception as e:
        print(json.dumps({"error": "could not build checklist: %s" % e}))
        return 0

    print(json.dumps(result, indent=2, default=str))
    return 0


if __name__ == "__main__":
    sys.exit(main())
