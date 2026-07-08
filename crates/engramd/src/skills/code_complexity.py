#!/usr/bin/env python3
"""code_complexity — Engram skill (no network). McCabe cyclomatic complexity for
each function in a Python source snippet.

Complexity starts at 1 per function; +1 per If/For/While; +1 per except handler
in a Try; +1 per extra BoolOp operand (and/or) beyond the first; +1 per `if`
clause in a comprehension. `with` blocks are not counted. Nested function/
async-def bodies are scored as their own separate entries and excluded from
their enclosing function's count.

Request (stdin): {"code": "def f(x):\\n    if x:\\n        return 1\\n    return 0\\n"}
Output (stdout): {functions: [{name, complexity, line}, ...], average_complexity,
                   high_complexity_functions}
"""
import ast
import json
import sys

_EXAMPLE = {"code": "def f(x):\n    if x:\n        return 1\n    return 0\n"}


def _function_complexity(func_node):
    complexity = 1

    def walk(node):
        nonlocal complexity
        for child in ast.iter_child_nodes(node):
            if isinstance(child, (ast.FunctionDef, ast.AsyncFunctionDef)):
                continue  # scored as its own separate entry
            if isinstance(child, (ast.If, ast.For, ast.While)):
                complexity += 1
            elif isinstance(child, ast.Try):
                complexity += len(child.handlers)
            elif isinstance(child, ast.BoolOp):
                complexity += max(len(child.values) - 1, 0)
            elif isinstance(child, ast.comprehension):
                complexity += len(child.ifs)
            walk(child)

    walk(func_node)
    return complexity


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e})); return 0
    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object", "example": _EXAMPLE})); return 0

    code = q.get("code")
    if not isinstance(code, str) or not code.strip():
        print(json.dumps({
            "error": "missing required field 'code' (string of Python source)",
            "example": _EXAMPLE,
        })); return 0

    try:
        tree = ast.parse(code)
    except SyntaxError as e:
        print(json.dumps({
            "error": "code does not parse as Python: %s" % e.msg,
            "line": e.lineno,
            "offset": e.offset,
        })); return 0
    except Exception as e:
        print(json.dumps({"error": "code does not parse as Python: %s" % e})); return 0

    try:
        functions = []
        for node in ast.walk(tree):
            if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
                functions.append({
                    "name": node.name,
                    "complexity": _function_complexity(node),
                    "line": node.lineno,
                })

        functions.sort(key=lambda f: f["complexity"], reverse=True)
        average_complexity = (
            sum(f["complexity"] for f in functions) / len(functions) if functions else 0
        )
        high_complexity_functions = [f["name"] for f in functions if f["complexity"] > 10]

        result = {
            "functions": functions,
            "average_complexity": average_complexity,
            "high_complexity_functions": high_complexity_functions,
        }
        print(json.dumps(result, indent=2, default=str)); return 0
    except Exception as e:
        print(json.dumps({"error": "code_complexity failed: %s" % e})); return 1


if __name__ == "__main__":
    sys.exit(main())
