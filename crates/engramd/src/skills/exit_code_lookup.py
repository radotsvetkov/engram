#!/usr/bin/env python3
"""exit_code_lookup — Engram skill (no network). Look up the meaning of a
shell/Linux/POSIX process exit code.

Request (stdin): {"code": 137}
Output (stdout): {code, meaning, category: "success"|"error"|"signal"}
"""
import json
import sys

_EXPLICIT = {
    0: "success",
    1: "general error / catchall",
    2: "misuse of shell builtin (e.g. bad usage/syntax)",
    124: "command timed out (common convention used by the `timeout` utility)",
    126: "command found but not executable (permission denied)",
    127: "command not found",
    128: "invalid argument to exit",
    130: "terminated by Ctrl-C (SIGINT)",
    137: "killed (SIGKILL) — often the OOM killer",
    139: "segmentation fault (SIGSEGV)",
    143: "terminated (SIGTERM)",
}

_SIGNAL_NAMES = {
    1: "SIGHUP", 2: "SIGINT", 3: "SIGQUIT", 6: "SIGABRT", 9: "SIGKILL",
    11: "SIGSEGV", 13: "SIGPIPE", 15: "SIGTERM",
}

_EXAMPLE = {"code": 137}


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e})); return 0
    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object", "example": _EXAMPLE})); return 0

    code = q.get("code")
    if isinstance(code, bool) or not isinstance(code, int):
        print(json.dumps({
            "error": "missing/invalid required field 'code' (integer exit code)",
            "example": _EXAMPLE,
        })); return 0

    if not (0 <= code <= 255):
        print(json.dumps({
            "error": (
                "code %d is out of range — POSIX exit codes are 0-255 "
                "(many shells wrap higher values mod 256)"
            ) % code,
            "example": _EXAMPLE,
        })); return 0

    try:
        if code in _EXPLICIT:
            meaning = _EXPLICIT[code]
        elif 129 <= code <= 255:
            signal_number = code - 128
            name = _SIGNAL_NAMES.get(signal_number, "unknown signal")
            meaning = "terminated by signal %d (%s)" % (signal_number, name)
        else:
            meaning = "application-specific exit status"

        category = "success" if code == 0 else ("signal" if 129 <= code <= 255 else "error")

        result = {"code": code, "meaning": meaning, "category": category}
        print(json.dumps(result, indent=2, default=str)); return 0
    except Exception as e:
        print(json.dumps({"error": "exit_code_lookup failed: %s" % e})); return 1


if __name__ == "__main__":
    sys.exit(main())
