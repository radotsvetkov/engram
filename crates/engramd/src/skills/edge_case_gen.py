#!/usr/bin/env python3
"""edge_case_gen — Engram skill (no network). Generate boundary/edge-case test values.

Given a data type (int, string, float, email, date) and, for int/float, optional
min/max bounds, returns a curated list of boundary values with a short rationale
for each — the kind of edge cases a QA engineer would probe an input validator
with. Static/deterministic generation; stdlib only.

Request (stdin): {"type": "int", "min": 1, "max": 100}
Output (stdout): {"type": "int", "edge_cases": [{"value": ..., "why": "..."}, ...]}
Some entries can't be represented as a literal JSON value (NaN, a 10,000-char
string, an embedded null byte) — those use a "description" key instead of "value".
"""
import json
import sys

TYPES = ("int", "string", "float", "email", "date")


def _bound_status(v, mn, mx):
    """Classify v relative to an optional [mn, mx] range."""
    if mn is not None and v < mn:
        return "below"
    if mx is not None and v > mx:
        return "above"
    return "inside"


def _boundary_why(label, value, status):
    if status in ("below", "above"):
        return "%s = %s — just outside the valid range — should be REJECTED by input validation" % (label, value)
    return "%s = %s — just inside the valid range — should be ACCEPTED" % (label, value)


def _numeric_cases(kind, mn, mx):
    cast = int if kind == "int" else float
    cases = [
        {"value": cast(0), "why": "zero — common edge case (falsy value, division-by-zero trigger, empty-count sentinel)"},
        {"value": cast(-1), "why": "negative one — common 'not found' sentinel / off-by-one boundary"},
        {"value": cast(1), "why": "smallest positive value — off-by-one boundary above zero"},
    ]
    if mn is not None:
        cases.append({"value": cast(mn), "why": "the minimum allowed value (inclusive boundary) — should be ACCEPTED"})
        below = cast(mn) - 1
        cases.append({"value": below, "why": _boundary_why("min - 1", below, _bound_status(below, mn, mx))})
        above = cast(mn) + 1
        cases.append({"value": above, "why": _boundary_why("min + 1", above, _bound_status(above, mn, mx))})
    if mx is not None:
        cases.append({"value": cast(mx), "why": "the maximum allowed value (inclusive boundary) — should be ACCEPTED"})
        below = cast(mx) - 1
        cases.append({"value": below, "why": _boundary_why("max - 1", below, _bound_status(below, mn, mx))})
        above = cast(mx) + 1
        cases.append({"value": above, "why": _boundary_why("max + 1", above, _bound_status(above, mn, mx))})

    big = 2 ** 31 - 1
    cases.append({"value": cast(big),
                  "why": "largest 32-bit signed integer (INT32_MAX) — boundary for 32-bit integer overflow"})
    cases.append({"value": cast(big + 1),
                  "why": "one past INT32_MAX — triggers overflow in fixed-width 32-bit signed integer handling"})
    cases.append({"value": cast(-(2 ** 31)),
                  "why": "large negative magnitude (INT32_MIN) — test negative-number / signed-underflow handling"})

    if kind == "float":
        cases.append({
            "description": "float('nan')",
            "why": "NaN — JSON cannot represent NaN natively; test in-language that NaN comparisons are "
                   "always False (nan != nan, even nan != itself) and that validation doesn't silently accept it",
        })
        cases.append({
            "value": 0.1 + 0.2,
            "why": "classic floating-point precision edge case — 0.1 + 0.2 != 0.3 exactly in IEEE-754 "
                   "double precision; equality checks need an epsilon/tolerance",
        })
    return cases


def _string_cases():
    return [
        {"value": "", "why": "empty string — test required-field / minimum-length validation"},
        {"value": "a", "why": "single character — minimum non-empty length"},
        {
            "description": {"length": 10000, "note": "very long string — test buffer/storage/column-length limits"},
            "why": "very long input can exceed buffer sizes, DB column limits, or degrade performance",
        },
        {"value": "héllo 👋 世界",
         "why": "unicode/multi-byte characters — test encoding and length-counting (codepoints vs bytes vs grapheme clusters)"},
        {"value": "'; DROP TABLE users; --",
         "why": "test that this is safely escaped, not executed/interpreted (SQL injection probe)"},
        {"value": "<script>alert(1)</script>",
         "why": "test that this is safely escaped, not executed/interpreted (XSS probe)"},
        {"value": "   ", "why": "whitespace-only — test that trimming/validation doesn't treat this as non-empty"},
        {"description": "a\\x00b",
         "why": "embedded null byte mid-string (shown escaped, not literal, since a raw NUL can break "
                "JSON/C-string handling) — test null-byte handling"},
    ]


def _email_cases():
    long_local = "a" * 70
    return [
        {"value": "user@example.com", "why": "well-formed baseline email"},
        {"value": "userexample.com", "why": "missing @ — invalid, should be rejected"},
        {"value": "user@", "why": "missing domain — invalid, should be rejected"},
        {"value": "user+tag@example.com",
         "why": "plus-addressing (RFC 5233) — valid; test that it isn't rejected or silently mangled"},
        {"value": "%s@example.com" % long_local,
         "why": "local part >64 chars — exceeds the RFC 5321 local-part length limit"},
        {"value": "user@exämple.com",
         "why": "internationalized domain name (IDN) — test Unicode/punycode domain handling"},
        {"value": "user..name@example.com",
         "why": "consecutive dots in the local part — invalid per RFC 5321, should be rejected"},
    ]


def _date_cases():
    return [
        {"value": "2028-02-29", "why": "valid leap day (2028 is a leap year) — test leap-year date parsing"},
        {"value": "2026-02-29",
         "why": "invalid — 2026 is not a leap year, February has only 28 days; should be rejected"},
        {"value": "0001-01-01", "why": "far-past date — test wide date-range support / year-zero-adjacent edge cases"},
        {"value": "9999-12-31", "why": "far-future date — test the upper bound of date-range support"},
        {"value": "2026-03-08",
         "why": "US daylight-saving-time spring-forward transition date (illustrative) — test timezone-aware "
                "handling of the skipped hour"},
        {"value": "2026-11-01",
         "why": "US daylight-saving-time fall-back transition date (illustrative) — test timezone-aware "
                "handling of the repeated/ambiguous hour"},
        {"value": "2026-13-45", "why": "invalid month (13) and day (45) — should be rejected by date parsing"},
    ]


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0

    example = {"type": "int", "min": 1, "max": 100}

    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object", "supported_types": list(TYPES), "example": example}))
        return 0

    type_raw = q.get("type")
    t = type_raw.strip().lower() if isinstance(type_raw, str) else ""
    if t not in TYPES:
        print(json.dumps({
            "error": "provide 'type'" if type_raw is None else "unsupported type %r" % (type_raw,),
            "supported_types": list(TYPES),
            "example": example,
        }))
        return 0

    mn_raw, mx_raw = q.get("min"), q.get("max")
    mn = mx = None
    if t in ("int", "float"):
        try:
            if mn_raw is not None:
                mn = int(mn_raw) if t == "int" else float(mn_raw)
            if mx_raw is not None:
                mx = int(mx_raw) if t == "int" else float(mx_raw)
        except Exception:
            print(json.dumps({"error": "'min'/'max' must be numeric for type %r" % t}))
            return 0
        if mn is not None and mx is not None and mn > mx:
            print(json.dumps({"error": "'min' (%s) must be <= 'max' (%s)" % (mn, mx)}))
            return 0

    try:
        if t == "int":
            cases = _numeric_cases("int", mn, mx)
        elif t == "float":
            cases = _numeric_cases("float", mn, mx)
        elif t == "string":
            cases = _string_cases()
        elif t == "email":
            cases = _email_cases()
        else:
            cases = _date_cases()
    except Exception as e:
        print(json.dumps({"error": "could not generate edge cases: %s" % e}))
        return 0

    print(json.dumps({"type": t, "edge_cases": cases}, indent=2, default=str))
    return 0


if __name__ == "__main__":
    sys.exit(main())
