#!/usr/bin/env python3
"""docstring_coverage — Engram skill (no network). Measure docstring coverage of
a Python snippet, via `ast`.

Counts the module plus every class, function, and async-function definition and
how many carry a docstring (`ast.get_docstring`). Reports per-category counts
and coverage_pct, an overall documented/total/coverage_pct, and a list of the
undocumented definitions (name, type, line), capped at 50.

Request (stdin): {"code": "def f():\\n    '''doc'''\\n    pass\\n\\ndef g():\\n    pass\\n"}
Output (stdout): {by_category, documented, total, coverage_pct, undocumented}
"""
import ast
import json
import sys

_EXAMPLE = {"code": "def f():\n    '''doc'''\n    pass\n\ndef g():\n    pass\n"}


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e}))
        return 0
    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object", "example": _EXAMPLE}))
        return 0

    code = q.get("code")
    if not isinstance(code, str) or not code.strip():
        print(json.dumps({
            "error": "missing required field 'code' (string of Python source)",
            "example": _EXAMPLE,
        }))
        return 0

    try:
        tree = ast.parse(code)
    except SyntaxError as e:
        print(json.dumps({
            "error": "code does not parse as Python: %s" % e.msg,
            "line": e.lineno,
            "offset": e.offset,
        }))
        return 0
    except Exception as e:
        print(json.dumps({"error": "code does not parse as Python: %s" % e}))
        return 0

    try:
        cats = {
            "module": {"documented": 0, "total": 0},
            "class": {"documented": 0, "total": 0},
            "function": {"documented": 0, "total": 0},
            "async_function": {"documented": 0, "total": 0},
        }
        undocumented = []

        def record(cat, name, has_doc, line):
            cats[cat]["total"] += 1
            if has_doc:
                cats[cat]["documented"] += 1
            else:
                undocumented.append({"type": cat, "name": name, "line": line})

        # module
        record("module", "<module>", ast.get_docstring(tree) is not None, 1)

        for node in ast.walk(tree):
            if isinstance(node, ast.ClassDef):
                record("class", node.name, ast.get_docstring(node) is not None, node.lineno)
            elif isinstance(node, ast.FunctionDef):
                record("function", node.name, ast.get_docstring(node) is not None, node.lineno)
            elif isinstance(node, ast.AsyncFunctionDef):
                record("async_function", node.name, ast.get_docstring(node) is not None, node.lineno)

        by_category = {}
        for cat, d in cats.items():
            total = d["total"]
            pct = round(100.0 * d["documented"] / total, 1) if total else None
            by_category[cat] = {
                "documented": d["documented"],
                "total": total,
                "coverage_pct": pct,
            }

        total_all = sum(d["total"] for d in cats.values())
        documented_all = sum(d["documented"] for d in cats.values())
        overall_pct = round(100.0 * documented_all / total_all, 1) if total_all else None

        undocumented.sort(key=lambda u: u["line"])
        capped = undocumented[:50]

        result = {
            "by_category": by_category,
            "documented": documented_all,
            "total": total_all,
            "coverage_pct": overall_pct,
            "undocumented": capped,
            "undocumented_count": len(undocumented),
        }
        if len(undocumented) > 50:
            result["undocumented_truncated"] = True
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "docstring_coverage failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
