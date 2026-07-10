#!/usr/bin/env python3
"""naming_convention_check — Engram skill (no network). Check identifiers against
a target naming convention and suggest a converted form for each.

Tests every identifier against a regex for the chosen convention (camelCase,
snake_case, PascalCase, kebab-case, or SCREAMING_SNAKE) and, regardless of
match, produces the identifier re-cased into that convention by splitting it
into words on case boundaries, digits, underscores and hyphens.

Request (stdin): {"identifiers": ["userId", "user_name", "APIKey"], "convention": "snake_case"}
Output (stdout): {convention, all_valid, results: [{name, matches, suggestion}]}
"""
import json
import re
import sys

_EXAMPLE = {"identifiers": ["userId", "user_name", "APIKey"], "convention": "snake_case"}

_PATTERNS = {
    "camelCase": re.compile(r"^[a-z][a-zA-Z0-9]*$"),
    "snake_case": re.compile(r"^[a-z][a-z0-9]*(_[a-z0-9]+)*$"),
    "PascalCase": re.compile(r"^[A-Z][a-zA-Z0-9]*$"),
    "kebab-case": re.compile(r"^[a-z][a-z0-9]*(-[a-z0-9]+)*$"),
    "SCREAMING_SNAKE": re.compile(r"^[A-Z][A-Z0-9]*(_[A-Z0-9]+)*$"),
}


def _split_words(name):
    # split on underscores/hyphens/spaces first, then camel/Pascal boundaries
    chunks = re.split(r"[_\-\s]+", name.strip())
    words = []
    for chunk in chunks:
        if not chunk:
            continue
        # ACRONYM followed by Word | leading-cap word | run of lowercase/digits | run of caps
        sub = re.findall(r"[A-Z]+(?=[A-Z][a-z0-9])|[A-Z]?[a-z0-9]+|[A-Z]+|[0-9]+", chunk)
        words.extend(sub if sub else [chunk])
    return [w for w in words if w]


def _convert(name, convention):
    words = _split_words(name)
    if not words:
        return name
    low = [w.lower() for w in words]
    if convention == "snake_case":
        return "_".join(low)
    if convention == "kebab-case":
        return "-".join(low)
    if convention == "SCREAMING_SNAKE":
        return "_".join(w.upper() for w in low)
    if convention == "PascalCase":
        return "".join(w[:1].upper() + w[1:] for w in low)
    if convention == "camelCase":
        pascal = "".join(w[:1].upper() + w[1:] for w in low)
        return pascal[:1].lower() + pascal[1:]
    return name


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e}))
        return 0
    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object", "example": _EXAMPLE}))
        return 0

    identifiers = q.get("identifiers")
    if not isinstance(identifiers, list) or not identifiers:
        print(json.dumps({
            "error": "missing required field 'identifiers' (non-empty list of strings)",
            "example": _EXAMPLE,
        }))
        return 0
    convention = q.get("convention")
    if convention not in _PATTERNS:
        print(json.dumps({
            "error": "'convention' must be one of: %s" % ", ".join(_PATTERNS),
            "example": _EXAMPLE,
        }))
        return 0

    try:
        pattern = _PATTERNS[convention]
        results = []
        for ident in identifiers:
            name = str(ident)
            matches = bool(pattern.match(name))
            suggestion = _convert(name, convention)
            results.append({
                "name": name,
                "matches": matches,
                "suggestion": suggestion,
            })
        all_valid = all(r["matches"] for r in results)
        print(json.dumps({
            "convention": convention,
            "all_valid": all_valid,
            "results": results,
        }, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "naming_convention_check failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
