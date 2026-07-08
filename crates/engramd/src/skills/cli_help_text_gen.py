#!/usr/bin/env python3
"""cli_help_text_gen — Engram skill (no network). Generate a conventional
argparse-style `--help` text block for a CLI, given a program name and
optional description/subcommands/flags.

Request (stdin): {"program_name": str, "description"?: str,
                   "subcommands"?: [{"name": str, "description": str}],
                   "flags"?: [{"flag": str, "description": str, "takes_value"?: bool}]}
Output (stdout): {"help_text": str}
"""
import json
import sys

_DEFAULT_HELP_FLAG = {"flag": "-h, --help", "description": "Show this help message and exit", "takes_value": False}


def _format_flag_label(flag_entry):
    flag = flag_entry.get("flag", "")
    takes_value = bool(flag_entry.get("takes_value"))
    if flag == "-h, --help":
        # Already fully-formed (the synthesized default entry).
        return flag
    label = flag if flag.startswith("-") else "--%s" % flag
    if takes_value:
        label = "%s <VALUE>" % label
    return label


def _build_section(title, entries, label_fn):
    if not entries:
        return None
    labels = [label_fn(e) for e in entries]
    width = max(len(l) for l in labels) if labels else 0
    lines = ["%s:" % title]
    for entry, label in zip(entries, labels):
        desc = entry.get("description") or ""
        lines.append("  %s  %s" % (label.ljust(width), desc) if desc else "  %s" % label)
    return "\n".join(lines)


def _generate_help_text(program_name, description, subcommands, flags):
    lines = []
    usage = "usage: %s [OPTIONS]" % program_name
    if subcommands:
        usage += " <COMMAND>"
    lines.append(usage)
    lines.append("")
    lines.append(description.strip() if description and description.strip() else "(no description provided)")
    lines.append("")

    if subcommands:
        section = _build_section("Commands", subcommands, lambda e: e.get("name", ""))
        lines.append(section)
        lines.append("")

    all_flags = list(flags) + [_DEFAULT_HELP_FLAG]
    section = _build_section("Options", all_flags, _format_flag_label)
    lines.append(section)

    return "\n".join(lines).rstrip("\n") + "\n"


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0
    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object",
                          "example": {"program_name": "mytool", "description": "Do useful things.",
                                      "subcommands": [{"name": "run", "description": "Run the tool"}],
                                      "flags": [{"flag": "-v, --verbose", "description": "Verbose output"}]}}))
        return 0

    program_name = q.get("program_name")
    if not isinstance(program_name, str) or not program_name.strip():
        print(json.dumps({
            "error": "provide non-empty 'program_name'",
            "example": {"program_name": "mytool", "description": "Do useful things.",
                        "subcommands": [{"name": "run", "description": "Run the tool"}],
                        "flags": [{"flag": "-v, --verbose", "description": "Verbose output"}]},
        }))
        return 0

    description = q.get("description")
    if description is not None and not isinstance(description, str):
        print(json.dumps({"error": "'description', if provided, must be a string"}))
        return 0

    subcommands = q.get("subcommands") or []
    if not isinstance(subcommands, list) or not all(isinstance(s, dict) for s in subcommands):
        print(json.dumps({"error": "'subcommands', if provided, must be a list of {name, description} objects"}))
        return 0

    flags = q.get("flags") or []
    if not isinstance(flags, list) or not all(isinstance(f, dict) for f in flags):
        print(json.dumps({"error": "'flags', if provided, must be a list of {flag, description, takes_value?} objects"}))
        return 0

    try:
        help_text = _generate_help_text(program_name.strip(), description, subcommands, flags)
    except Exception as e:
        print(json.dumps({"error": "help text generation failed: %s" % e}))
        return 1

    print(json.dumps({"help_text": help_text}, indent=2, default=str))
    return 0


if __name__ == "__main__":
    sys.exit(main())
