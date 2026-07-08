#!/usr/bin/env python3
"""mcp_server_scaffold — Engram skill (no network). Generate a minimal,
syntactically-valid Model Context Protocol (MCP) server in Python using the
FastMCP quick-start pattern from the official `mcp` SDK
(`from mcp.server.fastmcp import FastMCP`). For each requested tool this
emits an `@mcp.tool()`-decorated function (name sanitized to a valid Python
identifier) with a docstring from `description` and a `# TODO: implement`
placeholder body. This skill only WRITES the scaffold code — it does not
install the `mcp` package or run the generated server. The generated code
is verified with ast.parse before being returned.

Request (stdin): {
  "server_name": "weather-tools",
  "tools": [
    {"name": "get_forecast", "description": "Get a weather forecast for a city", "params": ["city"]},
    {"name": "get_alerts", "description": "Get active weather alerts for a region"}
  ]
}
Output (stdout): {filename, code, tool_count}
"""
import ast
import json
import keyword
import re
import sys

_EXAMPLE = {
    "server_name": "weather-tools",
    "tools": [
        {"name": "get_forecast", "description": "Get a weather forecast for a city", "params": ["city"]},
        {"name": "get_alerts", "description": "Get active weather alerts for a region"},
    ],
}


def _sanitize_identifier(name, fallback):
    s = re.sub(r"[^a-zA-Z0-9_]+", "_", str(name).strip().lower())
    s = re.sub(r"_+", "_", s).strip("_")
    if not s:
        s = fallback
    if s[0].isdigit():
        s = "_" + s
    if keyword.iskeyword(s):
        s = s + "_"
    return s


def _dedupe(name, seen):
    base = name
    suffix = 2
    while name in seen:
        name = "%s_%d" % (base, suffix)
        suffix += 1
    seen.add(name)
    return name


def _build_server(server_name, tools):
    lines = []
    lines.append("# Auto-generated MCP server scaffold.")
    lines.append("# Requires: pip install mcp")
    lines.append("from mcp.server.fastmcp import FastMCP")
    lines.append("")
    lines.append("mcp = FastMCP(%s)" % json.dumps(server_name))
    lines.append("")
    lines.append("")

    seen_func_names = set()
    for t in tools:
        func_name = _dedupe(_sanitize_identifier(t["name"], "tool"), seen_func_names)

        params = t.get("params") or []
        if params:
            seen_param_names = set()
            param_names = [_dedupe(_sanitize_identifier(p, "param"), seen_param_names) for p in params]
        else:
            param_names = ["input"]

        sig = ", ".join("%s: str" % p for p in param_names)

        lines.append("@mcp.tool()")
        lines.append("def %s(%s) -> str:" % (func_name, sig))
        lines.append("    %s" % json.dumps(t["description"]))
        lines.append("    # TODO: implement")
        lines.append('    return "not implemented"')
        lines.append("")
        lines.append("")

    lines.append('if __name__ == "__main__":')
    lines.append("    mcp.run()")
    lines.append("")
    return "\n".join(lines)


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e}))
        return 0

    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object", "example": _EXAMPLE}))
        return 0

    server_name = q.get("server_name")
    if not isinstance(server_name, str) or not server_name.strip():
        print(json.dumps({
            "error": "missing required field 'server_name' (non-empty string)",
            "example": _EXAMPLE,
        }))
        return 0
    server_name = server_name.strip()

    tools = q.get("tools")
    if not isinstance(tools, list) or len(tools) == 0:
        print(json.dumps({"error": "'tools' must be a non-empty list", "example": _EXAMPLE}))
        return 0

    validated = []
    for i, t in enumerate(tools):
        if not isinstance(t, dict):
            print(json.dumps({"error": "tool at index %d must be a JSON object" % i, "example": _EXAMPLE}))
            return 0
        name = t.get("name")
        if not isinstance(name, str) or not name.strip():
            print(json.dumps({"error": "tool at index %d missing non-empty 'name'" % i, "example": _EXAMPLE}))
            return 0
        description = t.get("description")
        if not isinstance(description, str) or not description.strip():
            print(json.dumps({
                "error": "tool at index %d missing non-empty 'description'" % i,
                "example": _EXAMPLE,
            }))
            return 0
        params = t.get("params")
        if params is not None:
            if not isinstance(params, list) or not all(isinstance(p, str) and p.strip() for p in params):
                print(json.dumps({
                    "error": "tool at index %d has invalid 'params'; must be a list of non-empty strings" % i,
                    "example": _EXAMPLE,
                }))
                return 0
        validated.append({"name": name, "description": description, "params": params or []})

    try:
        code = _build_server(server_name, validated)
        ast.parse(code)  # verify the generated code is syntactically valid Python
    except Exception as e:
        print(json.dumps({"error": "internal error building MCP server scaffold: %s" % e}))
        return 1

    slug = _sanitize_identifier(server_name, "server")
    filename = "%s_server.py" % slug

    print(json.dumps({
        "filename": filename,
        "code": code,
        "tool_count": len(validated),
    }, indent=2, default=str))
    return 0


if __name__ == "__main__":
    sys.exit(main())
