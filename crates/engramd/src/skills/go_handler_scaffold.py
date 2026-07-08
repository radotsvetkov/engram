#!/usr/bin/env python3
"""go_handler_scaffold — Engram skill (no network). Generate a Go `net/http`
handler function boilerplate for a resource, switching on `r.Method` with one
case per requested HTTP method.

Request (stdin): {"resource": "widgets", "methods": ["GET", "POST"]}
Output (stdout): {filename, code}
"""
import json
import re
import sys

_SUPPORTED = ["GET", "POST", "PUT", "PATCH", "DELETE"]


def _to_pascal_case(name):
    parts = re.split(r"[^A-Za-z0-9]+", name.strip())
    words = []
    for part in parts:
        if not part:
            continue
        sub = re.findall(r"[A-Z]+(?=[A-Z][a-z0-9])|[A-Z]?[a-z0-9]+|[A-Z]+", part)
        words.extend(sub if sub else [part])
    if not words:
        return ""
    return "".join(w[:1].upper() + w[1:] for w in words if w)


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e}))
        return 0
    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"resource": "widgets", "methods": ["GET", "POST"]},
        }))
        return 0

    raw_resource = q.get("resource")
    if not isinstance(raw_resource, str) or not raw_resource.strip():
        print(json.dumps({
            "error": "missing required field 'resource' (non-empty string)",
            "example": {"resource": "widgets", "methods": ["GET", "POST"]},
        }))
        return 0
    resource = raw_resource.strip().strip("/")
    if not re.match(r"^[A-Za-z0-9_-]+$", resource):
        print(json.dumps({"error": "'resource' must be a simple path segment (letters, numbers, -, _), got %r" % resource}))
        return 0

    methods = q.get("methods")
    if methods is None:
        methods = ["GET", "POST"]
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
        resource_pascal = _to_pascal_case(resource)
        func_name = "%sHandler" % resource_pascal
        slug = re.sub(r"[^A-Za-z0-9]+", "_", resource).strip("_").lower()

        lines = []
        lines.append("package handlers")
        lines.append("")
        lines.append("import (")
        lines.append("\t\"encoding/json\"")
        lines.append("\t\"net/http\"")
        lines.append(")")
        lines.append("")
        lines.append("func %s(w http.ResponseWriter, r *http.Request) {" % func_name)
        lines.append("\tswitch r.Method {")
        for method in normalized:
            lines.append("\tcase %s:" % ("http.MethodGet" if method == "GET" else
                                          "http.MethodPost" if method == "POST" else
                                          "http.MethodPut" if method == "PUT" else
                                          "http.MethodPatch" if method == "PATCH" else
                                          "http.MethodDelete"))
            lines.append("\t\t// TODO: handle %s /%s" % (method, resource))
            lines.append("\t\tw.Header().Set(\"Content-Type\", \"application/json\")")
            lines.append("\t\tw.WriteHeader(http.StatusOK)")
            lines.append("\t\tjson.NewEncoder(w).Encode(map[string]interface{}{")
            lines.append("\t\t\t\"resource\": \"%s\"," % resource)
            lines.append("\t\t\t\"method\":   \"%s\"," % method)
            lines.append("\t\t})")
        lines.append("\tdefault:")
        lines.append("\t\thttp.Error(w, \"method not allowed\", http.StatusMethodNotAllowed)")
        lines.append("\t}")
        lines.append("}")
        lines.append("")

        code = "\n".join(lines)
        filename = "%s_handler.go" % slug
        print(json.dumps({"filename": filename, "code": code}, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "go_handler_scaffold failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
