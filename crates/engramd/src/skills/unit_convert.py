#!/usr/bin/env python3
"""unit_convert — Engram skill (no network). Convert a value between units.

Pure compute: maps each unit to a base via a factor table (length, mass, data,
speed, time) and handles temperature (c/f/k) with formulas. from/to are
case-insensitive; converting across different kinds is rejected.

Request (stdin): {"value": 5, "from": "km", "to": "mi"}
Output (stdout): {value, from, to, result}
"""
import json
import sys

# Each category maps a unit (lowercase) -> factor relative to the category base.
# result = value * factor[from] / factor[to]
_CATEGORIES = {
    "length": {  # base: m
        "m": 1.0, "km": 1000.0, "cm": 0.01, "mm": 0.001,
        "mi": 1609.344, "yd": 0.9144, "ft": 0.3048, "in": 0.0254,
    },
    "mass": {  # base: g
        "g": 1.0, "kg": 1000.0, "mg": 0.001,
        "lb": 453.592, "oz": 28.3495, "t": 1e6,
    },
    "data": {  # base: byte
        "byte": 1.0, "kb": 1e3, "mb": 1e6, "gb": 1e9, "tb": 1e12,
        "kib": 1024.0, "mib": 1048576.0, "gib": 1073741824.0,
    },
    "speed": {  # base: mps
        "mps": 1.0, "kph": 0.277778, "mph": 0.44704, "kn": 0.514444,
    },
    "time": {  # base: s
        "s": 1.0, "min": 60.0, "h": 3600.0, "day": 86400.0,
    },
}

_TEMP = {"c", "f", "k"}


def _find_category(unit):
    for name, table in _CATEGORIES.items():
        if unit in table:
            return name
    return None


def _to_celsius(value, unit):
    if unit == "c":
        return value
    if unit == "f":
        return (value - 32.0) * 5.0 / 9.0
    if unit == "k":
        return value - 273.15
    raise ValueError("unknown temperature unit: %s" % unit)


def _from_celsius(celsius, unit):
    if unit == "c":
        return celsius
    if unit == "f":
        return celsius * 9.0 / 5.0 + 32.0
    if unit == "k":
        return celsius + 273.15
    raise ValueError("unknown temperature unit: %s" % unit)


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e}))
        return 0

    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"value": 5, "from": "km", "to": "mi"},
        }))
        return 0

    raw_value = q.get("value")
    u_from = q.get("from") or q.get("from_unit")
    u_to = q.get("to") or q.get("to_unit")

    if raw_value is None or u_from is None or u_to is None:
        print(json.dumps({
            "error": "provide 'value', 'from', and 'to'",
            "example": {"value": 5, "from": "km", "to": "mi"},
        }))
        return 0

    try:
        value = float(raw_value)
    except (TypeError, ValueError):
        print(json.dumps({
            "error": "'value' must be a number (got %r)" % (raw_value,),
            "example": {"value": 5, "from": "km", "to": "mi"},
        }))
        return 0

    f = str(u_from).strip().lower()
    t = str(u_to).strip().lower()

    try:
        # Temperature is special-cased (affine, not a simple factor).
        if f in _TEMP or t in _TEMP:
            if not (f in _TEMP and t in _TEMP):
                print(json.dumps({
                    "error": "cannot convert %s to %s (different kinds)" % (u_from, u_to),
                }))
                return 0
            result = _from_celsius(_to_celsius(value, f), t)
            print(json.dumps(
                {"value": value, "from": f, "to": t, "result": result},
                indent=2, default=str))
            return 0

        cat_from = _find_category(f)
        cat_to = _find_category(t)

        if cat_from is None:
            known = sorted(
                u for table in _CATEGORIES.values() for u in table) + sorted(_TEMP)
            print(json.dumps({
                "error": "unknown unit '%s'" % u_from,
                "known_units": known,
            }))
            return 0
        if cat_to is None:
            known = sorted(
                u for table in _CATEGORIES.values() for u in table) + sorted(_TEMP)
            print(json.dumps({
                "error": "unknown unit '%s'" % u_to,
                "known_units": known,
            }))
            return 0
        if cat_from != cat_to:
            print(json.dumps({
                "error": "cannot convert %s to %s (different kinds)" % (u_from, u_to),
            }))
            return 0

        table = _CATEGORIES[cat_from]
        result = value * table[f] / table[t]
        print(json.dumps(
            {"value": value, "from": f, "to": t, "result": result},
            indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "unit_convert failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
