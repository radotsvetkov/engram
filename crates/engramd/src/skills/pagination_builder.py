#!/usr/bin/env python3
"""pagination_builder — Engram skill (no network). Build a paginated SQL query,
offset-based or cursor (keyset) based.

The generated SQL is PARAMETERIZED — untrusted values (page size, cursor value)
are emitted as placeholders (? or $1) and returned in a separate `params` list,
never inlined into the SQL string. Table/column identifiers are validated to a
safe [A-Za-z0-9_] pattern since they can't be parameterized.

Request (stdin): {"strategy": "cursor", "cursor_column": "id", "last_seen_value": 4820,
  "page_size": 20, "table": "items", "order": "asc", "placeholder": "qmark"}
Output (stdout): {strategy, sql, params, note, ...}
"""
import json
import re
import sys

_IDENT_RE = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*$")


def _valid_ident(name):
    return isinstance(name, str) and bool(_IDENT_RE.match(name))


def _ph(style, n):
    """Return the nth (1-based) placeholder for the chosen style."""
    if style == "numbered":
        return "$%d" % n
    return "?"


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e}))
        return 0
    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"strategy": "cursor", "cursor_column": "id", "last_seen_value": 4820,
                        "page_size": 20, "table": "items"},
        }))
        return 0

    example = {
        "offset": {"strategy": "offset", "page": 3, "page_size": 20, "table": "items", "order": "asc"},
        "cursor": {"strategy": "cursor", "cursor_column": "id", "last_seen_value": 4820,
                   "page_size": 20, "table": "items", "order": "asc"},
    }

    strategy = (q.get("strategy") or "offset")
    if not isinstance(strategy, str) or strategy.lower() not in ("offset", "cursor"):
        print(json.dumps({"error": "strategy must be 'offset' or 'cursor'", "example": example}))
        return 0
    strategy = strategy.lower()

    table = q.get("table") or "items"
    order = (q.get("order") or "asc")
    order = order.lower() if isinstance(order, str) else "asc"
    if order not in ("asc", "desc"):
        order = "asc"

    # Placeholder style: qmark (?) default, or numbered ($1) for postgres.
    ph_style = q.get("placeholder") or q.get("placeholder_style") or "qmark"
    ph_style = "numbered" if str(ph_style).lower() in ("numbered", "postgres", "dollar", "$") else "qmark"

    if not _valid_ident(table):
        print(json.dumps({
            "error": "invalid table identifier %r (allowed: letters, digits, underscore; "
                     "must start with a letter/underscore)" % table,
            "example": example,
        }))
        return 0

    page_size = q.get("page_size", 20)
    if not (isinstance(page_size, int) and not isinstance(page_size, bool) and page_size > 0):
        print(json.dumps({"error": "page_size must be a positive integer", "example": example}))
        return 0

    try:
        if strategy == "offset":
            page = q.get("page", 1)
            if not (isinstance(page, int) and not isinstance(page, bool) and page >= 1):
                print(json.dumps({"error": "page must be an integer >= 1", "example": example["offset"]}))
                return 0
            offset = (page - 1) * page_size

            order_by = ""
            cursor_column = q.get("cursor_column") or q.get("order_by_column")
            if cursor_column is not None:
                if not _valid_ident(cursor_column):
                    print(json.dumps({"error": "invalid cursor_column identifier %r" % cursor_column,
                                      "example": example["offset"]}))
                    return 0
                order_by = " ORDER BY %s %s" % (cursor_column, order.upper())

            sql = "SELECT * FROM %s%s LIMIT %s OFFSET %s;" % (
                table, order_by, _ph(ph_style, 1), _ph(ph_style, 2))
            result = {
                "strategy": "offset",
                "sql": sql,
                "params": [page_size, offset],
                "param_names": ["limit", "offset"],
                "page": page,
                "page_size": page_size,
                "offset": offset,
                "total_pages_note": "total_pages = ceil(total_row_count / page_size); getting "
                                    "total_row_count needs a separate SELECT COUNT(*) query.",
                "note": ("OFFSET pagination is simple and supports random page access, but gets "
                         "SLOWER the deeper you page (the DB must scan and discard OFFSET rows), "
                         "and rows can DRIFT/duplicate/skip if data is inserted or deleted between "
                         "page loads. Prefer cursor/keyset pagination for large or fast-changing "
                         "datasets."),
            }
            print(json.dumps(result, indent=2, default=str))
            return 0

        # cursor / keyset
        cursor_column = q.get("cursor_column") or "id"
        if not _valid_ident(cursor_column):
            print(json.dumps({"error": "invalid cursor_column identifier %r" % cursor_column,
                              "example": example["cursor"]}))
            return 0

        has_cursor = "last_seen_value" in q and q.get("last_seen_value") is not None
        comparator = ">" if order == "asc" else "<"

        params = []
        param_names = []
        where = ""
        if has_cursor:
            where = " WHERE %s %s %s" % (cursor_column, comparator, _ph(ph_style, 1))
            params.append(q.get("last_seen_value"))
            param_names.append("last_seen_value")

        limit_ph = _ph(ph_style, len(params) + 1)
        sql = "SELECT * FROM %s%s ORDER BY %s %s LIMIT %s;" % (
            table, where, cursor_column, order.upper(), limit_ph)
        params.append(page_size)
        param_names.append("limit")

        result = {
            "strategy": "cursor",
            "sql": sql,
            "params": params,
            "param_names": param_names,
            "cursor_column": cursor_column,
            "order": order,
            "page_size": page_size,
            "first_page": not has_cursor,
            "next_cursor_hint": ("use the %s value of the LAST row in this page as "
                                 "'last_seen_value' for the next request" % cursor_column),
            "note": ("Cursor (keyset) pagination is STABLE under inserts/deletes and stays FAST "
                     "at any depth (it seeks via the index instead of counting past OFFSET rows), "
                     "but it cannot jump to an arbitrary page number and requires ordering on a "
                     "unique, sequential column (add a tiebreaker like the primary key if the "
                     "cursor column isn't unique). On the first page, omit 'last_seen_value'."),
        }
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "pagination_builder failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
