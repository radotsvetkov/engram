#!/usr/bin/env python3
"""translation_quality_check — Engram skill (no network). Heuristic sanity
checks on a (original, translated) text pair. This is NOT a real
translation-quality evaluator (no BLEU/COMET/human-eval scoring) — just
cheap, useful smell tests: length ratio sanity, placeholder/variable-token
preservation (a common, real localization bug), and untranslated-marker
detection.

Request (stdin): {"original": str, "translated": str,
                   "source_lang"?: str, "target_lang"?: str}
Output (stdout): {length_ratio, length_ratio_flag, missing_placeholders,
                   untranslated_marker_found, overall_pass, note}
"""
import json
import re
import sys

# Acceptable length-ratio band is intentionally wide — different language
# pairs (e.g. English -> German vs English -> Chinese) naturally vary a lot
# in character-count ratio, so this only catches gross drops/duplication.
_MIN_RATIO = 0.3
_MAX_RATIO = 3.0

_PLACEHOLDER_PATTERNS = [
    re.compile(r"\{\{[^{}]+\}\}"),   # {{name}}
    re.compile(r"\{[^{}]+\}"),      # {name}
    re.compile(r"%\([^)]+\)[sd]"),  # %(name)s
    re.compile(r"%[sd]"),           # %s
    re.compile(r"\$\{?[A-Za-z_][A-Za-z0-9_]*\}?"),  # $name / ${name}
    re.compile(r"<[^<>\s]+>"),      # <tag>-style
]

_UNTRANSLATED_MARKERS = ("TODO", "TRANSLATE", "FIXME")


def _extract_placeholders(text):
    found = []
    for pattern in _PLACEHOLDER_PATTERNS:
        found.extend(pattern.findall(text))
    # De-duplicate while preserving order.
    seen = set()
    unique = []
    for tok in found:
        if tok not in seen:
            seen.add(tok)
            unique.append(tok)
    return unique


def _has_doubled_whitespace(text):
    return bool(re.search(r"  +", text)) or bool(re.search(r"\n{3,}", text))


def _check(original, translated):
    orig_len = len(original)
    trans_len = len(translated)
    length_ratio = (trans_len / orig_len) if orig_len > 0 else None
    length_ratio_flag = False
    if length_ratio is not None and not (_MIN_RATIO <= length_ratio <= _MAX_RATIO):
        length_ratio_flag = True

    placeholders = _extract_placeholders(original)
    missing_placeholders = [tok for tok in placeholders if tok not in translated]

    untranslated_marker_found = any(marker in translated for marker in _UNTRANSLATED_MARKERS)
    doubled_whitespace = _has_doubled_whitespace(translated)

    overall_pass = (
        not length_ratio_flag
        and not missing_placeholders
        and not untranslated_marker_found
    )

    result = {
        "length_ratio": length_ratio,
        "length_ratio_flag": length_ratio_flag,
        "missing_placeholders": missing_placeholders,
        "untranslated_marker_found": untranslated_marker_found,
        "doubled_or_excess_whitespace_found": doubled_whitespace,
        "overall_pass": overall_pass,
        "note": "heuristic sanity checks only — NOT a real quality/BLEU-score evaluator; "
                "a pass here does not guarantee an accurate or fluent translation",
    }
    if length_ratio_flag:
        result["length_ratio_note"] = (
            "translated text is much shorter/longer than the original — verify nothing was "
            "dropped or duplicated"
        )
    return result


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0
    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object",
                          "example": {"original": "Hello {name}", "translated": "Hola {name}"}}))
        return 0

    original = q.get("original")
    translated = q.get("translated")
    if not isinstance(original, str) or not isinstance(translated, str) or not original:
        print(json.dumps({
            "error": "provide non-empty 'original' and 'translated' strings",
            "example": {"original": "Hello {name}, you have %s new messages",
                        "translated": "Hola {name}, tienes %s mensajes nuevos",
                        "source_lang": "en", "target_lang": "es"},
        }))
        return 0

    try:
        result = _check(original, translated)
    except Exception as e:
        print(json.dumps({"error": "check failed: %s" % e}))
        return 1

    result["source_lang"] = q.get("source_lang")
    result["target_lang"] = q.get("target_lang")
    print(json.dumps(result, indent=2, default=str))
    return 0


if __name__ == "__main__":
    sys.exit(main())
