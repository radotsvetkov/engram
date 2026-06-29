#!/usr/bin/env python3
"""text — Engram skill (keyless, no network). Text counts and transforms (pure compute).

Reads {text, op?} on stdin. Always computes counts {chars, chars_no_spaces, words,
lines, sentences}. Supports transforms {upper, lower, title, capitalize, reverse, slug}.
If op is one of those transform keys, returns {op, result}; otherwise returns
{counts:{...}, transforms:{...}}. No network.
"""
import json, sys, re


def _counts(text):
    # sentences: split on runs of .!? and count non-empty fragments
    sentences = [s for s in re.split(r"[.!?]+", text) if s.strip()]
    return {
        "chars": len(text),
        "chars_no_spaces": len(re.sub(r"\s", "", text)),
        "words": len(text.split()),
        "lines": text.count("\n") + 1 if text else 0,
        "sentences": len(sentences),
    }


def _slug(text):
    s = text.lower()
    s = re.sub(r"[^a-z0-9]+", "-", s)  # non-alnum -> hyphen (also collapses runs)
    s = re.sub(r"-+", "-", s)          # collapse
    return s.strip("-")                # trim leading/trailing hyphens


def _transforms(text):
    return {
        "upper": text.upper(),
        "lower": text.lower(),
        "title": text.title(),
        "capitalize": text.capitalize(),
        "reverse": text[::-1],
        "slug": _slug(text),
    }


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e})); return 0

    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"text": "Hello, world!", "op": "slug"},
        })); return 0

    text = q.get("text")
    if text is None or not isinstance(text, str):
        print(json.dumps({
            "error": "missing required field 'text' (string)",
            "example": {"text": "Hello, world!", "op": "slug"},
        })); return 0

    op = q.get("op")
    try:
        transforms = _transforms(text)
        if op is not None:
            if not isinstance(op, str) or op not in transforms:
                print(json.dumps({
                    "error": "unknown op: %r" % op,
                    "how_to_fix": "omit 'op' for full report, or use one of: "
                                  "upper, lower, title, capitalize, reverse, slug",
                    "example": {"text": "Hello, world!", "op": "slug"},
                })); return 0
            print(json.dumps({"op": op, "result": transforms[op]}, indent=2, default=str))
            return 0

        print(json.dumps({
            "counts": _counts(text),
            "transforms": transforms,
        }, indent=2, default=str)); return 0
    except Exception as e:
        print(json.dumps({"error": "text failed: %s" % e})); return 1


if __name__ == "__main__":
    sys.exit(main())
