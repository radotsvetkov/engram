#!/usr/bin/env python3
"""node_debug_config_gen — Engram skill (no network). Build a Node.js debugger config.

Generates a VS Code launch.json configuration (and the equivalent raw CLI
invocation) for debugging a Node.js program, either by launching it directly
or by attaching to an already-running process started with --inspect.

Request (stdin): {"mode"?: "launch"|"attach" = "launch", "program"?: "index.js",
                  "port"?: 9229, "args"?: ["--foo"], "name"?: "..."}
Output (stdout): {launch_json, cli_command}
"""
import json
import sys

VALID_MODES = ("launch", "attach")


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0

    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object"}))
        return 0

    mode = (q.get("mode") or "launch").lower()
    if mode not in VALID_MODES:
        print(json.dumps({
            "error": "'mode' must be one of: %s" % ", ".join(VALID_MODES),
            "example": {"mode": "launch", "program": "index.js"},
        }))
        return 0

    port = q.get("port", 9229)
    if not isinstance(port, int) or port <= 0:
        print(json.dumps({"error": "'port', if given, must be a positive integer"}))
        return 0

    name = q.get("name") or ("Launch Program" if mode == "launch" else "Attach to Node")
    extra_args = q.get("args") or []
    if not isinstance(extra_args, list) or not all(isinstance(a, str) for a in extra_args):
        print(json.dumps({"error": "'args', if given, must be a list of strings"}))
        return 0

    if mode == "launch":
        program = q.get("program")
        if not isinstance(program, str) or not program.strip():
            print(json.dumps({
                "error": "'program' is required for mode 'launch' — the entry-point .js file to run",
                "example": {"mode": "launch", "program": "${workspaceFolder}/index.js"},
            }))
            return 0
        cfg = {
            "type": "node",
            "request": "launch",
            "name": name,
            "program": program,
            "args": extra_args,
            "console": "integratedTerminal",
            "skipFiles": ["<node_internals>/**"],
        }
        cli_command = "node --inspect-brk=%d %s%s" % (
            port, program, ((" " + " ".join(extra_args)) if extra_args else "")
        )
    else:
        cfg = {
            "type": "node",
            "request": "attach",
            "name": name,
            "port": port,
            "skipFiles": ["<node_internals>/**"],
        }
        cli_command = "node --inspect=%d <your-already-running-or-restarted-script>.js" % port

    launch_json = {"version": "0.2.0", "configurations": [cfg]}
    print(json.dumps({
        "launch_json": launch_json,
        "cli_command": cli_command,
        "note": "paste 'launch_json' into .vscode/launch.json, or run 'cli_command' directly and attach a debugger client to the given port",
    }, indent=2, default=str))
    return 0


if __name__ == "__main__":
    sys.exit(main())
