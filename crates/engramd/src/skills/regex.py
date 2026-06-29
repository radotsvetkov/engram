#!/usr/bin/env python3
"""regex — Engram skill (no network). Test a regular expression against text.

Pure compute: compiles {pattern, text, flags?} where flags is a string that may
contain i (IGNORECASE), m (MULTILINE), s (DOTALL), and runs finditer over text.
Output {pattern, count, matches:[{match, groups, start, end} ...up to 100],
fullmatch:(bool re.fullmatch(text) is not None)}. Invalid regex -> {"error":...}.
"""
import json, sys, re

FLAG_MAP = {"i": re.IGNORECASE, "m": re.MULTILINE, "s": re.DOTALL}


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e})); return 0
    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"pattern": r"\d+", "text": "a1 b22 c333", "flags": "i"},
        })); return 0

    pattern = q.get("pattern")
    text = q.get("text")
    if pattern is None or text is None:
        print(json.dumps({
            "error": "missing required field 'pattern' and/or 'text'",
            "example": {"pattern": r"(\w)(\d)", "text": "a1 b2 c3", "flags": "i"},
        })); return 0
    if not isinstance(pattern, str) or not isinstance(text, str):
        print(json.dumps({
            "error": "'pattern' and 'text' must both be strings",
            "example": {"pattern": r"\bword\b", "text": "a word here", "flags": "m"},
        })); return 0

    flags_str = q.get("flags") or ""
    if not isinstance(flags_str, str):
        flags_str = str(flags_str)
    flags = 0
    for ch in flags_str.lower():
        if ch in FLAG_MAP:
            flags |= FLAG_MAP[ch]

    try:
        rx = re.compile(pattern, flags)
    except re.error as e:
        print(json.dumps({"error": "invalid regex: %s" % e})); return 0

    try:
        matches = []
        count = 0
        for m in rx.finditer(text):
            count += 1
            if len(matches) < 100:
                matches.append({
                    "match": m.group(0),
                    "groups": list(m.groups()),
                    "start": m.start(),
                    "end": m.end(),
                })
        fullmatch = rx.fullmatch(text) is not None
        result = {
            "pattern": pattern,
            "count": count,
            "matches": matches,
            "fullmatch": fullmatch,
        }
        print(json.dumps(result, indent=2, default=str)); return 0
    except Exception as e:
        print(json.dumps({"error": "regex failed: %s" % e})); return 1


if __name__ == "__main__":
    sys.exit(main())
