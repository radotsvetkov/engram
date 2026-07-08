#!/usr/bin/env python3
"""chmod_calc — Engram skill (no network). Convert between symbolic and octal
Unix file-permission representations.

Accepts a symbolic string like "rwxr-xr-x", an octal string like "755" or
"0755", or a plain integer like 755 (interpreted as octal digits — 755 means
rwxr-xr-x, not the decimal number 755). Shorter numerals (e.g. "5") are
left-padded with zeros, matching how the `chmod` utility itself treats them.

Request (stdin): {"mode": "755"}  or  {"mode": "rwxr-xr-x"}  or  {"mode": 755}
Output (stdout): {input_mode, octal, symbolic, owner, group, other}
"""
import json
import re
import sys

_SYMBOLIC_RE = re.compile(r'^[rwx-]{9}$')
_EXAMPLE = {"octal_example": {"mode": "755"}, "symbolic_example": {"mode": "rwxr-xr-x"}}


def _digit_to_perm(n):
    return {"read": bool(n & 4), "write": bool(n & 2), "execute": bool(n & 1)}


def _digit_to_symbolic(n):
    p = _digit_to_perm(n)
    return ("r" if p["read"] else "-") + ("w" if p["write"] else "-") + ("x" if p["execute"] else "-")


def _group_to_digit(chunk):
    r, w, x = chunk[0], chunk[1], chunk[2]
    if r not in ("r", "-") or w not in ("w", "-") or x not in ("x", "-"):
        raise ValueError("invalid permission characters %r" % (chunk,))
    return (4 if r == "r" else 0) + (2 if w == "w" else 0) + (1 if x == "x" else 0)


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e})); return 0
    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object", "example": _EXAMPLE})); return 0

    mode = q.get("mode")
    if mode is None:
        print(json.dumps({
            "error": "missing required field 'mode' (symbolic 'rwxr-xr-x', octal '755'/'0755', or int 755)",
            "example": _EXAMPLE,
        })); return 0

    if isinstance(mode, bool):
        print(json.dumps({"error": "'mode' must be a string or integer, not a boolean", "example": _EXAMPLE})); return 0

    try:
        input_mode = mode
        digits = None

        if isinstance(mode, str) and _SYMBOLIC_RE.match(mode.strip()):
            s = mode.strip()
            groups = [s[0:3], s[3:6], s[6:9]]
            try:
                digits = [_group_to_digit(g) for g in groups]
            except ValueError as e:
                print(json.dumps({"error": "invalid symbolic mode: %s" % e, "example": _EXAMPLE})); return 0
        else:
            if isinstance(mode, int):
                raw = str(mode)
            elif isinstance(mode, str):
                raw = mode.strip()
            else:
                print(json.dumps({"error": "'mode' must be a string or integer", "example": _EXAMPLE})); return 0

            if not raw or not raw.isdigit():
                print(json.dumps({
                    "error": (
                        "%r is not a valid mode — expected a 9-character symbolic string "
                        "(e.g. 'rwxr-xr-x') or an octal permission string/integer (e.g. '755', '0755', 755)"
                    ) % (mode,),
                    "example": _EXAMPLE,
                })); return 0

            if len(raw) > 4:
                print(json.dumps({
                    "error": "octal mode is too long: %r (expected 3 digits, e.g. '755')" % (mode,),
                    "example": _EXAMPLE,
                })); return 0
            if len(raw) == 4:
                if raw[0] != "0":
                    print(json.dumps({
                        "error": (
                            "4-digit octal mode %r is not supported (this tool doesn't model "
                            "setuid/setgid/sticky bits) — use exactly 3 digits, e.g. '755'"
                        ) % (mode,),
                        "example": _EXAMPLE,
                    })); return 0
                raw = raw[1:]
            elif len(raw) < 3:
                raw = raw.rjust(3, "0")

            try:
                digits = [int(c, 8) for c in raw]
            except ValueError:
                print(json.dumps({
                    "error": "octal digits must be 0-7, got %r" % (mode,),
                    "example": _EXAMPLE,
                })); return 0

        owner_digit, group_digit, other_digit = digits
        octal_str = "%d%d%d" % (owner_digit, group_digit, other_digit)
        symbolic_str = "".join(_digit_to_symbolic(d) for d in digits)

        result = {
            "input_mode": input_mode,
            "octal": octal_str,
            "symbolic": symbolic_str,
            "owner": _digit_to_perm(owner_digit),
            "group": _digit_to_perm(group_digit),
            "other": _digit_to_perm(other_digit),
        }
        print(json.dumps(result, indent=2, default=str)); return 0
    except Exception as e:
        print(json.dumps({"error": "chmod_calc failed: %s" % e})); return 1


if __name__ == "__main__":
    sys.exit(main())
