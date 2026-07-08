#!/usr/bin/env python3
"""readme_scaffold — Engram skill (no network). Generate a well-structured
README.md (title, description, installation, usage, license) from a project
name and optional details.

Request (stdin): {"project_name": "Engram", "description": "A self-improving agent daemon.", "install_command": "cargo install engram", "usage_example": "engram run", "license": "MIT"}
Output (stdout): {markdown}
"""
import json
import sys


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e}))
        return 0
    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"project_name": "MyProject", "description": "What it does."},
        }))
        return 0

    project_name = q.get("project_name")
    if not isinstance(project_name, str) or not project_name.strip():
        print(json.dumps({
            "error": "missing required field 'project_name' (non-empty string)",
            "example": {"project_name": "MyProject", "description": "What it does."},
        }))
        return 0
    project_name = project_name.strip()

    description = q.get("description")
    install_command = q.get("install_command")
    usage_example = q.get("usage_example")
    license_name = q.get("license")

    for field, val in (
        ("description", description),
        ("install_command", install_command),
        ("usage_example", usage_example),
        ("license", license_name),
    ):
        if val is not None and not isinstance(val, str):
            print(json.dumps({"error": "'%s' must be a string if provided" % field}))
            return 0

    try:
        lines = []
        lines.append("# %s" % project_name)
        lines.append("")
        if description and description.strip():
            lines.append(description.strip())
        else:
            lines.append("[Add a one-to-two sentence description of what %s does and who it's for.]" % project_name)
        lines.append("")

        lines.append("## Installation")
        lines.append("")
        if install_command and install_command.strip():
            lines.append("```")
            lines.append(install_command.strip())
            lines.append("```")
        else:
            lines.append("[Add installation instructions, e.g. a package manager command or build steps.]")
        lines.append("")

        lines.append("## Usage")
        lines.append("")
        if usage_example and usage_example.strip():
            lines.append("```")
            lines.append(usage_example.strip())
            lines.append("```")
        else:
            lines.append("[Add a minimal usage example showing the most common workflow.]")
        lines.append("")

        lines.append("## License")
        lines.append("")
        if license_name and license_name.strip():
            lines.append(license_name.strip())
        else:
            lines.append("Add a LICENSE file and reference it here.")
        lines.append("")

        markdown = "\n".join(lines)
        print(json.dumps({"markdown": markdown}, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "readme_scaffold failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
