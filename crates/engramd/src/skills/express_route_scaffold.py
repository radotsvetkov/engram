#!/usr/bin/env python3
"""express_route_scaffold — Engram skill (no network). Generate an Express.js
router boilerplate file for a resource, with one stub handler per requested
HTTP method.

Request (stdin): {"resource": "users", "methods": ["GET", "POST", "PUT", "DELETE"]}
Output (stdout): {filename, code}
"""
import json
import re
import sys

_SUPPORTED = ["GET", "POST", "PUT", "PATCH", "DELETE"]

# method -> (express fn, path suffix ("" for collection, "/:id" for item), status code, description)
_HANDLERS = {
    "GET": [
        ("get", "", 200, "list all {resource}"),
        ("get", "/:id", 200, "get a single {resource} by id"),
    ],
    "POST": [
        ("post", "", 201, "create a new {resource}"),
    ],
    "PUT": [
        ("put", "/:id", 200, "replace a {resource} by id"),
    ],
    "PATCH": [
        ("patch", "/:id", 200, "partially update a {resource} by id"),
    ],
    "DELETE": [
        ("delete", "/:id", 204, "delete a {resource} by id"),
    ],
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
            "example": {"resource": "users", "methods": ["GET", "POST"]},
        }))
        return 0

    resource = q.get("resource")
    if not isinstance(resource, str) or not resource.strip():
        print(json.dumps({
            "error": "missing required field 'resource' (non-empty string)",
            "example": {"resource": "users", "methods": ["GET", "POST", "PUT", "DELETE"]},
        }))
        return 0
    resource = resource.strip().strip("/")
    if not re.match(r"^[A-Za-z0-9_-]+$", resource):
        print(json.dumps({"error": "'resource' must be a simple path segment (letters, numbers, -, _), got %r" % resource}))
        return 0

    methods = q.get("methods")
    if methods is None:
        methods = ["GET", "POST", "PUT", "DELETE"]
    if not isinstance(methods, list) or not methods:
        print(json.dumps({
            "error": "'methods' must be a non-empty list of strings if provided",
            "supported_methods": _SUPPORTED,
        }))
        return 0

    normalized = []
    unknown = []
    for m in methods:
        if not isinstance(m, str):
            unknown.append(m)
            continue
        um = m.strip().upper()
        if um in _SUPPORTED:
            if um not in normalized:
                normalized.append(um)
        else:
            unknown.append(m)

    if unknown:
        print(json.dumps({
            "error": "unsupported method(s): %s" % ", ".join(str(u) for u in unknown),
            "supported_methods": _SUPPORTED,
        }))
        return 0

    try:
        lines = []
        lines.append("const express = require('express');")
        lines.append("const router = express.Router();")
        lines.append("")

        for method in normalized:
            for fn, suffix, status, desc in _HANDLERS[method]:
                path = "/%s%s" % (resource, suffix)
                desc_text = desc.format(resource=resource)
                lines.append("router.%s('%s', (req, res) => {" % (fn, path))
                lines.append("  // TODO: %s" % desc_text)
                if suffix == "/:id":
                    lines.append("  const { id } = req.params;")
                if status == 204:
                    lines.append("  res.status(204).end();")
                else:
                    lines.append("  res.status(%d).json({ message: '%s not implemented' });" % (
                        status, desc_text))
                lines.append("});")
                lines.append("")

        lines.append("module.exports = router;")
        lines.append("")

        code = "\n".join(lines)
        filename = "%s.routes.js" % resource
        print(json.dumps({"filename": filename, "code": code}, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "express_route_scaffold failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
