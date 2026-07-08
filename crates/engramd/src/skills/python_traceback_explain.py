#!/usr/bin/env python3
"""python_traceback_explain — Engram skill (no network). Parse a pasted
Python traceback, extract the exception type/message and the innermost
frame, and explain typical causes and fixes from a small built-in
knowledge table of common exceptions.

Request (stdin): {"traceback": "Traceback (most recent call last):\\n  File \\"app.py\\", line 3, in <module>\\n    d['x']\\nKeyError: 'x'"}
Output (stdout): {exception_type, exception_message, likely_location, typical_causes, common_fixes}
"""
import json
import re
import sys

# Matches the final "ExceptionType: message" or bare "ExceptionType" line,
# e.g. "KeyError: 'x'", "json.decoder.JSONDecodeError: Expecting value", "RecursionError".
_EXC_LINE_RE = re.compile(
    r"^(?P<type>\w+(?:\.\w+)*(?:Error|Exception|Warning))(?::\s*(?P<msg>.*))?$"
)
# Matches a traceback frame line, e.g. '  File "app.py", line 12, in foo'
_FRAME_RE = re.compile(
    r'File\s+"(?P<file>[^"]+)",\s+line\s+(?P<line>\d+)(?:,\s+in\s+(?P<func>.+))?'
)

_KNOWLEDGE = {
    "KeyError": {
        "typical_causes": [
            "accessing a dict key that doesn't exist",
            "typo in the key name, or the key was never set",
            "assuming a key is always present in external/JSON data",
        ],
        "common_fixes": [
            "use dict.get(key, default) instead of dict[key]",
            "check `if key in d:` before accessing",
            "use collections.defaultdict if a default value makes sense",
            "validate/normalize input data shape before accessing keys",
        ],
    },
    "IndexError": {
        "typical_causes": [
            "accessing a list/tuple index that is out of range",
            "off-by-one error in a loop bound",
            "operating on an empty list without checking length first",
        ],
        "common_fixes": [
            "check `if len(seq) > i:` before indexing",
            "use slicing (seq[i:i+1]) which never raises for out-of-range",
            "iterate with enumerate() instead of manual index math",
        ],
    },
    "TypeError": {
        "typical_causes": [
            "calling a function with the wrong number/type of arguments",
            "mixing incompatible types (e.g. str + int)",
            "calling None as if it were callable, or None.method()",
        ],
        "common_fixes": [
            "check the function signature and argument types being passed",
            "add explicit type conversions (str(), int(), float())",
            "guard against None before calling methods on a value",
        ],
    },
    "ValueError": {
        "typical_causes": [
            "passing a value of the right type but invalid content (e.g. int('abc'))",
            "unpacking a sequence into the wrong number of variables",
            "invalid argument value for a library function",
        ],
        "common_fixes": [
            "validate/sanitize input before conversion",
            "wrap risky conversions in try/except ValueError",
            "check the docs for the expected value range/format",
        ],
    },
    "AttributeError": {
        "typical_causes": [
            "calling a method/attribute that doesn't exist on the object",
            "the object is None where a real instance was expected",
            "typo in an attribute name, or wrong object type passed in",
        ],
        "common_fixes": [
            "print(type(obj)) to confirm you have the object you expect",
            "use hasattr(obj, 'attr') or getattr(obj, 'attr', default) defensively",
            "check for None before attribute access",
        ],
    },
    "NameError": {
        "typical_causes": [
            "using a variable before it's defined or after it's out of scope",
            "typo in a variable/function name",
            "forgetting to import a module or name",
        ],
        "common_fixes": [
            "define the variable earlier in the same scope",
            "double check spelling and case of the identifier",
            "add the missing import statement",
        ],
    },
    "ZeroDivisionError": {
        "typical_causes": [
            "dividing by a variable that evaluated to zero",
            "computing an average/rate over an empty collection (len == 0)",
        ],
        "common_fixes": [
            "check the divisor is non-zero before dividing",
            "guard len(collection) == 0 before computing rates/averages",
        ],
    },
    "FileNotFoundError": {
        "typical_causes": [
            "the file path is wrong or relative to an unexpected working directory",
            "the file hasn't been created yet, or was deleted/moved",
            "a typo in the filename or extension",
        ],
        "common_fixes": [
            "print(os.getcwd()) and use absolute paths to confirm location",
            "check os.path.exists(path) before opening",
            "create the file/directory first if it's expected to not exist yet",
        ],
    },
    "ImportError": {
        "typical_causes": [
            "the package isn't installed in the current environment",
            "circular imports between modules",
            "importing a name that doesn't exist in the target module",
        ],
        "common_fixes": [
            "pip install the missing package in the active virtualenv",
            "check for circular imports and restructure module boundaries",
            "verify the exact name/spelling exported by the module",
        ],
    },
    "ModuleNotFoundError": {
        "typical_causes": [
            "the package isn't installed in the current environment",
            "wrong virtualenv/interpreter is active",
            "typo in the module name",
        ],
        "common_fixes": [
            "pip install the missing package",
            "confirm `which python` / `sys.executable` matches the intended env",
            "check the module name spelling and PYTHONPATH",
        ],
    },
    "RecursionError": {
        "typical_causes": [
            "a recursive function is missing or has a broken base case",
            "genuinely deep recursion beyond Python's default recursion limit",
        ],
        "common_fixes": [
            "verify the base case terminates for all valid inputs",
            "convert the recursion to an iterative loop for deep cases",
            "as a last resort, sys.setrecursionlimit() (rarely the right fix)",
        ],
    },
    "json.decoder.JSONDecodeError": {
        "typical_causes": [
            "the input string is empty or not valid JSON",
            "trailing commas, single quotes, or comments in the JSON text",
            "parsing a response body that is actually HTML/plain text (e.g. an error page)",
        ],
        "common_fixes": [
            "print/log the raw text before json.loads() to inspect it",
            "confirm the source actually returns JSON (check Content-Type/status code)",
            "use a try/except around json.loads with a clear fallback",
        ],
    },
    "ConnectionError": {
        "typical_causes": [
            "the remote host is unreachable, DNS failed, or the service is down",
            "a firewall or VPN is blocking the connection",
            "the server reset/closed the connection mid-request",
        ],
        "common_fixes": [
            "retry with backoff for transient network issues",
            "verify connectivity (curl/ping) to the target host independently",
            "check for firewall/proxy/VPN interference",
        ],
    },
    "TimeoutError": {
        "typical_causes": [
            "the remote service is slow or unresponsive",
            "the configured timeout is too short for the operation",
            "network congestion or a hung connection",
        ],
        "common_fixes": [
            "increase the timeout if the operation is legitimately slow",
            "add retry logic with exponential backoff",
            "investigate whether the downstream service is degraded",
        ],
    },
}
# Alias so "ModuleNotFoundError" style questions also work when written as ImportError text.
_ALIASES = {
    "ModuleNotFoundError": "ModuleNotFoundError",
    "ImportError": "ImportError",
}


def _find_exception_line(lines):
    for line in reversed(lines):
        stripped = line.strip()
        if not stripped:
            continue
        m = _EXC_LINE_RE.match(stripped)
        if m:
            return m.group("type"), (m.group("msg") or "").strip() or None
    return None, None


def _find_innermost_frame(lines):
    # The innermost frame is the LAST "File ..." line before the exception line.
    for line in reversed(lines):
        m = _FRAME_RE.search(line)
        if m:
            return {
                "file": m.group("file"),
                "line": int(m.group("line")),
                "function": (m.group("func") or "").strip() or None,
            }
    return None


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e}))
        return 0
    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"traceback": "Traceback (most recent call last):\n  File \"app.py\", line 3, in <module>\n    d['x']\nKeyError: 'x'"},
        }))
        return 0

    tb = q.get("traceback")
    if not isinstance(tb, str) or not tb.strip():
        print(json.dumps({
            "error": "missing required field 'traceback' (non-empty string)",
            "example": {"traceback": "Traceback (most recent call last):\n  File \"app.py\", line 3, in <module>\n    d['x']\nKeyError: 'x'"},
        }))
        return 0

    try:
        lines = tb.splitlines()
        exc_type, exc_msg = _find_exception_line(lines)
        if not exc_type:
            print(json.dumps({
                "error": "input didn't look like a Python traceback — no line matching "
                         "'ExceptionType: message' (or bare 'ExceptionType') was found",
            }))
            return 0

        location = _find_innermost_frame(lines)
        info = _KNOWLEDGE.get(exc_type)
        result = {
            "exception_type": exc_type,
            "exception_message": exc_msg,
            "likely_location": location,
        }
        if info:
            result["typical_causes"] = info["typical_causes"]
            result["common_fixes"] = info["common_fixes"]
        else:
            result["typical_causes"] = []
            result["common_fixes"] = []
            result["note"] = (
                "'%s' isn't in the built-in knowledge table, but the type/message/location "
                "above were parsed from the traceback." % exc_type
            )
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "python_traceback_explain failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
