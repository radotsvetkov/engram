#!/usr/bin/env python3
"""index_advisor — Engram skill (no network). Suggest database indexes for a
query.

Either parses a simple SQL SELECT (regex extraction of WHERE / JOIN / ORDER BY
columns — best-effort, NOT a real SQL parser, so complex queries may be missed)
or uses explicit column lists you pass in. Recommends a composite index
(equality columns first, then range/ORDER BY), separate indexes for join keys,
and flags covering-index opportunities.

Request (stdin): {"query": "SELECT id,name FROM users WHERE status='active' AND
  created_at > '2024-01-01' ORDER BY created_at DESC"}
  OR {"where_columns": ["status"], "order_by_columns": ["created_at"],
      "join_columns": ["org_id"], "table": "users"}
Output (stdout): {table, recommended_indexes: [{columns, reason, ddl}], notes, parsed?}
"""
import json
import re
import sys

# Columns compared with = are good index prefixes; <,>,<=,>=,BETWEEN,LIKE are ranges.
_EQ_RE = re.compile(r"([A-Za-z_][\w\.]*)\s*=\s*(?:%s|\$\d+|\?|'[^']*'|\"[^\"]*\"|[-\w\.]+)")
_RANGE_RE = re.compile(
    r"([A-Za-z_][\w\.]*)\s*(?:<=|>=|<|>|\bBETWEEN\b|\bLIKE\b)", re.I)
_ORDER_RE = re.compile(r"\bORDER\s+BY\s+(.+?)(?:\bLIMIT\b|\bOFFSET\b|;|$)", re.I | re.S)
_JOIN_RE = re.compile(
    r"\bJOIN\b\s+[A-Za-z_][\w]*(?:\s+(?:AS\s+)?[A-Za-z_][\w]*)?\s+ON\s+(.+?)"
    r"(?=\bJOIN\b|\bWHERE\b|\bGROUP\b|\bORDER\b|\bLIMIT\b|;|$)", re.I | re.S)
_WHERE_RE = re.compile(
    r"\bWHERE\b(.+?)(?=\bGROUP\b|\bORDER\b|\bLIMIT\b|\bOFFSET\b|;|$)", re.I | re.S)
_FROM_RE = re.compile(r"\bFROM\s+([A-Za-z_][\w]*)", re.I)
_SELECT_COLS_RE = re.compile(r"\bSELECT\b\s+(.+?)\bFROM\b", re.I | re.S)


def _strip_alias(col):
    """table.col -> col ; keep bare col."""
    col = col.strip().strip('`"[]')
    if "." in col:
        col = col.split(".")[-1]
    return col.strip('`"[]')


def _dedup(seq):
    seen = []
    for x in seq:
        if x and x not in seen:
            seen.append(x)
    return seen


def _parse_sql(query):
    out = {"where_eq": [], "where_range": [], "join": [], "order_by": [], "select": []}
    where_m = _WHERE_RE.search(query)
    where_clause = where_m.group(1) if where_m else ""
    if where_clause:
        out["where_eq"] = _dedup(_strip_alias(m) for m in _EQ_RE.findall(where_clause))
        ranges = _dedup(_strip_alias(m) for m in _RANGE_RE.findall(where_clause))
        # A column can't be both an equality and a range prefix — prefer equality.
        out["where_range"] = [c for c in ranges if c not in out["where_eq"]]

    for on_clause in _JOIN_RE.findall(query):
        for a, b in re.findall(r"([A-Za-z_][\w\.]*)\s*=\s*([A-Za-z_][\w\.]*)", on_clause):
            out["join"].append(_strip_alias(a))
            out["join"].append(_strip_alias(b))
    out["join"] = _dedup(out["join"])

    order_m = _ORDER_RE.search(query)
    if order_m:
        for piece in order_m.group(1).split(","):
            piece = re.sub(r"\b(ASC|DESC)\b", "", piece, flags=re.I).strip()
            if piece:
                out["order_by"].append(_strip_alias(piece))
    out["order_by"] = _dedup(out["order_by"])

    sel_m = _SELECT_COLS_RE.search(query)
    if sel_m and "*" not in sel_m.group(1):
        out["select"] = _dedup(_strip_alias(c) for c in sel_m.group(1).split(","))
    return out


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e}))
        return 0
    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"query": "SELECT id FROM users WHERE status='active' ORDER BY created_at"},
        }))
        return 0

    example = {"query": "SELECT id,name FROM users WHERE status='active' AND created_at > '2024-01-01' ORDER BY created_at DESC"}

    query = q.get("query")
    where_cols = q.get("where_columns")
    join_cols = q.get("join_columns")
    order_cols = q.get("order_by_columns")
    table = q.get("table") or "your_table"

    have_explicit = any(isinstance(x, list) and x for x in (where_cols, join_cols, order_cols))
    if not (isinstance(query, str) and query.strip()) and not have_explicit:
        print(json.dumps({
            "error": "provide a 'query' string or explicit column lists "
                     "(where_columns / join_columns / order_by_columns)",
            "example": example,
        }))
        return 0

    try:
        parsed = None
        eq_cols, range_cols, join_keys, ob_cols, select_cols = [], [], [], [], []

        if isinstance(query, str) and query.strip():
            parsed = _parse_sql(query)
            eq_cols = parsed["where_eq"]
            range_cols = parsed["where_range"]
            join_keys = parsed["join"]
            ob_cols = parsed["order_by"]
            select_cols = parsed["select"]
            from_m = _FROM_RE.search(query)
            if from_m and (not q.get("table")):
                table = from_m.group(1)

        # Explicit lists override / supplement parsed output.
        if isinstance(where_cols, list) and where_cols:
            eq_cols = _dedup([str(c).strip() for c in where_cols if str(c).strip()])
            range_cols = []
        if isinstance(join_cols, list) and join_cols:
            join_keys = _dedup([str(c).strip() for c in join_cols if str(c).strip()])
        if isinstance(order_cols, list) and order_cols:
            ob_cols = _dedup([str(c).strip() for c in order_cols if str(c).strip()])

        recommended = []
        notes = [
            "SQL parsing is best-effort via regex, NOT a real parser — verify the "
            "extracted columns before applying, especially for subqueries, functions "
            "on columns (which defeat indexes), or dialect-specific syntax.",
            "Index selectivity matters: an index on a low-cardinality column (e.g. a "
            "boolean) rarely helps. Order composite-index columns by selectivity where "
            "you can.",
            "Every index speeds reads but slows writes (INSERT/UPDATE/DELETE) and uses "
            "storage — don't over-index; add indexes to match real query patterns.",
        ]

        # 1) Composite index: equality columns first, then range/ORDER BY columns.
        # (Equality-before-range is the classic B-tree left-prefix rule.)
        range_and_order = _dedup(range_cols + ob_cols)
        composite = _dedup(eq_cols + range_and_order)
        # Remove join keys from the composite (handled separately below).
        composite = [c for c in composite if c not in set(join_keys)]
        if composite:
            reason_bits = []
            if eq_cols:
                reason_bits.append("equality filter(s) on %s (index prefix)" % ", ".join(eq_cols))
            if range_cols:
                reason_bits.append("range filter(s) on %s" % ", ".join(range_cols))
            if ob_cols:
                reason_bits.append("ORDER BY on %s (a trailing sort column lets the index "
                                   "avoid a filesort)" % ", ".join(ob_cols))
            recommended.append({
                "columns": composite,
                "reason": "; ".join(reason_bits) or "columns used to filter/sort this query",
                "ddl": _index_ddl(table, composite),
            })

        # 2) Separate single-column indexes for each join key.
        for jk in join_keys:
            recommended.append({
                "columns": [jk],
                "reason": "join key %r — index both sides of a join to avoid full scans" % jk,
                "ddl": _index_ddl(table, [jk]),
            })

        # 3) Covering-index opportunity note.
        if select_cols and composite:
            extra = [c for c in select_cols if c not in set(composite)]
            if extra:
                notes.append(
                    "covering-index opportunity: the query also SELECTs %s. Adding these as "
                    "INCLUDE columns (Postgres) or trailing key columns lets the index satisfy "
                    "the query without touching the table (index-only scan)." % ", ".join(extra))

        if not recommended:
            notes.append("no filter, join, or sort columns were identified — no index "
                         "recommended. Indexes help WHERE / JOIN / ORDER BY, not unfiltered scans.")

        result = {"table": table, "recommended_indexes": recommended, "notes": notes}
        if parsed is not None:
            result["parsed"] = {
                "equality_columns": eq_cols,
                "range_columns": range_cols,
                "join_columns": join_keys,
                "order_by_columns": ob_cols,
                "select_columns": select_cols,
            }
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "index_advisor failed: %s" % e}))
        return 1


def _index_ddl(table, columns):
    safe_tbl = re.sub(r"[^\w]", "", str(table)) or "tbl"
    idx_name = "idx_%s_%s" % (safe_tbl, "_".join(re.sub(r"[^\w]", "", c) for c in columns))
    return "CREATE INDEX %s ON %s (%s);" % (idx_name, table, ", ".join(columns))


if __name__ == "__main__":
    sys.exit(main())
