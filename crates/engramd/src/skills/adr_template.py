#!/usr/bin/env python3
"""adr_template — Engram skill (no network). Fill a Michael Nygard style
Architecture Decision Record (ADR) Markdown template.

Request (stdin): {"title": "Use Postgres for primary storage", "status": "accepted",
                   "context": "...", "decision": "...", "consequences": "..."}
Output (stdout): {markdown}
"""
import json
import sys

_CONTEXT_PROMPT = (
    "[Describe the forces at play — technical, business, and constraints — "
    "that make this decision necessary]"
)
_DECISION_PROMPT = "[State the decision clearly, in full sentences, and explain the reasoning]"
_CONSEQUENCES_PROMPT = (
    "[Describe the resulting context after applying the decision — both positive and negative]"
)

_EXAMPLE = {"title": "Use Postgres for primary storage", "status": "proposed"}


def _text(v, prompt):
    if isinstance(v, str) and v.strip():
        return v.strip()
    return prompt


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e})); return 0
    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object", "example": _EXAMPLE})); return 0

    title = q.get("title")
    if not isinstance(title, str) or not title.strip():
        print(json.dumps({
            "error": "missing required field 'title' (string)",
            "example": _EXAMPLE,
        })); return 0

    status = q.get("status") or "proposed"
    if not isinstance(status, str) or not status.strip():
        status = "proposed"

    context = q.get("context")
    decision = q.get("decision")
    consequences = q.get("consequences")

    try:
        markdown = (
            "# %s\n\n"
            "## Status\n\n%s\n\n"
            "## Context\n\n%s\n\n"
            "## Decision\n\n%s\n\n"
            "## Consequences\n\n%s\n"
        ) % (
            title.strip(),
            status.strip(),
            _text(context, _CONTEXT_PROMPT),
            _text(decision, _DECISION_PROMPT),
            _text(consequences, _CONSEQUENCES_PROMPT),
        )
        print(json.dumps({"markdown": markdown}, indent=2, default=str)); return 0
    except Exception as e:
        print(json.dumps({"error": "adr_template failed: %s" % e})); return 1


if __name__ == "__main__":
    sys.exit(main())
