#!/usr/bin/env python3
"""changelog_gen — Engram skill (no network). Generate a Keep-a-Changelog style
changelog from a list of Conventional Commit messages.

feat -> Added, fix -> Fixed, perf/refactor -> Changed, docs -> Documentation,
revert -> Reverted. chore/ci/build/test/style are internal and excluded from
the rendered changelog (but counted). Any commit with a breaking flag ('!'
after type/scope) goes into a top-level "BREAKING CHANGES" section regardless
of its type. Unparseable lines go under "Other".

Request (stdin): {"commits": ["feat(auth): add OAuth login", "fix: crash on empty input"]}
Output (stdout): {sections, markdown, skipped_internal_count}
"""
import json
import re
import sys

_HEADER_RE = re.compile(r"^(?P<type>\w+)(\((?P<scope>[\w./-]+)\))?(?P<breaking>!)?: (?P<subject>.+)$")

_SECTION_MAP = {
    "feat": "Added",
    "fix": "Fixed",
    "perf": "Changed",
    "refactor": "Changed",
    "docs": "Documentation",
    "revert": "Reverted",
}
_INTERNAL_TYPES = {"chore", "ci", "build", "test", "style"}
_SECTION_ORDER = ["BREAKING CHANGES", "Added", "Fixed", "Changed", "Documentation", "Reverted", "Other"]

_EXAMPLE = {"commits": ["feat(auth): add OAuth login", "fix!: breaking change to auth"]}


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e})); return 0
    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object", "example": _EXAMPLE})); return 0

    commits = q.get("commits")
    if not isinstance(commits, list) or not commits:
        print(json.dumps({
            "error": "missing required field 'commits' (non-empty array of commit message strings)",
            "example": _EXAMPLE,
        })); return 0

    try:
        sections = {}
        skipped_internal_count = 0

        def _add(section, subject):
            sections.setdefault(section, []).append(subject)

        for raw in commits:
            if not isinstance(raw, str):
                _add("Other", str(raw))
                continue
            header = raw.strip().split("\n", 1)[0]
            m = _HEADER_RE.match(header)
            if not m:
                _add("Other", header)
                continue

            ctype = m.group("type").lower()
            scope = m.group("scope")
            breaking = m.group("breaking") is not None
            subject = m.group("subject").strip()
            display = "**%s**: %s" % (scope, subject) if scope else subject

            if breaking:
                _add("BREAKING CHANGES", display)
                continue

            if ctype in _INTERNAL_TYPES:
                skipped_internal_count += 1
                continue

            section = _SECTION_MAP.get(ctype, "Other")
            _add(section, display)

        lines = []
        rendered = set()
        for section in _SECTION_ORDER:
            items = sections.get(section)
            if not items:
                continue
            rendered.add(section)
            lines.append("## %s" % section)
            for item in items:
                lines.append("- %s" % item)
            lines.append("")
        for section, items in sections.items():
            if section in rendered:
                continue
            lines.append("## %s" % section)
            for item in items:
                lines.append("- %s" % item)
            lines.append("")

        markdown = "\n".join(lines).rstrip("\n")

        result = {
            "sections": sections,
            "markdown": markdown,
            "skipped_internal_count": skipped_internal_count,
        }
        print(json.dumps(result, indent=2, default=str)); return 0
    except Exception as e:
        print(json.dumps({"error": "changelog_gen failed: %s" % e})); return 1


if __name__ == "__main__":
    sys.exit(main())
