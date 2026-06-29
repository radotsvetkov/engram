#!/usr/bin/env python3
"""calc — Engram skill (no network). Safely evaluate a math expression.

Supports + - * / // % ** , parentheses, and math functions (sqrt, sin, cos, log,
pi, e, ...). Parsed with `ast` and a strict whitelist — NEVER `eval()` — so it
can't run arbitrary code. Stdlib only.

Request (stdin): {"expr": "sqrt(2) * (3 + 4)**2"}
Output (stdout): {expr, result}
"""
import ast
import json
import math
import operator
import sys

_OPS = {
    ast.Add: operator.add, ast.Sub: operator.sub, ast.Mult: operator.mul,
    ast.Div: operator.truediv, ast.FloorDiv: operator.floordiv, ast.Mod: operator.mod,
    ast.Pow: operator.pow, ast.USub: operator.neg, ast.UAdd: operator.pos,
}
_FUNCS = {k: getattr(math, k) for k in (
    "sqrt", "sin", "cos", "tan", "asin", "acos", "atan", "atan2", "log", "log2",
    "log10", "exp", "floor", "ceil", "fabs", "factorial", "gcd", "hypot", "degrees", "radians")}
_FUNCS["abs"] = abs
_FUNCS["round"] = round
_FUNCS["min"] = min
_FUNCS["max"] = max
_CONSTS = {"pi": math.pi, "e": math.e, "tau": math.tau, "inf": math.inf}


def _ev(node):
    if isinstance(node, ast.Constant):
        if isinstance(node.value, (int, float)):
            return node.value
        raise ValueError("only numbers allowed")
    if isinstance(node, ast.BinOp) and type(node.op) in _OPS:
        return _OPS[type(node.op)](_ev(node.left), _ev(node.right))
    if isinstance(node, ast.UnaryOp) and type(node.op) in _OPS:
        return _OPS[type(node.op)](_ev(node.operand))
    if isinstance(node, ast.Name) and node.id in _CONSTS:
        return _CONSTS[node.id]
    if isinstance(node, ast.Call) and isinstance(node.func, ast.Name) and node.func.id in _FUNCS:
        if node.keywords:
            raise ValueError("keyword args not allowed")
        return _FUNCS[node.func.id](*[_ev(a) for a in node.args])
    raise ValueError("unsupported expression")


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0
    expr = (q.get("expr") or q.get("expression") or "").strip()
    if not expr:
        print(json.dumps({"error": "provide 'expr'", "example": {"expr": "sqrt(2) * (3 + 4)**2"}}))
        return 0
    try:
        tree = ast.parse(expr, mode="eval")
        result = _ev(tree.body)
    except Exception as e:
        print(json.dumps({"error": "could not evaluate: %s" % e, "expr": expr}))
        return 0
    print(json.dumps({"expr": expr, "result": result}, indent=2, default=str))
    return 0


if __name__ == "__main__":
    sys.exit(main())
