#!/usr/bin/env python3
"""dotenv_parse — Engram skill (no network). Parse a .env file into key/values.

Handles KEY=value lines, `export KEY=...` prefixes, `#` comment and blank lines,
single/double-quoted values (quotes stripped), and inline comments after
UNQUOTED values. With mask_secrets, any key matching (case-insensitive)
key|secret|token|password|pwd|api|private|credential has its value masked to
first/last 2 chars — a full secret is NEVER echoed.

Request (stdin): {"content": "KEY=val\\n# c\\nAPI_KEY=abc", "mask_secrets"?: true}
Output (stdout): {variables, count, secret_keys_masked}
"""
import json, sys, re

_SECRET_RE = re.compile(r"(key|secret|token|password|pwd|api|private|credential)", re.IGNORECASE)


def _mask(value):
    if not isinstance(value, str) or value == "":
        return "****"
    if len(value) <= 4:
        return "*" * len(value)
    return value[:2] + "*" * (len(value) - 4) + value[-2:]


def _strip_inline_comment(s):
    # Remove an inline comment starting with ' #' (space then #) in unquoted values.
    # We look for a '#' preceded by whitespace (or at start).
    out = []
    i = 0
    while i < len(s):
        c = s[i]
        if c == "#" and (i == 0 or s[i - 1] in " \t"):
            break
        out.append(c)
        i += 1
    return "".join(out).rstrip()


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e})); return 0

    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"content": "PORT=8080\nAPI_KEY=secret123", "mask_secrets": True},
        })); return 0

    content = q.get("content")
    if not isinstance(content, str):
        print(json.dumps({
            "error": "missing required field 'content' (the .env text as a string)",
            "example": {"content": "# comment\nPORT=8080\nexport TOKEN=\"abc def\"", "mask_secrets": True},
        })); return 0

    # Mask secrets by default — a .env parser surfaces credentials, so an
    # explicit {"mask_secrets": false} is required to reveal raw values.
    mask = q.get("mask_secrets", True) is not False

    try:
        variables = {}
        secret_keys = []
        for raw_line in content.splitlines():
            line = raw_line.strip()
            if line == "" or line.startswith("#"):
                continue
            if line.startswith("export "):
                line = line[len("export "):].lstrip()
            if "=" not in line:
                continue  # not a KEY=value line; skip silently
            key, _, val = line.partition("=")
            key = key.strip()
            if key == "":
                continue
            val = val.strip()
            # Quoted value: strip surrounding matching quotes, keep contents verbatim.
            if len(val) >= 2 and val[0] == val[-1] and val[0] in ("'", '"'):
                val = val[1:-1]
            else:
                # Unquoted: strip an inline comment.
                val = _strip_inline_comment(val)

            is_secret = _SECRET_RE.search(key) is not None
            if mask and is_secret:
                variables[key] = _mask(val)
                secret_keys.append(key)
            else:
                variables[key] = val

        result = {
            "variables": variables,
            "count": len(variables),
            "secret_keys_masked": secret_keys,
        }
        print(json.dumps(result, indent=2, default=str)); return 0
    except Exception as e:
        print(json.dumps({"error": "dotenv_parse failed: %s" % e})); return 1


if __name__ == "__main__":
    sys.exit(main())
