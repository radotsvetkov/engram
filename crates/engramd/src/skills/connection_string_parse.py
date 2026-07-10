#!/usr/bin/env python3
"""connection_string_parse — Engram skill (no network). Parse or build a
database connection URI for postgres / mysql / mongodb / redis / sqlite.

Parsing splits scheme/host/port/user/database/params and fills the well-known
default port per scheme when absent. The password is ALWAYS masked as "***" in
the output — the real password is never echoed back. Building assembles a URI
using the supplied password but likewise masks it in the returned string.

Request (stdin): {"connection_string": "postgres://user:secret@db.host:5432/app?sslmode=require"}
  OR (build) {"scheme": "postgres", "host": "db.host", "user": "user",
              "password": "secret", "database": "app", "params": {"sslmode": "require"}}
Output (stdout): {parsed: {...}}  OR  {built: {uri_masked, note}}
"""
import json
import sys
from urllib.parse import urlsplit, parse_qs, quote

_DEFAULT_PORTS = {
    "postgres": 5432, "postgresql": 5432,
    "mysql": 3306, "mariadb": 3306,
    "mongodb": 27017, "mongodb+srv": 27017,
    "redis": 6379, "rediss": 6379,
    "sqlite": None,
}
_MASK = "***"


def _scheme_family(scheme):
    s = (scheme or "").lower()
    if s in ("postgresql", "postgres"):
        return "postgres"
    if s in ("mysql", "mariadb"):
        return "mysql"
    if s in ("mongodb", "mongodb+srv"):
        return "mongodb"
    if s in ("redis", "rediss"):
        return "redis"
    if s in ("sqlite", "sqlite3", "file"):
        return "sqlite"
    return s or "unknown"


def _parse(cs):
    # SQLite URIs look like sqlite:///path/to/db — urlsplit handles them, but the
    # "host" is empty and the path is the file.
    sp = urlsplit(cs)
    scheme = sp.scheme.lower()
    family = _scheme_family(scheme)

    if family == "sqlite":
        # sqlite:///abs/path or sqlite://relative — path carries the file.
        path = (sp.netloc + sp.path) if sp.netloc else sp.path
        return {
            "scheme": scheme or "sqlite",
            "family": "sqlite",
            "host": None,
            "port": None,
            "user": None,
            "password_masked": None,
            "database": path or None,
            "params": {k: v[0] if len(v) == 1 else v for k, v in parse_qs(sp.query).items()},
        }

    host = sp.hostname
    port = sp.port
    if port is None:
        port = _DEFAULT_PORTS.get(scheme)
    user = sp.username

    db = sp.path.lstrip("/") if sp.path else ""
    # Redis often encodes the DB index as the path (e.g. /0).
    params = {k: v[0] if len(v) == 1 else v for k, v in parse_qs(sp.query).items()}

    # NEVER echo the real password. Show a mask only if one was present.
    password_present = sp.password is not None and sp.password != ""

    return {
        "scheme": scheme,
        "family": family,
        "host": host,
        "port": port,
        "port_is_default": (sp.port is None and _DEFAULT_PORTS.get(scheme) is not None),
        "user": user,
        "password_masked": _MASK if password_present else None,
        "password_present": password_present,
        "database": db or None,
        "params": params,
    }


def _build(q):
    scheme = str(q.get("scheme") or "").strip().lower()
    if not scheme:
        raise ValueError("build requires a 'scheme' (postgres/mysql/mongodb/redis/sqlite)")
    family = _scheme_family(scheme)
    host = q.get("host")
    database = q.get("database")
    user = q.get("user")
    password = q.get("password")
    port = q.get("port")
    params = q.get("params") or {}

    if family == "sqlite":
        path = database or q.get("path") or ""
        if not path:
            raise ValueError("sqlite build requires a 'database'/'path' file path")
        uri = "%s:///%s" % (scheme, str(path).lstrip("/"))
        return {"uri_masked": uri, "note": "sqlite is file-based; no host/credentials."}

    if not host:
        raise ValueError("build requires a 'host' for %s" % family)
    if port is None:
        port = _DEFAULT_PORTS.get(scheme)

    # Assemble authority, masking the password in the ECHOED string.
    auth = ""
    if user:
        auth = quote(str(user), safe="")
        if password is not None and str(password) != "":
            auth += ":" + _MASK  # masked in output; the REAL password is used by callers
        auth += "@"

    netloc = "%s%s" % (auth, host)
    if port is not None:
        netloc += ":%d" % int(port)

    path = ""
    if database:
        path = "/" + str(database).lstrip("/")

    query = ""
    if isinstance(params, dict) and params:
        query = "?" + "&".join("%s=%s" % (quote(str(k), safe=""), quote(str(v), safe="")) for k, v in params.items())

    uri = "%s://%s%s%s" % (scheme, netloc, path, query)
    note = ("password is MASKED as '***' in this echoed URI. Substitute the real "
            "password (from your secret store, not source control) when connecting.")
    if password is None or str(password) == "":
        note = "no password supplied; URI has no credentials embedded."
    return {"uri_masked": uri, "note": note}


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e}))
        return 0
    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"connection_string": "postgres://user:secret@host:5432/db"},
        }))
        return 0

    example = {
        "parse": {"connection_string": "postgres://user:secret@db.host:5432/app?sslmode=require"},
        "build": {"scheme": "postgres", "host": "db.host", "user": "user",
                  "password": "secret", "database": "app", "params": {"sslmode": "require"}},
    }

    cs = q.get("connection_string") or q.get("uri") or q.get("url")
    has_build = bool(q.get("scheme"))

    if not (isinstance(cs, str) and cs.strip()) and not has_build:
        print(json.dumps({
            "error": "provide either 'connection_string' to parse or 'scheme'(+host...) to build",
            "example": example,
        }))
        return 0

    try:
        if isinstance(cs, str) and cs.strip():
            parsed = _parse(cs.strip())
            print(json.dumps({"parsed": parsed}, indent=2, default=str))
            return 0
        built = _build(q)
        print(json.dumps({"built": built}, indent=2, default=str))
        return 0
    except ValueError as e:
        print(json.dumps({"error": str(e), "example": example}))
        return 0
    except Exception as e:
        print(json.dumps({"error": "connection_string_parse failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
