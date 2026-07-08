#!/usr/bin/env python3
"""semver — Engram skill (no network). Parse, bump, or compare semantic
versions per semver.org 2.0.0, using the official regex and precedence rules.

Request (stdin): {"version": "1.2.3-beta.1+build5", "op"?: "parse"|"bump"|"compare",
                   "bump"?: "major"|"minor"|"patch", "other"?: "1.2.4"}
  "op" defaults to "parse" unless "other" is given (-> "compare") or "bump" is
  given (-> "bump").
Output (stdout):
  parse   -> {major, minor, patch, prerelease, buildmetadata, valid}
  bump    -> {from, to, bump}
  compare -> {a, b, result: -1|0|1, explanation}
"""
import json
import re
import sys

# The canonical semver.org regex (semver.org FAQ), with named groups.
_SEMVER_RE = re.compile(
    r"^(?P<major>0|[1-9]\d*)\.(?P<minor>0|[1-9]\d*)\.(?P<patch>0|[1-9]\d*)"
    r"(?:-(?P<prerelease>(?:0|[1-9]\d*|\d*[a-zA-Z-][0-9a-zA-Z-]*)"
    r"(?:\.(?:0|[1-9]\d*|\d*[a-zA-Z-][0-9a-zA-Z-]*))*))?"
    r"(?:\+(?P<buildmetadata>[0-9a-zA-Z-]+(?:\.[0-9a-zA-Z-]+)*))?$"
)


def _parse(version):
    if not isinstance(version, str):
        return None
    m = _SEMVER_RE.match(version.strip())
    if not m:
        return None
    d = m.groupdict()
    return {
        "major": int(d["major"]),
        "minor": int(d["minor"]),
        "patch": int(d["patch"]),
        "prerelease": d["prerelease"],
        "buildmetadata": d["buildmetadata"],
    }


def _compare_prerelease(pre_a, pre_b):
    ids_a = pre_a.split(".")
    ids_b = pre_b.split(".")
    for a, b in zip(ids_a, ids_b):
        a_num, b_num = a.isdigit(), b.isdigit()
        if a_num and b_num:
            ai, bi = int(a), int(b)
            if ai != bi:
                return -1 if ai < bi else 1
        elif a_num and not b_num:
            return -1  # numeric identifiers always have lower precedence
        elif b_num and not a_num:
            return 1
        else:
            if a != b:
                return -1 if a < b else 1
    if len(ids_a) != len(ids_b):
        return -1 if len(ids_a) < len(ids_b) else 1
    return 0


def _compare(pa, pb):
    for k in ("major", "minor", "patch"):
        if pa[k] != pb[k]:
            return -1 if pa[k] < pb[k] else 1
    pre_a, pre_b = pa["prerelease"], pb["prerelease"]
    if pre_a is None and pre_b is None:
        return 0
    if pre_a is None:
        return 1  # no prerelease > has prerelease
    if pre_b is None:
        return -1
    return _compare_prerelease(pre_a, pre_b)


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e})); return 0
    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"version": "1.2.3"},
        })); return 0

    version = q.get("version")
    if not isinstance(version, str) or not version.strip():
        print(json.dumps({
            "error": "missing required field 'version' (string)",
            "example": {"version": "1.2.3", "op": "parse"},
        })); return 0

    op = q.get("op") or None
    other = q.get("other") or None
    bump = q.get("bump") or None
    if not op:
        if other:
            op = "compare"
        elif bump:
            op = "bump"
        else:
            op = "parse"

    if op not in ("parse", "bump", "compare"):
        print(json.dumps({
            "error": "invalid 'op' %r — must be one of: parse, bump, compare" % op,
        })); return 0

    try:
        parsed = _parse(version)

        if op == "parse":
            if not parsed:
                print(json.dumps({
                    "valid": False,
                    "error": "%r is not a valid semantic version (semver.org 2.0.0)" % version,
                })); return 0
            result = dict(parsed)
            result["valid"] = True
            print(json.dumps(result, indent=2, default=str)); return 0

        if op == "bump":
            if not parsed:
                print(json.dumps({
                    "error": "%r is not a valid semantic version (semver.org 2.0.0)" % version,
                })); return 0
            if bump not in ("major", "minor", "patch"):
                print(json.dumps({
                    "error": "missing or invalid 'bump' %r — must be one of: major, minor, patch" % (bump,),
                    "example": {"version": version, "bump": "minor"},
                })); return 0
            major, minor, patch = parsed["major"], parsed["minor"], parsed["patch"]
            if bump == "major":
                major, minor, patch = major + 1, 0, 0
            elif bump == "minor":
                minor, patch = minor + 1, 0
            else:
                patch = patch + 1
            # A bump always produces a fresh release: prerelease and build
            # metadata from the source version are dropped in every case.
            new_version = "%d.%d.%d" % (major, minor, patch)
            print(json.dumps({
                "from": version, "to": new_version, "bump": bump,
            }, indent=2, default=str)); return 0

        # op == "compare"
        if not other:
            print(json.dumps({
                "error": "missing required field 'other' (string) for op=compare",
                "example": {"version": version, "other": "1.2.4"},
            })); return 0
        parsed_other = _parse(other)
        if not parsed or not parsed_other:
            bad = version if not parsed else other
            print(json.dumps({
                "error": "%r is not a valid semantic version (semver.org 2.0.0)" % bad,
            })); return 0
        result = _compare(parsed, parsed_other)
        if result == 0:
            explanation = "%s and %s have equal precedence" % (version, other)
        elif result < 0:
            explanation = "%s has lower precedence than %s" % (version, other)
        else:
            explanation = "%s has higher precedence than %s" % (version, other)
        print(json.dumps({
            "a": version, "b": other, "result": result, "explanation": explanation,
        }, indent=2, default=str)); return 0
    except Exception as e:
        print(json.dumps({"error": "semver failed: %s" % e})); return 1


if __name__ == "__main__":
    sys.exit(main())
