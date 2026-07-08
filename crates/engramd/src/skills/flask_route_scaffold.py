#!/usr/bin/env python3
"""flask_route_scaffold — Engram skill (no network). Generate a Flask
Blueprint boilerplate file for a resource, with one route stub per requested
HTTP method.

Request (stdin): {"resource": "orders", "methods": ["GET", "POST", "PUT", "DELETE"]}
Output (stdout): {filename, code}
"""
import json
import re
import sys

_SUPPORTED = ["GET", "POST", "PUT", "PATCH", "DELETE"]


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e}))
        return 0
    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"resource": "orders", "methods": ["GET", "POST"]},
        }))
        return 0

    resource = q.get("resource")
    if not isinstance(resource, str) or not resource.strip():
        print(json.dumps({
            "error": "missing required field 'resource' (non-empty string)",
            "example": {"resource": "orders", "methods": ["GET", "POST", "PUT", "DELETE"]},
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
        bp_name = "%s_bp" % re.sub(r"[^A-Za-z0-9]+", "_", resource).strip("_").lower()
        py_slug = re.sub(r"[^A-Za-z0-9]+", "_", resource).strip("_").lower()

        lines = []
        lines.append("from flask import Blueprint, jsonify, request")
        lines.append("")
        lines.append("%s = Blueprint('%s', __name__)" % (bp_name, resource))
        lines.append("")
        lines.append("")

        for method in normalized:
            if method == "GET":
                lines.append("@%s.route('/%s', methods=['GET'])" % (bp_name, resource))
                lines.append("def list_%s():" % py_slug)
                lines.append("    # TODO: list all %s" % resource)
                lines.append("    return jsonify([])")
                lines.append("")
                lines.append("")
                lines.append("@%s.route('/%s/<int:item_id>', methods=['GET'])" % (bp_name, resource))
                lines.append("def get_%s(item_id):" % py_slug)
                lines.append("    # TODO: get a single %s by id" % py_slug)
                lines.append("    return jsonify({'id': item_id})")
                lines.append("")
                lines.append("")
            elif method == "POST":
                lines.append("@%s.route('/%s', methods=['POST'])" % (bp_name, resource))
                lines.append("def create_%s():" % py_slug)
                lines.append("    data = request.get_json(silent=True) or {}")
                lines.append("    # TODO: create a new %s" % py_slug)
                lines.append("    return jsonify(data), 201")
                lines.append("")
                lines.append("")
            elif method == "PUT":
                lines.append("@%s.route('/%s/<int:item_id>', methods=['PUT'])" % (bp_name, resource))
                lines.append("def replace_%s(item_id):" % py_slug)
                lines.append("    data = request.get_json(silent=True) or {}")
                lines.append("    # TODO: replace a %s by id" % py_slug)
                lines.append("    return jsonify(data)")
                lines.append("")
                lines.append("")
            elif method == "PATCH":
                lines.append("@%s.route('/%s/<int:item_id>', methods=['PATCH'])" % (bp_name, resource))
                lines.append("def update_%s(item_id):" % py_slug)
                lines.append("    data = request.get_json(silent=True) or {}")
                lines.append("    # TODO: partially update a %s by id" % py_slug)
                lines.append("    return jsonify(data)")
                lines.append("")
                lines.append("")
            elif method == "DELETE":
                lines.append("@%s.route('/%s/<int:item_id>', methods=['DELETE'])" % (bp_name, resource))
                lines.append("def delete_%s(item_id):" % py_slug)
                lines.append("    # TODO: delete a %s by id" % py_slug)
                lines.append("    return '', 204")
                lines.append("")
                lines.append("")

        code = "\n".join(lines).rstrip() + "\n"
        filename = "%s.py" % resource
        print(json.dumps({"filename": filename, "code": code}, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "flask_route_scaffold failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
