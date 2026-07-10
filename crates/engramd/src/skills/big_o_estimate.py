#!/usr/bin/env python3
"""big_o_estimate — Engram skill (no network). Rough Big-O estimate for a Python
snippet from its structure, or a lookup for a named algorithm.

With `code`: parses with `ast` and estimates from the maximum nesting depth of
For/While loops and nested comprehensions (depth d -> O(n^d)), flags direct
self-recursion, and adds an O(n log n) term for sort()/sorted() calls. With
`pattern`: maps a named algorithm (e.g. "binary search") to its known
complexity via a static table. This is a structural heuristic, not a proof.

Request (stdin): {"code": "def f(a):\\n    for i in a:\\n        for j in a:\\n            print(i, j)\\n"}
             or  {"pattern": "binary search"}
Output (stdout): {estimated_time_complexity, max_loop_nesting, has_recursion, has_sort, reasoning, caveat}
"""
import ast
import json
import sys

_EXAMPLE = {"code": "def f(a):\n    for i in a:\n        for j in a:\n            print(i, j)\n"}

_PATTERN_TABLE = {
    "binary search": "O(log n)",
    "linear search": "O(n)",
    "bubble sort": "O(n^2)",
    "insertion sort": "O(n^2)",
    "selection sort": "O(n^2)",
    "merge sort": "O(n log n)",
    "quick sort": "O(n log n)",
    "quicksort": "O(n log n)",
    "heap sort": "O(n log n)",
    "heapsort": "O(n log n)",
    "hash lookup": "O(1)",
    "hash table lookup": "O(1)",
    "dictionary lookup": "O(1)",
    "bfs": "O(V + E)",
    "breadth first search": "O(V + E)",
    "dfs": "O(V + E)",
    "depth first search": "O(V + E)",
    "dijkstra": "O((V + E) log V)",
    "matrix multiplication": "O(n^3)",
    "fibonacci recursive": "O(2^n)",
    "naive fibonacci": "O(2^n)",
    "factorial": "O(n)",
    "two sum": "O(n)",
}


def _big_o(depth):
    if depth <= 0:
        return "O(1)"
    if depth == 1:
        return "O(n)"
    return "O(n^%d)" % depth


def _loop_depth(node, current=0):
    """Max nesting of For/While/comprehension inside `node` (exclusive of node itself)."""
    best = current

    def walk(n, depth):
        nonlocal best
        for child in ast.iter_child_nodes(n):
            if isinstance(child, (ast.For, ast.While, ast.AsyncFor)):
                d = depth + 1
                best = max(best, d)
                walk(child, d)
            elif isinstance(child, (ast.ListComp, ast.SetComp, ast.DictComp, ast.GeneratorExp)):
                # each `for` generator clause is a nesting level
                gens = len(getattr(child, "generators", []))
                d = depth + max(gens, 1)
                best = max(best, d)
                walk(child, d)
            else:
                walk(child, depth)

    walk(node, current)
    return best


def _detect_recursion(tree):
    recursive = []
    for node in ast.walk(tree):
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
            name = node.name
            for inner in ast.walk(node):
                if isinstance(inner, ast.Call):
                    f = inner.func
                    if isinstance(f, ast.Name) and f.id == name:
                        recursive.append(name)
                        break
                    if isinstance(f, ast.Attribute) and f.attr == name:
                        recursive.append(name)
                        break
    return recursive


def _detect_sort(tree):
    for node in ast.walk(tree):
        if isinstance(node, ast.Call):
            f = node.func
            if isinstance(f, ast.Name) and f.id == "sorted":
                return True
            if isinstance(f, ast.Attribute) and f.attr == "sort":
                return True
    return False


def _handle_code(code):
    tree = ast.parse(code)
    max_nesting = _loop_depth(tree, 0)
    recursive = _detect_recursion(tree)
    has_recursion = bool(recursive)
    has_sort = _detect_sort(tree)

    reasoning = []
    loop_term = _big_o(max_nesting)
    reasoning.append("Maximum loop/comprehension nesting depth is %d -> %s from loops." % (
        max_nesting, loop_term))

    complexity = loop_term
    if has_sort:
        reasoning.append("A sort() / sorted() call contributes an O(n log n) term.")
        if max_nesting <= 1:
            complexity = "O(n log n)"
        else:
            reasoning.append("Loop nesting dominates the sort term, so %s stands." % complexity)
    if has_recursion:
        reasoning.append(
            "Detected direct self-recursion in: %s. Recursive cost depends on the branching "
            "factor and depth (could be anything from O(log n) to exponential) and is NOT "
            "captured by loop nesting — inspect the recurrence manually." % ", ".join(sorted(set(recursive))))

    return {
        "estimated_time_complexity": complexity,
        "max_loop_nesting": max_nesting,
        "has_recursion": has_recursion,
        "recursive_functions": sorted(set(recursive)),
        "has_sort": has_sort,
        "reasoning": reasoning,
        "caveat": "This is a structural heuristic based on loop nesting / sort / recursion detection, "
                  "not a proof. It ignores hidden costs (e.g. O(n) work inside a single 'in' test, "
                  "library call complexity) and recursive recurrences.",
    }


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
    pattern = q.get("pattern")

    if isinstance(code, str) and code.strip():
        try:
            tree_ok = ast.parse(code)  # validate first for a clean error
            del tree_ok
        except SyntaxError as e:
            print(json.dumps({
                "error": "code does not parse as Python: %s" % e.msg,
                "line": e.lineno,
                "offset": e.offset,
            }))
            return 0
        try:
            result = _handle_code(code)
            print(json.dumps(result, indent=2, default=str))
            return 0
        except Exception as e:
            print(json.dumps({"error": "big_o_estimate failed: %s" % e}))
            return 1

    if isinstance(pattern, str) and pattern.strip():
        key = pattern.strip().lower()
        complexity = _PATTERN_TABLE.get(key)
        if complexity is None:
            print(json.dumps({
                "pattern": pattern,
                "estimated_time_complexity": None,
                "note": "No entry for %r in the known-algorithm table." % pattern,
                "known_patterns": sorted(_PATTERN_TABLE.keys()),
            }, indent=2, default=str))
            return 0
        print(json.dumps({
            "pattern": pattern,
            "estimated_time_complexity": complexity,
            "caveat": "Textbook average/typical complexity for the named algorithm; a specific "
                      "implementation may differ.",
        }, indent=2, default=str))
        return 0

    print(json.dumps({
        "error": "provide either 'code' (Python source) or 'pattern' (named algorithm)",
        "example": _EXAMPLE,
    }))
    return 0


if __name__ == "__main__":
    sys.exit(main())
