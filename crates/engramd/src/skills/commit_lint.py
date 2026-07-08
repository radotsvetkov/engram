#!/usr/bin/env python3
"""commit_lint — Engram skill (no network). Validate a commit message header
against the Conventional Commits spec (https://www.conventionalcommits.org).

Request (stdin): {"message": "feat(parser): add support for X"}
Output (stdout): {valid, type, scope, breaking, subject, errors, warnings}
"""
import json
import re
import sys

_VALID_TYPES = [
    "feat", "fix", "docs", "style", "refactor", "perf", "test",
    "build", "ci", "chore", "revert",
]

_HEADER_RE = re.compile(r"^(?P<type>\w+)(\((?P<scope>[\w./-]+)\))?(?P<breaking>!)?: (?P<subject>.+)$")


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e})); return 0
    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"message": "feat(api): add search endpoint"},
        })); return 0

    message = q.get("message")
    if not isinstance(message, str) or not message.strip():
        print(json.dumps({
            "error": "missing required field 'message' (string, the commit message)",
            "example": {"message": "fix(parser): handle empty input"},
        })); return 0

    try:
        header = message.split("\n", 1)[0]
        errors = []
        warnings = []
        m = _HEADER_RE.match(header)

        commit_type = None
        scope = None
        breaking = False
        subject = None

        if not m:
            errors.append(
                "header does not match Conventional Commits format: expected "
                "'type(scope)!: subject', e.g. 'feat(parser): add support for X'"
            )
        else:
            commit_type = m.group("type")
            scope = m.group("scope")
            breaking = m.group("breaking") is not None
            subject = m.group("subject")

            if commit_type not in _VALID_TYPES:
                errors.append(
                    "invalid type %r — must be one of: %s" % (commit_type, ", ".join(_VALID_TYPES))
                )

            if not subject.strip():
                errors.append("subject is empty")
            else:
                if subject.endswith("."):
                    warnings.append("subject should not end with a period")
                if subject[0].isupper():
                    warnings.append("subject should not start with an uppercase letter (prefer lowercase)")
                words = subject.split()
                first_word = words[0] if words else ""
                fw_lower = first_word.lower()
                if fw_lower.endswith("ed") or fw_lower.endswith("ing"):
                    warnings.append(
                        "subject may not be in imperative mood — prefer e.g. 'add' over "
                        "%r ('added'/'adding' style)" % first_word
                    )

        if len(header) > 100:
            errors.append("header is %d characters, exceeds the 100 character hard limit" % len(header))
        elif len(header) > 72:
            warnings.append("header is %d characters, longer than the ideal 72 character limit" % len(header))

        result = {
            "valid": len(errors) == 0,
            "type": commit_type,
            "scope": scope,
            "breaking": breaking,
            "subject": subject,
            "errors": errors,
            "warnings": warnings,
        }
        print(json.dumps(result, indent=2, default=str)); return 0
    except Exception as e:
        print(json.dumps({"error": "commit_lint failed: %s" % e})); return 1


if __name__ == "__main__":
    sys.exit(main())
