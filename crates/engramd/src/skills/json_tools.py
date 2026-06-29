#!/usr/bin/env python3
"""json_tools — Engram skill (no network). A tiny jq: inspect/transform JSON, pure compute.

Request: {"json": <a JSON STRING or an inline value>, "op": "pretty"|"minify"|"keys"|"length"|"get", "path": "a.b.0.c"}.
If "json" is a string it is parsed with json.loads; otherwise it is used as-is.
Output depends on op: pretty/minify -> {"result": "<text>"}; keys -> {"keys": [...]}; length -> {"length": N};
get -> {"path": "<path>", "value": <value>}. Bad JSON or a missing path segment yields {"error": ...}.
"""
import json, sys


def _walk(value, path):
    """Walk a dotted path. Integer segments index lists. Raises KeyError(segment-trace) if not found."""
    cur = value
    walked = []
    for seg in path.split("."):
        walked.append(seg)
        if isinstance(cur, dict):
            if seg in cur:
                cur = cur[seg]
            else:
                raise KeyError(".".join(walked))
        elif isinstance(cur, list):
            # list index must be an integer segment
            try:
                idx = int(seg)
            except (ValueError, TypeError):
                raise KeyError(".".join(walked))
            if -len(cur) <= idx < len(cur):
                cur = cur[idx]
            else:
                raise KeyError(".".join(walked))
        else:
            # cannot descend into a scalar
            raise KeyError(".".join(walked))
    return cur


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e})); return 0

    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"json": {"a": {"b": [1, 2, 3]}}, "op": "get", "path": "a.b.1"},
        })); return 0

    if "json" not in q:
        print(json.dumps({
            "error": "missing required field: json",
            "example": {"json": {"a": 1, "b": [10, 20]}, "op": "pretty"},
            "how_to_fix": "provide 'json' as a JSON string or an inline value; ops: pretty, minify, keys, length, get",
        })); return 0

    raw = q.get("json")
    op = (q.get("op") or "pretty")
    if not isinstance(op, str):
        op = "pretty"
    op = op.lower().strip()

    # If json is a string, parse it; otherwise use the inline value as-is.
    if isinstance(raw, str):
        try:
            value = json.loads(raw)
        except Exception as e:
            print(json.dumps({"error": "invalid JSON: %s" % e})); return 0
    else:
        value = raw

    try:
        if op == "pretty":
            print(json.dumps({"result": json.dumps(value, indent=2, default=str)})); return 0

        if op == "minify":
            print(json.dumps({"result": json.dumps(value, separators=(",", ":"), default=str)})); return 0

        if op == "keys":
            if isinstance(value, dict):
                print(json.dumps({"keys": list(value.keys())})); return 0
            print(json.dumps({"error": "keys requires a JSON object, got %s" % type(value).__name__})); return 0

        if op == "length":
            if isinstance(value, (dict, list, str)):
                print(json.dumps({"length": len(value)})); return 0
            print(json.dumps({"error": "length requires an object, array, or string, got %s" % type(value).__name__})); return 0

        if op == "get":
            path = q.get("path")
            if not isinstance(path, str) or path == "":
                print(json.dumps({
                    "error": "get requires a non-empty 'path'",
                    "example": {"json": {"a": {"b": [1, 2, 3]}}, "op": "get", "path": "a.b.0"},
                })); return 0
            try:
                found = _walk(value, path)
            except KeyError as ke:
                print(json.dumps({"error": "path not found: %s" % path})); return 0
            print(json.dumps({"path": path, "value": found}, default=str)); return 0

        print(json.dumps({
            "error": "unknown op: %s" % op,
            "how_to_fix": "use one of: pretty, minify, keys, length, get",
        })); return 0
    except Exception as e:
        print(json.dumps({"error": "json_tools failed: %s" % e})); return 1


if __name__ == "__main__":
    sys.exit(main())
