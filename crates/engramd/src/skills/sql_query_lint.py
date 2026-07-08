#!/usr/bin/env python3
"""sql_query_lint — Engram skill (no network). Lightweight heuristic review
of a SQL query string using regex/text checks — NOT a real SQL parser, so it
can miss issues in complex or dialect-specific queries. It only ever sees the
final SQL text, so it cannot detect unparameterized dynamic SQL / injection
risk — that must be checked in the calling code that builds the query, not
here.

Flags: UPDATE/DELETE with no WHERE clause anywhere in the query (critical);
SELECT * (warning); a SELECT with no LIMIT, no aggregate function, and no
WHERE clause (info).

Request (stdin): {"query": "DELETE FROM users"}
Output (stdout): {warnings: [{severity, issue}], query_type}
"""
import json
import re
import sys

_AGG_FUNCS_RE = re.compile(r"\b(COUNT|SUM|AVG|MIN|MAX)\s*\(", re.I)
_WHERE_RE = re.compile(r"\bWHERE\b", re.I)
_LIMIT_RE = re.compile(r"\bLIMIT\b", re.I)
_SELECT_STAR_RE = re.compile(r"\bSELECT\s+\*", re.I)
_UPDATE_SET_RE = re.compile(r"\bUPDATE\b.*\bSET\b", re.I | re.S)
_DELETE_FROM_RE = re.compile(r"\bDELETE\s+FROM\b", re.I)


def _detect_query_type(query):
    m = re.search(r"[A-Za-z]+", query)
    if not m:
        return "other"
    first = m.group(0).upper()
    if first in ("SELECT", "UPDATE", "DELETE", "INSERT"):
        return first
    return "other"


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e}))
        return 0
    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"query": "SELECT * FROM users"},
        }))
        return 0

    query = q.get("query")
    if not isinstance(query, str) or not query.strip():
        print(json.dumps({
            "error": "missing required field 'query' (non-empty SQL string)",
            "example": {"query": "DELETE FROM users WHERE id = 1"},
        }))
        return 0
    query = query.strip()

    try:
        query_type = _detect_query_type(query)
        has_where = bool(_WHERE_RE.search(query))
        warnings = []

        is_unfiltered_mutation = (_UPDATE_SET_RE.search(query) or _DELETE_FROM_RE.search(query)) and not has_where
        if is_unfiltered_mutation:
            warnings.append({
                "severity": "critical",
                "issue": "UPDATE/DELETE without a WHERE clause affects every row in the table",
            })

        if _SELECT_STAR_RE.search(query):
            warnings.append({
                "severity": "warning",
                "issue": "SELECT * pulls all columns — prefer explicit column lists for stability and performance",
            })

        if query_type == "SELECT" and not _LIMIT_RE.search(query) and not _AGG_FUNCS_RE.search(query) and not has_where:
            warnings.append({
                "severity": "info",
                "issue": "consider a LIMIT — this query has no filter and could return the entire table",
            })

        result = {"warnings": warnings, "query_type": query_type}
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "sql_query_lint failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
