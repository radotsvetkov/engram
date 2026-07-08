#!/usr/bin/env python3
"""react_component_scaffold — Engram skill (no network). Generate a React
functional component boilerplate file (JSX or TSX) from a name and an
optional list of prop names.

Request (stdin): {"name": "userCard", "props": ["userId", "onClose"], "typescript": true}
Output (stdout): {filename, code}
"""
import json
import re
import sys


def _to_pascal_case(name):
    # split on non-alphanumeric boundaries, underscores, hyphens, and
    # camelCase/PascalCase word boundaries
    parts = re.split(r"[^A-Za-z0-9]+", name.strip())
    words = []
    for part in parts:
        if not part:
            continue
        # further split camelCase runs like "userId" -> ["user", "Id"]
        sub = re.findall(r"[A-Z]+(?=[A-Z][a-z0-9])|[A-Z]?[a-z0-9]+|[A-Z]+", part)
        words.extend(sub if sub else [part])
    if not words:
        return ""
    return "".join(w[:1].upper() + w[1:] for w in words if w)


def _is_valid_identifier_list(props):
    ident_re = re.compile(r"^[A-Za-z_$][A-Za-z0-9_$]*$")
    return [p for p in props if isinstance(p, str) and ident_re.match(p.strip())]


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e}))
        return 0
    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"name": "UserCard", "props": ["userId", "onClose"], "typescript": True},
        }))
        return 0

    raw_name = q.get("name")
    if not isinstance(raw_name, str) or not raw_name.strip():
        print(json.dumps({
            "error": "missing required field 'name' (non-empty string)",
            "example": {"name": "UserCard", "props": ["userId", "onClose"], "typescript": True},
        }))
        return 0

    typescript = bool(q.get("typescript", False))
    raw_props = q.get("props") or []
    if not isinstance(raw_props, list):
        print(json.dumps({
            "error": "'props' must be a list of strings if provided",
            "example": {"name": "UserCard", "props": ["userId", "onClose"]},
        }))
        return 0

    try:
        component_name = _to_pascal_case(raw_name)
        if not component_name:
            print(json.dumps({"error": "could not derive a valid component name from %r" % raw_name}))
            return 0

        props = _is_valid_identifier_list(raw_props)
        props = [p.strip() for p in props]

        ext = "tsx" if typescript else "jsx"
        filename = "%s.%s" % (component_name, ext)

        lines = []
        if typescript:
            if props:
                interface_name = "%sProps" % component_name
                lines.append("interface %s {" % interface_name)
                for p in props:
                    lines.append("  %s: any;" % p)
                lines.append("}")
                lines.append("")
                destructure = "{ %s }" % ", ".join(props)
                lines.append("export default function %s(%s: %s) {" % (
                    component_name, destructure, interface_name))
            else:
                lines.append("export default function %s() {" % component_name)
        else:
            if props:
                destructure = "{ %s }" % ", ".join(props)
                lines.append("export default function %s(%s) {" % (component_name, destructure))
            else:
                lines.append("export default function %s() {" % component_name)

        lines.append("  return (")
        lines.append("    <div>")
        lines.append("      <h1>%s</h1>" % component_name)
        if props:
            for p in props:
                lines.append("      {/* TODO: render %s */}" % p)
        else:
            lines.append("      {/* TODO: implement %s */}" % component_name)
        lines.append("    </div>")
        lines.append("  );")
        lines.append("}")
        lines.append("")

        code = "\n".join(lines)
        result = {"filename": filename, "code": code}
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "react_component_scaffold failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
