#!/usr/bin/env python3
"""dedupe_list — Engram skill (no network). Remove duplicates from a list.

If items are objects and `key` is given, dedupes by that field; otherwise by
full-value equality (JSON-canonicalized so dicts/lists compare structurally).
Order is preserved; keep the first or last occurrence. Reports which key/values
were duplicated.

Request (stdin): {"items": [any], "key"?: "id", "keep"?: "first"|"last"}
Output (stdout): {deduped, original_count, deduped_count, duplicates_removed, duplicate_keys}
"""
import json, sys


def _canonical(v):
    return json.dumps(v, sort_keys=True, separators=(",", ":"), default=str)


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e})); return 0

    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"items": [1, 2, 2, 3], "keep": "first"},
        })); return 0

    items = q.get("items")
    if not isinstance(items, list):
        print(json.dumps({
            "error": "missing required field 'items' (a list)",
            "example": {"items": [{"id": 1}, {"id": 1}, {"id": 2}], "key": "id", "keep": "first"},
        })); return 0

    key = q.get("key")
    if key is not None and not isinstance(key, str):
        print(json.dumps({"error": "'key' must be a string field name"})); return 0

    keep = (q.get("keep") or "first")
    if not isinstance(keep, str):
        keep = "first"
    keep = keep.lower().strip()
    if keep not in ("first", "last"):
        print(json.dumps({"error": "'keep' must be 'first' or 'last'", "example": {"items": [1, 1, 2], "keep": "last"}})); return 0

    try:
        def id_of(item):
            if key is not None and isinstance(item, dict):
                # Items missing the key hash under a sentinel so they still dedupe sanely.
                return _canonical({"__k__": item.get(key, "\x00__MISSING__")})
            return _canonical(item)

        # First pass: count occurrences of each identity to know what's duplicated.
        counts = {}
        for item in items:
            counts[id_of(item)] = counts.get(id_of(item), 0) + 1

        deduped = []
        if keep == "first":
            seen = set()
            for item in items:
                ident = id_of(item)
                if ident not in seen:
                    seen.add(ident)
                    deduped.append(item)
        else:  # keep == "last"
            # Walk from the end, keep first-seen-from-end, then reverse to restore order.
            seen = set()
            tmp = []
            for item in reversed(items):
                ident = id_of(item)
                if ident not in seen:
                    seen.add(ident)
                    tmp.append(item)
            deduped = list(reversed(tmp))

        # Which identities appeared more than once? Report a human-friendly value.
        duplicate_keys = []
        reported = set()
        for item in items:
            ident = id_of(item)
            if counts[ident] > 1 and ident not in reported:
                reported.add(ident)
                if key is not None and isinstance(item, dict):
                    duplicate_keys.append(item.get(key))
                else:
                    duplicate_keys.append(item)

        result = {
            "deduped": deduped,
            "original_count": len(items),
            "deduped_count": len(deduped),
            "duplicates_removed": len(items) - len(deduped),
            "duplicate_keys": duplicate_keys,
        }
        print(json.dumps(result, indent=2, default=str)); return 0
    except Exception as e:
        print(json.dumps({"error": "dedupe_list failed: %s" % e})); return 1


if __name__ == "__main__":
    sys.exit(main())
