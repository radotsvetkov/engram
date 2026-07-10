#!/usr/bin/env python3
"""faker_data — Engram skill (no network). Generate deterministic fake records.

Given a schema {field: type}, emits `count` fake records seeded by `seed`
(random.Random) so output is reproducible. No faker lib, no network — small
static word/name/city lists are embedded. Supported types: name, first_name,
last_name, email, phone, city, country, company, date, datetime, uuid, int,
float, bool, word, sentence, url, ipv4. int/float accept {type,min,max}. Count
is capped at 1000.

Request (stdin): {"schema": {"name":"name","age":{"type":"int","min":18,"max":90}}, "count"?: 10, "seed"?: 42}
Output (stdout): {records, count}
"""
import json, sys, random

_FIRST = ["Alice", "Bob", "Carol", "David", "Eve", "Frank", "Grace", "Heidi",
          "Ivan", "Judy", "Mallory", "Niaj", "Olivia", "Peggy", "Rupert",
          "Sybil", "Trent", "Victor", "Walter", "Yasmin"]
_LAST = ["Smith", "Johnson", "Williams", "Brown", "Jones", "Garcia", "Miller",
         "Davis", "Rodriguez", "Martinez", "Nguyen", "Kim", "Patel", "Chen",
         "Ivanov", "Muller", "Rossi", "Silva", "Kowalski", "Hansen"]
_CITY = ["Springfield", "Riverside", "Franklin", "Greenville", "Bristol",
         "Clinton", "Fairview", "Salem", "Madison", "Georgetown", "Arlington",
         "Ashland", "Dover", "Oxford", "Newport"]
_COUNTRY = ["USA", "Canada", "Germany", "France", "Japan", "Brazil", "India",
            "Australia", "Spain", "Italy", "Mexico", "Sweden", "Norway",
            "Netherlands", "Portugal"]
_COMPANY_A = ["Acme", "Globex", "Initech", "Umbrella", "Soylent", "Hooli",
              "Vandelay", "Stark", "Wayne", "Wonka", "Cyberdyne", "Aperture",
              "Nakatomi", "Tyrell", "Gekko"]
_COMPANY_B = ["Corp", "Industries", "Labs", "Systems", "Group", "Holdings",
              "Solutions", "Partners", "Technologies", "Ventures"]
_WORDS = ["lorem", "ipsum", "dolor", "sit", "amet", "consectetur", "adipiscing",
          "elit", "sed", "eiusmod", "tempor", "incididunt", "labore", "magna",
          "aliqua", "veniam", "quis", "nostrud", "aliquip", "commodo"]
_TLD = ["com", "net", "org", "io", "co"]


def _hexid(rnd, n):
    return "".join(rnd.choice("0123456789abcdef") for _ in range(n))


def _gen(rnd, spec):
    if isinstance(spec, dict):
        t = str(spec.get("type", "word")).lower()
    else:
        t = str(spec).lower()

    if t == "first_name":
        return rnd.choice(_FIRST)
    if t == "last_name":
        return rnd.choice(_LAST)
    if t == "name":
        return rnd.choice(_FIRST) + " " + rnd.choice(_LAST)
    if t == "email":
        return "%s.%s@example.%s" % (rnd.choice(_FIRST).lower(), rnd.choice(_LAST).lower(), rnd.choice(_TLD))
    if t == "phone":
        return "+1-%03d-%03d-%04d" % (rnd.randint(200, 999), rnd.randint(200, 999), rnd.randint(0, 9999))
    if t == "city":
        return rnd.choice(_CITY)
    if t == "country":
        return rnd.choice(_COUNTRY)
    if t == "company":
        return rnd.choice(_COMPANY_A) + " " + rnd.choice(_COMPANY_B)
    if t == "date":
        return "%04d-%02d-%02d" % (rnd.randint(1990, 2025), rnd.randint(1, 12), rnd.randint(1, 28))
    if t == "datetime":
        return "%04d-%02d-%02dT%02d:%02d:%02dZ" % (
            rnd.randint(1990, 2025), rnd.randint(1, 12), rnd.randint(1, 28),
            rnd.randint(0, 23), rnd.randint(0, 59), rnd.randint(0, 59))
    if t == "uuid":
        return "%s-%s-4%s-%s%s-%s" % (
            _hexid(rnd, 8), _hexid(rnd, 4), _hexid(rnd, 3),
            rnd.choice("89ab"), _hexid(rnd, 3), _hexid(rnd, 12))
    if t == "int":
        lo = spec.get("min", 0) if isinstance(spec, dict) else 0
        hi = spec.get("max", 100) if isinstance(spec, dict) else 100
        try:
            lo, hi = int(lo), int(hi)
        except (TypeError, ValueError):
            lo, hi = 0, 100
        if lo > hi:
            lo, hi = hi, lo
        return rnd.randint(lo, hi)
    if t == "float":
        lo = spec.get("min", 0.0) if isinstance(spec, dict) else 0.0
        hi = spec.get("max", 1.0) if isinstance(spec, dict) else 1.0
        try:
            lo, hi = float(lo), float(hi)
        except (TypeError, ValueError):
            lo, hi = 0.0, 1.0
        if lo > hi:
            lo, hi = hi, lo
        return round(rnd.uniform(lo, hi), 4)
    if t == "bool":
        return rnd.choice([True, False])
    if t == "word":
        return rnd.choice(_WORDS)
    if t == "sentence":
        n = rnd.randint(4, 10)
        s = " ".join(rnd.choice(_WORDS) for _ in range(n))
        return s[0].upper() + s[1:] + "."
    if t == "url":
        return "https://%s.%s/%s" % (rnd.choice(_COMPANY_A).lower(), rnd.choice(_TLD), rnd.choice(_WORDS))
    if t == "ipv4":
        return "%d.%d.%d.%d" % (rnd.randint(1, 255), rnd.randint(0, 255), rnd.randint(0, 255), rnd.randint(1, 254))
    # Unknown type: return a marker rather than failing the whole batch.
    return "<?%s?>" % t


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e})); return 0

    ex = {"schema": {"name": "name", "email": "email", "age": {"type": "int", "min": 18, "max": 90}}, "count": 5, "seed": 42}
    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object", "example": ex})); return 0

    schema = q.get("schema")
    if not isinstance(schema, dict) or not schema:
        print(json.dumps({
            "error": "missing required field 'schema' (a non-empty {field: type} object)",
            "example": ex,
        })); return 0

    count = q.get("count")
    if count is None:
        count = 10
    if not isinstance(count, int) or isinstance(count, bool) or count < 0:
        print(json.dumps({"error": "'count' must be a non-negative integer", "example": ex})); return 0
    count = min(count, 1000)

    seed = q.get("seed")
    if seed is None:
        seed = 42

    try:
        rnd = random.Random(seed)
        records = []
        for _ in range(count):
            rec = {}
            for field, spec in schema.items():
                rec[str(field)] = _gen(rnd, spec)
            records.append(rec)
        print(json.dumps({"records": records, "count": len(records)}, indent=2, default=str)); return 0
    except Exception as e:
        print(json.dumps({"error": "faker_data failed: %s" % e})); return 1


if __name__ == "__main__":
    sys.exit(main())
