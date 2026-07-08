#!/usr/bin/env python3
"""openapi_lint — Engram skill (no network). Lint an OpenAPI/Swagger document
(JSON only — no YAML support, the stdlib has no YAML parser).

Checks required top-level keys (`openapi`/`swagger` version string, `info.title`,
`info.version`, a non-empty `paths` object), and for every path x HTTP method
checks that a `responses` key is present (an OpenAPI requirement -> error if
missing) and that `operationId` and a `summary`/`description` are present (best
practice -> warning if missing).

Request (stdin): {"spec": {...}}  or  {"spec": "{...json-encoded spec...}"}
Output (stdout): {valid, errors, warnings, endpoint_count, methods_used}
"""
import json
import sys

_METHODS = ("get", "post", "put", "patch", "delete", "options", "head")

_EXAMPLE = {
    "spec": {
        "openapi": "3.0.0",
        "info": {"title": "Sample API", "version": "1.0.0"},
        "paths": {
            "/ping": {"get": {"operationId": "ping", "summary": "health check", "responses": {"200": {"description": "ok"}}}}
        },
    }
}


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e})); return 0
    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object", "example": _EXAMPLE})); return 0

    spec = q.get("spec")
    if spec is None:
        print(json.dumps({
            "error": "missing required field 'spec' (a JSON OpenAPI/Swagger document, or a JSON string of one)",
            "example": _EXAMPLE,
        })); return 0

    if isinstance(spec, str):
        try:
            spec = json.loads(spec)
        except Exception as e:
            print(json.dumps({
                "error": (
                    "'spec' string is not valid JSON (%s) — only JSON OpenAPI/Swagger specs are "
                    "supported (no YAML — stdlib has no YAML parser); convert your spec to JSON first"
                ) % e,
            })); return 0

    if not isinstance(spec, dict):
        print(json.dumps({
            "error": "'spec' must be a JSON object (or a JSON string encoding one)",
            "example": _EXAMPLE,
        })); return 0

    try:
        errors = []
        warnings = []

        version = spec.get("openapi") or spec.get("swagger")
        if not version or not isinstance(version, str):
            errors.append("missing required top-level key 'openapi' or 'swagger' (version string)")

        info = spec.get("info")
        if not isinstance(info, dict):
            errors.append("missing required top-level key 'info' (object)")
        else:
            if not info.get("title"):
                errors.append("'info.title' is required")
            if not info.get("version"):
                errors.append("'info.version' is required")

        paths = spec.get("paths")
        if not isinstance(paths, dict) or not paths:
            errors.append("missing or empty required top-level key 'paths' (non-empty object)")
            paths = {}

        endpoint_count = 0
        methods_used = set()

        for path, item in paths.items():
            if not isinstance(item, dict):
                errors.append("path %r: value must be an object" % (path,))
                continue
            for method in _METHODS:
                if method not in item:
                    continue
                op = item[method]
                endpoint_count += 1
                methods_used.add(method)
                if not isinstance(op, dict):
                    errors.append("%s %s: operation must be an object" % (method.upper(), path))
                    continue
                if "responses" not in op:
                    errors.append("%s %s: missing required 'responses' key" % (method.upper(), path))
                if not op.get("operationId"):
                    warnings.append("%s %s: missing 'operationId' (best practice)" % (method.upper(), path))
                if not op.get("description") and not op.get("summary"):
                    warnings.append(
                        "%s %s: missing 'description'/'summary' (best practice)" % (method.upper(), path)
                    )

        result = {
            "valid": len(errors) == 0,
            "errors": errors,
            "warnings": warnings,
            "endpoint_count": endpoint_count,
            "methods_used": sorted(methods_used),
        }
        print(json.dumps(result, indent=2, default=str)); return 0
    except Exception as e:
        print(json.dumps({"error": "openapi_lint failed: %s" % e})); return 1


if __name__ == "__main__":
    sys.exit(main())
