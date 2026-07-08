#!/usr/bin/env python3
"""secrets_scan — Engram skill (no network). Scan text for leaked secrets.

Regex-scans arbitrary text for common secret formats (AWS keys, GitHub/Slack/
Stripe tokens, Google API keys, PEM private key headers) plus a low-confidence
generic "key/secret/token/password = ..." assignment pattern. Matches are
never echoed in full — only the first/last 4 characters are shown, joined
by "...".

Request (stdin): {"text": "...some text possibly containing secrets..."}
Output (stdout): {findings: [{type, masked_match, position, confidence}, ...],
                   finding_count}
"""
import json
import re
import sys

_PATTERNS = [
    ("AWS Access Key", re.compile(r"AKIA[0-9A-Z]{16}"), "high"),
    ("GitHub Token", re.compile(r"gh[pousr]_[A-Za-z0-9]{36,}"), "high"),
    ("Slack Token", re.compile(r"xox[baprs]-[A-Za-z0-9-]{10,}"), "high"),
    ("Stripe Key", re.compile(r"(sk|pk)_(live|test)_[A-Za-z0-9]{20,}"), "high"),
    ("Google API Key", re.compile(r"AIza[0-9A-Za-z_-]{35}"), "high"),
    ("Private Key Header", re.compile(r"-----BEGIN[ A-Z]*PRIVATE KEY-----"), "high"),
    ("Generic Secret Assignment",
     re.compile(r"(?i)(api[_-]?key|secret|token|password)\s*[:=]\s*['\"]?[A-Za-z0-9+/_=\-]{12,}"),
     "low"),
]


def _mask(match_text):
    if len(match_text) <= 8:
        return match_text[:2] + "..." if len(match_text) > 2 else "..."
    return match_text[:4] + "..." + match_text[-4:]


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e}))
        return 0

    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object",
                          "example": {"text": "aws_key = AKIAABCDEFGHIJKLMNOP"}}))
        return 0

    text = q.get("text")
    if text is None or text == "":
        print(json.dumps({"error": "provide 'text' to scan",
                          "example": {"text": "aws_key = AKIAABCDEFGHIJKLMNOP"}}))
        return 0

    if not isinstance(text, str):
        text = str(text)

    try:
        findings = []
        covered_spans = []
        for type_name, pattern, confidence in _PATTERNS:
            for m in pattern.finditer(text):
                span = m.span()
                # skip low-confidence hits that overlap a higher-confidence match
                if confidence == "low" and any(
                    span[0] < c[1] and span[1] > c[0] for c in covered_spans
                ):
                    continue
                if confidence == "high":
                    covered_spans.append(span)
                findings.append({
                    "type": type_name,
                    "masked_match": _mask(m.group(0)),
                    "position": span[0],
                    "confidence": confidence,
                })

        findings.sort(key=lambda f: f["position"])
        result = {"findings": findings, "finding_count": len(findings)}
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "secrets_scan failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
