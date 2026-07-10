#!/usr/bin/env python3
"""sql_format — Engram skill (no network). Best-effort SQL pretty-printer.

A pure-heuristic formatter (no sqlparse lib): uppercases recognized keywords,
puts major clauses (SELECT/FROM/WHERE/GROUP BY/ORDER BY/HAVING/LIMIT/JOIN/...) on
their own lines, indents JOIN and AND/OR, and splits the SELECT column list onto
indented lines. It is readability formatting, not a parser — it degrades
gracefully on unusual SQL rather than erroring.

Request (stdin): {"sql": "select a,b from t where x=1", "uppercase_keywords"?: true, "indent"?: 2}
Output (stdout): {formatted}
"""
import json, sys, re

# Keywords to uppercase (word-boundary, case-insensitive).
_KEYWORDS = [
    "SELECT", "DISTINCT", "FROM", "WHERE", "GROUP BY", "ORDER BY", "HAVING",
    "LIMIT", "OFFSET", "INSERT INTO", "VALUES", "UPDATE", "SET", "DELETE",
    "INNER JOIN", "LEFT OUTER JOIN", "RIGHT OUTER JOIN", "FULL OUTER JOIN",
    "LEFT JOIN", "RIGHT JOIN", "FULL JOIN", "CROSS JOIN", "OUTER JOIN", "JOIN",
    "ON", "AND", "OR", "NOT", "IN", "IS", "NULL", "LIKE", "BETWEEN", "AS",
    "UNION ALL", "UNION", "ASC", "DESC", "INTO", "COUNT", "SUM", "AVG", "MIN",
    "MAX", "EXISTS", "CASE", "WHEN", "THEN", "ELSE", "END",
]
# Longer phrases first so "GROUP BY" wins over "GROUP".
_KEYWORDS.sort(key=lambda k: -len(k))

# Clauses that start a new line at indent 0.
_NEWLINE_CLAUSES = [
    "SELECT", "FROM", "WHERE", "GROUP BY", "ORDER BY", "HAVING", "LIMIT",
    "OFFSET", "INSERT INTO", "VALUES", "UPDATE", "SET", "DELETE", "UNION ALL",
    "UNION",
]
# Clauses that start a new indented line.
_INDENT_CLAUSES = [
    "INNER JOIN", "LEFT OUTER JOIN", "RIGHT OUTER JOIN", "FULL OUTER JOIN",
    "LEFT JOIN", "RIGHT JOIN", "FULL JOIN", "CROSS JOIN", "OUTER JOIN", "JOIN",
    "AND", "OR",
]


def _uppercase_keywords(sql):
    def repl(m):
        return m.group(0).upper()
    for kw in _KEYWORDS:
        pattern = r"(?<![A-Za-z0-9_])" + re.escape(kw).replace(r"\ ", r"\s+") + r"(?![A-Za-z0-9_])"
        sql = re.sub(pattern, repl, sql, flags=re.IGNORECASE)
    return sql


def _split_top_level_commas(s):
    """Split on commas that are not inside parentheses."""
    parts = []
    depth = 0
    buf = []
    for ch in s:
        if ch == "(":
            depth += 1
            buf.append(ch)
        elif ch == ")":
            depth = max(0, depth - 1)
            buf.append(ch)
        elif ch == "," and depth == 0:
            parts.append("".join(buf).strip())
            buf = []
        else:
            buf.append(ch)
    if buf:
        parts.append("".join(buf).strip())
    return [p for p in parts if p != ""]


def _format(sql, indent):
    pad = " " * indent
    # Collapse whitespace (but this is best-effort; string literals with spaces
    # are left mostly intact since we don't reflow inside them token-by-token).
    sql = re.sub(r"\s+", " ", sql.strip())
    if sql == "":
        return ""

    # Insert newlines before newline clauses and indented clauses in ONE pass so
    # a short clause (JOIN) never re-matches inside a longer one (LEFT JOIN).
    all_clauses = sorted(set(_NEWLINE_CLAUSES + _INDENT_CLAUSES), key=lambda k: -len(k))
    indent_set = set(_INDENT_CLAUSES)
    alt = "|".join(re.escape(kw).replace(r"\ ", r"\s+") for kw in all_clauses)
    clause_re = re.compile(r"(?<![A-Za-z0-9_])(" + alt + r")(?![A-Za-z0-9_])", re.IGNORECASE)

    def _mark(m):
        norm = re.sub(r"\s+", " ", m.group(1).upper())
        marker = "\x00INDENT\x00" if norm in indent_set else "\x00NL\x00"
        return marker + m.group(1)

    sql = clause_re.sub(_mark, sql)

    # Now split on markers into lines.
    lines = []
    # Replace markers with a canonical split token but keep the kind.
    tokens = re.split(r"(\x00INDENT\x00|\x00NL\x00)", sql)
    kind = "NL"
    current = tokens[0].strip()
    if current:
        lines.append(("NL", current))
    i = 1
    while i < len(tokens):
        marker = tokens[i]
        text = tokens[i + 1].strip() if i + 1 < len(tokens) else ""
        kind = "INDENT" if marker == "\x00INDENT\x00" else "NL"
        if text:
            lines.append((kind, text))
        i += 2

    out = []
    for kind, text in lines:
        # Split SELECT column list onto indented lines.
        m = re.match(r"^(SELECT|select)(\s+DISTINCT|\s+distinct)?\s+(.*)$", text, flags=re.DOTALL)
        head = text.upper()
        if head.startswith("SELECT"):
            mm = re.match(r"^(SELECT(?:\s+DISTINCT)?)\s+(.*)$", text, flags=re.IGNORECASE | re.DOTALL)
            if mm:
                cols = _split_top_level_commas(mm.group(2))
                if len(cols) > 1:
                    out.append(mm.group(1))
                    for j, col in enumerate(cols):
                        comma = "," if j < len(cols) - 1 else ""
                        out.append(pad + col + comma)
                    continue
        if kind == "INDENT":
            out.append(pad + text)
        else:
            out.append(text)
    return "\n".join(out)


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e})); return 0

    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"sql": "select a, b from t where x = 1 and y = 2"},
        })); return 0

    sql = q.get("sql")
    if not isinstance(sql, str) or sql.strip() == "":
        print(json.dumps({
            "error": "missing required field 'sql' (the SQL statement as a string)",
            "example": {"sql": "select id, name from users u join orders o on o.uid=u.id where u.active=1", "uppercase_keywords": True, "indent": 2},
        })); return 0

    uppercase = q.get("uppercase_keywords")
    if uppercase is None:
        uppercase = True
    indent = q.get("indent")
    if not isinstance(indent, int) or isinstance(indent, bool) or indent < 0 or indent > 16:
        indent = 2

    try:
        work = sql
        if uppercase:
            work = _uppercase_keywords(work)
        formatted = _format(work, indent)
        print(json.dumps({"formatted": formatted}, indent=2, default=str)); return 0
    except Exception:
        # Degrade gracefully: return the input trimmed rather than erroring.
        print(json.dumps({"formatted": sql.strip(), "note": "returned input unchanged (could not format)"}, default=str)); return 0


if __name__ == "__main__":
    sys.exit(main())
