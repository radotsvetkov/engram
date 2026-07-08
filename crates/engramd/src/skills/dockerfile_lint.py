#!/usr/bin/env python3
"""dockerfile_lint — Engram skill (no network). Heuristic line-based Dockerfile
linter (not a full BuildKit parser).

Flags: FROM using :latest or no tag; missing USER instruction (runs as root);
`RUN apt-get install` without --no-install-recommends and apt list cleanup;
ADD used on a plain local file (COPY is safer/clearer than ADD's remote-fetch/
auto-extract behavior); missing HEALTHCHECK (informational); and a hardcoded-
secret heuristic on ENV/ARG variable names containing key/secret/password/token
that have a literal non-empty value.

Request (stdin): {"dockerfile": "FROM ubuntu\\nRUN apt-get update && apt-get install -y curl\\n"}
Output (stdout): {warnings, info, line_count, expose_count}
"""
import json
import re
import sys

_ARCHIVE_EXTS = (".tar", ".tar.gz", ".tgz", ".tar.bz2", ".tar.xz", ".zip")
_ASSIGN_RE = re.compile(r'([A-Za-z_][A-Za-z0-9_]*)=("(?:[^"\\]|\\.)*"|\'[^\']*\'|\S+)')
_SECRET_NAME_RE = re.compile(r'key|secret|password|token', re.I)
_APT_INSTALL_RE = re.compile(r'apt-get\s+install', re.I)
_APT_CLEANUP_RE = re.compile(r'rm\s+-rf\s+/var/lib/apt/lists', re.I)

_EXAMPLE = {"dockerfile": "FROM ubuntu:22.04\nUSER app\n"}


def _logical_lines(text):
    """Join backslash-continued physical lines into logical instruction lines."""
    logical = []
    buf = []
    for raw_line in text.splitlines():
        stripped_end = raw_line.rstrip()
        if stripped_end.endswith("\\"):
            buf.append(stripped_end[:-1])
            continue
        buf.append(raw_line)
        logical.append(" ".join(s.strip() for s in buf) if len(buf) > 1 else buf[0])
        buf = []
    if buf:
        logical.append(" ".join(s.strip() for s in buf) if len(buf) > 1 else buf[0])
    return logical


def _strip_quotes(v):
    v = v.strip()
    if len(v) >= 2 and v[0] == v[-1] and v[0] in "\"'":
        return v[1:-1]
    return v


def _extract_assignments(instr, rest):
    rest = rest.strip()
    if not rest:
        return []
    if "=" in rest:
        return [(name, _strip_quotes(val)) for name, val in _ASSIGN_RE.findall(rest)]
    parts = rest.split(None, 1)
    if instr == "ENV" and len(parts) == 2:
        return [(parts[0], parts[1].strip())]
    return [(parts[0], "")] if parts else []


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e})); return 0
    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object", "example": _EXAMPLE})); return 0

    dockerfile = q.get("dockerfile")
    if not isinstance(dockerfile, str) or not dockerfile.strip():
        print(json.dumps({
            "error": "missing required field 'dockerfile' (string, the Dockerfile contents)",
            "example": _EXAMPLE,
        })); return 0

    try:
        warnings = []
        info = []
        has_user = False
        has_healthcheck = False
        expose_count = 0

        parsed = []  # (instruction, rest)
        for line in _logical_lines(dockerfile):
            stripped = line.strip()
            if not stripped or stripped.startswith("#"):
                continue
            parts = stripped.split(None, 1)
            instr = parts[0].upper()
            rest = parts[1] if len(parts) > 1 else ""
            parsed.append((instr, rest))

        for idx, (instr, rest) in enumerate(parsed):
            if instr == "FROM":
                image_ref = rest.split()[0] if rest.split() else ""
                pinned = False
                if "@" in image_ref:
                    pinned = True
                else:
                    last_slash = image_ref.rfind("/")
                    last_colon = image_ref.rfind(":")
                    if last_colon > last_slash and image_ref[last_colon + 1:] != "latest":
                        pinned = True
                if not pinned and image_ref:
                    warnings.append(
                        "FROM %s: pin a specific base image tag, not :latest or unpinned, "
                        "for reproducible builds" % image_ref
                    )
            elif instr == "USER":
                has_user = True
            elif instr == "HEALTHCHECK":
                has_healthcheck = True
            elif instr == "EXPOSE":
                expose_count += 1
            elif instr == "RUN" and _APT_INSTALL_RE.search(rest):
                has_recommends = "--no-install-recommends" in rest
                has_cleanup = bool(_APT_CLEANUP_RE.search(rest)) or any(
                    _APT_CLEANUP_RE.search(parsed[j][1]) for j in range(idx + 1, len(parsed))
                )
                if not has_recommends and not has_cleanup:
                    warnings.append(
                        "RUN %s: apt-get install without --no-install-recommends and apt list "
                        "cleanup bloats the image" % rest
                    )
            elif instr == "ADD":
                tokens = rest.split()
                if len(tokens) >= 2:
                    for src in tokens[:-1]:
                        is_url = src.lower().startswith(("http://", "https://"))
                        is_archive = src.lower().endswith(_ARCHIVE_EXTS)
                        if not is_url and not is_archive:
                            info.append(
                                "ADD %s: prefer COPY over ADD for plain local files — ADD's "
                                "remote-fetch/auto-extract behavior is easy to trigger by accident" % rest
                            )
                            break

            if instr in ("ENV", "ARG"):
                for name, value in _extract_assignments(instr, rest):
                    if _SECRET_NAME_RE.search(name) and value.strip():
                        warnings.append(
                            "possible hardcoded secret in %s %s — pass secrets at runtime instead"
                            % (instr, name)
                        )

        if not has_user:
            warnings.append("container runs as root — add a USER instruction")
        if not has_healthcheck:
            info.append("no HEALTHCHECK instruction defined (not required, but recommended)")

        result = {
            "warnings": warnings,
            "info": info,
            "line_count": len(dockerfile.splitlines()),
            "expose_count": expose_count,
        }
        print(json.dumps(result, indent=2, default=str)); return 0
    except Exception as e:
        print(json.dumps({"error": "dockerfile_lint failed: %s" % e})); return 1


if __name__ == "__main__":
    sys.exit(main())
