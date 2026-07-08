#!/usr/bin/env python3
"""gitignore_gen — Engram skill (no network). Generate a .gitignore file for
one or more well-known stacks.

Request (stdin): {"stacks": ["node", "python", "rust", "macos", "vscode"]}
Output (stdout): {gitignore_content, stacks_included, skipped}
"""
import json
import sys

_STACKS = {
    "node": [
        "node_modules/", "npm-debug.log*", "yarn-debug.log*", "yarn-error.log*",
        ".npm", "dist/", "build/", ".env", ".env.local",
    ],
    "python": [
        "__pycache__/", "*.py[cod]", ".venv/", "venv/", "env/", "*.egg-info/",
        ".pytest_cache/", ".mypy_cache/", ".ruff_cache/", "dist/", "build/", "*.egg",
    ],
    "rust": [
        "/target/",
        "# Cargo.lock is NOT ignored by default (binaries should commit it).",
        "# If this crate is a library, uncomment the next line:",
        "# Cargo.lock",
        "**/*.rs.bk",
    ],
    "go": [
        "bin/", "*.exe", "*.exe~", "*.dll", "*.so", "*.dylib", "*.test", "*.out", "vendor/",
    ],
    "java": [
        "*.class", "target/", "*.jar", "*.war", ".gradle/", "build/",
    ],
    "macos": [
        ".DS_Store", ".AppleDouble", ".LSOverride", "._*", ".Spotlight-V100", ".Trashes",
    ],
    "windows": [
        "Thumbs.db", "ehthumbs.db", "Desktop.ini", "$RECYCLE.BIN/",
    ],
    "vscode": [
        ".vscode/*", "!.vscode/extensions.json",
    ],
    "jetbrains": [
        ".idea/",
    ],
    "docker": [
        "*.pid", "docker-compose.override.yml",
    ],
    "terraform": [
        ".terraform/", "*.tfstate", "*.tfstate.backup", "*.tfvars", ".terraform.lock.hcl",
    ],
}


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e})); return 0
    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"stacks": ["node", "python", "macos"]},
        })); return 0

    stacks = q.get("stacks")
    if not isinstance(stacks, list) or not stacks:
        print(json.dumps({
            "error": "missing required field 'stacks' (non-empty list of strings)",
            "example": {"stacks": ["node", "python", "macos"]},
            "supported_stacks": sorted(_STACKS),
        })); return 0

    try:
        included = []
        skipped = []
        blocks = []
        for raw in stacks:
            name = str(raw).strip().lower()
            if not name:
                continue
            if name in _STACKS:
                if name not in included:
                    included.append(name)
                    blocks.append("# --- %s ---\n%s" % (name, "\n".join(_STACKS[name])))
            else:
                skipped.append(str(raw))

        if not included:
            print(json.dumps({
                "error": "no recognized stacks in %r" % (stacks,),
                "supported_stacks": sorted(_STACKS),
            })); return 0

        content = "\n\n".join(blocks) + "\n"
        result = {
            "gitignore_content": content,
            "stacks_included": included,
            "skipped": skipped,
        }
        if skipped:
            result["supported_stacks"] = sorted(_STACKS)
        print(json.dumps(result, indent=2, default=str)); return 0
    except Exception as e:
        print(json.dumps({"error": "gitignore_gen failed: %s" % e})); return 1


if __name__ == "__main__":
    sys.exit(main())
