#!/usr/bin/env python3
"""diff — Engram skill (no network). Unified diff between two texts (pure compute).

Computes a unified diff of two strings using difflib. Request shape:
{"a": "<text>", "b": "<text>", "label_a": "a", "label_b": "b"}.
Output: {"diff": <unified diff text>, "added": <int>, "removed": <int>, "changed": <bool>}.
"""
import json, sys, difflib

def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e})); return 0

    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"a": "hello\nworld", "b": "hello\nthere", "label_a": "old", "label_b": "new"},
        })); return 0

    a = q.get("a")
    b = q.get("b")
    if a is None and b is None:
        print(json.dumps({
            "error": "provide at least one of 'a' or 'b'",
            "example": {"a": "hello\nworld", "b": "hello\nthere", "label_a": "old", "label_b": "new"},
        })); return 0

    # Coerce to strings; treat a missing side as empty text.
    a = "" if a is None else (a if isinstance(a, str) else str(a))
    b = "" if b is None else (b if isinstance(b, str) else str(b))

    label_a = q.get("label_a") or "a"
    label_b = q.get("label_b") or "b"
    label_a = label_a if isinstance(label_a, str) else str(label_a)
    label_b = label_b if isinstance(label_b, str) else str(label_b)

    try:
        lines = list(difflib.unified_diff(
            a.splitlines(),
            b.splitlines(),
            fromfile=label_a,
            tofile=label_b,
            lineterm="",
        ))
        added = sum(1 for ln in lines if ln.startswith("+") and not ln.startswith("+++"))
        removed = sum(1 for ln in lines if ln.startswith("-") and not ln.startswith("---"))
        result = {
            "diff": "\n".join(lines),
            "added": added,
            "removed": removed,
            "changed": a != b,
        }
        print(json.dumps(result, indent=2, default=str)); return 0
    except Exception as e:
        print(json.dumps({"error": "diff failed: %s" % e})); return 1

if __name__ == "__main__":
    sys.exit(main())
