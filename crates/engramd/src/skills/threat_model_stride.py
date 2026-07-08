#!/usr/bin/env python3
"""threat_model_stride — Engram skill (no network). STRIDE threat-modeling framework.

Applies Microsoft's STRIDE framework (Spoofing, Tampering, Repudiation,
Information Disclosure, Denial of Service, Elevation of Privilege) to a
described system. This is a DEFENSIVE PLANNING / analytical methodology tool
— it produces guiding questions for a security reviewer, it does not scan,
probe, or exploit anything.

For each STRIDE category it returns a definition and 2-3 guiding questions. If
`components` (a list of component names) is provided, it additionally uses
simple keyword heuristics on each component's name and the system description
to flag which 2-3 STRIDE categories are most relevant to that component (or
notes that no specific signal matched, in which case all six should be
reviewed).

Request (stdin): {"system_description": "...", "components"?: ["auth service", "database", ...]}
Output (stdout): {stride_categories: [...], component_considerations: [...]}
"""
import json
import sys

STRIDE_CATEGORIES = [
    {
        "category": "Spoofing",
        "definition": "Spoofing is when an attacker impersonates a user, device, or system "
                      "component to gain unauthorized access or trust.",
        "guiding_questions": [
            "How does this component verify the identity of users/services calling it?",
            "Could an attacker impersonate a legitimate caller (stolen credentials, forged "
            "tokens, IP/DNS spoofing)?",
            "Is authentication mutual where it needs to be, not just a one-way check?",
        ],
    },
    {
        "category": "Tampering",
        "definition": "Tampering is the unauthorized modification of data or code, either in "
                      "transit or at rest.",
        "guiding_questions": [
            "What prevents data from being modified in transit (e.g. TLS, signing) or at "
            "rest (e.g. checksums, access controls)?",
            "Could an attacker alter requests, stored records, or configuration without "
            "detection?",
            "Is there integrity verification (hashes, signatures, audit trails) on critical "
            "data paths?",
        ],
    },
    {
        "category": "Repudiation",
        "definition": "Repudiation is when a user or system denies having performed an "
                      "action, and there is insufficient evidence to prove otherwise.",
        "guiding_questions": [
            "Are actions logged with enough detail (who, what, when) to prove they occurred?",
            "Could a user or service plausibly deny an action due to missing or tamperable "
            "logs?",
            "Are logs protected from deletion or modification by the actors they record?",
        ],
    },
    {
        "category": "Information Disclosure",
        "definition": "Information Disclosure is the exposure of information to individuals "
                      "or systems that are not authorized to see it.",
        "guiding_questions": [
            "What sensitive data does this component handle, and who/what can access it?",
            "Could error messages, logs, or side channels leak sensitive information?",
            "Is data encrypted at rest and in transit, with least-privilege access controls?",
        ],
    },
    {
        "category": "Denial of Service",
        "definition": "Denial of Service is an attack that degrades or disables the "
                      "availability of a system or service for legitimate users.",
        "guiding_questions": [
            "Can this component be overwhelmed by excessive requests, large payloads, or "
            "resource-exhausting inputs?",
            "Are there rate limits, quotas, or circuit breakers to contain abuse?",
            "What is the blast radius if this component becomes unavailable — does it "
            "cascade to other components?",
        ],
    },
    {
        "category": "Elevation of Privilege",
        "definition": "Elevation of Privilege is when an attacker gains capabilities or "
                      "access beyond what they were authorized for.",
        "guiding_questions": [
            "Does this component enforce authorization checks on every privileged action, "
            "not just authentication?",
            "Could a lower-privileged user or service reach admin-only functionality via a "
            "bug or misconfiguration?",
            "Are privilege boundaries (roles, scopes, sandboxes) enforced consistently "
            "across all entry points?",
        ],
    },
]

ALL_CATEGORY_NAMES = [c["category"] for c in STRIDE_CATEGORIES]

# (keywords to match against "component name + system description", categories
# most relevant when one of those keywords appears). Checked in order; a
# component can match multiple rules, and matched categories are unioned
# (deduplicated, capped at 3, in order of first match).
COMPONENT_HEURISTICS = [
    (["auth", "login", "sso", "oauth", "session", "identity", "credential"],
     ["Spoofing", "Elevation of Privilege"]),
    (["database", "db", "storage", "store", "bucket", "s3", "file system",
      "filesystem", "cache", "data store"],
     ["Tampering", "Information Disclosure"]),
    (["api", "gateway", "public", "endpoint", "load balancer", "proxy", "ingress"],
     ["Denial of Service", "Spoofing"]),
    (["log", "audit"],
     ["Repudiation", "Information Disclosure"]),
    (["queue", "worker", "job", "scheduler", "cron"],
     ["Denial of Service", "Tampering"]),
    (["payment", "billing", "transaction", "invoice"],
     ["Tampering", "Information Disclosure", "Repudiation"]),
    (["frontend", "client", "browser", "ui"],
     ["Spoofing", "Tampering"]),
]


def _match(haystack):
    matched_categories = []
    matched_signals = []
    for keywords, categories in COMPONENT_HEURISTICS:
        hit = next((kw for kw in keywords if kw in haystack), None)
        if hit is None:
            continue
        matched_signals.append(hit)
        for cat in categories:
            if cat not in matched_categories:
                matched_categories.append(cat)
    return matched_categories, matched_signals


def _considerations_for(component, system_description):
    # Prioritize the component's own name — that's the strongest, most
    # specific signal. Only fall back to the (shared, often keyword-rich)
    # system description if the name alone gives no signal, so multiple
    # components don't all collapse to the same result just because the
    # description happens to mention several keywords.
    matched_categories, matched_signals = _match(str(component).lower())
    source = "component name"
    if not matched_categories:
        matched_categories, matched_signals = _match(str(system_description).lower())
        source = "system description"

    if not matched_categories:
        return {
            "component": component,
            "most_relevant_categories": list(ALL_CATEGORY_NAMES),
            "rationale": "no specific keyword signal matched — review all STRIDE "
                         "categories for this component",
        }

    return {
        "component": component,
        "most_relevant_categories": matched_categories[:3],
        "rationale": "matched keyword signal(s) in %s: %s" % (
            source, ", ".join(matched_signals)),
    }


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e}))
        return 0

    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {
                "system_description": "A public API gateway that authenticates users "
                                       "and reads/writes to a Postgres database",
                "components": ["auth service", "api gateway", "database"],
            },
        }))
        return 0

    system_description = q.get("system_description")
    if not system_description or not str(system_description).strip():
        print(json.dumps({
            "error": "missing required field: system_description",
            "example": {
                "system_description": "A public API gateway that authenticates users "
                                       "and reads/writes to a Postgres database",
                "components": ["auth service", "api gateway", "database"],
            },
        }))
        return 0
    system_description = str(system_description).strip()

    components = q.get("components") or []
    if not isinstance(components, list):
        print(json.dumps({
            "error": "'components' must be a list of strings if provided",
            "example": {"components": ["auth service", "api gateway", "database"]},
        }))
        return 0

    try:
        component_considerations = []
        for comp in components:
            comp_name = str(comp).strip()
            if not comp_name:
                continue
            component_considerations.append(_considerations_for(comp_name, system_description))

        result = {
            "stride_categories": STRIDE_CATEGORIES,
            "component_considerations": component_considerations,
        }
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "threat_model_stride failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
