#!/usr/bin/env python3
"""cors_check — Engram skill (no network). Analyze a pasted-in CORS config for
common misconfigurations.

This does NOT fetch a live URL — it inspects the literal CORS response header
VALUES you provide (e.g. copied from your server config or from inspecting a
response in devtools/curl) and flags known-risky combinations.

Request (stdin): {
    "access_control_allow_origin": "*",
    "access_control_allow_credentials": "true",
    "access_control_allow_methods": "GET, POST, PUT, DELETE",
    "access_control_allow_headers": "Content-Type, Authorization"
}
(all fields optional — send whichever headers your config sets)
Output (stdout): {
    "findings": [{"severity": "critical"|"warning"|"info", "issue", "explanation"}],
    "config_echoed": {...}
}
"""
import json
import sys

_EXAMPLE = {
    "access_control_allow_origin": "*",
    "access_control_allow_credentials": "true",
    "access_control_allow_methods": "GET, POST, PUT, DELETE",
    "access_control_allow_headers": "Content-Type, Authorization",
}

_RISKY_METHODS = {"DELETE", "PUT", "PATCH", "*"}


def _truthy(v):
    if isinstance(v, bool):
        return v
    if isinstance(v, str):
        return v.strip().lower() == "true"
    return False


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0

    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object", "example": _EXAMPLE}))
        return 0

    fields = (
        "access_control_allow_origin",
        "access_control_allow_credentials",
        "access_control_allow_methods",
        "access_control_allow_headers",
    )
    if not any(f in q and q[f] not in (None, "") for f in fields):
        print(json.dumps({
            "error": "provide at least one CORS header value to analyze",
            "example": _EXAMPLE,
        }))
        return 0

    try:
        origin = q.get("access_control_allow_origin")
        origin = origin.strip() if isinstance(origin, str) else origin
        credentials_raw = q.get("access_control_allow_credentials")
        credentials = _truthy(credentials_raw)
        methods = q.get("access_control_allow_methods")
        methods = methods.strip() if isinstance(methods, str) else methods
        headers = q.get("access_control_allow_headers")
        headers = headers.strip() if isinstance(headers, str) else headers

        findings = []

        if origin == "*" and credentials:
            findings.append({
                "severity": "critical",
                "issue": "Access-Control-Allow-Origin: * combined with Access-Control-Allow-Credentials: true",
                "explanation": (
                    "Browsers themselves reject this exact combination (the fetch/XHR spec forbids "
                    "a wildcard origin when credentials are allowed), so it may look 'safe' in practice. "
                    "But some reverse proxies, API gateways, and web frameworks 'help' by silently "
                    "rewriting a wildcard configuration into a dynamic reflection of the request's Origin "
                    "header whenever credentials are involved, in order to satisfy the browser. That "
                    "rewrite reintroduces the vulnerability: any origin can then make credentialed "
                    "requests (cookies, HTTP auth) and read the response, which is equivalent to "
                    "disabling the same-origin policy for this endpoint. Fix: enumerate an explicit "
                    "allowlist of trusted origins instead of '*'."
                ),
            })
        elif origin == "*":
            findings.append({
                "severity": "info",
                "issue": "Access-Control-Allow-Origin is '*' (no credentials allowed)",
                "explanation": (
                    "Any origin can read non-credentialed responses from this endpoint. This is "
                    "often intentional for public APIs, but confirm the response never contains "
                    "sensitive or per-user data."
                ),
            })
        elif isinstance(origin, str) and origin and origin != "null":
            # Can't tell from a single static value whether the server reflects the request's
            # Origin dynamically (vs. a fixed allowlisted value) — flag the general risk.
            findings.append({
                "severity": "warning",
                "issue": "Access-Control-Allow-Origin is a single non-wildcard value (%r)" % origin,
                "explanation": (
                    "This looks like a fixed origin, which is good — but if this value is actually "
                    "produced by reflecting the incoming request's Origin header back verbatim "
                    "(a common pattern to 'support multiple origins' without a real allowlist), it is "
                    "equivalent to a wildcard: every origin would see its own value reflected and pass "
                    "the browser's check. Verify the value is looked up against a fixed allowlist of "
                    "trusted origins server-side, not copied from the request."
                ),
            })

        if origin in (None, ""):
            findings.append({
                "severity": "info",
                "issue": "Access-Control-Allow-Origin not set",
                "explanation": (
                    "With no CORS headers, browsers enforce the same-origin policy by default — "
                    "cross-origin scripts cannot read responses from this endpoint. This is often "
                    "fine and is not a problem by itself unless you actually need cross-origin access."
                ),
            })

        if isinstance(methods, str) and methods:
            listed = [m.strip().upper() for m in methods.split(",") if m.strip()]
            broad = sorted(set(listed) & _RISKY_METHODS)
            if broad:
                findings.append({
                    "severity": "warning",
                    "issue": "Access-Control-Allow-Methods includes broad/state-changing methods: %s" % ", ".join(broad),
                    "explanation": (
                        "Listed methods: %s. This just reports what is configured — whether it's "
                        "intentional depends on your API. If cross-origin callers don't need to "
                        "DELETE/PUT/PATCH (or a wildcard method set), narrow this list to only what's "
                        "required." % ", ".join(listed)
                    ),
                })

        config_echoed = {
            "access_control_allow_origin": origin,
            "access_control_allow_credentials": credentials_raw,
            "access_control_allow_methods": methods,
            "access_control_allow_headers": headers,
        }
        result = {"findings": findings, "config_echoed": config_echoed}
    except Exception as e:
        print(json.dumps({"error": "could not analyze CORS config: %s" % e}))
        return 0

    print(json.dumps(result, indent=2, default=str))
    return 0


if __name__ == "__main__":
    sys.exit(main())
