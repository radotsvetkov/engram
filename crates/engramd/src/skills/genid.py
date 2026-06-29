#!/usr/bin/env python3
"""genid — Engram skill (no network). Generate UUIDs or secure passwords/tokens.

Reads a JSON request {what?:"uuid", count?:1, length?:20, symbols?:true} on stdin.
what is one of uuid|password|token. uuid -> uuid.uuid4() strings; password ->
secrets.choice over ascii_letters+digits (+symbols if requested); token ->
secrets.token_urlsafe(length). count is clamped 1..50, length clamped 4..128.
Emits {"what":..., "values":[...]} on stdout.
"""
import json, sys, uuid, secrets, string


def _clamp(value, lo, hi, default):
    try:
        n = int(value)
    except (TypeError, ValueError):
        n = default
    if n < lo:
        n = lo
    if n > hi:
        n = hi
    return n


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e})); return 0
    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"what": "password", "count": 3, "length": 20, "symbols": True},
        })); return 0

    what = str(q.get("what", "uuid") or "uuid").strip().lower()
    if what not in ("uuid", "password", "token"):
        print(json.dumps({
            "error": "unknown 'what': %r (use uuid|password|token)" % what,
            "example": {"what": "uuid", "count": 5},
        })); return 0

    count = _clamp(q.get("count", 1), 1, 50, 1)
    length = _clamp(q.get("length", 20), 4, 128, 20)
    symbols = bool(q.get("symbols", True))

    try:
        values = []
        if what == "uuid":
            for _ in range(count):
                values.append(str(uuid.uuid4()))
        elif what == "password":
            alphabet = string.ascii_letters + string.digits
            if symbols:
                alphabet += "!@#$%^&*-_=+"
            for _ in range(count):
                values.append("".join(secrets.choice(alphabet) for _ in range(length)))
        else:  # token
            for _ in range(count):
                values.append(secrets.token_urlsafe(length))

        result = {"what": what, "count": count, "values": values}
        if what != "uuid":
            result["length"] = length
        if what == "password":
            result["symbols"] = symbols
        print(json.dumps(result, indent=2, default=str)); return 0
    except Exception as e:
        print(json.dumps({"error": "genid failed: %s" % e})); return 1


if __name__ == "__main__":
    sys.exit(main())
