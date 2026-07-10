#!/usr/bin/env python3
"""sql_ddl_gen — Engram skill (no network). Generate a CREATE TABLE statement.

Maps generic column types (int/bigint/text/varchar(n)/bool/timestamp/uuid/
decimal/serial) to dialect-specific SQL for postgres/mysql/sqlite and emits
PRIMARY KEY, NOT NULL, UNIQUE, DEFAULT and FOREIGN KEY ... REFERENCES clauses.
Best-effort string assembly — not a validating SQL compiler.

Request (stdin): {"table": "users", "columns": [{"name": "id", "type": "serial",
  "primary_key": true}, {"name": "email", "type": "varchar(255)", "unique": true,
  "nullable": false}], "dialect": "postgres"}
Output (stdout): {dialect, ddl}
"""
import json
import re
import sys

_DIALECTS = ("postgres", "mysql", "sqlite")

# Generic base type -> per-dialect concrete type.
_TYPE_MAP = {
    "int": {"postgres": "INTEGER", "mysql": "INT", "sqlite": "INTEGER"},
    "integer": {"postgres": "INTEGER", "mysql": "INT", "sqlite": "INTEGER"},
    "bigint": {"postgres": "BIGINT", "mysql": "BIGINT", "sqlite": "INTEGER"},
    "smallint": {"postgres": "SMALLINT", "mysql": "SMALLINT", "sqlite": "INTEGER"},
    "text": {"postgres": "TEXT", "mysql": "TEXT", "sqlite": "TEXT"},
    "bool": {"postgres": "BOOLEAN", "mysql": "TINYINT(1)", "sqlite": "INTEGER"},
    "boolean": {"postgres": "BOOLEAN", "mysql": "TINYINT(1)", "sqlite": "INTEGER"},
    "timestamp": {"postgres": "TIMESTAMP", "mysql": "DATETIME", "sqlite": "TEXT"},
    "datetime": {"postgres": "TIMESTAMP", "mysql": "DATETIME", "sqlite": "TEXT"},
    "date": {"postgres": "DATE", "mysql": "DATE", "sqlite": "TEXT"},
    "uuid": {"postgres": "UUID", "mysql": "CHAR(36)", "sqlite": "TEXT"},
    "float": {"postgres": "DOUBLE PRECISION", "mysql": "DOUBLE", "sqlite": "REAL"},
    "double": {"postgres": "DOUBLE PRECISION", "mysql": "DOUBLE", "sqlite": "REAL"},
    "json": {"postgres": "JSONB", "mysql": "JSON", "sqlite": "TEXT"},
}


def _map_type(raw, dialect):
    """Return (sql_type, is_serial). Handles varchar(n)/char(n)/decimal(p,s)."""
    t = raw.strip()
    low = t.lower()

    # serial / autoincrement primary keys are handled specially per dialect.
    if low in ("serial", "autoincrement", "auto_increment"):
        return None, True

    # Parameterised types: varchar(n), char(n), decimal(p,s), numeric(p,s).
    m = re.match(r"^(varchar|char|character varying|decimal|numeric)\s*(\([^)]*\))?$", low)
    if m:
        base = m.group(1)
        params = m.group(2) or ""
        if base in ("varchar", "character varying"):
            name = "VARCHAR" if dialect != "sqlite" else "TEXT"
            if dialect == "sqlite":
                return "TEXT", False
            return "%s%s" % (name, params if params else "(255)"), False
        if base == "char":
            if dialect == "sqlite":
                return "TEXT", False
            return "CHAR%s" % (params if params else "(1)"), False
        # decimal / numeric
        if dialect == "sqlite":
            return "REAL", False
        return "DECIMAL%s" % (params if params else "(10,2)"), False

    if low in _TYPE_MAP:
        return _TYPE_MAP[low][dialect], False

    # Unknown type: pass through verbatim (upper-cased) — best effort.
    return t.upper(), False


def _quote_ident(name, dialect="postgres"):
    if dialect == "mysql":
        return "`%s`" % name.replace("`", "``")
    return '"%s"' % name.replace('"', '""')


def _build_column(col, dialect):
    name = col.get("name")
    raw_type = col.get("type")
    if not isinstance(name, str) or not name.strip():
        raise ValueError("each column needs a non-empty 'name'")
    if not isinstance(raw_type, str) or not raw_type.strip():
        raise ValueError("column %r needs a non-empty 'type'" % name)

    is_pk = bool(col.get("primary_key"))
    sql_type, is_serial = _map_type(raw_type, dialect)

    parts = [_quote_ident(name, dialect)]

    if is_serial:
        if dialect == "postgres":
            parts.append("SERIAL")
        elif dialect == "mysql":
            parts.append("INT AUTO_INCREMENT")
        else:  # sqlite
            parts.append("INTEGER")
    else:
        parts.append(sql_type)

    if is_pk:
        if is_serial and dialect == "sqlite":
            # SQLite rowid alias must be exactly "INTEGER PRIMARY KEY AUTOINCREMENT".
            parts.append("PRIMARY KEY AUTOINCREMENT")
        else:
            parts.append("PRIMARY KEY")

    # NOT NULL — a PRIMARY KEY is implicitly NOT NULL, so skip the redundancy.
    nullable = col.get("nullable")
    if nullable is False and not is_pk:
        parts.append("NOT NULL")

    if col.get("unique") and not is_pk:
        parts.append("UNIQUE")

    if "default" in col and col.get("default") is not None:
        dv = col.get("default")
        if isinstance(dv, bool):
            if dialect == "postgres":
                parts.append("DEFAULT %s" % ("TRUE" if dv else "FALSE"))
            else:
                parts.append("DEFAULT %d" % (1 if dv else 0))
        elif isinstance(dv, (int, float)):
            parts.append("DEFAULT %s" % dv)
        else:
            s = str(dv)
            # Treat bare SQL functions / keywords as raw, quote everything else.
            if re.match(r"^[A-Za-z_][A-Za-z0-9_]*(\([^)]*\))?$", s) and (
                s.upper() in ("CURRENT_TIMESTAMP", "NOW()", "CURRENT_DATE", "NULL",
                              "TRUE", "FALSE", "GEN_RANDOM_UUID()", "UUID()")
                or s.endswith(")")
            ):
                parts.append("DEFAULT %s" % s)
            else:
                parts.append("DEFAULT '%s'" % s.replace("'", "''"))

    # Inline column-level foreign key.
    fk = col.get("foreign_key")
    if fk:
        ref = _parse_fk(fk)
        if ref:
            parts.append("REFERENCES %s (%s)" % (_quote_ident(ref[0], dialect), _quote_ident(ref[1], dialect)))

    return " ".join(parts)


def _parse_fk(fk):
    """Accept {'table':..,'column':..} or 'table(col)' or 'table.col'."""
    if isinstance(fk, dict):
        t = fk.get("table")
        c = fk.get("column") or fk.get("ref_column") or "id"
        if isinstance(t, str) and t.strip():
            return (t.strip(), str(c).strip())
        return None
    if isinstance(fk, str):
        m = re.match(r"^\s*([A-Za-z_][\w]*)\s*[\(\.]\s*([A-Za-z_][\w]*)\s*\)?\s*$", fk)
        if m:
            return (m.group(1), m.group(2))
        # bare table name -> assume "id"
        if re.match(r"^[A-Za-z_][\w]*$", fk.strip()):
            return (fk.strip(), "id")
    return None


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e}))
        return 0
    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"table": "users", "columns": [{"name": "id", "type": "serial", "primary_key": True}]},
        }))
        return 0

    table = q.get("table")
    columns = q.get("columns")
    example = {
        "table": "users",
        "columns": [
            {"name": "id", "type": "serial", "primary_key": True},
            {"name": "email", "type": "varchar(255)", "unique": True, "nullable": False},
        ],
        "dialect": "postgres",
    }
    if not isinstance(table, str) or not table.strip():
        print(json.dumps({"error": "missing required field 'table' (non-empty name)", "example": example}))
        return 0
    if not isinstance(columns, list) or not columns:
        print(json.dumps({"error": "missing required field 'columns' (at least one column)", "example": example}))
        return 0

    dialect = (q.get("dialect") or "postgres")
    if not isinstance(dialect, str) or dialect.lower() not in _DIALECTS:
        print(json.dumps({
            "error": "unsupported dialect %r; use one of %s" % (dialect, ", ".join(_DIALECTS)),
            "example": example,
        }))
        return 0
    dialect = dialect.lower()

    try:
        col_defs = []
        for col in columns:
            if not isinstance(col, dict):
                raise ValueError("each column must be a JSON object")
            col_defs.append("  " + _build_column(col, dialect))

        # Table-level foreign keys via {"foreign_keys": [{column, table, ref_column}]}.
        for fk in (q.get("foreign_keys") or []):
            if not isinstance(fk, dict):
                continue
            local = fk.get("column")
            ref = _parse_fk(fk)
            if isinstance(local, str) and ref:
                col_defs.append("  FOREIGN KEY (%s) REFERENCES %s (%s)" % (
                    _quote_ident(local, dialect), _quote_ident(ref[0], dialect), _quote_ident(ref[1], dialect)))

        ddl = "CREATE TABLE %s (\n%s\n);" % (_quote_ident(table.strip(), dialect), ",\n".join(col_defs))
        result = {"dialect": dialect, "ddl": ddl}
        print(json.dumps(result, indent=2, default=str))
        return 0
    except ValueError as e:
        print(json.dumps({"error": str(e), "example": example}))
        return 0
    except Exception as e:
        print(json.dumps({"error": "sql_ddl_gen failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
