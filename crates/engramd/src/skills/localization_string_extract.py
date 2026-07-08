#!/usr/bin/env python3
"""localization_string_extract — Engram skill (no network). Regex-heuristic
scan of source code for hardcoded, user-facing string literals that are
NOT already wrapped in a translation/i18n call — candidates worth pulling
into an i18n catalog. This is NOT a real parser (no AST, no tokenizer) —
just line/regex heuristics, and it says so in the output.

Request (stdin): {"code": str, "source_type"?: "python"|"javascript"|"jsx_tsx" = "python"}
Output (stdout): {candidates: [{text, line_number, already_wrapped: false}, ...],
                   candidate_count, note}
"""
import json
import re
import sys

# Matches Python string literals: triple-quoted first (so a triple-quoted
# string isn't mistaken for three single-quoted ones), then single/double.
_PY_STRING_RE = re.compile(
    r"""(?P<prefix>[a-zA-Z]{0,2})(?P<quote>'''|\"\"\"|'|")(?P<body>.*?)(?P=quote)""",
    re.DOTALL,
)

# Matches JS/JSX/TSX string literals: template literals (backtick), then
# single/double quoted. (Doesn't attempt to parse `${...}` interpolation
# beyond checking whether any static text surrounds it.)
_JS_STRING_RE = re.compile(
    r"""(?P<prefix>)(?P<quote>`|'|")(?P<body>.*?)(?P=quote)""",
    re.DOTALL,
)

_PY_I18N_CALL_RE = re.compile(r"(?:_|gettext|ugettext|ngettext|pgettext)\s*\($")
_JS_I18N_CALL_RE = re.compile(r"(?:\bt|i18n\.t|useTranslation|<Trans)\s*[\(>]?\s*$")

_LOOKS_LIKE_PATH_OR_ID = re.compile(r"^\S+$")  # no whitespace anywhere
_HAS_PATH_LIKE_CHARS = re.compile(r"[/._]")

_MIN_LEN = 3  # candidates with len <= 2 are filtered as noise


def _line_number_at(text, index):
    return text.count("\n", 0, index) + 1


def _is_noise(text):
    stripped = text.strip()
    if not stripped:
        return True
    if len(stripped) <= 2:
        return True
    # Identifier/path/URL-looking: no whitespace AND contains a path-ish char
    # throughout (/, ., or _) -- e.g. "foo/bar", "a.b.c", "some_key", "x.py".
    if _LOOKS_LIKE_PATH_OR_ID.match(stripped) and _HAS_PATH_LIKE_CHARS.search(stripped):
        return True
    return False


def _static_text_present(body, interp_pattern):
    """True if `body` has any non-whitespace text outside of interpolation
    placeholders matched by `interp_pattern` (f-string {expr} or JS `${expr}`)."""
    remainder = interp_pattern.sub("", body)
    return bool(remainder.strip())


_PY_FSTRING_INTERP_RE = re.compile(r"\{[^{}]*\}")
_JS_TEMPLATE_INTERP_RE = re.compile(r"\$\{[^{}]*\}")


def _extract_python(code):
    candidates = []
    for m in _PY_STRING_RE.finditer(code):
        prefix = (m.group("prefix") or "").lower()
        body = m.group("body")
        start = m.start()

        # Skip already-wrapped calls: look at the text immediately preceding
        # the string literal (ignoring whitespace) for an i18n call pattern.
        # `start` is the index of the prefix group itself (e.g. the 'f' in
        # f"..."), so code[:start] already excludes any quote-prefix chars.
        preceding_trimmed = code[:start].rstrip()
        if _PY_I18N_CALL_RE.search(preceding_trimmed):
            continue

        if "f" in prefix:
            if not _static_text_present(body, _PY_FSTRING_INTERP_RE):
                continue

        if _is_noise(body):
            continue

        candidates.append({
            "text": body,
            "line_number": _line_number_at(code, start),
            "already_wrapped": False,
        })
    return candidates


def _extract_js(code):
    candidates = []
    for m in _JS_STRING_RE.finditer(code):
        quote = m.group("quote")
        body = m.group("body")
        start = m.start()

        preceding = code[:start]
        preceding_trimmed = preceding.rstrip()
        if _JS_I18N_CALL_RE.search(preceding_trimmed):
            continue

        if quote == "`":
            if not _static_text_present(body, _JS_TEMPLATE_INTERP_RE):
                continue

        if _is_noise(body):
            continue

        candidates.append({
            "text": body,
            "line_number": _line_number_at(code, start),
            "already_wrapped": False,
        })
    return candidates


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0
    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object",
                          "example": {"code": "print('Hello world')", "source_type": "python"}}))
        return 0

    code = q.get("code")
    if not isinstance(code, str) or not code.strip():
        print(json.dumps({
            "error": "provide non-empty 'code'",
            "example": {"code": "print('Hello world')\n_('Already translated')", "source_type": "python"},
        }))
        return 0

    source_type = (q.get("source_type") or "python").strip().lower()
    if source_type not in ("python", "javascript", "jsx_tsx"):
        print(json.dumps({
            "error": "invalid 'source_type' — must be one of: python, javascript, jsx_tsx",
            "example": {"code": "t('already wrapped'); const x = 'Hardcoded string';", "source_type": "javascript"},
        }))
        return 0

    try:
        if source_type == "python":
            candidates = _extract_python(code)
        else:
            candidates = _extract_js(code)
    except Exception as e:
        print(json.dumps({"error": "extraction failed: %s" % e}))
        return 1

    result = {
        "candidates": candidates,
        "candidate_count": len(candidates),
        "note": "regex heuristic extraction, not a full parser — review candidates manually; "
                "meant to surface strings that likely SHOULD be moved into an i18n catalog",
    }
    print(json.dumps(result, indent=2, default=str))
    return 0


if __name__ == "__main__":
    sys.exit(main())
