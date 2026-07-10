#!/usr/bin/env python3
"""code_smell_detect — Engram skill (no network). Structural code-smell detector
for a Python snippet, via `ast`.

Flags: long functions (>50 lines warn, >100 strong), too many parameters (>5),
deep nesting (>4 levels), too many local variables (>15), magic numbers
(numeric literals other than -1/0/1/2 in comparisons/args), god classes (>15
methods), and single-character identifier names. Each smell reports a type,
location, detail and severity.

Request (stdin): {"code": "def f(a, b, c, d, e, f):\\n    return a\\n"}
Output (stdout): {smells: [{type, location, line, detail, severity}], smell_count, summary}
"""
import ast
import json
import sys

_EXAMPLE = {"code": "def f(a, b, c, d, e, f):\n    return a\n"}
_ALLOWED_MAGIC = {-1, 0, 1, 2}


def _func_length(node):
    lines = [node.lineno]
    for n in ast.walk(node):
        if hasattr(n, "lineno") and isinstance(getattr(n, "lineno"), int):
            lines.append(n.lineno)
        end = getattr(n, "end_lineno", None)
        if isinstance(end, int):
            lines.append(end)
    return max(lines) - min(lines) + 1


def _max_depth(node, depth=0):
    best = depth
    nesting = (ast.If, ast.For, ast.While, ast.With, ast.Try,
               ast.AsyncFor, ast.AsyncWith)
    for child in ast.iter_child_nodes(node):
        if isinstance(child, (ast.FunctionDef, ast.AsyncFunctionDef)):
            continue  # nested funcs scored on their own
        if isinstance(child, nesting):
            best = max(best, _max_depth(child, depth + 1))
        else:
            best = max(best, _max_depth(child, depth))
    return best


def _param_count(node):
    a = node.args
    total = len(a.posonlyargs) + len(a.args) + len(a.kwonlyargs)
    if a.vararg:
        total += 1
    if a.kwarg:
        total += 1
    return total


def _local_names(node):
    names = set()
    for n in ast.walk(node):
        if isinstance(n, (ast.FunctionDef, ast.AsyncFunctionDef)) and n is not node:
            continue
        if isinstance(n, ast.Name) and isinstance(n.ctx, ast.Store):
            names.add(n.id)
    return names


def _magic_numbers(node):
    hits = []
    for n in ast.walk(node):
        if isinstance(n, ast.Compare):
            for operand in [n.left] + list(n.comparators):
                if isinstance(operand, ast.Constant) and isinstance(operand.value, (int, float)) \
                        and not isinstance(operand.value, bool) and operand.value not in _ALLOWED_MAGIC:
                    hits.append((operand.lineno, operand.value))
        elif isinstance(n, ast.Call):
            for arg in n.args:
                if isinstance(arg, ast.Constant) and isinstance(arg.value, (int, float)) \
                        and not isinstance(arg.value, bool) and arg.value not in _ALLOWED_MAGIC:
                    hits.append((arg.lineno, arg.value))
    return hits


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
        smells = []

        # functions
        for node in ast.walk(tree):
            if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
                loc = "function %s" % node.name
                length = _func_length(node)
                if length > 100:
                    smells.append({"type": "long_function", "location": loc, "line": node.lineno,
                                   "detail": "%d lines (>100)" % length, "severity": "high"})
                elif length > 50:
                    smells.append({"type": "long_function", "location": loc, "line": node.lineno,
                                   "detail": "%d lines (>50)" % length, "severity": "medium"})

                params = _param_count(node)
                if params > 5:
                    smells.append({"type": "too_many_parameters", "location": loc, "line": node.lineno,
                                   "detail": "%d parameters (>5)" % params,
                                   "severity": "high" if params > 8 else "medium"})

                depth = _max_depth(node, 0)
                if depth > 4:
                    smells.append({"type": "deep_nesting", "location": loc, "line": node.lineno,
                                   "detail": "nesting depth %d (>4)" % depth, "severity": "medium"})

                locals_count = len(_local_names(node))
                if locals_count > 15:
                    smells.append({"type": "too_many_locals", "location": loc, "line": node.lineno,
                                   "detail": "%d local variables (>15)" % locals_count, "severity": "medium"})

                for arg in list(node.args.posonlyargs) + list(node.args.args) + list(node.args.kwonlyargs):
                    if len(arg.arg) == 1 and arg.arg not in ("_",):
                        smells.append({"type": "single_char_name", "location": loc, "line": node.lineno,
                                       "detail": "single-character parameter name %r" % arg.arg,
                                       "severity": "low"})

        # magic numbers (module-wide, deduped by (line,value))
        magic = sorted(set(_magic_numbers(tree)))
        if magic:
            preview = ", ".join("%s@L%d" % (v, ln) for ln, v in magic[:8])
            smells.append({"type": "magic_number", "location": "module", "line": magic[0][0],
                           "detail": "%d magic numeric literal(s) in comparisons/args: %s" % (len(magic), preview),
                           "severity": "low"})

        # god classes
        for node in ast.walk(tree):
            if isinstance(node, ast.ClassDef):
                methods = [n for n in node.body if isinstance(n, (ast.FunctionDef, ast.AsyncFunctionDef))]
                if len(methods) > 15:
                    smells.append({"type": "god_class", "location": "class %s" % node.name, "line": node.lineno,
                                   "detail": "%d methods (>15)" % len(methods), "severity": "high"})

        smells.sort(key=lambda s: (s["line"], s["type"]))

        sev_order = {"high": 0, "medium": 1, "low": 2}
        counts = {}
        for s in smells:
            counts[s["severity"]] = counts.get(s["severity"], 0) + 1
        if smells:
            parts = ["%d %s" % (counts[k], k) for k in sorted(counts, key=lambda x: sev_order.get(x, 9))]
            summary = "%d smell(s) found: %s." % (len(smells), ", ".join(parts))
        else:
            summary = "No structural code smells detected by the built-in heuristics."

        print(json.dumps({
            "smells": smells,
            "smell_count": len(smells),
            "summary": summary,
        }, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "code_smell_detect failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
