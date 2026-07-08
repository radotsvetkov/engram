#!/usr/bin/env python3
"""graphql_query_validate — Engram skill (no network). Lightweight GraphQL query
sanity checker.

NOT a full GraphQL grammar parser — this only checks brace/paren/bracket
balance, that the document starts with a recognized operation keyword (or '{'
for an anonymous query), duplicate top-level named operations, and a rough
field-count estimate. It cannot catch schema-level or semantic errors.

Request (stdin): {"query": "query GetUser { user(id: 1) { id name } }"}
Output (stdout): {valid, operation_type, braces_balanced, duplicate_operation_names,
                   field_count_estimate, errors, note}
"""
import json
import re
import sys

_OP_KEYWORD_RE = re.compile(r'^(query|mutation|subscription|fragment)\b')
_NAMED_OP_RE = re.compile(r'(?:query|mutation|subscription)\s+(\w+)')
_FIELD_WITH_SELECTION_RE = re.compile(r'[A-Za-z_][A-Za-z0-9_]*\s*(?:\([^)]*\))?\s*\{')
_LEAF_LINE_RE = re.compile(r'^[A-Za-z_][A-Za-z0-9_]*(\s*:\s*[A-Za-z_][A-Za-z0-9_]*)?(\([^)]*\))?$')
_LEAF_KEYWORDS = {"query", "mutation", "subscription", "fragment", "on"}

_NOTE = "this is a lightweight sanity check, not a full GraphQL grammar parser"
_EXAMPLE = {"query": "query GetUser { user(id: 1) { id name } }"}


def _check_balance(s):
    pairs = {')': '(', ']': '[', '}': '{'}
    opens = set(pairs.values())
    stack = []
    in_string = False
    escape = False
    for i, ch in enumerate(s):
        if in_string:
            if escape:
                escape = False
            elif ch == '\\':
                escape = True
            elif ch == '"':
                in_string = False
            continue
        if ch == '"':
            in_string = True
            continue
        if ch in opens:
            stack.append((ch, i))
        elif ch in pairs:
            if not stack or stack[-1][0] != pairs[ch]:
                return False, i
            stack.pop()
    if stack:
        return False, stack[0][1]
    return True, None


def _estimate_field_count(query):
    start = query.find('{')
    body = query[start:] if start != -1 else query
    count = len(_FIELD_WITH_SELECTION_RE.findall(body))
    for line in body.splitlines():
        s = line.strip()
        if not s or s in ("{", "}") or "{" in s or "}" in s:
            continue
        if _LEAF_LINE_RE.match(s):
            ident = s.split(":")[0].split("(")[0].strip()
            if ident.lower() not in _LEAF_KEYWORDS:
                count += 1
    return count


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e})); return 0
    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object", "example": _EXAMPLE})); return 0

    query = q.get("query")
    if not isinstance(query, str) or not query.strip():
        print(json.dumps({
            "error": "missing required field 'query' (string, GraphQL document text)",
            "example": _EXAMPLE,
        })); return 0

    try:
        errors = []

        balanced, bad_pos = _check_balance(query)
        if not balanced:
            if bad_pos is not None:
                errors.append(
                    "braces/parens/brackets are not balanced (first problem near character %d)" % bad_pos
                )
            else:
                errors.append("braces/parens/brackets are not balanced")

        trimmed = query.strip()
        m = _OP_KEYWORD_RE.match(trimmed)
        if m:
            operation_type = m.group(1)
        elif trimmed.startswith("{"):
            operation_type = "anonymous_query"
        else:
            operation_type = None
            errors.append(
                "document must start with 'query', 'mutation', 'subscription', 'fragment', "
                "or '{' for an anonymous query"
            )

        names = _NAMED_OP_RE.findall(query)
        seen = set()
        duplicate_operation_names = []
        for n in names:
            if n in seen and n not in duplicate_operation_names:
                duplicate_operation_names.append(n)
            seen.add(n)
        if duplicate_operation_names:
            errors.append("duplicate operation name(s): %s" % ", ".join(duplicate_operation_names))

        field_count_estimate = _estimate_field_count(query)

        valid = balanced and operation_type is not None and not duplicate_operation_names

        result = {
            "valid": valid,
            "operation_type": operation_type,
            "braces_balanced": balanced,
            "duplicate_operation_names": duplicate_operation_names,
            "field_count_estimate": field_count_estimate,
            "errors": errors,
            "note": _NOTE,
        }
        print(json.dumps(result, indent=2, default=str)); return 0
    except Exception as e:
        print(json.dumps({"error": "graphql_query_validate failed: %s" % e})); return 1


if __name__ == "__main__":
    sys.exit(main())
