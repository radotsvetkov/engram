#!/usr/bin/env python3
"""migration_template — Engram skill (no network). Scaffold a paired up/down
database migration for a common schema change.

Supports create_table, add_column, drop_column, add_index, and rename (table or
column) for postgres/mysql. Emits forward (up) SQL and its reversing (down) SQL,
plus a timestamp-prefixed migration name. Destructive/irreversible operations
carry a warning — a down migration cannot restore data lost by dropping a column
or table.

Request (stdin): {"name": "add email to users", "operation": "add_column",
  "table": "users", "details": {"column": "email", "type": "VARCHAR(255)",
  "nullable": false}, "dialect": "postgres"}
Output (stdout): {migration_name, operation, up_sql, down_sql, warning, notes}
"""
import json
import re
import sys
from datetime import datetime, timezone

_OPS = ("create_table", "add_column", "drop_column", "add_index", "rename")
_IDENT_RE = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*$")


def _slug(name):
    s = re.sub(r"[^a-z0-9]+", "_", (name or "migration").lower()).strip("_")
    return s or "migration"


def _q(name, dialect):
    if dialect == "mysql":
        return "`%s`" % str(name).replace("`", "``")
    return '"%s"' % str(name).replace('"', '""')


def _valid_ident(name):
    return isinstance(name, str) and bool(_IDENT_RE.match(name))


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e}))
        return 0
    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"name": "add email to users", "operation": "add_column",
                        "table": "users", "details": {"column": "email", "type": "VARCHAR(255)"}},
        }))
        return 0

    example = {"name": "add email to users", "operation": "add_column", "table": "users",
               "details": {"column": "email", "type": "VARCHAR(255)", "nullable": False},
               "dialect": "postgres"}

    name = q.get("name")
    operation = q.get("operation")
    table = q.get("table")
    details = q.get("details") or {}
    if not isinstance(details, dict):
        details = {}
    dialect = (q.get("dialect") or "postgres")
    dialect = dialect.lower() if isinstance(dialect, str) else "postgres"
    if dialect not in ("postgres", "mysql"):
        dialect = "postgres"

    if not isinstance(name, str) or not name.strip():
        print(json.dumps({"error": "missing required field 'name' (short description)", "example": example}))
        return 0
    if not isinstance(operation, str) or operation.lower() not in _OPS:
        print(json.dumps({
            "error": "operation must be one of: %s" % ", ".join(_OPS),
            "example": example,
        }))
        return 0
    operation = operation.lower()
    if not isinstance(table, str) or not table.strip():
        print(json.dumps({"error": "missing required field 'table'", "example": example}))
        return 0
    if not _valid_ident(table.strip()):
        print(json.dumps({"error": "invalid table identifier %r" % table, "example": example}))
        return 0
    table = table.strip()

    ts = datetime.now(timezone.utc).strftime("%Y%m%d%H%M%S")
    migration_name = "%s_%s" % (ts, _slug(name))

    try:
        up, down, warning = _emit(operation, table, details, dialect)
        notes = [
            "Review the generated SQL before running it — details are filled from your "
            "'details' object and defaults where fields were omitted.",
            "Wrap each migration in a transaction where your engine supports transactional "
            "DDL (Postgres does; MySQL mostly does NOT — DDL auto-commits, so a failed "
            "multi-statement migration can leave the schema half-applied).",
        ]
        result = {
            "migration_name": migration_name,
            "operation": operation,
            "dialect": dialect,
            "up_sql": up,
            "down_sql": down,
            "warning": warning,
            "notes": notes,
        }
        print(json.dumps(result, indent=2, default=str))
        return 0
    except ValueError as e:
        print(json.dumps({"error": str(e), "example": example}))
        return 0
    except Exception as e:
        print(json.dumps({"error": "migration_template failed: %s" % e}))
        return 1


def _emit(operation, table, details, dialect):
    """Return (up_sql, down_sql, warning)."""
    tq = _q(table, dialect)

    if operation == "create_table":
        cols = details.get("columns")
        if not isinstance(cols, list) or not cols:
            # Provide a minimal scaffold to fill in.
            id_type = "SERIAL PRIMARY KEY" if dialect == "postgres" else "INT AUTO_INCREMENT PRIMARY KEY"
            up = ("CREATE TABLE %s (\n  %s %s,\n  -- TODO: add columns\n  %s TIMESTAMP DEFAULT %s\n);"
                  % (tq, _q("id", dialect), id_type, _q("created_at", dialect),
                     "CURRENT_TIMESTAMP"))
        else:
            lines = []
            for c in cols:
                if not isinstance(c, dict):
                    continue
                cn = c.get("name")
                ct = c.get("type", "TEXT")
                if not _valid_ident(str(cn)):
                    raise ValueError("create_table column name %r is invalid" % cn)
                seg = "%s %s" % (_q(cn, dialect), str(ct).upper())
                if c.get("primary_key"):
                    seg += " PRIMARY KEY"
                if c.get("nullable") is False and not c.get("primary_key"):
                    seg += " NOT NULL"
                lines.append("  " + seg)
            up = "CREATE TABLE %s (\n%s\n);" % (tq, ",\n".join(lines))
        down = "DROP TABLE %s;" % tq
        warning = ("DESTRUCTIVE on rollback: the down migration DROPs the table, permanently "
                   "deleting all its data. This cannot be undone.")
        return up, down, warning

    if operation == "add_column":
        col = details.get("column") or details.get("name")
        if not _valid_ident(str(col)):
            raise ValueError("add_column requires details.column (a valid identifier)")
        ctype = str(details.get("type", "TEXT")).upper()
        cq = _q(col, dialect)
        segment = "%s %s" % (cq, ctype)
        if details.get("nullable") is False:
            segment += " NOT NULL"
        if "default" in details and details.get("default") is not None:
            dv = details.get("default")
            if isinstance(dv, bool):
                segment += " DEFAULT %s" % (("TRUE" if dv else "FALSE") if dialect == "postgres" else ("1" if dv else "0"))
            elif isinstance(dv, (int, float)):
                segment += " DEFAULT %s" % dv
            else:
                segment += " DEFAULT '%s'" % str(dv).replace("'", "''")
        up = "ALTER TABLE %s ADD COLUMN %s;" % (tq, segment)
        down = "ALTER TABLE %s DROP COLUMN %s;" % (tq, cq)
        warning = None
        if details.get("nullable") is False and "default" not in details:
            warning = ("adding a NOT NULL column WITHOUT a default fails on a non-empty table. "
                       "Add a default, or run it in steps: add nullable -> backfill -> set NOT NULL.")
        return up, down, warning

    if operation == "drop_column":
        col = details.get("column") or details.get("name")
        if not _valid_ident(str(col)):
            raise ValueError("drop_column requires details.column (a valid identifier)")
        cq = _q(col, dialect)
        col_type = str(details.get("type", "TEXT")).upper()
        up = "ALTER TABLE %s DROP COLUMN %s;" % (tq, cq)
        # Down can only re-add the column shape — the DATA is gone.
        down = ("ALTER TABLE %s ADD COLUMN %s %s;  -- NOTE: recreates the column but CANNOT "
                "restore dropped data" % (tq, cq, col_type))
        warning = ("DESTRUCTIVE and IRREVERSIBLE: dropping a column permanently deletes its data. "
                   "The down migration only re-adds an empty column of type %s — the original "
                   "values are lost. Back up / export the column before applying." % col_type)
        return up, down, warning

    if operation == "add_index":
        cols = details.get("columns")
        if isinstance(cols, str):
            cols = [cols]
        if not isinstance(cols, list) or not cols:
            raise ValueError("add_index requires details.columns (a column name or list)")
        for c in cols:
            if not _valid_ident(str(c)):
                raise ValueError("add_index column %r is invalid" % c)
        unique = bool(details.get("unique"))
        idx = details.get("index_name") or ("idx_%s_%s" % (table, "_".join(str(c) for c in cols)))
        if not _valid_ident(str(idx)):
            raise ValueError("index_name %r is invalid" % idx)
        iq = _q(idx, dialect)
        col_list = ", ".join(_q(c, dialect) for c in cols)
        up = "CREATE %sINDEX %s ON %s (%s);" % ("UNIQUE " if unique else "", iq, tq, col_list)
        if dialect == "mysql":
            down = "DROP INDEX %s ON %s;" % (iq, tq)
        else:
            down = "DROP INDEX %s;" % iq
        warning = None
        return up, down, warning

    if operation == "rename":
        # Rename a table (details.new_name) or a column (details.column + details.new_name).
        new_name = details.get("new_name") or details.get("to")
        if not _valid_ident(str(new_name)):
            raise ValueError("rename requires details.new_name (a valid identifier)")
        col = details.get("column") or details.get("from_column")
        if col:
            if not _valid_ident(str(col)):
                raise ValueError("rename column %r is invalid" % col)
            cq = _q(col, dialect)
            nq = _q(new_name, dialect)
            up = "ALTER TABLE %s RENAME COLUMN %s TO %s;" % (tq, cq, nq)
            down = "ALTER TABLE %s RENAME COLUMN %s TO %s;" % (tq, nq, cq)
        else:
            nq = _q(new_name, dialect)
            if dialect == "mysql":
                up = "RENAME TABLE %s TO %s;" % (tq, nq)
                down = "RENAME TABLE %s TO %s;" % (nq, tq)
            else:
                up = "ALTER TABLE %s RENAME TO %s;" % (tq, nq)
                down = "ALTER TABLE %s RENAME TO %s;" % (nq, tq)
        warning = ("renames can break application code, views, and foreign keys that reference the "
                   "old name — deploy the code change and the migration together, or use a "
                   "expand/contract (add new -> dual-write -> drop old) rollout.")
        return up, down, warning

    raise ValueError("unsupported operation %r" % operation)


if __name__ == "__main__":
    sys.exit(main())
