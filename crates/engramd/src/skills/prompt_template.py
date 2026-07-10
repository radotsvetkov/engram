#!/usr/bin/env python3
"""prompt_template — Engram skill (no network). Render or analyze a {placeholder} template.

Placeholders are {name} (word characters). Never raises on missing or extra variables
(does NOT use str.format). analyze: list all placeholders, provided vs missing. render:
substitute provided vars, leave unknown ones as {name} and list them under 'unresolved'.

Request (stdin): {"template": "Hi {name}, from {city}", "variables": {"name": "Sam"}, "action": "render"}
Output (stdout): {action, placeholders, provided, missing|unresolved, rendered?}
"""
import json, sys, re

_PLACEHOLDER_RE = re.compile(r"\{(\w+)\}")


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e})); return 0

    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"template": "Hi {name}", "variables": {"name": "Sam"}, "action": "render"},
        })); return 0

    template = q.get("template")
    if template is None or not isinstance(template, str):
        print(json.dumps({
            "error": "missing required field 'template' (string with {placeholders})",
            "example": {"template": "Hi {name}, from {city}", "variables": {"name": "Sam"}, "action": "render"},
        })); return 0

    variables = q.get("variables")
    if variables is None:
        variables = {}
    if not isinstance(variables, dict):
        print(json.dumps({"error": "'variables' must be an object mapping name -> value"})); return 0

    action = q.get("action", "render")
    if action not in ("render", "analyze"):
        print(json.dumps({"error": "'action' must be 'render' or 'analyze'"})); return 0

    try:
        seen = []
        for name in _PLACEHOLDER_RE.findall(template):
            if name not in seen:
                seen.append(name)
        provided = [n for n in seen if n in variables]
        missing = [n for n in seen if n not in variables]
        extra = [k for k in variables if k not in seen]

        if action == "analyze":
            result = {
                "action": "analyze",
                "placeholders": seen,
                "placeholder_count": len(seen),
                "provided": provided,
                "missing": missing,
                "extra_variables": extra,
            }
        else:
            def _sub(m):
                key = m.group(1)
                return str(variables[key]) if key in variables else m.group(0)
            rendered = _PLACEHOLDER_RE.sub(_sub, template)
            result = {
                "action": "render",
                "placeholders": seen,
                "provided": provided,
                "unresolved": missing,
                "extra_variables": extra,
                "rendered": rendered,
            }
        print(json.dumps(result, indent=2, default=str)); return 0
    except Exception as e:
        print(json.dumps({"error": "prompt_template failed: %s" % e})); return 1


if __name__ == "__main__":
    sys.exit(main())
