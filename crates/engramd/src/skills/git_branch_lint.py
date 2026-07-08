#!/usr/bin/env python3
"""git_branch_lint — Engram skill (no network). Validate a git branch name
against a feature/fix/chore-style naming convention: lowercase prefix, '/',
lowercase-hyphenated slug (e.g. "feature/add-login").

Request (stdin): {"branch": "feature/add-login"}
Output (stdout): {valid, prefix, slug} on a valid conventional branch,
  {valid: true, protected: true, note} for reserved names (main/master/develop/dev),
  or {valid: false, errors, suggested_branch} otherwise.
"""
import json
import re
import sys

_PREFIXES = ("feature", "feat", "fix", "bugfix", "hotfix", "chore", "docs", "release", "refactor", "test")
_VALID_RE = re.compile(r'^(' + "|".join(_PREFIXES) + r')/[a-z0-9]+(-[a-z0-9]+)*$')
_RESERVED = ("main", "master", "develop", "dev")

_EXAMPLE = {"branch": "feature/add-login"}


def _suggest(branch):
    s = branch.strip().lower()
    s = re.sub(r'[_\s]+', '-', s)
    s = re.sub(r'[^a-z0-9/-]+', '', s)
    s = re.sub(r'-{2,}', '-', s).strip('-')

    if "/" in s:
        prefix, _, rest = s.partition("/")
        rest = re.sub(r'-{2,}', '-', rest).strip('-')
        if prefix in _PREFIXES:
            return "%s/%s" % (prefix, rest) if rest else "%s/change" % prefix
        slug = rest or prefix
        return "feature/%s" % slug if slug else "feature/change"

    tokens = [t for t in s.split("-") if t]
    found_prefix = None
    remaining = tokens
    for i, tok in enumerate(tokens):
        if tok in _PREFIXES:
            found_prefix = tok
            remaining = tokens[:i] + tokens[i + 1:]
            break
    slug = "-".join(remaining) or "change"
    prefix = found_prefix or "feature"
    return "%s/%s" % (prefix, slug)


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e})); return 0
    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object", "example": _EXAMPLE})); return 0

    branch = q.get("branch")
    if not isinstance(branch, str) or not branch.strip():
        print(json.dumps({
            "error": "missing required field 'branch' (string, the branch name)",
            "example": _EXAMPLE,
        })); return 0

    branch = branch.strip()

    try:
        if branch in _RESERVED:
            print(json.dumps({
                "valid": True,
                "protected": True,
                "note": "reserved branch name, not subject to the prefix/slug convention",
            }, indent=2, default=str)); return 0

        m = _VALID_RE.match(branch)
        if m:
            prefix, rest = branch.split("/", 1)
            print(json.dumps({
                "valid": True,
                "prefix": prefix,
                "slug": rest,
            }, indent=2, default=str)); return 0

        errors = []
        if any(c.isupper() for c in branch):
            errors.append("contains uppercase letters — branch names should be all lowercase")
        if "_" in branch or " " in branch:
            errors.append(
                "contains underscores or spaces — use hyphens instead, e.g. %r"
                % re.sub(r'[_\s]+', '-', branch).lower()
            )
        if "/" not in branch:
            errors.append("missing the '/' separator between prefix and slug, e.g. 'feature/your-slug'")
        else:
            prefix = branch.split("/", 1)[0]
            if prefix.lower() not in _PREFIXES:
                errors.append(
                    "missing a recognized prefix — must be one of: %s" % ", ".join(_PREFIXES)
                )
        if not errors:
            errors.append(
                "does not match the required pattern <prefix>/<lowercase-hyphenated-slug>, "
                "e.g. 'feature/add-login'"
            )

        print(json.dumps({
            "valid": False,
            "errors": errors,
            "suggested_branch": _suggest(branch),
        }, indent=2, default=str)); return 0
    except Exception as e:
        print(json.dumps({"error": "git_branch_lint failed: %s" % e})); return 1


if __name__ == "__main__":
    sys.exit(main())
